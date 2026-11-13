//! Changes to the app's core and workspace connections.

use super::*;

impl App {
    pub(super) fn connect_link(&self, link: PendingConnect) {
        let (account, signing_out) = self.with_state(|state| (state.account(), state.signing_out));
        if account.state == SignInState::SignedIn {
            let current = account.workspace_id.unwrap_or_default().to_lowercase();
            if !current.is_empty() && current != link.workspace_id.to_lowercase() {
                self.record(
                    TelemetryOperation::ConnectionLink,
                    TelemetryOutcome::Failed,
                    Some(TelemetryFailure::WorkspaceConflict),
                );
                // Going to Account clears the page's error, so the error
                // follows it.
                self.go(Page::Settings(Pane::Account));
                self.show_error("This app is connected to another account or workspace. Log out in Account before connecting a different workspace.");
            } else {
                self.record(
                    TelemetryOperation::ConnectionLink,
                    TelemetryOutcome::Succeeded,
                    None,
                );
                self.update_state(|state| state.page_error = None);
                self.render_page();
                self.show_window();
            }
            return;
        }
        if State::signing_in(account.state) || signing_out {
            self.show_error(
                "Finish or cancel the current operation before opening another connection link.",
            );
            return;
        }
        self.record(
            TelemetryOperation::ConnectionLink,
            TelemetryOutcome::Succeeded,
            None,
        );
        self.update_state(|state| {
            state.pending_connect = Some(link);
            state.sign_in_error = None;
        });
        self.render();
        self.show_window();
    }

    pub(super) fn forget_account_view(&self) {
        self.update_state(|state| {
            state.forget_account_view();
            state.claude_delivery = None;
            state.claude_delivery_pending.clear();
            state.claude_delivery_error = None;
        });
        self.inner.in_flight.borrow_mut().clear();
        self.inner.rerun.borrow_mut().clear();
        self.inner.claude_delivery_changes.set(0);
        if let Some(shell) = self.inner.shell.borrow_mut().take() {
            shell.pages.messages.close_all();
        }
    }

    /// The event connection came up or went down.
    pub fn on_connection(&self, connected: bool, error: Option<String>) {
        let claude_interrupted = !connected && self.inner.claude_delivery_changes.replace(0) > 0;
        // A core still running another build, as a manual install can leave
        // behind, is said, as on macOS.
        let stale = connected.then(|| self.inner.bridge.stale_core()).flatten();
        let (changed, resumed) = self.update_state(|state| {
            let resumed = connected && !state.core_connected && state.view == Some(View::Connected);
            state.core_connected = connected;
            if connected {
                state.core_error = None;
                state.stale_core = stale;
                return (false, resumed);
            }
            state.connection_interrupted();
            if claude_interrupted {
                state.claude_delivery_error = Some(CoreError::interrupted().message);
                state.claude_delivery_pending.interrupted();
                state.sync_claude_delivery();
            }
            state.core_error = error;
            // Keep the last account and workspace in the connected shell.
            // Other screens must await a fresh snapshot to settle sign-in.
            if state.view != Some(View::Connected) {
                state.snapshot = None;
            }
            // The core that took a sign-in or answered a sign-out is gone;
            // the next one's snapshot says where the account stands.
            state.starting_sign_in = false;
            if state.sign_out_answered {
                state.signing_out = false;
            }
            (true, false)
        });
        if changed {
            self.inner.in_flight.borrow_mut().clear();
            self.inner.rerun.borrow_mut().clear();
        }
        if resumed {
            self.refresh_all();
            self.preload_session_titles();
        }
        if connected {
            // A signed-in window keeps its snapshot across a reconnect, so
            // links held while the core was away go now.
            self.replay_links();
        }
        if changed {
            self.render();
            if self.with_state(State::connected) {
                self.render_chrome();
                self.refresh_session_title_cells();
                if self.page() == Page::Settings(Pane::Relay) {
                    self.render_settings();
                }
            }
        }
    }
}

/// Whether `url` is a web page: only those go to the browser.
pub(in crate::ui) fn web_link(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}
