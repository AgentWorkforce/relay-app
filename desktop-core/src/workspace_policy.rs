//! Workspace policy (v2): the Agent Relay Desktop settings an
//! organization's owner or admin sets for every member's computer:
//! capture, spawning, auto-join, inbound delivery and analytics.
//!
//! Cloud serves it as the `workspace_policy` object of
//! `GET /workspaces/{ws}/access/me`, next to the v1 capture policy
//! (`history::operator_policy`), which stays as it was. Both are read from
//! the same answer: `OperatorPolicy::refresh` makes one request and feeds
//! both, so this document is read at sign-in, at a workspace refresh and on
//! the capture policy's cadence.
//!
//! The document's `revision` is its own counter, apart from the capture
//! policy's, which uploads carry in `X-Agent-Relay-Capture-Policy-Revision`.
//!
//! - This build pins the allowlist in [`keys`]: an unknown section, tier or
//!   key, a key in a tier it may not appear in, or a value it cannot take is
//!   dropped (and logged by name, never value); the rest applies. A
//!   `consent` entry that is not a person-only key is dropped too.
//! - A document that is not version 2, or whose `revision`, `managed`,
//!   `consent` or `sections` is malformed, is a failed read.
//! - A successful read without the object (or Cloud not serving the route)
//!   is the unset policy; a failed read keeps the cached document. Only a
//!   revision at least the cached one applies.
//! - The last document read is kept per workspace in
//!   `workspace-policy.json` in the app data directory, which sign-out
//!   keeps.
//!
//! This is the reader only: nothing here enforces a setting yet.

use agent_relay_node::relay::cloud_session::CloudError;
use agent_relay_node::relay::settings_policy::SettingsPolicy;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub mod keys;
pub mod settings;

pub use crate::history::operator_policy::{ENFORCED, MANAGED};
pub use keys::{Key, SectionName, Tier};

/// The cached documents, in the app data directory.
pub const FILE: &str = "workspace-policy.json";
/// The `access/me` field that carries the document.
pub const FIELD: &str = "workspace_policy";
/// The only document version this build reads.
pub const VERSION: u32 = 2;
/// An unset policy's revision.
pub const UNSET_REVISION: i64 = 1;

/// One section's two tiers.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Section {
    pub locked: BTreeMap<Key, Value>,
    pub defaults: BTreeMap<Key, Value>,
}

impl Section {
    fn tier(&self, tier: Tier) -> &BTreeMap<Key, Value> {
        match tier {
            Tier::Locked => &self.locked,
            Tier::Defaults => &self.defaults,
        }
    }

    fn tier_mut(&mut self, tier: Tier) -> &mut BTreeMap<Key, Value> {
        match tier {
            Tier::Locked => &mut self.locked,
            Tier::Defaults => &mut self.defaults,
        }
    }
}

/// The document's five sections.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Sections {
    pub capture: Section,
    pub spawn: Section,
    pub join: Section,
    pub delivery: Section,
    pub analytics: Section,
}

impl Sections {
    pub fn get(&self, name: SectionName) -> &Section {
        match name {
            SectionName::Capture => &self.capture,
            SectionName::Spawn => &self.spawn,
            SectionName::Join => &self.join,
            SectionName::Delivery => &self.delivery,
            SectionName::Analytics => &self.analytics,
        }
    }

    fn get_mut(&mut self, name: SectionName) -> &mut Section {
        match name {
            SectionName::Capture => &mut self.capture,
            SectionName::Spawn => &mut self.spawn,
            SectionName::Join => &mut self.join,
            SectionName::Delivery => &mut self.delivery,
            SectionName::Analytics => &mut self.analytics,
        }
    }
}

/// The organization's workspace policy, as this build reads it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Value")]
pub struct WorkspacePolicy {
    pub version: u32,
    pub revision: i64,
    pub managed: bool,
    /// The person-only keys whose operator value applies only after the
    /// person agrees.
    pub consent: BTreeSet<String>,
    pub sections: Sections,
}

impl Default for WorkspacePolicy {
    fn default() -> WorkspacePolicy {
        WorkspacePolicy::unset()
    }
}

impl WorkspacePolicy {
    /// The policy of an organization that set nothing.
    pub fn unset() -> WorkspacePolicy {
        WorkspacePolicy {
            version: VERSION,
            revision: UNSET_REVISION,
            managed: false,
            consent: BTreeSet::new(),
            sections: Sections::default(),
        }
    }

    /// The operator's locked value of `key`, which the person cannot change.
    pub fn locked(&self, key: Key) -> Option<&Value> {
        self.sections.get(key.section()).locked.get(&key)
    }

    /// The operator's initial value of `key`, which the person may change.
    pub fn default(&self, key: Key) -> Option<&Value> {
        self.sections.get(key.section()).defaults.get(&key)
    }

    pub fn is_locked(&self, key: Key) -> bool {
        self.locked(key).is_some()
    }

    /// Whether the operator's value of `key` waits for the person's
    /// agreement: a person-only key that `consent` names.
    pub fn needs_consent(&self, key: Key) -> bool {
        key.person_only() && self.consent.contains(key.name())
    }

    pub fn revision(&self) -> i64 {
        self.revision
    }

    pub fn managed(&self) -> bool {
        self.managed
    }
}

/// Why a document is a failed read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Malformed(pub &'static str);

impl std::fmt::Display for Malformed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "workspace policy has an unreadable {}", self.0)
    }
}

impl std::error::Error for Malformed {}

impl TryFrom<Value> for WorkspacePolicy {
    type Error = Malformed;

    fn try_from(value: Value) -> Result<WorkspacePolicy, Malformed> {
        parse_document(&value)
    }
}

/// Reads one document under the allowlist.
pub fn parse_document(value: &Value) -> Result<WorkspacePolicy, Malformed> {
    let object = value.as_object().ok_or(Malformed("document"))?;
    let version = object.get("version").and_then(Value::as_u64);
    if version != Some(u64::from(VERSION)) {
        return Err(Malformed("version"));
    }
    let revision = object
        .get("revision")
        .and_then(Value::as_i64)
        .ok_or(Malformed("revision"))?;
    let managed = match object.get("managed") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(managed)) => *managed,
        Some(_) => return Err(Malformed("managed")),
    };
    let consent = match object.get("consent") {
        None | Some(Value::Null) => BTreeSet::new(),
        Some(Value::Array(entries)) => read_consent(entries),
        Some(_) => return Err(Malformed("consent")),
    };
    let sections = match object.get("sections") {
        None | Some(Value::Null) => Sections::default(),
        Some(Value::Object(sections)) => read_sections(sections),
        Some(_) => return Err(Malformed("sections")),
    };
    Ok(WorkspacePolicy {
        version: VERSION,
        revision,
        managed,
        consent,
        sections,
    })
}

fn ignored(what: impl std::fmt::Display) {
    eprintln!("workspace policy: ignored {what}");
}

fn read_consent(entries: &[Value]) -> BTreeSet<String> {
    let mut consent = BTreeSet::new();
    for entry in entries {
        match entry.as_str().and_then(Key::from_name) {
            Some(key) if key.person_only() => {
                consent.insert(key.name().to_string());
            }
            Some(key) => ignored(format_args!("consent entry {}", key.name())),
            None => match entry.as_str() {
                Some(name) => ignored(format_args!("unknown consent entry {name}")),
                None => ignored("a consent entry that is not a name"),
            },
        }
    }
    consent
}

fn read_sections(sections: &Map<String, Value>) -> Sections {
    let mut read = Sections::default();
    for (name, section) in sections {
        let Some(section_name) = SectionName::from_name(name) else {
            ignored(format_args!("section {name}"));
            continue;
        };
        let Some(tiers) = section.as_object() else {
            ignored(format_args!("section {name}"));
            continue;
        };
        // Locked first: a key in both tiers keeps its locked value.
        for tier in [Tier::Locked, Tier::Defaults] {
            match tiers.get(tier.name()) {
                None | Some(Value::Null) => {}
                Some(Value::Object(values)) => {
                    read_tier(read.get_mut(section_name), section_name, tier, values)
                }
                Some(_) => ignored(format_args!("{name}.{}", tier.name())),
            }
        }
        for unknown in tiers.keys().filter(|tier| Tier::from_name(tier).is_none()) {
            ignored(format_args!("{name}.{unknown}"));
        }
    }
    read
}

fn read_tier(
    section: &mut Section,
    section_name: SectionName,
    tier: Tier,
    values: &Map<String, Value>,
) {
    let path = |key: &str| format!("{}.{}.{key}", section_name.name(), tier.name());
    for (name, value) in values {
        let Some(key) = Key::from_name(name).filter(|key| key.section() == section_name) else {
            ignored(format_args!("unknown key {}", path(name)));
            continue;
        };
        if !key.allowed_in(tier) {
            ignored(format_args!("{} outside its tier", path(name)));
            continue;
        }
        if tier == Tier::Defaults && section.tier(Tier::Locked).contains_key(&key) {
            ignored(format_args!("{} also locked", path(name)));
            continue;
        }
        match key.read(value) {
            Some(value) => {
                section.tier_mut(tier).insert(key, value);
            }
            None => ignored(format_args!("unreadable {}", path(name))),
        }
    }
}

/// What one read of the workspace policy found.
#[derive(Clone, Debug, PartialEq)]
pub enum Fetched {
    /// Cloud answered with a document.
    Policy(Box<WorkspacePolicy>),
    /// Cloud answered without one: the policy is unset.
    Absent,
    /// Cloud could not be asked, or its document was unreadable.
    Failed,
    /// An enrollment key's exchange answered with a document for the
    /// workspace the sign-in connects to, which the exchange does not name:
    /// kept provisionally, until the first read of that workspace replaces it
    /// whatever its revision.
    Seed(Box<WorkspacePolicy>),
}

/// Reads `access/me`'s answer: no `workspace_policy` field (or `null`) is
/// [`Fetched::Absent`]; a malformed document is [`Fetched::Failed`].
pub fn parse_access_me(me: &Value) -> Fetched {
    match me.get(FIELD) {
        None | Some(Value::Null) => Fetched::Absent,
        Some(document) => match parse_document(document) {
            Ok(policy) => Fetched::Policy(Box::new(policy)),
            Err(error) => {
                eprintln!("{error}");
                Fetched::Failed
            }
        },
    }
}

/// What a failed `access/me` request makes of the policy: Cloud not serving
/// the route is no policy; any other failure is a failed read.
pub fn from_error(error: &CloudError) -> Fetched {
    match error {
        CloudError::Status {
            status: 404 | 405 | 501,
            ..
        } => Fetched::Absent,
        _ => Fetched::Failed,
    }
}

/// The operator policy's environment override
/// (`operator_policy::ENVIRONMENT_KEY`, test and debug builds only): the v1
/// document, or a path to one, may carry a `workspace_policy` object the
/// way `access/me` does. `null` is none; a path that cannot be read is a
/// failed read.
pub fn from_environment(value: &str) -> Fetched {
    let trimmed = value.trim();
    let text = if trimmed == "null" || trimmed.starts_with('{') {
        value.to_string()
    } else {
        match std::fs::read_to_string(value) {
            Ok(text) => text,
            Err(_) => return Fetched::Failed,
        }
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Null) => Fetched::Absent,
        Ok(document @ Value::Object(_)) => parse_access_me(&document),
        _ => Fetched::Failed,
    }
}

/// One workspace's cached policy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub workspace_id: String,
    /// The highest revision applied, kept when a read finds no policy so a
    /// revision never goes backwards.
    pub revision: i64,
    pub policy: WorkspacePolicy,
    /// Seeded from an enrollment key's exchange, not read for this
    /// workspace yet: the first read replaces it whatever its revision.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub provisional: bool,
}

/// `entry` after a read that found `fetched`. A failure changes nothing; an
/// older revision is refused; no policy is the unset one. A provisional
/// (seeded) entry gives way to any read, and a seed never replaces a read.
pub fn apply(entry: Option<Entry>, workspace_id: &str, fetched: Fetched) -> Option<Entry> {
    let fresh = |policy: Box<WorkspacePolicy>, provisional| Entry {
        workspace_id: workspace_id.to_string(),
        revision: policy.revision,
        policy: *policy,
        provisional,
    };
    match fetched {
        Fetched::Failed => entry,
        Fetched::Absent => entry.map(|entry| match entry.provisional {
            true => fresh(Box::default(), false),
            false => Entry {
                policy: WorkspacePolicy::unset(),
                ..entry
            },
        }),
        Fetched::Policy(policy) => match entry {
            Some(entry) if !entry.provisional && policy.revision < entry.revision => Some(entry),
            _ => Some(fresh(policy, false)),
        },
        Fetched::Seed(policy) => match entry {
            Some(entry) if !entry.provisional => Some(entry),
            _ => Some(fresh(policy, true)),
        },
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    workspaces: BTreeMap<String, Entry>,
    /// The file does not hold `workspaces` yet: the last write failed, so
    /// the next [`WorkspacePolicies::record`] writes even if nothing changed.
    #[serde(skip)]
    unwritten: bool,
}

/// Reads the cache at `file`, one workspace at a time: a malformed entry
/// is dropped and the others are kept. Returns what was read and why
/// anything was not (no file is not a problem).
fn load(file: &Path) -> (Saved, Vec<String>) {
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (Saved::default(), Vec::new())
        }
        Err(error) => {
            return (
                Saved::default(),
                vec![format!("could not read {FILE}: {error}")],
            )
        }
    };
    let workspaces = match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(mut document)) => match document.remove("workspaces") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(workspaces)) => workspaces,
            Some(_) => {
                return (
                    Saved::default(),
                    vec![format!("{FILE} has unreadable workspaces")],
                )
            }
        },
        Ok(_) => return (Saved::default(), vec![format!("{FILE} is not an object")]),
        Err(error) => {
            return (
                Saved::default(),
                vec![format!("{FILE} is unreadable: {error}")],
            )
        }
    };
    let mut saved = Saved::default();
    let mut problems = Vec::new();
    for (workspace_id, entry) in workspaces {
        match serde_json::from_value::<Entry>(entry) {
            Ok(entry) => {
                saved.workspaces.insert(workspace_id, entry);
            }
            Err(error) => problems.push(format!("{FILE} entry {workspace_id} dropped: {error}")),
        }
    }
    (saved, problems)
}

/// The workspace policies cached for this computer's workspaces.
pub struct WorkspacePolicies {
    file: PathBuf,
    saved: Mutex<Saved>,
    /// The settings policy last handed to the core in this run (`None` until
    /// the first; `Some(None)` once none applies), so it is handed over only
    /// when it changes.
    applied_settings: Mutex<Option<Option<SettingsPolicy>>>,
    /// The last failure to hand the settings policy to the core, with the
    /// policy it was handing over, so it is reported once rather than on
    /// every tick.
    settings_failure: Mutex<Option<(Option<SettingsPolicy>, String)>>,
    /// The policy an enrollment key's exchange answered with, and the sign-in
    /// attempt it belongs to, until that attempt connects a workspace.
    enrollment_seed: Mutex<Option<(u64, WorkspacePolicy)>>,
}

impl WorkspacePolicies {
    /// The policies cached in `app_data_dir`. A malformed workspace entry
    /// is dropped and logged and the rest are kept; an unreadable cache
    /// starts empty, logged, and the next read rewrites it.
    pub fn new(app_data_dir: &Path) -> WorkspacePolicies {
        let file = app_data_dir.join(FILE);
        let (saved, problems) = load(&file);
        for problem in problems {
            eprintln!("workspace policy: {problem}");
        }
        WorkspacePolicies {
            file,
            saved: Mutex::new(saved),
            applied_settings: Mutex::new(None),
            settings_failure: Mutex::new(None),
            enrollment_seed: Mutex::new(None),
        }
    }

    /// Keeps the policy sign-in `attempt`'s enrollment key answered with
    /// (`None`: it answered with none) for the workspace it connects to.
    pub fn hold_seed(&self, attempt: u64, policy: Option<WorkspacePolicy>) {
        *self
            .enrollment_seed
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = policy.map(|policy| (attempt, policy));
    }

    /// Caches the held policy for `workspace_id`, the workspace sign-in
    /// attempt `current` connects to, provisionally (`Fetched::Seed`): a
    /// workspace already read keeps its own policy, and the first read of
    /// this one replaces the seed whatever its revision. A policy held for
    /// another attempt is dropped.
    pub fn take_seed(&self, current: u64, workspace_id: &str) {
        let seed = self
            .enrollment_seed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some((attempt, policy)) = seed {
            if attempt == current && !workspace_id.is_empty() {
                self.record(workspace_id, Fetched::Seed(Box::new(policy)));
            }
        }
    }

    /// Records `message` as the latest failure to hand `wanted` over;
    /// answers whether it differs from the one before (worth showing): a
    /// failure for another policy, a lift included, is new even with the same
    /// message.
    pub fn settings_failure_is_new(&self, wanted: &Option<SettingsPolicy>, message: &str) -> bool {
        let mut last = self
            .settings_failure
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let failure = (wanted.clone(), message.to_string());
        let new = last.as_ref() != Some(&failure);
        *last = Some(failure);
        new
    }

    /// Whether `wanted` differs from the settings policy last handed to the
    /// core in this run.
    pub fn settings_changed(&self, wanted: &Option<SettingsPolicy>) -> bool {
        let applied = self
            .applied_settings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        applied.as_ref() != Some(wanted)
    }

    /// Records `applied` as handed to the core.
    pub fn settings_applied(&self, applied: Option<SettingsPolicy>) {
        *self
            .settings_failure
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        *self
            .applied_settings
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(applied);
    }

    /// The cached entry for `workspace_id`.
    pub fn entry(&self, workspace_id: &str) -> Option<Entry> {
        let saved = self.saved.lock().unwrap_or_else(|e| e.into_inner());
        saved.workspaces.get(workspace_id).cloned()
    }

    /// What applies to `workspace_id`: the cached document, else unset.
    pub fn policy(&self, workspace_id: &str) -> WorkspacePolicy {
        self.entry(workspace_id)
            .map(|entry| entry.policy)
            .unwrap_or_default()
    }

    /// Caches what a read of `workspace_id` found, writing the file when the
    /// entry changed or the last write failed.
    pub fn record(&self, workspace_id: &str, fetched: Fetched) {
        let mut saved = self.saved.lock().unwrap_or_else(|e| e.into_inner());
        let cached = saved.workspaces.get(workspace_id).cloned();
        let applied = apply(cached.clone(), workspace_id, fetched);
        if applied == cached && !saved.unwritten {
            return;
        }
        match applied {
            Some(entry) => saved.workspaces.insert(workspace_id.to_string(), entry),
            None => saved.workspaces.remove(workspace_id),
        };
        let bytes = serde_json::to_vec_pretty(&*saved).unwrap_or_default();
        let written = self
            .file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                atomicwrites::AtomicFile::new(&self.file, atomicwrites::AllowOverwrite)
                    .write(|f| std::io::Write::write_all(f, &bytes))
                    .map_err(std::io::Error::other)
            });
        saved.unwritten = written.is_err();
        if let Err(error) = written {
            eprintln!("workspace policy: could not write {FILE}: {error}");
        }
    }
}

#[cfg(test)]
mod tests;
