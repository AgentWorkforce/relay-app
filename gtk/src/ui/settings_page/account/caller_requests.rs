//! Service-caller requests waiting for the person's decision
//! (`spawn_callers.list` and `spawn_callers.decide`). The wording matches
//! `CallerRequestsSection` in the Mac's `SettingsView.swift`.

use super::*;
use gtk::glib;
use agent_relay_desktop_core::contract::method::{SpawnCallersDecide, SpawnCallersList};
use agent_relay_desktop_core::contract::service_callers::{
    CallerDecideParams, CallerListParams, CallerState, CallerSummary,
};

pub(crate) const TITLE: &str = "Caller requests";
pub(crate) const DESCRIPTION: &str =
    "Apps that ask to start agents on this computer wait here until you approve or deny them.";
pub(crate) const LOADING: &str = "Checking for requests…";
pub(crate) const EMPTY: &str = "No caller is waiting for a decision.";
pub(crate) const SIGNED_OUT: &str = "Sign in to see caller requests.";
pub(crate) const UNATTESTED: &str =
    "This build of Agent Relay can't confirm it is the signed app, \
     so caller requests can't be listed or decided here.";

/// What a caller asks for, in the person's words.
pub(crate) fn caller_request_detail(caller: &CallerSummary) -> String {
    let harnesses = if caller.harnesses.is_empty() {
        "no".to_string()
    } else {
        caller.harnesses.join(", ")
    };
    let roots = if caller.directory_roots.is_empty() {
        "no folder".to_string()
    } else {
        caller.directory_roots.join(", ")
    };
    match caller.state {
        CallerState::PendingRotation => {
            format!("Asks for a new secret to keep starting {harnesses} agents in {roots}.")
        }
        _ => format!("Wants to start {harnesses} agents in {roots}."),
    }
}

/// A core error, in the person's words.
pub(crate) fn caller_request_error(message: &str) -> String {
    match message {
        "app_attestation_unavailable" => UNATTESTED.to_string(),
        "not_signed_in" => SIGNED_OUT.to_string(),
        other => format!("Agent Relay couldn't complete that ({other})."),
    }
}

fn waiting(caller: &CallerSummary) -> bool {
    matches!(
        caller.state,
        CallerState::PendingProvision | CallerState::PendingRotation
    )
}

impl App {
    /// The requests below Account health, as on the Mac.
    pub(super) fn caller_requests_section(&self, column: &gtk::Box) {
        let (section, group) = settings_section(TITLE, Some(DESCRIPTION));
        let (signed_in, core_connected, requests) = self.with_state(|state| {
            (
                state.connected(),
                state.core_connected,
                state.caller_requests.clone(),
            )
        });
        let body = vbox(12, &[]);
        widgets::pad(&body, 20, None, None);
        if !signed_in {
            // A later sign-in, perhaps as someone else, loads afresh.
            self.update_state(|state| state.caller_requests = Default::default());
            body.append(&text(SIGNED_OUT, &["settings-row-detail"]).wrapped());
            group.append(&body);
            column.append(&section);
            return;
        }
        if requests.pending.is_none() && !requests.loading && requests.error.is_none() {
            // The first draw asks once, after this render.
            let weak = self.downgrade();
            glib::idle_add_local_once(move || {
                if let Some(app) = weak.upgrade() {
                    app.load_caller_requests();
                }
            });
        }
        let waiting: Vec<CallerSummary> = requests
            .pending
            .clone()
            .unwrap_or_default()
            .into_iter()
            .filter(waiting)
            .collect();
        if requests.loading && requests.pending.is_none() {
            body.append(&text(LOADING, &["settings-row-detail"]));
        } else if waiting.is_empty() && requests.error.is_none() {
            body.append(&text(EMPTY, &["settings-row-detail"]));
        }
        for caller in waiting {
            let row = vbox(6, &[]);
            row.append(&text(&caller.name, &["settings-row-title"]));
            row.append(&text(&caller_request_detail(&caller), &["settings-row-detail"]).wrapped());
            let actions = hbox(12, &[]);
            let busy = requests.deciding.is_some() || !core_connected;
            for (label, approved, classes) in [
                ("Approve", true, &["primary"][..]),
                ("Deny", false, &[][..]),
            ] {
                let id = caller.caller_id.clone();
                let weak = self.downgrade();
                let action = button(label, classes, move || {
                    if let Some(app) = weak.upgrade() {
                        app.decide_caller_request(&id, approved);
                    }
                });
                action.set_sensitive(!busy);
                actions.append(&action);
            }
            row.append(&actions);
            body.append(&row);
        }
        if let Some(error) = &requests.error {
            body.append(&text(error, &["warn"]).wrapped());
        }
        let refresh = button("Refresh", &[], self.on(App::load_caller_requests));
        refresh.set_sensitive(core_connected && !requests.loading);
        body.append(&refresh);
        group.append(&body);
        column.append(&section);
    }

    pub fn load_caller_requests(&self) {
        let start = self.update_state(|state| {
            if !state.core_connected || state.caller_requests.loading {
                return false;
            }
            state.caller_requests.loading = true;
            state.caller_requests.error = None;
            true
        });
        if !start {
            return;
        }
        let identity = self.with_state(|state| state.request_identity_revision);
        self.call::<SpawnCallersList>(
            CallerListParams {
                caller_secret: None,
            },
            move |app, result| {
                app.update_state(|state| {
                    state.caller_requests.loading = false;
                    if state.request_identity_revision != identity {
                        return;
                    }
                    match result {
                        Ok(list) => state.caller_requests.pending = Some(list.callers),
                        Err(error) => {
                            state.caller_requests.pending = Some(Vec::new());
                            state.caller_requests.error =
                                Some(caller_request_error(&error.message));
                        }
                    }
                });
                app.render_settings();
            },
        );
        self.render_settings();
    }

    pub fn decide_caller_request(&self, caller_id: &str, approved: bool) {
        let start = self.update_state(|state| {
            if !state.core_connected || state.caller_requests.deciding.is_some() {
                return false;
            }
            state.caller_requests.deciding = Some(caller_id.to_string());
            state.caller_requests.error = None;
            true
        });
        if !start {
            return;
        }
        let identity = self.with_state(|state| state.request_identity_revision);
        self.call::<SpawnCallersDecide>(
            CallerDecideParams {
                caller_id: caller_id.to_string(),
                approved,
                caller_secret: None,
            },
            move |app, result| {
                let decided = app.update_state(|state| {
                    state.caller_requests.deciding = None;
                    if state.request_identity_revision != identity {
                        return false;
                    }
                    match result {
                        Ok(_) => true,
                        Err(error) => {
                            state.caller_requests.error =
                                Some(caller_request_error(&error.message));
                            false
                        }
                    }
                });
                if decided {
                    app.load_caller_requests();
                } else {
                    app.render_settings();
                }
            },
        );
        self.render_settings();
    }
}
