//! The teammate-context hook in the provider chooser and in Settings: what
//! the chooser's line shows and saves, and Settings' Turn On and Turn Off.

use super::*;

impl ToolsUi {
    /// The chooser offers the teammate-context line once Claude Code is here
    /// or chosen, as on the Mac.
    pub fn offers_teammate_context(&self) -> bool {
        self.report
            .as_ref()
            .is_some_and(|report| report.teammate_context.is_some())
            || self.selected.contains("claude")
    }

    /// What the chooser's line shows, as on the Mac: the person's edit, else
    /// the saved state, else checked where nothing is saved yet.
    pub fn teammate_choice(&self) -> bool {
        self.teammate_draft
            .or_else(|| self.teammate_context())
            .unwrap_or(true)
    }

    /// What Continue saves: the person's edit, or the checked default the
    /// line shows where nothing is saved yet. An untouched line that shows
    /// the saved state saves nothing, so a reopened chooser never undoes
    /// Settings.
    pub(super) fn teammate_to_save(&self) -> Option<bool> {
        if !self.offers_teammate_context() {
            return None;
        }
        self.teammate_draft
            .or_else(|| self.teammate_context().is_none().then_some(true))
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
