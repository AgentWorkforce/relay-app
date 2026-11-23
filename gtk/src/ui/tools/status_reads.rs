//! Passive status reads must not interrupt menus or supersede explicit actions.
use super::*;

/// What a failed status read says; unlike an action's error, a later
/// successful read clears it.
const READ_ERROR: &str = "Couldn’t load provider status. Try again.";

impl App {
    pub(super) fn read_tools_status(&self, preserve_error: bool) {
        if !self.bridge().offers(agent_relay_desktop_core::contract::method::ToolsStatus::NAME) {
            return;
        }
        let start = self.update_state(|state| {
            if state.tools.loading || state.tools.busy {
                return false;
            }
            state.tools.loading = true;
            true
        });
        if !start {
            return;
        }
        if self.page() == Page::Settings(Pane::Tools)
            && self.with_state(|state| state.tools.report.is_none())
        {
            self.render_settings();
        }
        let identity = self.with_state(|state| {
            (
                state.request_identity_revision,
                state.tools.operation_revision,
            )
        });
        self.view_call::<agent_relay_desktop_core::contract::method::ToolsStatus>(NoParams {}, move |app, result| {
            if app.with_state(|state| {
                (
                    state.request_identity_revision,
                    state.tools.operation_revision,
                )
            }) != identity
            {
                return;
            }
            let changed = app.update_state(|state| {
                let initial = state.tools.report.is_none();
                let previous = serde_json::to_value(&state.tools.report).ok();
                let old_error = state.tools.error.clone();
                match result {
                    Ok(report) => {
                        // A failed action's error survives passive reads, as
                        // on the Mac; a failed read clears once one succeeds.
                        let error = state
                            .tools
                            .error
                            .clone()
                            .filter(|error| preserve_error && error != READ_ERROR);
                        state.tools.read(report);
                        state.tools.error = error;
                    }
                    Err(_) => {
                        state.tools.loading = false;
                        if !preserve_error || state.tools.error.is_none() {
                            state.tools.error = Some(READ_ERROR.into());
                        }
                    }
                }
                // While an error shows, its Try again button follows the
                // read: a page drawn mid-read showed it disabled.
                initial
                    || previous != serde_json::to_value(&state.tools.report).ok()
                    || old_error != state.tools.error
                    || state.tools.error.is_some()
                    // A chooser opened mid-read was drawn with loading=true.
                    // Re-enable it even when the report itself did not change.
                    || state.tools.needs_onboarding()
            });
            if changed {
                app.tools_changed();
            }
        });
    }
}
