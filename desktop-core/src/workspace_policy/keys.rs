//! The allowlist this build pins: every key the workspace policy may set,
//! the section it belongs to, the tiers it may appear in, whether it is
//! person-only, and the values it takes. A key, value or tier outside it is
//! never applied.

use crate::history::operator_policy::settings::{
    MAX_OOPS_WINDOW_SECONDS, SHARING_MODES, UPLOAD_CONTROL_OPERATOR, UPLOAD_CONTROL_USER,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One of the document's five sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SectionName {
    Capture,
    Spawn,
    Join,
    Delivery,
    Analytics,
}

impl SectionName {
    pub const ALL: [SectionName; 5] = [
        SectionName::Capture,
        SectionName::Spawn,
        SectionName::Join,
        SectionName::Delivery,
        SectionName::Analytics,
    ];

    /// The section's name in the document.
    pub fn name(self) -> &'static str {
        match self {
            SectionName::Capture => "capture",
            SectionName::Spawn => "spawn",
            SectionName::Join => "join",
            SectionName::Delivery => "delivery",
            SectionName::Analytics => "analytics",
        }
    }

    pub fn from_name(name: &str) -> Option<SectionName> {
        SectionName::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// A section's two tiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// An operator value the person cannot change.
    Locked,
    /// An initial value the person may change.
    Defaults,
}

impl Tier {
    pub fn name(self) -> &'static str {
        match self {
            Tier::Locked => "locked",
            Tier::Defaults => "defaults",
        }
    }

    pub fn from_name(name: &str) -> Option<Tier> {
        match name {
            "locked" => Some(Tier::Locked),
            "defaults" => Some(Tier::Defaults),
            _ => None,
        }
    }
}

/// Every key the workspace policy may set, as the allowlist names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
    // capture
    SharingMode,
    UploadControl,
    OopsWindowSeconds,
    UserRetroactiveDelete,
    // spawn
    AcceptSpawns,
    SpawnAllowTeammates,
    SpawnMaxAgents,
    SpawnNativeInput,
    SpawnRequesterApprovals,
    SpawnRepositories,
    SpawnFullAccessClaude,
    SpawnFullAccessCodex,
    SpawnBillingClaude,
    SpawnBillingCodex,
    // join
    AutoJoinGeneral,
    AutoJoinRepositories,
    TrustWorkspaceMembers,
    // delivery
    InboundDefault,
    TrustedRelaySenders,
    AdminSenders,
    // analytics
    AnalyticsEnabled,
}

/// The most entries a list setting holds.
pub const MAX_LIST_ENTRIES: usize = 200;
/// The longest entry of a list of names, in characters.
pub const MAX_LIST_ENTRY_CHARS: usize = 200;
/// The longest part of an `auto_join_repositories` entry.
pub const MAX_REPOSITORY_PART: usize = 100;
const BILLING: &[&str] = &["subscription", "api", "unspecified"];

impl Key {
    pub const ALL: [Key; 21] = [
        Key::SharingMode,
        Key::UploadControl,
        Key::OopsWindowSeconds,
        Key::UserRetroactiveDelete,
        Key::AcceptSpawns,
        Key::SpawnAllowTeammates,
        Key::SpawnMaxAgents,
        Key::SpawnNativeInput,
        Key::SpawnRequesterApprovals,
        Key::SpawnRepositories,
        Key::SpawnFullAccessClaude,
        Key::SpawnFullAccessCodex,
        Key::SpawnBillingClaude,
        Key::SpawnBillingCodex,
        Key::AutoJoinGeneral,
        Key::AutoJoinRepositories,
        Key::TrustWorkspaceMembers,
        Key::InboundDefault,
        Key::TrustedRelaySenders,
        Key::AdminSenders,
        Key::AnalyticsEnabled,
    ];

    /// The key's name in the document.
    pub fn name(self) -> &'static str {
        match self {
            Key::SharingMode => "sharing_mode",
            Key::UploadControl => "upload_control",
            Key::OopsWindowSeconds => "oops_window_seconds",
            Key::UserRetroactiveDelete => "user_retroactive_delete",
            Key::AcceptSpawns => "accept_spawns",
            Key::SpawnAllowTeammates => "spawn_allow_teammates",
            Key::SpawnMaxAgents => "spawn_max_agents",
            Key::SpawnNativeInput => "spawn_native_input",
            Key::SpawnRequesterApprovals => "spawn_requester_approvals",
            Key::SpawnRepositories => "spawn_repositories",
            Key::SpawnFullAccessClaude => "spawn_full_access_claude",
            Key::SpawnFullAccessCodex => "spawn_full_access_codex",
            Key::SpawnBillingClaude => "spawn_billing_claude",
            Key::SpawnBillingCodex => "spawn_billing_codex",
            Key::AutoJoinGeneral => "auto_join_general",
            Key::AutoJoinRepositories => "auto_join_repositories",
            Key::TrustWorkspaceMembers => "trust_workspace_members",
            Key::InboundDefault => "inbound_default",
            Key::TrustedRelaySenders => "trusted_relay_senders",
            Key::AdminSenders => "admin_senders",
            Key::AnalyticsEnabled => "analytics_enabled",
        }
    }

    pub fn from_name(name: &str) -> Option<Key> {
        Key::ALL.into_iter().find(|key| key.name() == name)
    }

    /// The section the key belongs to.
    pub fn section(self) -> SectionName {
        match self {
            Key::SharingMode
            | Key::UploadControl
            | Key::OopsWindowSeconds
            | Key::UserRetroactiveDelete => SectionName::Capture,
            Key::AcceptSpawns
            | Key::SpawnAllowTeammates
            | Key::SpawnMaxAgents
            | Key::SpawnNativeInput
            | Key::SpawnRequesterApprovals
            | Key::SpawnRepositories
            | Key::SpawnFullAccessClaude
            | Key::SpawnFullAccessCodex
            | Key::SpawnBillingClaude
            | Key::SpawnBillingCodex => SectionName::Spawn,
            Key::AutoJoinGeneral | Key::AutoJoinRepositories | Key::TrustWorkspaceMembers => {
                SectionName::Join
            }
            Key::InboundDefault | Key::TrustedRelaySenders | Key::AdminSenders => {
                SectionName::Delivery
            }
            Key::AnalyticsEnabled => SectionName::Analytics,
        }
    }

    /// Whether the key may appear in `tier`: capture keys and the
    /// server-computed `admin_senders` are locked only.
    pub fn allowed_in(self, tier: Tier) -> bool {
        match self.section() {
            SectionName::Capture => tier == Tier::Locked,
            _ if self == Key::AdminSenders => tier == Tier::Locked,
            _ => true,
        }
    }

    /// Whether the operator's value applies only after the person on the
    /// computer agrees, when `consent` names the key.
    pub fn person_only(self) -> bool {
        matches!(
            self,
            Key::AcceptSpawns
                | Key::SpawnAllowTeammates
                | Key::SpawnNativeInput
                | Key::SpawnRequesterApprovals
                | Key::SpawnRepositories
                | Key::SpawnFullAccessClaude
                | Key::SpawnFullAccessCodex
        )
    }

    /// `value` as this build applies it, or `None` when the key cannot take
    /// it. `auto_join_repositories` keeps its readable entries, lower-cased
    /// and without duplicates.
    pub fn read(self, value: &Value) -> Option<Value> {
        let readable = match self {
            Key::SharingMode => one_of(value, SHARING_MODES),
            Key::UploadControl => one_of(value, &[UPLOAD_CONTROL_OPERATOR, UPLOAD_CONTROL_USER]),
            Key::OopsWindowSeconds => in_range(value, 0, MAX_OOPS_WINDOW_SECONDS),
            Key::SpawnMaxAgents => in_range(value, 1, 32),
            Key::SpawnBillingClaude | Key::SpawnBillingCodex => one_of(value, BILLING),
            Key::InboundDefault => one_of(value, &["deliver", "hold"]),
            Key::SpawnRepositories | Key::TrustedRelaySenders => names(value),
            Key::AdminSenders => value
                .as_array()
                .is_some_and(|entries| entries.iter().all(Value::is_string)),
            Key::AutoJoinRepositories => return repositories(value),
            Key::UserRetroactiveDelete
            | Key::AcceptSpawns
            | Key::SpawnAllowTeammates
            | Key::SpawnNativeInput
            | Key::SpawnRequesterApprovals
            | Key::SpawnFullAccessClaude
            | Key::SpawnFullAccessCodex
            | Key::AutoJoinGeneral
            | Key::TrustWorkspaceMembers
            | Key::AnalyticsEnabled => value.is_boolean(),
        };
        readable.then(|| value.clone())
    }
}

fn one_of(value: &Value, allowed: &[&str]) -> bool {
    value.as_str().is_some_and(|text| allowed.contains(&text))
}

fn in_range(value: &Value, low: u64, high: u64) -> bool {
    value
        .as_u64()
        .is_some_and(|number| (low..=high).contains(&number))
}

/// A list of 0..=200 names, each 1..=200 characters.
fn names(value: &Value) -> bool {
    value.as_array().is_some_and(|entries| {
        entries.len() <= MAX_LIST_ENTRIES
            && entries.iter().all(|entry| {
                entry
                    .as_str()
                    .is_some_and(|text| (1..=MAX_LIST_ENTRY_CHARS).contains(&text.chars().count()))
            })
    })
}

/// `auto_join_repositories`: the `owner/name` and `owner/*` entries this
/// build reads, lower-cased, duplicates collapsed in order. Unreadable
/// entries are dropped one by one; more than 200 readable ones drop the key.
fn repositories(value: &Value) -> Option<Value> {
    let entries = value.as_array()?;
    let mut kept: Vec<String> = Vec::new();
    let mut dropped = 0usize;
    for entry in entries {
        match entry.as_str().and_then(repository) {
            Some(entry) if !kept.contains(&entry) => kept.push(entry),
            Some(_) => {}
            None => dropped += 1,
        }
    }
    if dropped > 0 {
        eprintln!(
            "workspace policy: ignored {dropped} unreadable {} entries",
            Key::AutoJoinRepositories.name()
        );
    }
    (kept.len() <= MAX_LIST_ENTRIES).then(|| Value::from(kept))
}

/// One `auto_join_repositories` entry lower-cased, if it is `owner/name`
/// or `owner/*` with each part `[A-Za-z0-9_.-]{1,100}`.
pub fn repository(entry: &str) -> Option<String> {
    let (owner, name) = entry.split_once('/')?;
    (part(owner) && (name == "*" || part(name))).then(|| entry.to_ascii_lowercase())
}

fn part(text: &str) -> bool {
    (1..=MAX_REPOSITORY_PART).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}
