//! Pause and resume: the state asked for shows at once, the last click wins
//! while the uploader answers, and a failed final click falls back.
use super::*;

impl App {
    pub fn toggle_pause(&self) {
        let paused = self.with_state(|state| state.is_paused());
        self.set_paused(!paused);
    }

    /// Pauses or resumes messages and uploads. The pause shows at once; the
    /// last click wins while the uploader answers, and a failed final
    /// click falls back to the confirmed state.
    pub fn set_paused(&self, paused: bool) {
        // The operator runs uploads: the person neither pauses nor resumes
        // them, and the core refuses it as well. The Mac's `setPaused`.
        if !self.with_state(|state| {
            state.connected()
                && !state
                    .status
                    .as_ref()
                    .is_some_and(|status| status.uploads_operator_run)
        }) {
            return;
        }
        let driving = self.update_state(|state| {
            let driving = state.paused_wanted.is_pending();
            state.paused_wanted.begin(paused);
            driving
        });
        self.render_pause();
        if !driving {
            self.drive_pause();
        }
    }

    /// Everything that shows whether uploads are paused.
    fn render_pause(&self) {
        self.render_status_bar();
        if self.page() == Page::Sessions {
            self.render_page();
        }
        self.refresh_history_retention();
        self.update_tray();
    }

    /// Sends the pause asked for while it differs from the confirmed one.
    fn drive_pause(&self) {
        let Some((paused, token)) = self.with_state(|state| {
            let wanted = *state.paused_wanted.wanted()?;
            let confirmed = state.status.as_ref().is_some_and(|status| status.paused);
            if wanted == confirmed {
                return None;
            }
            Some((wanted, state.paused_wanted.token()))
        }) else {
            self.update_state(|state| state.paused_wanted.clear());
            self.render_pause();
            return;
        };
        let finished = move |app: &App, error: Option<CoreError>| {
            let again = app.update_state(|state| match &error {
                // A later click still gets its turn; a failed final click
                // falls back to the confirmed state and says why.
                Some(_) => !state.paused_wanted.finish(token),
                None => {
                    if let Some(status) = state.status.as_mut() {
                        status.paused = paused;
                    }
                    !state.paused_wanted.finish(token)
                }
            });
            if let Some(error) = error.filter(|_| !again) {
                app.show_error(&error.message);
            }
            if again {
                app.drive_pause();
            } else {
                app.render_pause();
            }
        };
        let started = if paused {
            self.uploader_command::<agent_relay_desktop_core::contract::method::UploaderPause>(NoParams {}, "Pausing…", finished)
        } else {
            self.uploader_command::<agent_relay_desktop_core::contract::method::UploaderResume>(NoParams {}, "Resuming…", finished)
        };
        if !started {
            self.update_state(|state| state.paused_wanted.clear());
            self.render_pause();
            self.show_error(crate::ui::settings_page::UPLOADER_BUSY);
        }
    }
}
