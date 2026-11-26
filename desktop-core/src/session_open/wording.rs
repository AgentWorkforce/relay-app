//! Every word the Open button shows, once, so macOS and GTK say the same
//! thing. Each `match` is exhaustive: a new core [`Reason`], terminal or
//! app does not compile until it has its code and sentence here.
use session_navigation::model::{App, Precision, Reason, TerminalKind};

use super::contract::Strings;

/// Reasons this layer adds before or around the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Local {
    InvalidRequest,
    UnsupportedSource,
    UnknownTerminal,
    AlreadyOpening,
    DesktopIdUnmapped,
    SeveralLocations,
    HomeUnavailable,
}

impl std::fmt::Display for Local {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(local(*self).1)
    }
}

impl std::error::Error for Local {}

/// A stable code and one sentence.
pub type Words = (&'static str, &'static str);

/// Said when nothing was started; the apps used it before this engine.
pub const NOTHING_STARTED: &str = "No new session was started.";

pub fn local(reason: Local) -> Words {
    match reason {
        Local::InvalidRequest => (
            "invalid_request",
            "Agent Relay could not read this request to open the session. No new session was started.",
        ),
        Local::UnsupportedSource => (
            "unsupported_source",
            "Opening sessions from this provider is not supported. No new session was started.",
        ),
        Local::UnknownTerminal => (
            "unknown_terminal",
            "The terminal chosen in Settings is not one Agent Relay can open sessions in. Choose another terminal.",
        ),
        Local::AlreadyOpening => ("already_opening", "This session is already being opened."),
        Local::DesktopIdUnmapped => (
            "desktop_id_unmapped",
            "This desktop session could not be matched to its provider session. No new session was started.",
        ),
        Local::SeveralLocations => (
            "several_locations",
            "This session is open in more than one place. Choose where to open it.",
        ),
        Local::HomeUnavailable => (
            "home_unavailable",
            "Agent Relay could not find your home folder, so it cannot look for this session. No new session was started.",
        ),
    }
}

/// The code and sentence for each core reason. The outer `match` is
/// exhaustive (a new reason must be placed in a group to compile); each
/// group then words its own reasons, and `every_reason_has_its_own_words`
/// checks no reason reaches a group's fallback.
pub fn reason(reason: Reason) -> Words {
    match reason {
        Reason::InvalidSessionId
        | Reason::EvidenceIncomplete
        | Reason::SessionNotFound
        | Reason::Archived
        | Reason::ChildSession
        | Reason::AmbiguousIdentity
        | Reason::PriorIdentityOnly
        | Reason::DesktopRecordMissing
        | Reason::OtherAccount
        | Reason::SelectionGone => identity(reason),
        Reason::CwdUnknown
        | Reason::CwdMissing
        | Reason::CwdConflict
        | Reason::AppNotInstalled
        | Reason::ToolNotInstalled
        | Reason::SchemeHandlerMismatch
        | Reason::UnverifiedVersion
        | Reason::ServerVersionMismatch
        | Reason::EnvironmentUnknown
        | Reason::ForeignRuntime
        | Reason::NoDefaultTerminal => prerequisite(reason),
        Reason::LaunchPending
        | Reason::LiveNotLocatable
        | Reason::RunningHeadless
        | Reason::PossiblyOpenElsewhere
        | Reason::NoSessionDeepLink
        | Reason::SessionBusy
        | Reason::RevivalCollides
        | Reason::PossiblySecondView
        | Reason::WindowAmbiguous
        | Reason::PaneSelectionUnavailable => ownership(reason),
        Reason::PermissionDenied
        | Reason::TakeOverRaced
        | Reason::ReservationStoreFailed
        | Reason::TargetGone
        | Reason::WrongDestination
        | Reason::DispatchFailed
        | Reason::PlatformUnsupported
        | Reason::HostUnsupported
        | Reason::HostSettingDisabled
        | Reason::PermissionRequired
        | Reason::WaylandNoRaise => host(reason),
    }
}

/// Never reached for a reason of the group (see [`reason`]).
const UNLISTED: Words = (
    "unlisted",
    "Opening this session is unavailable. No new session was started.",
);

/// Which session, and whether it can be named at all.
fn identity(reason: Reason) -> Words {
    match reason {
        Reason::InvalidSessionId => ("invalid_session_id", "This session id is not valid for its provider. No new session was started."),
        Reason::EvidenceIncomplete => ("evidence_incomplete", "Local process or provider ownership could not be fully inspected. The session may still be running; no new session was started."),
        Reason::SessionNotFound => ("session_not_found", "This session was not found in its provider’s records. No new session was started."),
        Reason::Archived => ("archived", "This session is archived, and reopening it from Agent Relay is unavailable. No new session was started."),
        Reason::ChildSession => ("child_session", "This is a subagent’s session, which cannot be reopened on its own. No new session was started."),
        Reason::AmbiguousIdentity => ("ambiguous_identity", "More than one record claims this session, so Agent Relay cannot tell which one to open. No new session was started."),
        Reason::PriorIdentityOnly => ("prior_identity_only", "This session is only recorded as an earlier conversation of another session. No new session was started."),
        Reason::DesktopRecordMissing => ("desktop_record_missing", "The desktop application’s record of this session is missing. No new session was started."),
        Reason::OtherAccount => ("other_account", "This session belongs to another account in the desktop application. No new session was started."),
        Reason::SelectionGone => ("selection_gone", "This session has moved or ended. Refresh and choose its current location."),
        _ => UNLISTED,
    }
}

/// What a launch needs and did not find.
fn prerequisite(reason: Reason) -> Words {
    match reason {
        Reason::CwdUnknown => ("cwd_unknown", "This session’s working directory is unknown, so it cannot be resumed safely. No new session was started."),
        Reason::CwdMissing => ("cwd_missing", "This session’s working directory no longer exists. No new session was started."),
        Reason::CwdConflict => ("cwd_conflict", "This session’s records disagree about its working directory. No new session was started."),
        Reason::AppNotInstalled => ("app_not_installed", "The application needed to open this session is not installed. No new session was started."),
        Reason::ToolNotInstalled => ("tool_not_installed", "The provider’s command-line tool is not installed, so this session cannot be resumed. No new session was started."),
        Reason::SchemeHandlerMismatch => ("scheme_handler_mismatch", "Another application handles this provider’s session links, so the link was not sent. No new session was started."),
        Reason::UnverifiedVersion => ("unverified_version", "This version of the provider has not been verified for opening sessions from Agent Relay. No new session was started."),
        Reason::ServerVersionMismatch => ("server_version_mismatch", "The running OpenCode server is a different version from the OpenCode command-line tool. No new session was started."),
        Reason::EnvironmentUnknown => ("environment_unknown", "The provider storage this session lives in is not known, so resuming could open the wrong store. No new session was started."),
        Reason::ForeignRuntime => ("foreign_runtime", "This session runs in an environment Agent Relay cannot open from this machine. No new session was started."),
        Reason::NoDefaultTerminal => ("no_default_terminal", "No default terminal is set. Choose one in Settings, then try again."),
        _ => UNLISTED,
    }
}

/// Where the session already runs, or may.
fn ownership(reason: Reason) -> Words {
    match reason {
        Reason::LaunchPending => ("launch_pending", "A resume request was already sent. The provider has not confirmed it opened. Check the earlier launch before trying again."),
        Reason::LiveNotLocatable => ("live_not_locatable", "This session is running, but its exact window cannot be located. No new session was started."),
        Reason::RunningHeadless => ("running_headless", "This session has a running background owner without a navigable window. No new session was started."),
        Reason::PossiblyOpenElsewhere => ("possibly_open_elsewhere", "This session may already be open in another process. Confirm it is not open there before resuming it here."),
        Reason::NoSessionDeepLink => ("no_session_deep_link", "The session is running in its desktop application, but its provider-owned session link is unavailable. No new session was started."),
        Reason::SessionBusy => ("session_busy", "The provider reports a turn running in this session right now. Try again when it finishes."),
        Reason::RevivalCollides => ("revival_collides", "Reopening this desktop session would resume a conversation that is running elsewhere. No new session was started."),
        Reason::PossiblySecondView => ("possibly_second_view", "This may open a second view of a session that is already open in a window Agent Relay cannot locate."),
        Reason::WindowAmbiguous => ("window_ambiguous", "Several windows could hold this session, and Agent Relay cannot tell which one. Switch to it yourself."),
        Reason::PaneSelectionUnavailable => ("pane_selection_unavailable", "The original application can be shown, but exact pane focus is unavailable. No new session was started."),
        _ => UNLISTED,
    }
}

/// What the terminal, app or platform allowed.
fn host(reason: Reason) -> Words {
    match reason {
        Reason::PermissionDenied => ("permission_denied", "Agent Relay was not allowed to control that application. No new session was started."),
        Reason::TakeOverRaced => ("take_over_raced", "The background process did not stop cleanly, so the session was not resumed."),
        Reason::ReservationStoreFailed => ("reservation_store_failed", "Couldn’t read or save pending session launches. Resume is blocked."),
        Reason::TargetGone => ("target_gone", "This session’s process has ended or changed. Refresh and try again."),
        Reason::WrongDestination => ("wrong_destination", "The application did not show this session. No alternative session was started."),
        Reason::DispatchFailed => ("dispatch_failed", "The request could not be sent to the application. No new session was started."),
        Reason::PlatformUnsupported => ("platform_unsupported", "Opening this session is not supported on this platform. No new session was started."),
        Reason::HostUnsupported => ("host_unsupported", "Agent Relay cannot open sessions in this terminal. No new session was started."),
        Reason::HostSettingDisabled => ("host_setting_disabled", "The terminal’s remote control is turned off. Turn it on in the terminal’s settings to open this session from Agent Relay."),
        Reason::PermissionRequired => ("permission_required", "Agent Relay needs permission to control this terminal. Allow it in System Settings, then try again."),
        Reason::WaylandNoRaise => ("wayland_no_raise", "This desktop does not let Agent Relay bring another window forward on Wayland. No new session was started."),
        _ => UNLISTED,
    }
}

pub fn terminal_name(terminal: TerminalKind) -> &'static str {
    match terminal {
        TerminalKind::Ghostty => "Ghostty",
        TerminalKind::AppleTerminal => "Terminal",
        TerminalKind::ITerm2 => "iTerm2",
        TerminalKind::Kitty => "Kitty",
        TerminalKind::WezTerm => "WezTerm",
        TerminalKind::Alacritty => "Alacritty",
        TerminalKind::GnomeTerminal => "GNOME Terminal",
        TerminalKind::Konsole => "Konsole",
        TerminalKind::Xterm => "XTerm",
        TerminalKind::WindowsTerminal => "Windows Terminal",
        TerminalKind::ConsoleHost => "Console",
    }
}

/// The id [`super::contract::Request::default_terminal`] uses.
pub fn terminal_id(terminal: TerminalKind) -> &'static str {
    match terminal {
        TerminalKind::Ghostty => "ghostty",
        TerminalKind::AppleTerminal => "apple_terminal",
        TerminalKind::ITerm2 => "iterm2",
        TerminalKind::Kitty => "kitty",
        TerminalKind::WezTerm => "wezterm",
        TerminalKind::Alacritty => "alacritty",
        TerminalKind::GnomeTerminal => "gnome_terminal",
        TerminalKind::Konsole => "konsole",
        TerminalKind::Xterm => "xterm",
        TerminalKind::WindowsTerminal => "windows_terminal",
        TerminalKind::ConsoleHost => "console_host",
    }
}

pub fn app_name(app: App) -> &'static str {
    match app {
        App::ClaudeDesktop => "Claude",
        App::CodexDesktop => "Codex",
        App::OpenCodeDesktop => "OpenCode",
        App::Ghostty
        | App::AppleTerminal
        | App::ITerm2
        | App::Kitty
        | App::WezTerm
        | App::Alacritty
        | App::GnomeTerminal
        | App::Konsole
        | App::Xterm
        | App::WindowsTerminal
        | App::ConsoleHost => app.terminal().map_or("the application", terminal_name),
    }
}

pub fn precision(precision: Precision) -> &'static str {
    match precision {
        Precision::ExactConversation => "Opened this session.",
        Precision::RightWindow => {
            "Brought this session’s window forward. It may show another conversation now."
        }
        Precision::AppOnly => {
            "Brought the application forward. Exact pane selection is unavailable."
        }
    }
}

/// Said for a dispatch nothing could confirm, by what was dispatched.
pub mod unverified {
    pub const FOCUS: &str =
        "Asked the terminal to select this session. The selection could not be confirmed.";
    pub const SHOW: &str =
        "Showed the application. Exact pane selection is unavailable for this terminal.";
    pub const LINK: &str = "Sent the exact session link to its application. The application does not confirm session selection.";
    pub const RESUME: &str =
        "Sent the resume request for this session. The provider has not confirmed it opened.";
    pub const ATTACH: &str =
        "Opened a terminal window attached to this session’s tmux pane. It could not be confirmed.";
}

pub mod titles {
    pub const CHOOSE: &str = "Open running session";
    pub const UNAVAILABLE: &str = "Session opening unavailable";
}

/// The fixed words around the button ([`Strings`]): in every answer, and
/// printed by `session-open strings` for the apps' no-answer fallback.
pub fn strings() -> Strings {
    let s = str::to_owned;
    Strings {
        open_session: s("Open session"),
        checking: s("Checking session location…"),
        opening: s("Opening session…"),
        resume_default: s("Resume in default terminal"),
        resolve_launch: s("Resolve earlier launch…"),
        launch_question: s("Did you close or stop the earlier launch?"),
        launch_detail: s("The earlier terminal or application may still be starting the provider. Close or stop that launch before allowing another resume request."),
        launch_closed: s("I closed or stopped it"),
        launch_wait: s("Keep waiting"),
        cancel: s("Cancel"),
        take_over_detail: s("Agent Relay will stop the background process running this session, check that it exited, then resume the session here."),
        helper_missing: s("This copy of Agent Relay is missing its session helper, so it cannot open sessions. Reinstall Agent Relay."),
        rejected: s("The session helper could not read this request. Update Agent Relay."),
        plan_failed: s("Couldn’t check this session’s location. Try again."),
        open_unknown: s("Agent Relay did not get an answer, so it is unknown whether the session opened. Check before trying again."),
        open_not_started: s("Agent Relay could not start opening this session. No new session was started."),
        already_opening: s(local(Local::AlreadyOpening).1),
    }
}
