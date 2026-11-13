//! The core's listings every surface shares: the uploader's status and
//! sessions (the status bar, the sessions table), and this computer's
//! sessions, agents and the workspace roster (the tray and the sessions
//! table). Pages draw them; this reads them and switches sessions on and off
//! the relay.

use super::app::App;
use super::state::{Page, Pane, Section};
use crate::bridge::CoreError;
use crate::models::session_key;
use agent_relay_node::relay::contract::agents::{
    LimitParams, LocalSession, NameParams, RegisterParams, SessionRegistration,
};
use agent_relay_node::relay::contract::directory::RosterParams;
use agent_relay_node::relay::contract::{method, Method, NoParams};
use agent_relay_node::relay::protocol::ErrorKind;
use agent_relay_node::relay::store::Target;
use gtk4::prelude::WidgetExt;

mod activity_clock;
mod facets;
mod pause;
mod receive;
mod registration;
mod session_queries;
mod session_reads;

impl App {
    pub fn refresh_request_log(&self) {
        if !self.bridge().offers(method::DiagnosticsRequests::NAME) {
            self.update_state(|state| state.request_log_error = Some("Request log unavailable. The background service may need to be updated or restarted.".into()));
            return;
        }
        if self.update_state(|state| {
            if state.reading_requests {
                return true;
            }
            state.reading_requests = true;
            false
        }) {
            return;
        }
        let identity_revision = self.with_state(|state| state.request_identity_revision);
        self.view_call::<method::DiagnosticsRequests>(NoParams {}, move |app, result| {
            app.update_state(|state| {
                state.reading_requests = false;
                match result {
                    Ok(log) => {
                        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
                        let plan = state.snapshot.as_ref().and_then(|snapshot| snapshot.workspace.workspace_plan.as_deref());
                        let reason = crate::models::relaycast_upgrade_from_log(&log.entries, plan, state.request_identity_started_at_ms, now_ms, &state.seen_request_failures);
                        if let Some(reason) = reason.filter(|_| identity_revision == state.request_identity_revision && state.last_relaycast_upgrade_at.is_none_or(|at| at.elapsed().as_secs() >= 900)) {
                            state.relaycast_upgrade = Some(reason);
                            state.last_relaycast_upgrade_at = Some(std::time::Instant::now());
                        }
                        if let Some(newest) = log.entries.iter().find(|entry| entry.failed && !state.seen_request_failures.contains(&entry.id)) {
                            state.request_failure = Some(newest.clone());
                        }
                        state.seen_request_failures.extend(log.entries.iter().filter(|entry| entry.failed).map(|entry| entry.id.clone()));
                        if state.seen_request_failures.len() > 1000 {
                            state.seen_request_failures = log.entries.iter().filter(|entry| entry.failed).map(|entry| entry.id.clone()).collect();
                        }
                        state.request_entries = log.entries;
                        state.request_log_error = None;
                    }
                    Err(_) => state.request_log_error = Some("Request log unavailable. The background service may need to be updated or restarted.".into()),
                }
            });
            app.render_status_bar();
            if app.page() == Page::Settings(Pane::Diagnostics) { app.render_settings(); }
        });
    }

    pub fn open_failed_request(&self) {
        self.update_state(|state| {
            state.selected_request = state.request_failure.take();
            state.request_failures_only = false;
            state.request_scroll = true;
        });
        self.go(Page::Settings(Pane::Diagnostics));
    }

    // Uploads.

    /// Reads the uploader's status, and its sessions when `sessions` (or
    /// when none were read yet). A change announced mid-refresh is read once
    /// this one ends.
    pub fn refresh_uploader(&self, sessions: bool) {
        if !self.with_state(|state| state.connected())
            || !self.bridge().offers(agent_relay_desktop_core::contract::method::UploaderStatus::NAME)
        {
            return;
        }
        self.refresh_upload_status();
        if sessions || self.with_state(|state| state.sessions_refreshed.is_none()) {
            self.refresh_upload_sessions();
        }
    }

    fn refresh_upload_status(&self) {
        let deferred = self.update_state(|state| {
            if state.busy || state.refreshing_uploader {
                state.uploader_stale = true;
                return true;
            }
            state.uploader_stale = false;
            state.refreshing_uploader = true;
            false
        });
        if deferred {
            return;
        }
        let generation = self.with_state(|state| state.generation);
        let current = move |app: &App| {
            app.with_state(|state| state.generation == generation && state.connected())
        };
        self.call::<agent_relay_desktop_core::contract::method::UploaderStatus>(NoParams {}, move |app, result| {
            if app.with_state(|state| state.generation) != generation {
                return;
            }
            app.update_state(|state| state.refreshing_uploader = false);
            if !current(app) {
                return;
            }
            match result {
                Ok(status) => app.upload_status_answered(&status),
                Err(error) => app.refresh_failed(&error),
            }
            app.refresh_if_stale();
        });
    }

    fn refresh_if_stale(&self) {
        let (status, sessions) = self.with_state(|state| {
            (
                state.connected()
                    && !state.busy
                    && state.uploader_stale
                    && !state.refreshing_uploader,
                state.connected()
                    && !state.busy
                    && state.stale_sessions
                    && !state.refreshing_sessions,
            )
        });
        if status {
            self.refresh_upload_status();
        }
        if sessions {
            self.refresh_upload_sessions();
        }
    }

    fn refresh_failed(&self, error: &CoreError) {
        if error.kind() == Some(ErrorKind::NotSignedIn) {
            return;
        }
        self.update_state(|state| {
            state.session_error = Some(
                "Could not refresh statuses just now. Agent Relay will try again automatically."
                    .into(),
            )
        });
        if self.page() == Page::Sessions {
            self.render_page();
        }
    }

    pub(super) fn update_uploader_views(&self) {
        if !self.with_state(|state| state.connected()) {
            return;
        }
        let busy = self.update_state(|state| {
            if !state.busy {
                state.session_error = None;
            }
            state.busy
        });
        match self.page() {
            Page::Sessions => self.render_page(),
            Page::Settings(pane) if !busy => self.render_uploader_settings(pane),
            _ => {}
        }
        self.render_status_bar();
        if self.page().section() == Section::Mine {
            self.render_sidebar();
        }
        self.update_tray();
    }

    /// Runs one uploader change at a time; `finished` gets its failure.
    /// Returns whether the change started: it does not while another runs.
    pub(super) fn uploader_command<M>(
        &self,
        params: M::Params,
        working: &str,
        finished: impl FnOnce(&App, Option<CoreError>) + 'static,
    ) -> bool
    where
        M: Method + 'static,
        M::Params: Send + 'static,
        M::Result: Send + 'static,
    {
        if self.with_state(|state| state.busy) {
            return false;
        }
        self.update_state(|state| {
            state.busy = true;
            state.work_description = Some(working.to_string()).filter(|text| !text.is_empty());
        });
        if self.page() == Page::Sessions {
            self.render_page();
        }
        self.view_call::<M>(params, move |app, result| {
            let error = result.err();
            app.update_state(|state| {
                state.busy = false;
                state.work_description = None;
                state.session_error = error.as_ref().map(|error| error.message.clone());
            });
            finished(app, error);
            app.refresh_uploader(true);
        });
        true
    }

    // The relay.

    pub fn refresh_relay(&self) {
        self.refresh_local();
        self.refresh_agents();
        self.refresh_roster();
    }

    /// A reveal asks for discovery without changing the selected page.
    /// The core answers from cache, coalesces work, then invalidates new rows.
    pub(super) fn prepare_window_reveal(&self) {
        self.update_state(|state| state.background_requested = false);
        if !self.window().is_visible() {
            self.refresh_local();
        }
    }

    pub fn refresh_local(&self) {
        // The spinner shows a read under way; a core without
        // `sessions.list` starts none.
        if !self.bridge().offers(method::SessionsList::NAME) {
            return;
        }
        let read = self.update_state(|state| {
            state.local_reads += 1;
            state.local_reads
        });
        let title_revision = self.with_state(|state| state.title_write_revision);
        // Asked again mid-read, it reads again as a newer read.
        self.once::<method::SessionsList>(
            "local",
            NoParams {},
            App::refresh_local,
            move |app, result| {
                match result {
                    Err(error) => {
                        app.update_state(|state| state.relay_load_error = Some(error.message))
                    }
                    Ok(listed) => {
                        app.update_state(|state| state.relay_load_error = None);
                        app.apply_local_after_title_write(read, title_revision, listed.sessions);
                        app.update_state(|state| state.settle());
                    }
                }
                app.relay_changed();
            },
        );
    }

    pub fn refresh_agents(&self) {
        self.once::<method::AgentsList>(
            "agents",
            NoParams {},
            App::refresh_agents,
            |app, result| {
                if let Ok(listed) = result {
                    app.update_state(|state| {
                        state.local_agents = listed.agents;
                        state.archived_agents = listed.archived;
                    });
                    app.relay_changed();
                }
            },
        );
    }

    /// The workspace roster, which says which sessions' agents are idle;
    /// signed out, it reads as empty.
    pub fn refresh_roster(&self) {
        let params = RosterParams {
            where_runs: None,
            include_idle: false,
            q: None,
        };
        self.once::<method::RosterList>("roster", params, App::refresh_roster, |app, result| {
            match result {
                // Any other failure keeps the last roster.
                Err(error) if error.kind() != Some(ErrorKind::NotSignedIn) => return,
                result => app.update_state(|state| {
                    state.roster = result.map(|roster| roster.agents).unwrap_or_default();
                }),
            }
            app.relay_changed();
        });
    }

    /// New relay data: every surface that shows it.
    pub(super) fn relay_changed(&self) {
        if !self.with_state(|state| state.connected()) {
            return;
        }
        self.refresh_presentation_query();
        if matches!(self.page(), Page::Sessions | Page::Settings(Pane::Relay)) {
            self.render_page();
        }
        self.render_chrome();
        self.update_tray();
        self.schedule_activity_check();
    }

    /// Puts `session` on the relay or takes it off. The switch shows the new
    /// state at once; one change per session runs at a time, and the last
    /// state asked for wins.
    pub fn toggle_relay(&self, session: &LocalSession, on: bool) {
        if on && self.session_hidden(&session.upload_key().unwrap_or_else(|| session.target.key()))
        {
            return;
        }
        let key = session.target.key();
        let running = self.update_state(|state| {
            state.wanted.insert(key.clone(), on);
            state.relay_errors.remove(&key);
            !state.busy_targets.insert(key.clone())
        });
        self.relay_changed();
        if !running {
            self.drive(key, session.target.clone());
        }
    }
}
