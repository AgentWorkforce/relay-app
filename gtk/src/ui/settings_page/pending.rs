//! Settings controls whose change shows before the core answers: the
//! upload policy, launch at login, and the switch rows' pending spinner.
use super::*;

/// A switch row whose change may be on its way to the core: the switch
/// holds the position asked for, with a spinner beside it, until the
/// core answers.
pub(in crate::ui) fn pending_toggle_row(
    title: &str,
    detail: &str,
    icon_name: &str,
    active: bool,
    sensitive: bool,
    pending: bool,
    on: impl Fn(bool) + 'static,
) -> gtk::Box {
    let switch = gtk::Switch::new();
    crate::ui::widgets::action_cursor(&switch);
    switch.set_active(active);
    switch.set_sensitive(sensitive);
    widgets::name(&switch, title);
    switch.connect_active_notify(move |switch| on(switch.is_active()));
    settings_row(
        title,
        detail,
        icon_name,
        Some(&switch_accessory(&switch, pending)),
        &[],
    )
    .0
}

/// A switch with, while `pending`, the spinner of its change under way.
pub(in crate::ui) fn switch_accessory(switch: &gtk::Switch, pending: bool) -> gtk::Widget {
    let holder = hbox(8, &[]);
    holder.set_valign(gtk::Align::Start);
    if pending {
        let spinner = gtk::Spinner::new();
        spinner.set_valign(gtk::Align::Center);
        widgets::name(&spinner, "Saving…");
        spinner.start();
        holder.append(&spinner);
    }
    switch.set_valign(gtk::Align::Center);
    holder.append(switch);
    holder.upcast()
}

impl App {
    /// Asks, then changes which sessions upload. The policy asked for shows
    /// at once; the last choice wins while the uploader answers, and a
    /// failed final choice falls back to the last confirmed one.
    pub fn change_sharing(&self, mode: SharingMode) {
        let current = self.with_state(|state| state.sharing_mode());
        if current.is_none_or(|current| current == mode) {
            return;
        }
        let answer = self.confirm(Confirmation::new(
            mode.confirmation_title(),
            &format!(
                "{}\n\nChanging this rebuilds the upload queue and can take a few minutes. Previously uploaded data is not deleted.",
                mode.explanation()
            ),
            mode.confirmation_action(),
        ));
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let (true, Some(app)) = (answer.await, weak.upgrade()) else {
                return;
            };
            let driving = app.update_state(|state| {
                state.settings_error = None;
                let driving = state.sharing_mode_wanted.is_pending();
                state.sharing_mode_wanted.begin(mode);
                driving
            });
            app.render_settings();
            if !driving {
                app.drive_sharing();
            }
        });
    }

    /// Sends the policy asked for while it differs from the confirmed one:
    /// one change at a time, the last one asked for wins.
    fn drive_sharing(&self) {
        let Some((mode, token)) = self.with_state(|state| {
            let wanted = *state.sharing_mode_wanted.wanted()?;
            let confirmed = state.status.as_ref()?.sharing_mode;
            if wanted == confirmed {
                return None;
            }
            Some((wanted, state.sharing_mode_wanted.token()))
        }) else {
            self.update_state(|state| state.sharing_mode_wanted.clear());
            self.render_settings();
            return;
        };
        let started = self.uploader_command::<agent_relay_desktop_core::contract::method::UploaderSharingSet>(
            SharingParams { mode },
            "Changing the upload policy…",
            move |app, error| {
                let again = app.update_state(|state| match error {
                    Some(error) => {
                        // A later choice still gets its turn; a failed final
                        // choice falls back to the confirmed policy.
                        if state.sharing_mode_wanted.finish(token) {
                            state.settings_error = Some(error.message);
                            false
                        } else {
                            true
                        }
                    }
                    None => {
                        if let Some(status) = state.status.as_mut() {
                            status.sharing_mode = mode;
                        }
                        !state.sharing_mode_wanted.finish(token)
                    }
                });
                if again {
                    app.drive_sharing();
                } else {
                    app.render_settings();
                }
            },
        );
        if !started {
            self.update_state(|state| {
                state.sharing_mode_wanted.clear();
                state.settings_error = Some(UPLOADER_BUSY.into());
            });
            self.render_settings();
        }
    }

    /// One startup choice owns both the tray app and the upload service.
    pub fn change_autostart(&self, enabled: bool) {
        if self.inner.updating.get() {
            return;
        }
        self.startup_choice_made();
        let driving = self.update_state(|state| {
            state.settings_error = None;
            let driving = state.login_item_wanted.is_pending();
            state.login_item_wanted.begin(enabled);
            driving
        });
        self.render_settings();
        if !driving {
            self.drive_autostart();
        }
    }

    fn drive_autostart(&self) {
        let Some((enabled, token)) = self.with_state(|state| {
            Some((
                *state.login_item_wanted.wanted()?,
                state.login_item_wanted.token(),
            ))
        }) else {
            return;
        };
        let service = self.inner.service.clone();
        self.blocking(move || {
            match service.startup_registration() {
                Ok(Some(previous)) => (Some(previous), service.set_start_at_login(enabled)),
                // Without a service (e.g. a non-systemd Linux desktop), the
                // GUI login entry starts the app's hosted upload core.
                Ok(None) => (None, Ok(())),
                Err(error) => (None, Err(error)),
            }
        }, move |app, (previous, result)| {
            if let Err(error) = result {
                app.finish_autostart(enabled, token, Err(error));
                return;
            }
            app.call::<agent_relay_desktop_core::contract::method::LoginItemSet>(LoginItemParams { enabled }, move |app, result| {
                if let Err(error) = result {
                    let service = app.inner.service.clone();
                    app.blocking(move || previous.map_or(Ok(()), |previous| service.set_start_at_login(previous)), move |app, rollback| {
                        let mut message = error.message;
                        if let Err(error) = rollback {
                            message.push_str(&format!(" The service login setting could not be restored: {error}"));
                        }
                        app.finish_autostart(enabled, token, Err(message));
                    });
                } else {
                    app.finish_autostart(enabled, token, Ok(()));
                }
            });
        });
    }

    fn finish_autostart(&self, enabled: bool, token: u64, result: Result<(), String>) {
        self.record(
            TelemetryOperation::LaunchAtLogin,
            if result.is_ok() {
                TelemetryOutcome::Succeeded
            } else {
                TelemetryOutcome::Failed
            },
            result.is_err().then_some(TelemetryFailure::SystemSetting),
        );
        let latest = self.update_state(|state| {
            if result.is_ok() {
                state.login_item = Some(enabled);
                if let Some(service) = state.service_state.as_mut().filter(|s| s.installed) {
                    service.enabled = enabled;
                }
            }
            let latest = state.login_item_wanted.finish(token);
            if latest {
                state.settings_error = result.err().map(|error| {
                    format!("Agent Relay could not change the login setting: {error}")
                });
            }
            latest
        });
        if latest {
            self.refresh_service();
            self.settings_changed();
        } else {
            self.drive_autostart();
        }
    }
}
