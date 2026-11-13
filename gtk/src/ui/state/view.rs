//! The top-level screen selected from the latest core state.

use super::*;
use crate::models::SpawnHandover;

/// The spawn-cap field's edit, kept across Settings redraws.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpawnCapEdit {
    /// Unsaved text.
    pub draft: Option<String>,
    /// A save is on its way: one at a time, as on macOS.
    pub saving: bool,
    /// The field had keyboard focus; a redraw gives it back.
    pub focused: bool,
}

impl Section {
    pub const ALL: [Section; 2] = [Section::Mine, Section::Settings];

    pub fn title(self) -> &'static str {
        match self {
            Section::Mine => "Sessions",
            Section::Settings => "Settings",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Section::Mine => "view-list-symbolic",
            Section::Settings => "emblem-system-symbolic",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            Section::Mine => "Sessions",
            Section::Settings => "Settings",
        }
    }

    /// The page the section opens on.
    pub fn home(self) -> Page {
        match self {
            Section::Mine => Page::Sessions,
            Section::Settings => Page::Settings(Pane::General),
        }
    }
}

impl State {
    /// Whether the session's switch can be flipped.
    pub fn can_toggle(&self, session: &LocalSession, presence: Presence) -> bool {
        !SpawnHandover::can_end(Some(session))
            && (presence.is_on()
                || presence.is_changing()
                || (session.live && self.has_workspace()))
    }

    /// What a session's switch does, or why it can't.
    pub fn switch_help(&self, session: &LocalSession, presence: Presence) -> &'static str {
        if SpawnHandover::can_end(Some(session)) {
            "Use End Agent to stop this spawned agent"
        } else if presence.is_on() {
            "Take off the relay"
        } else if !session.live {
            "Start this session to put it on the relay"
        } else if !self.has_workspace() {
            "Connect a relay workspace first"
        } else if session.upload_key().is_none() {
            "Put on the relay to message it by name"
        } else {
            "Put on the relay to message it by name. Sessions on the relay are uploaded."
        }
    }

    /// Retire answers from the previous core while retaining the signed-in
    /// shell and its last useful snapshot until the replacement answers.
    pub fn connection_interrupted(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.reading_requests = false;
        self.refreshing_uploader = false;
        self.refreshing_sessions = false;
        self.uploader_stale = false;
        self.stale_sessions = false;
        self.busy = false;
        self.work_description = None;
        self.requesting.clear();
        self.stopping.clear();
        // Answers to these never arrive from the old core: what was asked
        // for falls back to the last confirmed state.
        self.sharing_mode_wanted.clear();
        self.paused_wanted.clear();
        self.pending_upload_action = None;
        self.pending_settings.clear();
        self.settings = self.confirmed_settings.clone();
        self.busy_uploads.clear();
        self.interrupt_session_queries();
        self.busy_targets.clear();
        for edit in self.session_titles.values_mut() {
            edit.interrupted();
        }
        self.title_write_revision = self.title_write_revision.wrapping_add(1);
        self.loading_title_overviews.clear();
        self.title_overview_reads.clear();
        self.title_overviews.clear();
    }

    /// The screen to show now, and what it is drawn from.
    pub fn view_for(&self) -> (View, ViewKey) {
        if self.snapshot.is_none() {
            // Keep the signed-in shell and its last answers while the core
            // restarts. The connection indicator carries the live state.
            if self.view == Some(View::Connected) {
                return (View::Connected, ViewKey::None);
            }
            return (
                View::Loading,
                ViewKey::Loading {
                    error: self.core_error.clone(),
                },
            );
        }
        if self.signing_out {
            return (View::SigningOut, ViewKey::None);
        }
        if self.account_connected() {
            return (View::Connected, ViewKey::None);
        }
        let account = self.account();
        if account.phase.as_deref() == Some("selecting_workspace") {
            return (View::ChoosingWorkspace, ViewKey::None);
        }
        if State::signing_in(account.state) {
            return (View::SigningIn, ViewKey::None);
        }
        (
            View::SignIn,
            ViewKey::SignIn {
                state: account.state,
                error: self.sign_in_error.clone().or(account.error),
                action: account.action,
                connecting: self.pending_connect.is_some(),
                starting: self.starting_sign_in,
            },
        )
    }

    /// What was shown for the account that just left. Answers to its
    /// refreshes still in flight are dropped when they arrive.
    pub fn forget_account_view(&mut self) {
        self.connection_interrupted();
        self.status = None;
        self.sessions.clear();
        self.session_query.local_sessions = None;
        self.session_query.archived_keys = None;
        self.session_query.cursor = None;
        self.session_query.known_revision = None;
        self.session_query.facets = None;
        self.session_query.facets_loading = false;
        self.session_query.verification_pending = false;
        self.session_query.hidden_confirmed.clear();
        self.session_query.next_cursor = None;
        self.session_query.revision += 1;
        self.sessions_refreshed = None;
        self.upload_errors.clear();
        self.upload_selection_revision += 1;
        self.session_selection.clear();
        self.hiding_sessions.clear();
        self.upload_action_error = None;
        self.unsent_upload_action = None;
        self.session_error = None;
        self.roster.clear();
        self.relay_load_error = None;
        self.local_sessions.clear();
        self.local_agents.clear();
        self.wanted.clear();
        self.relay_errors.clear();
        self.session_titles.clear();
        self.spawn_cap = Default::default();
        self.connect_requested = false;
        self.renew_requested = false;
        self.workspace_connect_error = None;
        // Rebuilding the signed-in shell lands where a fresh launch does.
        self.page = Page::Sessions;
        self.page_error = None;
    }
}

#[path = "session_queries.rs"]
mod session_queries;
pub(super) use session_queries::SessionQueryState;
