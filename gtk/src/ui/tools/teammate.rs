//! Teammate context in the provider chooser and in Settings: what
//! the chooser's line shows and saves, and Settings' Turn On and Turn Off.

use super::*;

/// The providers whose sessions Agent Relay's core gives teammate context
/// (desktop-core's `teammate_context::PROVIDERS`), as on the Mac.
pub const TEAMMATE_CONTEXT_PROVIDERS: [&str; 4] = ["claude", "codex", "copilot", "devin"];

impl ToolsUi {
    /// The chooser offers the teammate-context line once a provider it
    /// reaches is here or chosen, as on the Mac.
    pub fn offers_teammate_context(&self) -> bool {
        self.report
            .as_ref()
            .is_some_and(|report| report.teammate_context.is_some())
            || TEAMMATE_CONTEXT_PROVIDERS
                .iter()
                .any(|id| self.selected.contains(*id))
    }

    /// What the chooser's line shows, as on the Mac: the person's edit, else
    /// the saved state, else unchecked where nothing is saved yet: the hook
    /// is off by default.
    pub fn teammate_choice(&self) -> bool {
        self.teammate_draft
            .or_else(|| self.teammate_context())
            .unwrap_or(false)
    }

    /// What Continue saves: only the person's edit, as on the Mac. An
    /// untouched line saves nothing, so the hook stays off by default and a
    /// reopened chooser never undoes Settings.
    pub(super) fn teammate_to_save(&self) -> Option<bool> {
        if !self.offers_teammate_context() {
            return None;
        }
        self.teammate_draft
    }

    /// The teammate-context hook's state, or `None` where Claude Code is
    /// neither on this computer nor chosen.
    pub fn teammate_context(&self) -> Option<bool> {
        self.report.as_ref().and_then(|report| report.teammate_context)
    }
}

impl App {
    /// Settings' Turn On and Turn Off for the teammate-context hook.
    pub fn set_teammate_context(&self, enabled: bool) {
        if !self.start_tools_action(
            agent_relay_desktop_core::contract::method::ToolsTeammateContext::NAME,
        ) {
            return;
        }
        let identity = self.with_state(|state| state.request_identity_revision);
        self.view_call::<agent_relay_desktop_core::contract::method::ToolsTeammateContext>(
            agent_relay_desktop_core::contract::tool_integrations::ToolsTeammateContextParams {
                enabled,
            },
            move |app, result| app.finish_tools_action(result, false, identity),
        );
    }
}
