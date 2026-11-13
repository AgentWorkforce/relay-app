//! Local visibility preferences and the sharing shutdown required by hiding.
use super::app::App;
use crate::models::{
    additional_hidden_key_for_sessions, current_hidden_key, relay_session_for, same_hidden_session,
};
use agent_relay_desktop_core::contract::account::SessionKeysParams;
use gtk4::glib;
use std::{collections::BTreeSet, time::Duration};

mod pending;

const HIDDEN: &str = "hiddenSessions.v1";
const SHOW: &str = "showHiddenSessions.v1";
impl App {
    pub fn hidden_sessions(&self) -> BTreeSet<String> {
        self.prefs()
            .get(HIDDEN)
            .and_then(|value| serde_json::from_value::<Vec<String>>(value.clone()).ok())
            .unwrap_or_default()
            .iter()
            .map(|key| current_hidden_key(key))
            .collect()
    }
    pub(super) fn migrate_hidden_sessions(
        &self,
        sessions: &[agent_relay_node::relay::contract::agents::LocalSession],
    ) {
        let mut stored = self.hidden_sessions();
        let additions: Vec<_> = stored
            .iter()
            .filter_map(|key| additional_hidden_key_for_sessions(key, sessions))
            .filter(|key| !stored.contains(key))
            .collect();
        if additions.is_empty() {
            return;
        }
        stored.extend(additions);
        let _ = self
            .inner
            .prefs
            .borrow_mut()
            .set(HIDDEN, serde_json::json!(stored));
    }
    pub fn session_hidden(&self, key: &str) -> bool {
        self.hidden_sessions().contains(key)
    }
    pub fn show_hidden_sessions(&self) -> bool {
        self.prefs()
            .get(SHOW)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }
    pub fn set_show_hidden_sessions(&self, show: bool) {
        let _ = self.inner.prefs.borrow_mut().set(SHOW, show.into());
        if !show {
            let hidden = self.hidden_sessions();
            self.update_state(|state| state.session_selection.retain(|key| !hidden.contains(key)));
        }
        self.session_query_changed();
        self.refresh_uploader(true);
        self.update_tray();
        self.render_sessions();
    }
    pub(super) fn reconcile_hidden_sessions(&self) {
        let hidden = self.hidden_sessions();
        let keys = self.with_state(|state| {
            hidden
                .iter()
                .filter(|key| {
                    !state.hiding_sessions.contains(*key)
                        && (!state.session_query.hidden_confirmed.contains(*key)
                            || state
                                .sessions
                                .iter()
                                .any(|s| crate::models::session_key(s) == **key && s.included)
                            || relay_session_for(&state.local_sessions, key)
                                .is_some_and(|s| s.registration.is_some()))
                })
                .cloned()
                .collect::<Vec<_>>()
        });
        if !keys.is_empty() {
            self.set_sessions_hidden(keys, true);
        }
    }
    pub fn set_sessions_hidden(&self, keys: Vec<String>, hidden: bool) {
        let keys: BTreeSet<_> = keys
            .into_iter()
            .filter(|key| !self.with_state(|s| s.hiding_sessions.contains(key)))
            .collect();
        let mut stored = self.hidden_sessions();
        let previous = stored.clone();
        if hidden {
            for key in &keys {
                stored.insert(key.clone());
            }
        } else {
            stored.retain(|stored| !keys.iter().any(|key| same_hidden_session(stored, key)));
        }
        if stored != previous
            && self
                .inner
                .prefs
                .borrow_mut()
                .set(HIDDEN, serde_json::json!(stored))
                .is_err()
        {
            return;
        }
        if stored != previous {
            self.update_state(|s| s.session_query.hidden_retry_attempt = 0);
            self.session_query_changed();
            self.refresh_uploader(true);
        }
        self.update_tray();
        if !hidden {
            self.update_state(|s| {
                s.session_query
                    .hidden_confirmed
                    .retain(|stored| !keys.iter().any(|key| same_hidden_session(stored, key)))
            });
            self.render_sessions();
            return;
        }
        let generation = self.with_state(|state| state.generation);
        for key in keys {
            self.update_state(|state| {
                state.hiding_sessions.insert(key.clone());
                state.session_query.hidden_confirmed.remove(&key);
                state.session_selection.remove(&key);
            });
            let session =
                self.with_state(|state| relay_session_for(&state.local_sessions, &key).cloned());
            if let Some(session) = &session {
                self.toggle_relay(session, false);
            }
            self.stop_hidden_session(key, session, generation);
        }
        self.render_sessions();
    }
    fn defer_hiding_session(&self, key: &str) {
        self.update_state(|s| {
            s.hiding_sessions.remove(key);
            s.upload_errors
                .insert(key.into(), "Waiting to stop uploads…".into());
        });
        self.retry_hidden_sessions();
        self.render_sessions();
    }
    fn retry_hidden_sessions(&self) {
        if !self.with_state(|s| {
            s.core_connected
                && s.account_state() == agent_relay_node::relay::state::SignInState::SignedIn
        }) {
            return;
        }
        let request = self.update_state(|s| {
            if s.session_query.hidden_retry_scheduled {
                return None;
            }
            s.session_query.hidden_retry_scheduled = true;
            let delay = 1u64 << s.session_query.hidden_retry_attempt.min(5);
            s.session_query.hidden_retry_attempt =
                (s.session_query.hidden_retry_attempt + 1).min(5);
            Some((s.generation, delay))
        });
        let Some((generation, delay)) = request else {
            return;
        };
        let weak = self.downgrade();
        glib::timeout_add_local_once(Duration::from_secs(delay), move || {
            let Some(app) = weak.upgrade() else {
                return;
            };
            if app.with_state(|s| s.generation) != generation {
                return;
            }
            app.update_state(|s| s.session_query.hidden_retry_scheduled = false);
            if app.with_state(|s| s.core_connected) {
                app.reconcile_hidden_sessions();
            }
        });
    }
    fn finish_hiding_session(&self, key: &str, error: Option<String>) {
        let rolled_back = error.is_some();
        if error.is_some() {
            let mut hidden = self.hidden_sessions();
            hidden.retain(|stored| !same_hidden_session(stored, key));
            let _ = self
                .inner
                .prefs
                .borrow_mut()
                .set(HIDDEN, serde_json::json!(hidden));
        }
        self.update_state(|state| {
            state.hiding_sessions.remove(key);
            if let Some(error) = error {
                state
                    .upload_errors
                    .insert(key.into(), format!("Could not hide session: {error}"));
            } else {
                state.session_query.hidden_confirmed.insert(key.into());
                state.session_query.hidden_retry_attempt = 0;
                state.upload_selection_revision += 1;
                state.upload_errors.remove(key);
                if let Some(session) = state
                    .sessions
                    .iter_mut()
                    .find(|s| crate::models::session_key(s) == key)
                {
                    session.included = false;
                    session.status =
                        agent_relay_desktop_core::contract::account::UploadShareStatus::NotShared;
                }
            }
        });
        if rolled_back {
            self.session_query_changed();
            self.refresh_uploader(true);
        }
        self.update_tray();
        self.render_sessions();
    }
}
