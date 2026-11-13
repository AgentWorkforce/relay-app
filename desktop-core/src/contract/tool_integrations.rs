//! Per-machine, explicitly selected hosted MCP integrations.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
pub struct ToolsReport {
    pub onboarding_complete: bool,
    pub account_health: Option<String>,
    pub tools: Vec<ToolIntegration>,
    /// The teammate-context hook: a Claude Code SessionStart hook telling each
    /// session to check teammates' session summaries before it starts. Absent
    /// where Claude Code is neither on this computer nor chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teammate_context: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct ToolIntegration {
    pub id: String,
    pub name: String,
    pub detected: bool,
    /// Local installation witness for initial onboarding only. Absence is not
    /// availability; saved choices and Cloud connections remain independent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub onboarding_available: Option<bool>,
    pub supported: bool,
    pub selected: bool,
    pub state: String,
    pub status_label: String,
    pub detail: String,
    pub can_install: bool,
    pub can_repair: bool,
    pub can_uninstall: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct ToolsSelectParams {
    pub tool_ids: Vec<String>,
    /// The chooser's teammate-context choice, saved before the providers so
    /// the setup they start already follows it. Absent leaves it as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teammate_context: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct ToolsTeammateContextParams {
    pub enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
pub struct ToolsManageParams {
    pub tool_id: String,
    pub action: String,
}
