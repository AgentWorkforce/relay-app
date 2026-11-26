//! The immutable, plain-data snapshot `decide` reads. Everything decide may use
//! is here and nothing else. Uncertainty is explicit: a fact is
//! [`Fact::Known`] or [`Fact::Unknown`], never a default.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::hosts::{PaneRef, Surfaces, WindowId};
use crate::model::{
    App, BootId, Host, Platform, ProcessIdentity, Runtime, SessionRef, TerminalKind, Tool,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Fact<T> {
    Known(T),
    Unknown,
}

impl<T> Fact<T> {
    pub fn known(&self) -> Option<&T> {
        match self {
            Self::Known(value) => Some(value),
            Self::Unknown => None,
        }
    }

    pub fn known_mut(&mut self) -> Option<&mut T> {
        match self {
            Self::Known(value) => Some(value),
            Self::Unknown => None,
        }
    }

    pub fn is_known(&self) -> bool {
        matches!(self, Self::Known(_))
    }
}

impl<T: PartialEq> Fact<T> {
    pub fn is(&self, value: &T) -> bool {
        self.known() == Some(value)
    }
}

/// Executable classification from the kernel's executable path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Exe {
    ClaudeCli,
    ClaudeDesktopApp,
    CodexCli,
    CodexDesktopApp,
    OpenCodeCli,
    OpenCodeDesktopApp,
    /// `grok` or `grok-<version>` (relay `grok.rs` `is_grok_process`).
    GrokCli,
    Ghostty,
    AppleTerminal,
    ITerm2,
    Kitty,
    WezTerm,
    Alacritty,
    /// `gnome-terminal-server`: one process for all its windows.
    GnomeTerminal,
    Konsole,
    Xterm,
    WindowsTerminal,
    /// `conhost.exe` / `OpenConsole.exe`: the classic console host, a child
    /// of the first console process it serves.
    ConsoleHost,
    /// A tmux server (no tty) or client (with the outer terminal's tty).
    Tmux,
    /// `/usr/bin/login`, which runs as root between a terminal and its shell
    /// (VERIFIED for Ghostty, Claude facts D).
    Login,
    Shell,
    Other,
}

/// One row of the kernel process table.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub identity: ProcessIdentity,
    pub ppid: u32,
    /// Effective uid. `/usr/bin/login` between a terminal and its shell runs
    /// with effective uid 0 but real uid of the person (VERIFIED, Ghostty
    /// 1.4.0, native N15), so no walk may stop at an effective-uid mismatch.
    pub euid: u32,
    /// Real uid: the person who started the chain.
    pub ruid: u32,
    pub exe: Exe,
    /// `/dev/ttysNNN` or `/dev/pts/N`, or `None` for `??` and on Windows
    /// (which has no ttys).
    pub tty: Option<String>,
    pub argv: Vec<String>,
    pub cwd: Fact<String>,
    /// The window the process declares it runs in (X11 `WINDOWID` from its
    /// environment). A claim only: it counts once [`Surfaces::windows`]
    /// shows that window owned by the process's own terminal.
    pub window: Option<WindowId>,
}

/// What is frontmost right now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Front {
    /// The frontmost application. `Known(None)`: none (a locked screen).
    pub app: Fact<Option<App>>,
    /// The focused pane's tty when the front app is a terminal (needs
    /// Automation permission to read on macOS). `Known(None)` when the front
    /// app is not a terminal.
    pub pane_tty: Fact<Option<String>>,
    /// The focused pane of a pane-API terminal ([`PaneRef::KittyWindow`],
    /// [`PaneRef::WezTermPane`]). `Known(None)` when not applicable.
    pub pane: Fact<Option<PaneRef>>,
    /// The foreground OS window (X11 active window, Win32 foreground
    /// window). `Known(None)` when not applicable.
    pub window: Fact<Option<WindowId>>,
    /// The conversation shown by a frontmost desktop app, in that provider's
    /// own id space (Claude: `local_…`). `Known(None)` means a home screen.
    pub view: Fact<Option<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    pub app: App,
    pub installed: bool,
    pub version: Option<String>,
    pub running: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolInfo {
    pub tool: Tool,
    /// `Known(None)` = not installed.
    pub version: Fact<Option<String>>,
    /// The resolved absolute executable a launch would run. `None` when no
    /// executable was found (no `claude` is on PATH on the surveyed Mac,
    /// Claude facts correction 4), whatever `version` says.
    pub path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemeHandler {
    pub scheme: String,
    pub bundle: Option<String>,
}

/// What an unresolved reservation was made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pending {
    /// A terminal launch (it may still start in its window).
    Launch,
    /// A provider link that may revive a desktop session (Claude `resume`).
    Link,
}

/// A durable record that a process-starting plan was dispatched and has not
/// yet been resolved by observed ownership.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reservation {
    pub session: SessionRef,
    pub made_at: u64,
    /// The tty of the window the launch opened, once the executor reported it.
    pub tty: Option<String>,
    /// A process was observed on `tty` since the launch. A terminal reports
    /// the new window's tty before its first process appears (Ghostty within
    /// ~1.5 s, native N17), so only after that does an empty tty mean the
    /// window closed.
    #[serde(default)]
    pub seen: bool,
    /// The boot the reservation was made in. Nothing dispatched in an
    /// earlier boot can still start. `None` for a record without one.
    pub boot: Option<BootId>,
    pub via: Pending,
}

/// An assumption an adapter has verified natively for one component version.
/// Policy relies on it only while the observed version matches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Capability {
    /// The Claude CLI rewrites `<config>/sessions/<pid>.json` `sessionId`
    /// whenever its conversation changes (/clear, /resume). UNVERIFIED
    /// (Claude summary 5); when verified, the registry is authoritative.
    ClaudeRegistryFollowsSession { cli_version: String },
    /// The Grok TUI rewrites its `active_sessions.json` record when it moves
    /// to another session. UNVERIFIED (facts-grok 3).
    GrokRegistryFollowsSession { cli_version: String },
}

/// Everything known about one session at one moment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub session: SessionRef,
    pub observed_at: u64,
    pub platform: Platform,
    /// Where the session executes. `processes`, `tools`, `cwd` and
    /// `environment` all describe this runtime (for a WSL session: the
    /// distribution's process table, the tools on its PATH, its paths).
    pub runtime: Fact<Runtime>,
    pub boot: Fact<BootId>,
    pub processes: Fact<Vec<ProcessInfo>>,
    pub front: Front,
    /// Applications this crate knows, for this platform.
    pub apps: Fact<Vec<AppInfo>>,
    pub tools: Vec<ToolInfo>,
    /// URL scheme handlers; `bundle` is the platform handler string
    /// ([`crate::model::AppIdentifier::handler`]).
    pub schemes: Fact<Vec<SchemeHandler>>,
    /// Terminals, panes, tmux servers and windows.
    pub surfaces: Surfaces,
    /// The host this session was last observed in by background discovery.
    pub last_seen_host: Fact<Host>,
    /// The host it was first observed in.
    pub original_host: Fact<Host>,
    /// The Windows Terminal profile (`WT_PROFILE_ID`) its window was
    /// opened with, preserved by a resume. `Known(None)`: none.
    pub terminal_profile: Fact<Option<String>>,
    /// The operating system's default terminal as the adapter read it
    /// (macOS: the handler of `public.unix-executable`; Linux:
    /// `xdg-terminals.list`, `x-terminal-emulator`, GNOME's setting;
    /// Windows: the console delegation). `Known(None)`: read, and none is
    /// set or it is not a terminal this crate models. `Unknown`: unreadable.
    pub default_terminal: Fact<Option<TerminalKind>>,
    pub reservation: Fact<Option<Reservation>>,
    /// The session's working directory from provider records.
    pub cwd: Fact<String>,
    pub cwd_exists: Fact<bool>,
    /// The provider-home and database selectors the session was recorded
    /// under (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `OPENCODE_DB`, …).
    /// `Known(empty)` means the default home.
    pub environment: Fact<BTreeMap<String, String>>,
    /// Natively verified assumptions for the installed versions.
    pub capabilities: Vec<Capability>,
    pub provider: ProviderEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderEvidence {
    Claude(ClaudeEvidence),
    Codex(CodexEvidence),
    OpenCode(OpenCodeEvidence),
    Grok(GrokEvidence),
}

// ---------------------------------------------------------------- Claude ---

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Entrypoint {
    Cli,
    Desktop,
    Sdk,
    Other,
}

/// One `<config>/sessions/<pid>.json` registry file (Claude facts C).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryClaim {
    pub pid: u32,
    pub proc_start: u64,
    /// Whether the file was written during the current boot (mtime against
    /// boot time). Files survive reboots and crashes.
    pub written_this_boot: Fact<bool>,
    pub session_id: String,
    pub host_session: Option<String>,
    pub spare: bool,
    pub entrypoint: Entrypoint,
}

/// How a desktop record claims a CLI id (Claude facts A, `cliHandles`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ClaimKind {
    /// `cliSessionId ?? unarchivedCliSessionId`: the transcript currently open.
    Live,
    /// The desktop id is `local_` + this id (an import) but the record now
    /// holds another conversation: right session, not this conversation.
    Stem,
    /// `preClearCliSessionId`, `priorCliSessionIds`, `overflowCliSessionIds`
    /// or a stale `unarchivedCliSessionId`.
    Prior,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DesktopClaim {
    pub local_id: String,
    pub archived: bool,
    pub kind: ClaimKind,
    pub cwd: Option<String>,
    /// `<account>/<org>` from the record's directory (Claude facts A).
    pub account: String,
    /// The record's current conversation, `cliSessionId ??
    /// unarchivedCliSessionId`: what the app resumes when it revives the
    /// record, whichever id the link named.
    pub current: Option<String>,
    /// `lastFocusedAt` (epoch ms): set when the app focuses the session,
    /// untouched by a `continue` link on an archived record (VERIFIED,
    /// Claude.app 5.1231.2, native N1, N3, N5). `Known(None)` is a record
    /// that was never focused (`null`); `Unknown` a record without the field.
    pub last_focused: Fact<Option<u64>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transcript {
    pub cwd: Option<String>,
    pub entrypoint: Entrypoint,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaudeEvidence {
    /// Every registry file, not only this session's.
    pub registry: Fact<Vec<RegistryClaim>>,
    /// Desktop records that claim this CLI id in any way.
    pub desktop: Fact<Vec<DesktopClaim>>,
    pub transcript: Fact<Option<Transcript>>,
    /// The desktop app's current `<account>/<org>`: links act on it only
    /// (Claude facts F, "Multiple accounts or orgs").
    pub account: Fact<String>,
}

// ----------------------------------------------------------------- Codex ---

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodexOrigin {
    /// `originator: "Codex Desktop"`.
    Desktop,
    /// `originator: "codex-tui"`.
    Tui,
    Exec,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadInfo {
    pub archived: bool,
    pub origin: CodexOrigin,
    pub subagent: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodexEvidence {
    pub thread: Fact<Option<ThreadInfo>>,
    /// Pids with an open handle on `thread-writer-locks/<id>.lock`. A lock
    /// means "loaded somewhere", not "open in a window" (Codex facts 12).
    pub lock_holders: Fact<Vec<u32>>,
    /// IPC `thread-owner-discovery`: does a desktop window own the thread?
    pub window_owner: Fact<bool>,
}

// -------------------------------------------------------------- OpenCode ---

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerState {
    Up { pid: u32, version: String },
    Down,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCodeSession {
    pub archived: bool,
    pub child: bool,
    pub directory: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCodeEvidence {
    pub server: Fact<ServerState>,
    /// `Known(None)` only after a positive not-found from the API or the
    /// database: never inferred from silence.
    pub session: Fact<Option<OpenCodeSession>>,
    pub busy: Fact<bool>,
    /// Saved tab lists. They survive quit, so they are hints, not liveness.
    pub tui_tab_hint: bool,
    pub desktop_tab_hint: bool,
    /// Every session in the desktop's saved tabs. Launching the desktop
    /// restores them (OpenCode facts D(ii)).
    pub desktop_tabs: Fact<Vec<String>>,
    /// Other OpenCode sessions with an unresolved launch reservation: their
    /// TUI may start any moment, so the desktop must not restore them.
    pub reserved: Fact<Vec<String>>,
}

// ------------------------------------------------------------------ Grok ---

/// One `<grok home>/active_sessions.json` record (relay `grok.rs`
/// `decode_active_sessions`): written by a running TUI, removed on exit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrokActive {
    pub session_id: String,
    pub pid: u32,
    pub cwd: Option<String>,
    /// `opened_at` in epoch seconds; the process must have started no later
    /// (desktop-core `providers.rs`: a recycled pid starts after it).
    pub opened_at: Option<u64>,
}

/// `<grok home>/sessions/<encoded cwd>/<id>/summary.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrokSession {
    /// `info.cwd` (the directory name decodes to the same).
    pub cwd: String,
    /// `session_kind == "subagent"`.
    pub subagent: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrokEvidence {
    /// Every record, not only this session's.
    pub active: Fact<Vec<GrokActive>>,
    /// `Known(None)` only after a positive not-found (no session directory
    /// with this id under any cwd): never inferred from silence.
    pub session: Fact<Option<GrokSession>>,
}
