//! Local process observations. These are navigation hints, never delivery authority.
use agent_relay_node::relay::contract::{agents::SessionTitleIdentity, Method};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Provider-owned paths only; never persist tokens or a full process environment.
pub const PROVIDER_ENVIRONMENT_KEYS: &[&str] = &[
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "CODEX_SQLITE_HOME",
    "GROK_HOME",
];

/// Where a session is running, for the desktop's Open button.
pub struct SessionsLocation;

impl Method for SessionsLocation {
    const NAME: &'static str = "sessions.location";
    type Params = SessionTitleIdentity;
    type Result = SessionLocationReport;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
pub struct SessionLocation {
    pub application: String,
    pub bundle_id: String,
    pub pid: i32,
    pub process_start: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tty: Option<String>,
    pub observed_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_executable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_pid: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_process_start: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_identity: Option<String>,
    /// Observed effective provider store paths. An empty map proves no overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_environment: Option<BTreeMap<String, String>>,
    /// False means the observed process environment was unreadable or its paths
    /// could not be resolved. Absent is an older, unobserved record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_environment_known: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus_capability: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_capability: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionLocationReport {
    /// False when local process observations are unavailable. Host capabilities
    /// determine whether an exact pane can be focused.
    pub supported: bool,
    /// A verified owner exists even if its UI cannot be located.
    pub running: bool,
    /// Every verified location, including simultaneous owners. Never pick one arbitrarily.
    pub current: Vec<SessionLocation>,
    /// Last observed locations survive process exit and core restarts. Never focus these.
    pub previous: Vec<SessionLocation>,
    /// A provider-owned desktop identity mapped from this CLI session.
    pub desktop_link: Option<String>,
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original: Option<SessionLocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<SessionLocation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_state: Option<String>,
}

/// Revalidated on every request, including a selection made in a previous response.
pub struct SessionsOpenPlan;
impl Method for SessionsOpenPlan {
    const NAME: &'static str = "sessions.open_plan";
    type Params = SessionOpenRequest;
    type Result = SessionOpenPlan;
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpenRequest {
    pub source: String,
    pub session_id: String,
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default)]
    pub selected: Option<SessionLocation>,
    #[serde(default)]
    pub prefer_terminal: bool,
    #[serde(default)]
    pub use_default_terminal: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionOpenAction {
    Focus,
    ShowApplication,
    OpenLink,
    Resume,
    Choose,
    Unavailable,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionResumeCommand {
    pub executable: String,
    pub args: Vec<String>,
    pub cwd: String,
    /// Some(map), including empty, clears PROVIDER_ENVIRONMENT_KEYS before
    /// applying these paths. None retains legacy current-configuration behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<BTreeMap<String, String>>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionOpenPlan {
    pub action: SessionOpenAction,
    pub location: Option<SessionLocation>,
    pub url: Option<String>,
    pub command: Option<SessionResumeCommand>,
    pub choices: Vec<SessionLocation>,
    pub reason: Option<String>,
    #[serde(default)]
    pub can_resume_default: bool,
}
