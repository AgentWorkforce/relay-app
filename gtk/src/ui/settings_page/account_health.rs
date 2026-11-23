//! Account profile and authentication health controls.

use super::*;

impl App {
    /// Account health between Profile and Workspace, as on the Mac.
    pub(super) fn account_health_section(&self, column: &gtk::Box) {
        let (section, group) = settings_section("Account health", None);
        let (account, checking, error, core_connected) = self.with_state(|state| {
            (
                state.account(),
                state.checking_account_health,
                state.account_health_error.clone(),
                state.core_connected,
            )
        });
        let (label, detail, _) = crate::ui::state::account_health(&account);
        let health = vbox(12, &[]);
        widgets::pad(&health, 20, None, None);
        health.append(&text(
            if checking {
                "Checking sign-in"
            } else if error.is_some() {
                "Health check failed"
            } else {
                label
            },
            &["settings-row-title"],
        ));
        health.append(
            &text(
                if checking {
                    "Checking your account and workspace access."
                } else {
                    error.as_deref().unwrap_or(&detail)
                },
                &["settings-row-detail"],
            )
            .wrapped(),
        );
        let actions = hbox(12, &[]);
        let check = button(
            if checking {
                "Checking…"
            } else {
                "Check health"
            },
            &[],
            self.on(App::check_account_health),
        );
        check.set_sensitive(core_connected && !checking);
        actions.append(&check);
        if account.auth_health.as_deref() == Some("sign_in_required") {
            let renew = button("Sign in again", &["primary"], self.on(App::renew_sign_in));
            renew.set_sensitive(!self.with_state(|state| state.renew_requested));
            actions.append(&renew);
        }
        health.append(&actions);
        if let Some(error) = self.with_state(|state| state.sign_in_error.clone()) {
            health.append(&text(&error, &["warn"]).wrapped());
        }
        group.append(&health);
        column.append(&section);
    }

    pub fn check_account_health(&self) {
        let start = self.update_state(|state| {
            if !state.core_connected || state.checking_account_health {
                return false;
            }
            state.checking_account_health = true;
            state.account_health_error = None;
            true
        });
        if !start {
            return;
        }
        self.relay_changed();
        self.render_settings();
        let identity = self.with_state(|state| state.request_identity_revision);
        self.call::<agent_relay_desktop_core::contract::method::AccountStatus>(NoParams {}, move |app, result| {
            app.update_state(|state| {
                state.checking_account_health = false;
                if state.request_identity_revision != identity {
                    return;
                }
                match result {
                    Ok(report) => {
                        if let Some(snapshot) = state.snapshot.as_mut() {
                            snapshot.account = report.account;
                        }
                    }
                    Err(error) => state.account_health_error = Some(error.message),
                }
            });
            app.relay_changed();
            app.render_settings();
        });
    }
}
