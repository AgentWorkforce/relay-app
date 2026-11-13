//! What the window shows, as data: the screen the account is on, where the
//! person navigated, and the core's answers the shell and Settings draw.
//! Nothing here touches GTK; [`super::app`] owns one [`State`] on the main
//! thread and redraws from it.

use super::widgets::Color;

mod account_health;
pub use account_health::{account_health, Connection};
mod connection;
mod diagnostics;
mod initialization;
mod pending;
pub mod view;
use crate::models::{
    apply_settings_change, Overview, PendingStatuses, PendingValue, Presence, SessionFilter,
    SessionUploadState, StatusView,
};
use crate::service::ServiceState;
use crate::updates::Release;
use agent_relay_desktop_core::install::InstallKind;
use agent_relay_node::relay::contract::account::SignInLink;
use agent_relay_desktop_core::contract::account::UploadSession;
use agent_relay_node::relay::contract::agents::{
    ArchivedAgent, LocalAgent, LocalSession, SettingsChange, SettingsValues,
};
use agent_relay_node::relay::contract::claude_delivery::ClaudeDeliveryStatus;
use agent_relay_node::relay::people::RosterAgent;
use agent_relay_node::relay::push::PushState;
use agent_relay_node::relay::state::{
    AccountAction, AccountStatus, SignInState, Status, WorkspaceState, WorkspaceStatus,
};
use agent_relay_desktop_core::contract::account::{SharingMode, Stage};
use std::collections::{BTreeSet, HashMap, HashSet};

/// The window's top-level content areas, shared with macOS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Mine,
    Settings,
}

mod panes;
pub use panes::Pane;

/// A signed-in page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Sessions,
    Messages,
    Settings(Pane),
}

impl Page {
    pub fn section(self) -> Section {
        match self {
            Page::Sessions | Page::Messages => Section::Mine,
            Page::Settings(_) => Section::Settings,
        }
    }

    /// The page stack's child for this page.
    pub fn stack_name(self) -> &'static str {
        match self {
            Page::Sessions => "sessions",
            Page::Messages => "messages",
            Page::Settings(_) => "settings",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Page::Sessions => "Sessions",
            Page::Messages => "Messages",
            Page::Settings(pane) => pane.title(),
        }
    }

    pub fn subtitle(self) -> &'static str {
        match self {
            Page::Sessions => "This machine",
            Page::Messages => "Message an agent by name or exact address.",
            Page::Settings(pane) => pane.subtitle(),
        }
    }
}

pub fn session_filter_icon(filter: SessionFilter) -> &'static str {
    match filter {
        SessionFilter::All => "folder-symbolic",
        SessionFilter::Pending => "view-refresh-symbolic",
        SessionFilter::Uploaded => "emblem-ok-symbolic",
        SessionFilter::Local => "drive-harddisk-symbolic",
    }
}

/// The window's screens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// No snapshot yet, or the core went away.
    Loading,
    /// Log out answered or running; the account has not left the snapshot.
    SigningOut,
    SigningIn,
    ChoosingWorkspace,
    SignIn,
    Connected,
}

/// What a screen is drawn from; a screen is redrawn when this changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewKey {
    None,
    Loading {
        error: Option<String>,
    },
    SignIn {
        state: SignInState,
        error: Option<String>,
        action: Option<AccountAction>,
        connecting: bool,
        starting: bool,
    },
}

/// Where a software update stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Available,
    Downloading,
    Installing,
    Unavailable,
    Failed,
}

impl UpdateState {
    pub fn busy(self) -> bool {
        matches!(
            self,
            UpdateState::Checking | UpdateState::Downloading | UpdateState::Installing
        )
    }
}

/// An `agentrelay://connect` link the next sign-in uses. The link is for
/// the site the core was built for (it parsed it), so only the workspace
/// is kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingConnect {
    pub workspace_id: String,
}

/// Service-caller requests (`spawn_callers.list`) and the decision in
/// flight, as on the Mac.
#[derive(Clone, Debug, Default)]
pub struct CallerRequests {
    /// `None` until the first answer.
    pub pending: Option<Vec<agent_relay_desktop_core::contract::service_callers::CallerSummary>>,
    pub loading: bool,
    /// The caller id a decision is being saved for.
    pub deciding: Option<String>,
    pub error: Option<String>,
}

/// Everything the window draws from.
pub struct State {
    pub snapshot: Option<Status>,
    pub core_error: Option<String>,
    /// The core runs another build than this app carries. Only a connection
    /// sets or clears it, so clearing `core_error` never hides it.
    pub stale_core: Option<String>,
    pub core_connected: bool,
    pub tools: super::tools::ToolsUi,
    pub checking_account_health: bool,
    pub account_health_error: Option<String>,
    /// Service-caller requests waiting for the person's decision.
    pub caller_requests: CallerRequests,
    /// The screen shown and what it was drawn from; `None` before the first.
    pub view: Option<View>,
    pub view_key: ViewKey,
    pub launch_recorded: bool,
    pub background_requested: bool,

    // Sign-in.
    pub sign_in_link: Option<SignInLink>,
    pub opened_links: HashSet<String>,
    pub pending_connect: Option<PendingConnect>,
    /// Links that arrive while the core is not connected, or before its
    /// first snapshot, wait for it to parse them; each with how many times
    /// it was already retried after the core could not be reached.
    pub pending_links: Vec<(String, u8)>,
    /// How long startup waits for the core before saying it is taking too
    /// long, and a link held while the core is away waits for it
    /// ([`crate::models::STARTUP_TIMEOUT`]; tests shorten it).
    pub startup_timeout: std::time::Duration,
    /// Why the last connection link failed, for Diagnostics; cleared when a
    /// later link is parsed.
    pub link_error: Option<String>,
    pub sign_in_error: Option<String>,
    /// The site's login configuration once asked for; unavailable while
    /// the answer is on its way.
    pub login_config: Option<agent_relay_desktop_core::contract::account::LoginConfig>,
    /// Each `account.sign_in` is numbered and only the latest one's answer is
    /// applied. From the click until a snapshot shows the account leaving
    /// the state it was clicked in, or the first snapshot after the answer,
    /// no second one starts.
    pub sign_in_requests: u64,
    pub starting_sign_in: bool,
    pub sign_in_from: Option<SignInState>,
    pub sign_in_answered: bool,
    /// From Log out until the core has answered and its snapshot no longer
    /// shows the account.
    pub signing_out: bool,
    pub sign_out_answered: bool,
    pub preview_count: usize,

    // Navigation, as on the Mac.
    pub page: Page,
    pub session_source: Option<String>,
    pub session_search: String,
    pub restored_default_session_view: bool,
    pub page_error: Option<String>,

    // Uploads.
    pub status: Option<StatusView>,
    pub sessions: Vec<UploadSession>,
    pub session_query: view::SessionQueryState,
    /// When `uploader.sessions.list` last answered, epoch milliseconds.
    pub sessions_refreshed: Option<i64>,
    /// Sessions asked to start or stop uploading, by `SOURCE:ID`, while
    /// the change runs.
    pub requesting: BTreeSet<String>,
    pub stopping: BTreeSet<String>,
    pub busy_uploads: BTreeSet<String>,
    pub upload_errors: std::collections::BTreeMap<String, String>,
    pub upload_selection_revision: u64,
    /// The rows selected in My sessions, by `SOURCE:ID`.
    pub session_selection: BTreeSet<String>,
    pub hiding_sessions: BTreeSet<String>,
    /// Why compacting the journal or retrying stopped uploads failed.
    pub upload_action_error: Option<String>,
    /// The upload policy and the pause asked for while the uploader
    /// answers; the last click wins.
    pub sharing_mode_wanted: PendingValue<SharingMode>,
    pub paused_wanted: PendingValue<bool>,
    /// A retry or compact asked for and not yet answered (its stage shows
    /// under way), and one the core never received, with whether to resend.
    pub pending_upload_action: Option<crate::models::StageAction>,
    pub unsent_upload_action: Option<(crate::models::StageAction, bool)>,
    pub upload_details_open: bool,
    pub settings_details_open: HashSet<String>,
    pub diagnostics_copied_at: Option<std::time::Instant>,
    pub request_entries: Vec<agent_relay_node::relay::request_log::RequestEntry>,
    pub request_failure: Option<agent_relay_node::relay::request_log::RequestEntry>,
    pub selected_request: Option<agent_relay_node::relay::request_log::RequestEntry>,
    pub seen_request_failures: HashSet<String>,
    pub relaycast_upgrade: Option<crate::models::RelaycastUpgradeReason>,
    /// Set up in Capy copied its prompt.
    pub capy_prompt_copied: bool,
    pub last_relaycast_upgrade_at: Option<std::time::Instant>,
    pub request_identity_revision: u64,
    pub request_identity_started_at_ms: u64,
    pub request_failures_only: bool,
    pub request_scroll: bool,
    pub reading_requests: bool,
    pub request_log_error: Option<String>,
    pub busy: bool,
    pub work_description: Option<String>,
    pub refreshing_uploader: bool,
    pub refreshing_sessions: bool,
    pub uploader_stale: bool,
    pub stale_sessions: bool,
    pub session_error: Option<String>,

    // The relay: this computer's sessions and agents, and the workspace
    // roster.
    pub local_sessions: Vec<LocalSession>,
    pub local_agents: Vec<LocalAgent>,
    /// Agents that left the relay because their session was archived.
    pub archived_agents: Vec<ArchivedAgent>,
    pub roster: Vec<RosterAgent>,
    pub relay_load_error: Option<String>,
    /// The state last asked for per session (`Target::key`) while a change
    /// runs; the last one asked for wins.
    pub wanted: HashMap<String, bool>,
    pub busy_targets: HashSet<String>,
    pub relay_errors: HashMap<String, String>,
    pub title_overviews: HashMap<String, Overview>,
    pub title_overview_reads: HashMap<String, i64>,
    pub loading_title_overviews: HashSet<String>,
    pub title_write_revision: u64,
    pub session_titles: HashMap<String, crate::models::SessionTitleEdit>,
    /// Each `sessions.list` read is numbered; an older answer arriving after
    /// a newer one is dropped.
    pub local_reads: u64,
    pub local_applied: u64,

    // The workspace and Settings.
    /// An `account.connect_workspace` this view started and has not had
    /// answered; `connecting_workspace` also reads the snapshot.
    pub connect_requested: bool,
    /// An `account.sign_in` renewing the session while this computer stays
    /// connected (the sign-in-again notice's button).
    pub renew_requested: bool,
    /// The core answered that renewal with its approval running, so the
    /// renewal ends at the first snapshot without one, even when a lagging
    /// event stream skipped the `pending_approval` snapshot itself.
    pub renew_answered: bool,
    pub workspace_connect_error: Option<String>,
    pub inbound_error: Option<String>,
    pub grok_leader_error: Option<String>,
    pub opencode_plugin_error: Option<String>,
    /// OpenClaw's row error, and the Agent Relay tools row under it.
    pub openclaw: crate::models::OpenClawView,
    pub trusted_senders_error: Option<String>,
    pub trusted_senders_draft: Option<String>,
    pub hermes_error: Option<String>,
    /// Why the Devin and Cursor CLI hook rows' last changes failed.
    pub hook_errors: crate::models::HookRowErrors,
    pub claude_folders_error: Option<String>,
    /// `None` until `settings.get` answers. What is shown: the confirmed
    /// values with every pending change applied in order.
    pub settings: Option<SettingsValues>,
    /// The core's last answer.
    pub confirmed_settings: Option<SettingsValues>,
    /// `settings.set` calls not yet answered, oldest first, by token.
    pub pending_settings: Vec<(u64, SettingsChange)>,
    /// The latest `settings.set`'s token; only its answer is the truth.
    pub settings_write_token: u64,
    pub settings_error: Option<String>,
    /// Unsaved cap text survives status-driven Settings rebuilds.
    pub spawn_cap: view::SpawnCapEdit,
    /// Starting at login as the system last reported it, and as asked for
    /// while the system answers.
    pub login_item: Option<bool>,
    pub login_item_wanted: PendingValue<bool>,
    /// The shown statuses: the core's last answer under the changes on their way.
    pub claude_delivery: Option<ClaudeDeliveryStatus>,
    pub claude_delivery_pending: PendingStatuses<ClaudeDeliveryStatus>,
    pub claude_delivery_error: Option<String>,
    pub service_state: Option<ServiceState>,
    pub service_error: Option<String>,

    // Updates.
    pub install_kind: InstallKind,
    pub update_state: UpdateState,
    pub update_release: Option<Release>,
    pub update_message: Option<String>,
    pub update_checked: Option<String>,

    /// The account view a call belongs to: an answer that arrives after its
    /// view is gone is dropped.
    pub generation: u64,
    pub poll_ticks: u32,
}

impl State {
    pub fn account_state(&self) -> SignInState {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.account.state)
            .unwrap_or_default()
    }

    pub fn workspace(&self) -> WorkspaceStatus {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.workspace.clone())
            .unwrap_or_default()
    }

    pub fn has_workspace(&self) -> bool {
        self.workspace().state == WorkspaceState::Found
    }

    pub fn workspace_name(&self) -> String {
        let account = self.account();
        account
            .workspace_name
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Workspace".into())
    }

    /// A sign-in runs: approval in the browser, or preparing.
    pub fn signing_in(state: SignInState) -> bool {
        matches!(state, SignInState::PendingApproval | SignInState::Preparing)
    }

    /// The snapshot shows the account approving a change in the browser (a
    /// workspace connection): it keeps its person, which a new sign-in
    /// starts without.
    pub fn approving_change(&self) -> bool {
        let account = self.account();
        account.state == SignInState::PendingApproval && account.person.is_some()
    }

    /// A workspace connection runs: asked for here and not yet answered, or
    /// its approval shows in the snapshot.
    pub fn connecting_workspace(&self) -> bool {
        self.connect_requested || (self.approving_change() && !self.renew_requested)
    }

    /// A renewal of the session the snapshot shows ended: its approval is
    /// over. Answers whether it did.
    pub fn settle_renewal(&mut self, before: Option<SignInState>) -> bool {
        let began = before == Some(SignInState::PendingApproval) || self.renew_answered;
        if !self.renew_requested || !began || self.account_state() == SignInState::PendingApproval {
            return false;
        }
        self.renew_requested = false;
        self.renew_answered = false;
        true
    }

    /// After a new snapshot: whether a sign-in just finished, which opens
    /// Sessions. A renewal approved in the browser keeps the page it began on.
    pub fn sign_in_landed(&mut self, before: Option<SignInState>) -> bool {
        !self.settle_renewal(before)
            && before.is_some_and(State::signing_in)
            && self.account_state() == SignInState::SignedIn
    }

    /// Whether the snapshot shows an account the connected view is for:
    /// signed in, preparing its sessions, or, from the account's own view,
    /// approving a change while keeping its person (a workspace
    /// connection). A new sign-in starts with no person.
    pub fn account_connected(&self) -> bool {
        let account = self.account();
        match account.state {
            SignInState::SignedIn => true,
            SignInState::Preparing
                if account.phase.as_deref() == Some("preparing_local_sessions") =>
            {
                true
            }
            _ => {
                self.approving_change()
                    && matches!(self.view, Some(View::Connected | View::SigningOut))
            }
        }
    }

    /// While signing out: whether the snapshot still shows the account being
    /// signed out. Sign-out starts from the account's own view, so any
    /// attempt the snapshot shows (a workspace connection's approval,
    /// preparing) is that account's.
    pub fn account_present(&self) -> bool {
        let state = self.account_state();
        state == SignInState::SignedIn || State::signing_in(state)
    }

    pub fn connected(&self) -> bool {
        self.view == Some(View::Connected)
    }

    // What the relay looks like, for the sidebar, banners and status bar.

    /// Paused as asked for, until the uploader answers.
    pub fn is_paused(&self) -> bool {
        self.paused_wanted
            .shown(self.status.as_ref().is_some_and(|status| status.paused))
    }

    /// The full indexed history, independent of loaded rows and search filters.
    pub fn local_session_count_text(&self) -> String {
        match &self.status {
            Some(status)
                if !status.upgrading
                    && self.account().phase.as_deref() != Some("preparing_local_sessions") =>
            {
                let count = status.total_sessions;
                format!(
                    "{count} {} on this machine",
                    if count == 1 { "session" } else { "sessions" }
                )
            }
            _ => "Counting sessions…".into(),
        }
    }

    pub fn upload_status_text(&self) -> String {
        if !self.connected() {
            return if self.snapshot.is_none() {
                "Connecting…".into()
            } else {
                "Not connected".into()
            };
        }
        if self.account().phase.as_deref() == Some("preparing_local_sessions") {
            return "Preparing uploads".into();
        }
        let Some(status) = &self.status else {
            return "Checking uploads".into();
        };
        if self.is_paused() {
            return "Uploads paused".into();
        }
        if status.stage == Stage::UpToDate {
            "Uploads up to date".into()
        } else {
            status.headline.clone()
        }
    }

    pub fn upload_status_color(&self) -> Color {
        let Some(status) = self.status.as_ref().filter(|_| self.connected()) else {
            return Color::Grey;
        };
        match status.stage {
            Stage::UpToDate => Color::Green,
            Stage::Uploading | Stage::Preparing => Color::Accent,
            Stage::NeedsAttention | Stage::Conflict | Stage::Offline => Color::Orange,
            _ => Color::Grey,
        }
    }

    pub fn update_available(&self) -> bool {
        self.update_state == UpdateState::Available
    }

    /// The person may delete their sessions from Cloud. When the operator
    /// withdraws it every delete control is removed; the core refuses the
    /// deletion as well.
    pub fn can_delete_from_cloud(&self) -> bool {
        self.status
            .as_ref()
            .is_none_or(|status| status.can_delete_from_cloud)
    }

    /// Whether the session list can be changed now.
    pub fn sessions_read_only(&self) -> bool {
        self.busy
            || self.status.as_ref().is_none_or(|status| {
                status.upgrading
                    || status.sharing_mode == agent_relay_desktop_core::contract::account::SharingMode::All
            })
            || self.account_state() != SignInState::SignedIn
    }

    // Relay state.

    /// A session's presence: the state asked for wins while it runs; the
    /// snapshot's push channel, else the registration's, says whether
    /// messages push in.
    pub fn presence_of(&self, session: &LocalSession) -> Presence {
        let key = session.target.key();
        let registration = session.registration.as_ref();
        // A busy target with nothing asked for is switching to the other
        // state; a provisional registration (no address yet) is on its
        // way on.
        let wanted = self.wanted.get(&key).copied().or_else(|| {
            self.busy_targets
                .contains(&key)
                .then_some(registration.is_none_or(|registration| registration.address.is_empty()))
        });
        let push = registration.and_then(|registration| {
            self.snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.push.get(&registration.name))
                .or(registration.push.as_ref())
                .map(|state| *state == PushState::Live)
        });
        Presence::of(registration.is_some(), wanted, session.live, push)
    }

    /// The agent on the relay a session is.
    pub fn agent_for(&self, session: &LocalSession) -> Option<&LocalAgent> {
        let key = session.target.key();
        self.local_agents
            .iter()
            .find(|agent| agent.target.key() == key)
    }

    pub fn session_count(&self, filter: SessionFilter) -> usize {
        self.sessions
            .iter()
            .filter(|session| filter.matches(SessionUploadState::of(session, self.status.as_ref())))
            .count()
    }

    /// The history sources the session list holds, sorted.
    pub fn session_sources(&self) -> Vec<String> {
        self.sessions
            .iter()
            .map(|session| session.source.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_core;
    use agent_relay_node::relay::contract::{method, reader};
    use serde_json::json;

    fn status(value: serde_json::Value) -> Status {
        reader::result::<method::StatusGet>(value).unwrap()
    }

    fn state_with(snapshot: serde_json::Value) -> State {
        let mut state = State::new(InstallKind::Source);
        state.snapshot = Some(status(snapshot));
        state
    }

    fn found(mut snapshot: serde_json::Value) -> serde_json::Value {
        snapshot["workspace"]["state"] = json!("found");
        snapshot["workspace"]["workspace_name"] = json!("Acme");
        snapshot
    }

    #[test]
    fn pi_setup_error_is_visible_after_connecting_and_old_cores_still_work() {
        let mut snapshot = fake_core::signed_in();
        assert_eq!(state_with(snapshot.clone()).relay_problem(), None);
        let error =
            "Pi extension could not be installed: a different extension uses agent-relay.js.";
        snapshot["agents"]["integration_error"] = json!(error);
        assert_eq!(state_with(snapshot).relay_problem().as_deref(), Some(error));
    }

    #[test]
    fn workspace_header_names_the_upload_destination_not_another_relay() {
        let mut snapshot = fake_core::signed_in();
        snapshot["account"]["workspace_name"] = json!("Upload workspace");
        snapshot["account"]["workspace_id"] = json!("0f8fad5b-d9cb-469f-a165-70867728950e");
        snapshot["workspace"]["workspace_name"] = json!("Another relay");
        assert_eq!(
            state_with(snapshot.clone()).workspace_name(),
            "Upload workspace"
        );
        snapshot["account"]["workspace_name"] = json!(null);
        assert_eq!(state_with(snapshot).workspace_name(), "Workspace");
    }

    #[test]
    fn the_screen_follows_the_snapshot() {
        let mut state = State::new(InstallKind::Source);
        assert_eq!(
            state.view_for(),
            (View::Loading, ViewKey::Loading { error: None })
        );
        state.snapshot = Some(status(fake_core::snapshot(json!({}))));
        assert_eq!(state.view_for().0, View::SignIn);
        state.snapshot = Some(status(fake_core::snapshot(
            json!({"state": "pending_approval", "phase": "waiting_for_browser"}),
        )));
        assert_eq!(state.view_for().0, View::SigningIn);
        state.snapshot = Some(status(fake_core::signed_in()));
        assert_eq!(state.view_for().0, View::Connected);
        state.view = Some(View::Connected);
        state.snapshot = None;
        assert_eq!(state.view_for().0, View::Connected);
        state.snapshot = Some(status(fake_core::signed_in()));
        state.signing_out = true;
        assert_eq!(state.view_for().0, View::SigningOut);
    }

    #[test]
    fn a_workspace_approval_keeps_the_account_view_only_from_it() {
        let mut approving = fake_core::signed_in();
        approving["account"]["state"] = json!("pending_approval");
        let mut state = state_with(approving);
        assert!(state.approving_change());
        assert_eq!(state.view_for().0, View::SigningIn);
        state.view = Some(View::Connected);
        assert_eq!(state.view_for().0, View::Connected);
        assert!(state.connecting_workspace());
    }

    #[test]
    fn a_renewed_sign_in_is_no_workspace_connection_and_settles_once_approved() {
        let mut approving = fake_core::signed_in();
        approving["account"]["state"] = json!("pending_approval");
        let mut state = state_with(approving);
        state.view = Some(View::Connected);
        state.renew_requested = true;
        assert_eq!(state.view_for().0, View::Connected);
        assert!(!state.connecting_workspace());
        // Still waiting for the browser: nothing settles.
        assert!(!state.settle_renewal(Some(SignInState::PendingApproval)));
        state.snapshot = Some(status(fake_core::signed_in()));
        assert!(state.settle_renewal(Some(SignInState::PendingApproval)));
        assert!(!state.renew_requested);
        assert!(!state.settle_renewal(Some(SignInState::PendingApproval)));
    }

    #[test]
    fn an_answered_renewal_ends_without_its_approval_snapshot() {
        // The event stream lagged: the approval's snapshot was replaced by
        // the one after it, and the renewal failed.
        let mut state = state_with(fake_core::signed_in());
        state.renew_requested = true;
        assert!(!state.settle_renewal(Some(SignInState::SignedIn)));
        state.renew_answered = true;
        assert!(state.settle_renewal(Some(SignInState::SignedIn)));
        assert!(!state.renew_requested);
        assert!(!state.renew_answered);
    }

    #[test]
    fn the_connection_says_what_stops_the_relay_first() {
        let mut state = state_with(found(fake_core::signed_in()));
        state.view = Some(View::Connected);
        state.snapshot.as_mut().unwrap().account.auth_health = Some("ready".into());
        assert_eq!(state.connection().label, "Connected");
        assert!(state.connection().healthy);
        state.core_error = Some("Agent Relay stopped.".into());
        let connection = state.connection();
        assert_eq!(
            (connection.label, connection.help.as_str()),
            ("Needs attention", "Agent Relay stopped.")
        );
        state.snapshot = None;
        state.core_error = None;
        state.launch_recorded = true;
        let connection = state.connection();
        assert_eq!(connection.label, "Reconnecting");
        assert_eq!(
            connection.help,
            "Agent Relay is reconnecting to its background service."
        );
        let mut missing = state_with(fake_core::signed_in());
        missing.view = Some(View::Connected);
        assert_eq!(missing.connection().label, "No relay workspace");
    }

    #[test]
    fn upload_status_reads_as_the_status_bar_says_it() {
        let mut state = State::new(InstallKind::Source);
        assert_eq!(state.upload_status_text(), "Connecting…");
        state.snapshot = Some(status(fake_core::signed_in()));
        assert_eq!(state.upload_status_text(), "Not connected");
        state.view = Some(View::Connected);
        assert_eq!(state.upload_status_text(), "Checking uploads");
        assert_eq!(state.upload_status_color(), Color::Grey);
    }

    fn local(value: serde_json::Value) -> State {
        let mut state = state_with(found(fake_core::signed_in()));
        state.view = Some(View::Connected);
        state.local_sessions = reader::result::<method::SessionsList>(json!({ "sessions": value }))
            .unwrap()
            .sessions;
        state
    }

    fn session(
        kind: &str,
        id: &str,
        live: bool,
        registration: serde_json::Value,
    ) -> serde_json::Value {
        json!({"target": {"kind": kind, "id": id}, "title": id, "project": null, "directory": null,
               "live": live, "updated_at_ms": null, "registration": registration,
               "suggested_name": id})
    }

    #[test]
    fn a_switch_says_what_it_does_or_why_it_cannot() {
        let registered = json!({"name": "web", "address": "web@desk", "added_by_session": false,
                                "push": null});
        let spawned = json!({"name": "worker", "address": "worker@desk", "added_by_session": false,
                             "push": null,
                             "spawn": {"harness": "claude", "can_take_over": true, "taken_over": false}});
        let state = local(json!([
            session("codex", "a", true, serde_json::Value::Null),
            session("claude-cloud", "b", true, serde_json::Value::Null),
            session("codex", "c", false, serde_json::Value::Null),
            session("codex", "d", false, registered),
            session("claude-terminal", "e", true, spawned),
        ]));
        let help: Vec<&str> = state
            .local_sessions
            .iter()
            .map(|session| state.switch_help(session, state.presence_of(session)))
            .collect();
        assert_eq!(
            help,
            [
                "Put on the relay to message it by name. Sessions on the relay are uploaded.",
                "Put on the relay to message it by name",
                "Start this session to put it on the relay",
                "Take off the relay",
                "Use End Agent to stop this spawned agent",
            ]
        );
        let spawned = state.local_sessions.last().unwrap();
        assert!(!state.can_toggle(spawned, state.presence_of(spawned)));
    }

    #[test]
    fn leaving_the_account_forgets_what_its_pages_held() {
        let mut state = local(json!([session(
            "codex",
            "a",
            true,
            serde_json::Value::Null
        )]));
        assert_eq!(state.page, Page::Sessions);
        state.page = Page::Settings(Pane::About);
        state.hiding_sessions.insert("codex:a".into());
        state
            .session_query
            .hidden_confirmed
            .insert("codex:a".into());
        state.session_query.hidden_retry_scheduled = true;
        state.session_query.hidden_retry_attempt = 2;
        state.requesting.insert("codex:a".into());
        state.reading_requests = true;
        state.spawn_cap.draft = Some("12".into());
        let edit = state.session_titles.entry("codex:a".into()).or_default();
        edit.supported = Some(true);
        edit.begin("Private title draft");
        state.title_overviews.insert(
            "codex:a".into(),
            Overview {
                title: "Private generated title".into(),
                ..Default::default()
            },
        );
        state.title_overview_reads.insert("codex:a".into(), 123);
        state.loading_title_overviews.insert("codex:a".into());
        state.forget_account_view();
        assert_eq!(state.page, Page::Sessions);
        assert!(state.hiding_sessions.is_empty());
        assert!(state.session_query.hidden_confirmed.is_empty());
        assert!(!state.session_query.hidden_retry_scheduled);
        assert_eq!(state.session_query.hidden_retry_attempt, 0);
        assert!(state.session_titles.is_empty());
        assert!(state.title_overviews.is_empty());
        assert!(state.title_overview_reads.is_empty());
        assert!(state.loading_title_overviews.is_empty());
        assert!(state.requesting.is_empty());
        assert!(!state.reading_requests);
        assert_eq!(state.spawn_cap, Default::default());
    }
}
