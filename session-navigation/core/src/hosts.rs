//! Terminal hosts: what an adapter can do with each terminal on this machine
//! (reported, never assumed from the name), the panes, multiplexer panes and
//! OS windows it can see, and how a pane is addressed.
//!
//! A pane is always located from evidence: on Unix by its tty and the kernel
//! process table; on Windows by process id + creation time and the window
//! the console host or terminal owns. A pane id, window handle or socket is
//! never trusted on its own: every [`PaneRef`] that can be recycled carries
//! the identity it must still belong to, and an executor must re-check it
//! before acting.
use serde::{Deserialize, Serialize};

use crate::evidence::Fact;
use crate::model::{ProcessIdentity, TerminalKind};

/// An OS window: an X11 window id or a Win32 `HWND`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WindowId(pub u64);

/// How one pane (or window) is addressed.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PaneRef {
    /// Found by its tty through the terminal's scripting (macOS Ghostty,
    /// Terminal, iTerm2: `every terminal whose tty is …`).
    Tty { tty: String },
    /// A Kitty window (Kitty's name for a pane), through `kitty @ --to
    /// <socket>`; needs `allow_remote_control` and `listen_on`.
    KittyWindow { socket: String, id: u64 },
    /// A WezTerm pane through `wezterm cli` on the mux socket.
    WezTermPane { socket: String, id: u64 },
    /// An X11 top-level window, raised as a whole. `owner` is the process
    /// that owned it when observed; a reused id belongs to someone else.
    X11Window {
        id: WindowId,
        owner: ProcessIdentity,
    },
    /// A Win32 top-level window, raised as a whole. `owner` as above
    /// (process id + creation time).
    WindowsWindow {
        hwnd: WindowId,
        owner: ProcessIdentity,
    },
    /// A tmux pane `%N` on the server at `socket` whose server process is
    /// `server` (a restarted server recycles pane ids). It is shown through
    /// the attached client on `client_tty`, whose own pane in the outer
    /// terminal is `outer`.
    TmuxPane {
        socket: String,
        server: ProcessIdentity,
        pane: String,
        client_tty: String,
        outer: Box<PaneRef>,
    },
}

impl PaneRef {
    /// Whether focusing this ref selects one pane (rather than raising a
    /// window that may show another tab).
    pub fn is_window(&self) -> bool {
        matches!(self, Self::X11Window { .. } | Self::WindowsWindow { .. })
    }
}

/// Whether the adapter can do one thing with one terminal right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ability {
    Available,
    /// Supported, but a setting is off.
    NeedsSetting(Setting),
    /// Supported, but needs a permission the person has not granted.
    NeedsPermission(Permission),
    Unsupported(Unsupported),
}

impl Ability {
    pub fn available(self) -> bool {
        self == Self::Available
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Setting {
    /// Kitty `allow_remote_control` plus a `listen_on` socket.
    KittyRemoteControl,
    /// WezTerm's mux socket (`WEZTERM_UNIX_SOCKET`).
    WezTermMuxSocket,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Permission {
    /// macOS Automation (Apple Events) for the terminal.
    MacAutomation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unsupported {
    /// The terminal has no pane API (Alacritty, Windows Terminal: its
    /// session GUID is not a pane-selection API).
    NoPaneApi,
    /// Wayland exposes no generic, verified window activation.
    Wayland,
    /// The terminal does not exist on this platform.
    NotOnThisPlatform,
    /// This adapter does not implement it (yet).
    NotImplemented,
}

/// What the adapter can do with one terminal on this machine. A terminal
/// missing from the list supports nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSupport {
    pub terminal: TerminalKind,
    /// Locate panes with their tty or process and focus one exactly
    /// ([`PaneRef::Tty`], [`PaneRef::KittyWindow`], [`PaneRef::WezTermPane`]).
    pub focus_pane: Ability,
    /// Raise one OS window found in [`Surfaces::windows`].
    pub raise_window: Ability,
    /// Bring the application forward without choosing a window.
    pub activate_app: Ability,
    /// Open a new window that runs an argv in a directory.
    pub launch: Ability,
    /// A raised window may show another tab than the session's (Terminal.app,
    /// Windows Terminal, GNOME Terminal, Konsole tabs), so raising it is
    /// app-level precision.
    pub tabs_share_window: bool,
}

impl TerminalSupport {
    /// Everything unsupported (`NotImplemented`).
    pub fn none(terminal: TerminalKind) -> Self {
        let no = Ability::Unsupported(Unsupported::NotImplemented);
        Self {
            terminal,
            focus_pane: no,
            raise_window: no,
            activate_app: no,
            launch: no,
            tabs_share_window: true,
        }
    }

    /// A tty-scripted macOS terminal (Ghostty, Terminal, iTerm2): focus by
    /// tty, activate, launch.
    pub fn scripted(terminal: TerminalKind) -> Self {
        Self {
            terminal,
            focus_pane: Ability::Available,
            raise_window: Ability::Unsupported(Unsupported::NotImplemented),
            activate_app: Ability::Available,
            launch: Ability::Available,
            tabs_share_window: terminal != TerminalKind::Ghostty,
        }
    }
}

/// A pane listed by a terminal's own pane API (Kitty, WezTerm).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    pub terminal: TerminalKind,
    /// [`PaneRef::KittyWindow`] or [`PaneRef::WezTermPane`].
    pub pane: PaneRef,
    /// The pane's tty, when the API reports it (WezTerm `tty_name`).
    pub tty: Option<String>,
    /// The pane's own process (its shell), when the API reports it.
    pub pid: Option<u32>,
}

/// One pane of a tmux server (`list-panes -a -F`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxPaneInfo {
    /// `#{pane_id}`: `%N`.
    pub pane: String,
    /// `#{pane_tty}`.
    pub tty: String,
    /// `#{session_id}`: `$N`.
    pub session: String,
    /// `#{window_id}`: `@N`.
    pub window: String,
}

/// One attached client (`list-clients -F`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxClientInfo {
    /// `#{client_tty}`: the client's tty in the outer terminal.
    pub tty: String,
    pub pid: u32,
    /// `#{session_id}` the client shows.
    pub session: String,
    /// The pane active in that session's current window.
    pub active_pane: String,
}

/// One tmux server, by socket.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxServer {
    pub socket: String,
    /// The server process. Pane ids are only meaningful for this
    /// incarnation.
    pub server: ProcessIdentity,
    pub panes: Vec<TmuxPaneInfo>,
    pub clients: Vec<TmuxClientInfo>,
}

/// A visible top-level OS window and the process that owns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: WindowId,
    pub owner: ProcessIdentity,
}

/// Everything the adapter reports about terminal hosts. Each list is
/// `Unknown` when it could not be read, and `Known(empty)` when there is
/// nothing (or the mechanism does not apply on this platform).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Surfaces {
    pub terminals: Fact<Vec<TerminalSupport>>,
    pub panes: Fact<Vec<PaneInfo>>,
    pub tmux: Fact<Vec<TmuxServer>>,
    pub windows: Fact<Vec<WindowInfo>>,
}

impl Surfaces {
    /// The macOS shape of the first round: Ghostty and Terminal scripted by
    /// tty, every other terminal unsupported, no pane API, tmux or window
    /// lists.
    pub fn mac_scripted() -> Self {
        Self {
            terminals: Fact::Known(vec![
                TerminalSupport::scripted(TerminalKind::Ghostty),
                TerminalSupport::scripted(TerminalKind::AppleTerminal),
            ]),
            panes: Fact::Known(Vec::new()),
            tmux: Fact::Known(Vec::new()),
            windows: Fact::Known(Vec::new()),
        }
    }

    pub fn support(&self, terminal: TerminalKind) -> Fact<TerminalSupport> {
        match &self.terminals {
            Fact::Known(list) => Fact::Known(
                list.iter()
                    .find(|t| t.terminal == terminal)
                    .copied()
                    .unwrap_or_else(|| TerminalSupport::none(terminal)),
            ),
            Fact::Unknown => Fact::Unknown,
        }
    }
}
