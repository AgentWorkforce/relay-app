//! Operator policy: upload settings a workspace's operator enforces for
//! every member's computer.
//!
//! Cloud serves the policy as the `policy` object of
//! `GET /workspaces/{ws}/access/me`:
//! `{"version": 1, "revision": 7, "settings": {"sharing_mode": "all"}}`. A
//! key present in `settings` is enforced; an absent key is the person's to
//! choose. The last document read is kept per workspace in `policy.json` in
//! the app data directory, apart from the install's `config.json`.
//!
//! - A successful read without a `policy` (or Cloud not serving the route
//!   yet) is user mode, and lifts whatever was enforced before.
//! - A failed read keeps the cached document: an enforced setting never
//!   relaxes offline. Only a revision at least the cached one applies.
//! - A sign-in with nothing cached for its workspace is pending: uploads
//!   are withheld (the job is paused, nothing is deleted) until a read
//!   succeeds. An install that predates this build and has nothing cached
//!   keeps the person's mode until its first read.
//! - A site build may compile in defaults (`DESKTOP_POLICY_DEFAULTS`,
//!   checked by `build.rs` with [`settings`]' rules): they are enforced
//!   wherever nothing has been read yet, signed in with nothing cached,
//!   before the first read and with an unreadable cache, in place of
//!   withholding. The first read replaces them wholesale: a setting the
//!   read policy leaves out is the person's, whatever the defaults said.
//! - Only the policy's own pause (`Entry::withheld`) is lifted by a read. A
//!   pause the person makes while withheld clears the flag, so it stays.
//!   The probe pauses the job before the desktop records the flag; a
//!   desktop that stops in between leaves the job paused for the person to
//!   resume, which fails safe: nothing uploads that the policy might forbid.
//! - Sign-out keeps the cache; it applies again on the next sign-in.
//!
//! The desktop only applies the policy; Cloud's ingest is the backstop.

use crate::contract::account::SharingMode;
use agent_relay_node::relay::cloud_session::{segment, CloudAccess, CloudError};
use agent_relay_node::relay::protocol::{ErrorKind, RpcError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

mod pass;
pub mod settings;

pub use pass::{pass_policy, pass_policy_with, pass_revision};

pub use settings::{
    MAX_OOPS_WINDOW_SECONDS, OOPS_WINDOW_SECONDS, SHARING_MODE, UPLOAD_CONTROL,
    UPLOAD_CONTROL_OPERATOR, UPLOAD_CONTROL_USER, USER_RETROACTIVE_DELETE,
};

/// A policy document or a path to one, used instead of Cloud's answer. Read
/// only by test and debug builds.
pub const ENVIRONMENT_KEY: &str = "RELAY_DESKTOP_OPERATOR_POLICY";
/// The cached documents, in the app data directory.
pub const FILE: &str = "policy.json";
/// The `access/me` field that carries the policy document.
pub const POLICY_FIELD: &str = "policy";
/// The document's enforced settings.
pub const SETTINGS_FIELD: &str = "settings";
/// How often the policy is read again while signed in.
pub const REFRESH_EVERY: Duration = Duration::from_secs(15 * 60);
/// The refusal of a change to an enforced setting.
pub const ENFORCED: &str = "This setting is set by your organization.";
/// Shown where a control the policy sets was removed; the Mac's
/// `OrganizationPolicy.managed` and GTK's `MANAGED_BY_ORGANIZATION` say the
/// same.
pub const MANAGED: &str = "Managed by your organization.";
/// The refusal to resume uploads while the policy is unknown.
pub const WITHHELD: &str = "Uploads wait until your organization's upload policy can be checked.";
/// The refusal to delete from Cloud while the policy is unknown.
pub const WITHHELD_DELETE: &str =
    "Deleting from Cloud waits until your organization's upload policy can be checked.";

/// The request header that names the policy revision this computer applies,
/// sent with each upload and each direct-upload credentials ask so the
/// server can refuse an upload made under an older policy (409
/// `capture_policy_stale`, retried until the next read applies the new one).
pub const REVISION_HEADER: &str = "X-Agent-Relay-Capture-Policy-Revision";

/// Where Cloud serves the policy for `workspace_id`.
pub fn access_me_path(workspace_id: &str) -> String {
    format!("/workspaces/{}/access/me", segment(workspace_id))
}

/// The operator's document.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Document {
    pub version: u32,
    pub revision: i64,
    #[serde(default)]
    pub settings: BTreeMap<String, Value>,
}

/// What one read of the policy found.
#[derive(Clone, Debug, PartialEq)]
pub enum Fetched {
    /// Cloud answered with a document.
    Policy(Document),
    /// Cloud answered with no policy: every setting is the person's.
    Absent,
    /// Cloud could not be asked, or its answer was unreadable.
    Failed,
}

/// Reads `access/me`'s answer: no `policy` field (or `null`) is
/// [`Fetched::Absent`]; a document whose known settings this build cannot
/// read is [`Fetched::Failed`], so nothing is relaxed over it.
pub fn parse_access_me(me: &Value) -> Fetched {
    match me.get(POLICY_FIELD) {
        None | Some(Value::Null) => Fetched::Absent,
        Some(policy) => parse_document(policy),
    }
}

fn parse_document(policy: &Value) -> Fetched {
    let Ok(document) = serde_json::from_value::<Document>(policy.clone()) else {
        return Fetched::Failed;
    };
    if !valid_settings(&document.settings) {
        return Fetched::Failed;
    }
    Fetched::Policy(document)
}

/// Whether this build can read every setting it knows in `settings`.
fn valid_settings(settings: &BTreeMap<String, Value>) -> bool {
    settings::unreadable_setting(settings).is_none()
}

/// The settings this build enforces until its workspace's policy is read:
/// `DESKTOP_POLICY_DEFAULTS` as `build.rs` checked it and passed it on, on
/// a site build only; `None` on every other build. Test builds never take
/// them, so the suite is the same whatever it is built with: tests name
/// their defaults ([`OperatorPolicy::with_defaults`], [`upload_hold_with`]).
pub fn compiled_defaults() -> Option<&'static BTreeMap<String, Value>> {
    static DEFAULTS: std::sync::OnceLock<Option<BTreeMap<String, Value>>> =
        std::sync::OnceLock::new();
    if cfg!(test) {
        return None;
    }
    DEFAULTS
        .get_or_init(|| built_defaults(option_env!("DESKTOP_POLICY_DEFAULTS_JSON")))
        .as_ref()
}

/// The defaults `build.rs` passed on: it always sets the variable, empty
/// for none. A value that does not pass (which `build.rs` would have
/// refused) is none, so the build withholds instead.
fn built_defaults(built: Option<&str>) -> Option<BTreeMap<String, Value>> {
    settings::parse_defaults(built.unwrap_or_default())
        .ok()
        .flatten()
}

/// The oops window `settings` enforce, in milliseconds: only under
/// [`UPLOAD_CONTROL_OPERATOR`] and only when positive. Anything else
/// uploads at once.
pub fn oops_window_ms(settings: &BTreeMap<String, Value>) -> Option<i64> {
    if settings.get(UPLOAD_CONTROL).and_then(Value::as_str) != Some(UPLOAD_CONTROL_OPERATOR) {
        return None;
    }
    let seconds = settings.get(OOPS_WINDOW_SECONDS)?.as_u64()?;
    (seconds > 0 && seconds <= MAX_OOPS_WINDOW_SECONDS).then(|| seconds as i64 * 1000)
}

/// What the cached policy makes of one workspace's uploads, as the
/// collector reads it before each delivery pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadHold {
    /// Records upload as soon as they are captured.
    None,
    /// Each session's records wait this many milliseconds after its last
    /// activity: the oops window.
    Window(i64),
    /// Nothing uploads: the policy is withheld since sign-in, or the cache
    /// cannot be read, so what it enforces is unknown (on a build without
    /// defaults).
    Blocked,
}

/// [`UploadHold`] for `workspace_id` from the cache in `app_data_dir`. An
/// empty workspace is not signed in, and has no policy. The build's
/// [`compiled_defaults`] stand in where nothing was read.
pub fn upload_hold(app_data_dir: &Path, workspace_id: &str) -> UploadHold {
    upload_hold_with(app_data_dir, workspace_id, compiled_defaults())
}

/// [`upload_hold`] with `defaults` in place of the build's.
pub fn upload_hold_with(
    app_data_dir: &Path,
    workspace_id: &str,
    defaults: Option<&BTreeMap<String, Value>>,
) -> UploadHold {
    pass::policy_read(app_data_dir, workspace_id, defaults).0
}

/// What `standing` makes of uploads.
fn hold_of(standing: Standing) -> UploadHold {
    match standing {
        Standing::User => UploadHold::None,
        Standing::Withheld => UploadHold::Blocked,
        Standing::Enforced(settings) => match oops_window_ms(&settings) {
            Some(window) => UploadHold::Window(window),
            None => UploadHold::None,
        },
    }
}

/// The cache could not be read, or holds settings this build cannot read.
#[derive(Debug)]
pub struct Unreadable;

/// The cache at `file`: empty when there is none yet.
fn read_saved(file: &Path) -> Result<Saved, Unreadable> {
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Saved::default()),
        Err(_) => return Err(Unreadable),
    };
    let saved: Saved = serde_json::from_slice(&bytes).map_err(|_| Unreadable)?;
    if saved
        .workspaces
        .values()
        .all(|entry| valid_settings(&entry.settings))
    {
        Ok(saved)
    } else {
        Err(Unreadable)
    }
}

/// The sharing mode `UploaderStatus.enforced_settings` enforces, if any.
pub fn enforced_sharing_in(settings: &BTreeMap<String, String>) -> Option<SharingMode> {
    settings
        .get(SHARING_MODE)
        .and_then(|mode| serde_json::from_value(Value::String(mode.clone())).ok())
}

/// Whether `UploaderStatus.enforced_settings` says the operator runs
/// uploads, so the person's pause and resume are removed.
pub fn uploads_operator_run_in(settings: &BTreeMap<String, String>) -> bool {
    settings.get(UPLOAD_CONTROL).map(String::as_str) == Some(UPLOAD_CONTROL_OPERATOR)
}

/// Whether `UploaderStatus.enforced_settings` leaves the person deleting
/// their sessions from Cloud; only a present `false` withdraws it.
pub fn delete_allowed_in(settings: &BTreeMap<String, String>) -> bool {
    settings.get(USER_RETROACTIVE_DELETE).map(String::as_str) != Some("false")
}

/// One workspace's cached policy.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub workspace_id: String,
    /// The document's revision; `None` before any document.
    pub revision: Option<i64>,
    /// When the policy was last read (Unix milliseconds); `None` while a
    /// sign-in waits for its first read.
    pub fetched_at: Option<i64>,
    /// The enforced settings.
    pub settings: BTreeMap<String, Value>,
    /// The person's own values of the settings enforced over them, given
    /// back when the operator lifts the setting.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub user_settings: BTreeMap<String, Value>,
    /// Uploads were paused here while the policy was unknown.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub withheld: bool,
}

/// What applies to a workspace now.
#[derive(Clone, Debug, PartialEq)]
pub enum Standing {
    /// Every setting is the person's.
    User,
    /// No policy was read since sign-in: new uploads wait.
    Withheld,
    /// These settings are enforced.
    Enforced(BTreeMap<String, Value>),
}

/// `entry` after a read that found `fetched` at `now`. A failure changes
/// nothing; an older revision only counts as a read.
pub fn apply(
    entry: Option<Entry>,
    workspace_id: &str,
    fetched: Fetched,
    now: i64,
) -> Option<Entry> {
    let document = match fetched {
        Fetched::Failed => return entry,
        Fetched::Policy(document) => Some(document),
        Fetched::Absent => None,
    };
    let mut entry = entry.unwrap_or_else(|| Entry {
        workspace_id: workspace_id.to_string(),
        ..Entry::default()
    });
    match document {
        Some(document) => {
            if entry
                .revision
                .is_none_or(|cached| document.revision >= cached)
            {
                entry.revision = Some(document.revision);
                entry.settings = document.settings;
            }
        }
        None => entry.settings.clear(),
    }
    entry.fetched_at = Some(now);
    Some(entry)
}

/// What `entry` makes of the workspace, on a build without defaults.
pub fn standing(entry: Option<&Entry>) -> Standing {
    standing_with(entry, None)
}

/// What `entry` makes of a signed-in workspace on a build with `defaults`:
/// until a read, the defaults; after one, the policy read alone.
pub fn standing_with(
    entry: Option<&Entry>,
    defaults: Option<&BTreeMap<String, Value>>,
) -> Standing {
    match entry {
        None => match defaults {
            Some(defaults) => enforcing(defaults),
            None => Standing::User,
        },
        Some(entry) if entry.fetched_at.is_none() => unread(defaults),
        Some(entry) => enforcing(&entry.settings),
    }
}

/// What applies with nothing read: the defaults, else withheld.
fn unread(defaults: Option<&BTreeMap<String, Value>>) -> Standing {
    defaults.map_or(Standing::Withheld, enforcing)
}

/// `settings` enforced; none is the person's own.
fn enforcing(settings: &BTreeMap<String, Value>) -> Standing {
    if settings.is_empty() {
        Standing::User
    } else {
        Standing::Enforced(settings.clone())
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    workspaces: BTreeMap<String, Entry>,
}

/// The operator policy for this computer's workspaces.
pub struct OperatorPolicy {
    file: PathBuf,
    /// [`ENVIRONMENT_KEY`]'s value, in test and debug builds only.
    environment: Option<String>,
    saved: Mutex<Saved>,
    /// The workspace last read and when.
    last_read: Mutex<Option<(String, Instant)>>,
    /// One read at a time.
    reading: tokio::sync::Mutex<()>,
    /// The cache could not be read when this started, so what it enforces
    /// is unknown: a signed-in workspace is withheld (or under `defaults`)
    /// until a read rewrites it.
    unreadable: std::sync::atomic::AtomicBool,
    /// What a signed-in workspace is under until its policy is read, in
    /// place of withholding: the build's [`compiled_defaults`].
    defaults: Option<BTreeMap<String, Value>>,
}

impl OperatorPolicy {
    /// The policy cached in `app_data_dir`, with the build's defaults.
    pub fn new(app_data_dir: &Path, environment: Option<String>) -> OperatorPolicy {
        OperatorPolicy::with_defaults(app_data_dir, environment, compiled_defaults().cloned())
    }

    /// [`OperatorPolicy::new`] with `defaults` in place of the build's.
    pub fn with_defaults(
        app_data_dir: &Path,
        environment: Option<String>,
        defaults: Option<BTreeMap<String, Value>>,
    ) -> OperatorPolicy {
        let file = app_data_dir.join(FILE);
        let (saved, unreadable) = match read_saved(&file) {
            Ok(saved) => (saved, false),
            Err(Unreadable) => (Saved::default(), true),
        };
        #[cfg(not(any(test, debug_assertions)))]
        let environment = {
            let _ = environment;
            None
        };
        OperatorPolicy {
            file,
            environment,
            saved: Mutex::new(saved),
            last_read: Mutex::new(None),
            reading: tokio::sync::Mutex::new(()),
            unreadable: std::sync::atomic::AtomicBool::new(unreadable),
            defaults,
        }
    }

    fn change<T>(&self, change: impl FnOnce(&mut Saved) -> T) -> T {
        let mut saved = self.saved.lock().unwrap_or_else(|e| e.into_inner());
        let result = change(&mut saved);
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
        if written.is_ok() {
            self.unreadable
                .store(false, std::sync::atomic::Ordering::SeqCst);
        }
        result
    }

    /// The cached entry for `workspace_id`.
    pub fn entry(&self, workspace_id: &str) -> Option<Entry> {
        let saved = self.saved.lock().unwrap_or_else(|e| e.into_inner());
        saved.workspaces.get(workspace_id).cloned()
    }

    /// What applies to `workspace_id`; nothing is enforced without one.
    pub fn standing(&self, workspace_id: Option<&str>) -> Standing {
        match workspace_id {
            Some(_) if self.unreadable() => unread(self.defaults.as_ref()),
            Some(workspace_id) => {
                standing_with(self.entry(workspace_id).as_ref(), self.defaults.as_ref())
            }
            None => Standing::User,
        }
    }

    fn unreadable(&self) -> bool {
        self.unreadable.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Whether the defaults stand in for `workspace_id`'s policy, which
    /// has not been read.
    fn defaulted(&self, workspace_id: &str) -> bool {
        self.defaults.is_some()
            && (self.unreadable()
                || self
                    .entry(workspace_id)
                    .is_none_or(|entry| entry.fetched_at.is_none()))
    }

    /// The enforced settings, as `UploaderStatus.enforced_settings` reports
    /// them.
    /// [`OOPS_WINDOW_SECONDS`] is reported only where it is honoured: under
    /// [`UPLOAD_CONTROL_OPERATOR`] and positive.
    pub fn enforced_settings(&self, workspace_id: Option<&str>) -> BTreeMap<String, String> {
        match self.standing(workspace_id) {
            Standing::Enforced(mut settings) => {
                if oops_window_ms(&settings).is_none() {
                    settings.remove(OOPS_WINDOW_SECONDS);
                }
                settings
                    .into_iter()
                    .map(|(key, value)| match value {
                        Value::String(text) => (key, text),
                        other => (key, other.to_string()),
                    })
                    .collect()
            }
            _ => BTreeMap::new(),
        }
    }

    /// The enforced sharing mode, if any.
    pub fn enforced_sharing(&self, workspace_id: Option<&str>) -> Option<SharingMode> {
        match self.standing(workspace_id) {
            Standing::Enforced(settings) => settings
                .get(SHARING_MODE)
                .and_then(|mode| serde_json::from_value(mode.clone()).ok()),
            _ => None,
        }
    }

    /// The enforced value of `key`, if any.
    fn enforced(&self, workspace_id: Option<&str>, key: &str) -> Option<Value> {
        match self.standing(workspace_id) {
            Standing::Enforced(mut settings) => settings.remove(key),
            _ => None,
        }
    }

    /// Whether the operator runs uploads, so the person may not pause or
    /// resume them.
    pub fn controls_uploads(&self, workspace_id: Option<&str>) -> bool {
        self.enforced(workspace_id, UPLOAD_CONTROL)
            .is_some_and(|control| control == UPLOAD_CONTROL_OPERATOR)
    }

    /// Refuses a person's pause or resume while the operator runs uploads.
    pub fn check_pause(&self, workspace_id: Option<&str>) -> Result<(), RpcError> {
        if self.controls_uploads(workspace_id) {
            return Err(enforced());
        }
        Ok(())
    }

    /// Refuses a person's deletion of their sessions from Cloud when the
    /// operator does not allow it. A deletion waits for the first policy
    /// read, like resuming uploads.
    pub fn check_delete(&self, workspace_id: Option<&str>) -> Result<(), RpcError> {
        match self.standing(workspace_id) {
            Standing::Withheld => Err(RpcError::new(ErrorKind::Refused, WITHHELD_DELETE)),
            _ => match self.enforced(workspace_id, USER_RETROACTIVE_DELETE) {
                Some(Value::Bool(false)) => Err(enforced()),
                _ => Ok(()),
            },
        }
    }

    /// Refuses a person's change of the sharing mode to `requested` when
    /// the operator enforces another.
    pub fn check_sharing(
        &self,
        workspace_id: Option<&str>,
        requested: SharingMode,
    ) -> Result<(), RpcError> {
        match self.enforced_sharing(workspace_id) {
            Some(mode) if mode != requested => Err(enforced()),
            _ => Ok(()),
        }
    }

    /// Refuses excluding sessions while every session is enforced to
    /// upload, unless the operator leaves an oops window: then a session may
    /// be kept back while it waits in it, which the collector checks per
    /// session.
    pub fn check_exclusion(&self, workspace_id: Option<&str>) -> Result<(), RpcError> {
        if self.oops_window_ms(workspace_id).is_some() {
            return Ok(());
        }
        match self.enforced_sharing(workspace_id) {
            Some(SharingMode::All) => Err(enforced()),
            _ => Ok(()),
        }
    }

    /// The oops window the policy enforces on `workspace_id`, if any.
    pub fn oops_window_ms(&self, workspace_id: Option<&str>) -> Option<i64> {
        match self.standing(workspace_id) {
            Standing::Enforced(settings) => oops_window_ms(&settings),
            _ => None,
        }
    }

    /// Refuses resuming uploads the policy withholds.
    pub fn check_resume(&self, workspace_id: Option<&str>) -> Result<(), RpcError> {
        match self.standing(workspace_id) {
            Standing::Withheld => Err(RpcError::new(ErrorKind::Refused, WITHHELD)),
            _ => Ok(()),
        }
    }

    /// A sign-in to `workspace_id`: with nothing cached, its uploads wait
    /// for the first read.
    pub fn sign_in(&self, workspace_id: &str) {
        if self.entry(workspace_id).is_none() {
            self.change(|saved| {
                saved.workspaces.insert(
                    workspace_id.to_string(),
                    Entry {
                        workspace_id: workspace_id.to_string(),
                        ..Entry::default()
                    },
                );
            });
        }
    }

    /// Whether `workspace_id` is due a read: while withheld or under the
    /// defaults, always; else [`REFRESH_EVERY`] after the last.
    pub fn due(&self, workspace_id: &str) -> bool {
        if self.standing(Some(workspace_id)) == Standing::Withheld || self.defaulted(workspace_id) {
            return true;
        }
        let last = self.last_read.lock().unwrap_or_else(|e| e.into_inner());
        last.as_ref()
            .is_none_or(|(read, at)| read != workspace_id || at.elapsed() >= REFRESH_EVERY)
    }

    /// Reads `workspace_id`'s policy and caches what it finds; answers what
    /// applies now.
    pub async fn refresh(&self, cloud: &dyn CloudAccess, workspace_id: &str) -> Standing {
        let _one = self.reading.lock().await;
        let fetched = match self.environment.as_deref() {
            Some(value) => from_environment(value),
            None => match cloud
                .request("GET", access_me_path(workspace_id), None)
                .await
            {
                Ok(me) => parse_access_me(&me),
                // Cloud does not serve the access routes yet: no policy.
                Err(CloudError::Status {
                    status: 404 | 405 | 501,
                    ..
                }) => Fetched::Absent,
                Err(_) => Fetched::Failed,
            },
        };
        *self.last_read.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((workspace_id.to_string(), Instant::now()));
        if fetched != Fetched::Failed {
            let now = chrono::Utc::now().timestamp_millis();
            self.change(|saved| {
                let entry = saved.workspaces.remove(workspace_id);
                if let Some(entry) = apply(entry, workspace_id, fetched, now) {
                    saved.workspaces.insert(workspace_id.to_string(), entry);
                }
            });
        }
        self.standing(Some(workspace_id))
    }

    /// Keeps `value` as the person's own `key` while it is enforced over,
    /// unless one is kept already.
    pub fn remember(&self, workspace_id: &str, key: &str, value: Value) {
        self.change(|saved| {
            if let Some(entry) = saved.workspaces.get_mut(workspace_id) {
                entry.user_settings.entry(key.to_string()).or_insert(value);
            }
        });
    }

    /// The person's own `key`, kept while it was enforced over.
    pub fn remembered(&self, workspace_id: &str, key: &str) -> Option<Value> {
        self.entry(workspace_id)
            .and_then(|entry| entry.user_settings.get(key).cloned())
    }

    /// Drops the person's kept `key` once it is given back.
    pub fn forget(&self, workspace_id: &str, key: &str) {
        self.change(|saved| {
            if let Some(entry) = saved.workspaces.get_mut(workspace_id) {
                entry.user_settings.remove(key);
            }
        });
    }

    /// Whether uploads wait for the first read since sign-in.
    pub fn withholds(&self, workspace_id: Option<&str>) -> bool {
        self.standing(workspace_id) == Standing::Withheld
    }

    /// Records whether uploads were paused here while withheld.
    pub fn set_withheld(&self, workspace_id: &str, withheld: bool) {
        self.change(|saved| {
            if let Some(entry) = saved.workspaces.get_mut(workspace_id) {
                entry.withheld = withheld;
            }
        });
    }
}

fn enforced() -> RpcError {
    RpcError::new(ErrorKind::Refused, ENFORCED)
}

/// [`ENVIRONMENT_KEY`]: a document (`null` for none), or a path to one; a
/// path that cannot be read is a failed read.
fn from_environment(value: &str) -> Fetched {
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
        Ok(policy) => parse_document(&policy),
        Err(_) => Fetched::Failed,
    }
}

#[cfg(test)]
mod tests;
