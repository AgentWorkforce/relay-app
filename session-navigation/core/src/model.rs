//! Core vocabulary shared by every step. Plain data, no behaviour beyond
//! constant lookups.
use serde::{Deserialize, Serialize};

use crate::evidence::Fact;

/// A coding-agent provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Provider {
    Claude,
    Codex,
    OpenCode,
    /// The `grok` CLI (terminal only: no desktop app or deep link is known).
    Grok,
}

impl Provider {
    pub const ALL: [Provider; 4] = [
        Provider::Claude,
        Provider::Codex,
        Provider::OpenCode,
        Provider::Grok,
    ];
}

/// The session a person asked to return to. For Claude this is the CLI
/// conversation id (a UUID); for Codex the thread id (UUIDv7); for OpenCode
/// the `ses_…` id; for Grok the session directory's UUID.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SessionRef {
    pub provider: Provider,
    pub id: String,
}

impl SessionRef {
    pub fn new(provider: Provider, id: impl Into<String>) -> Self {
        Self {
            provider,
            id: id.into(),
        }
    }

    /// Claude, Codex and Grok ids are UUIDs, compared case-insensitively
    /// (Codex facts A1; the Claude CLI writes lower case; relay's Grok
    /// `is_uuid` accepts either case). Every comparison and every URL uses
    /// the lower-case form.
    pub fn normalized(&self) -> Self {
        match self.provider {
            Provider::Claude | Provider::Codex | Provider::Grok => {
                Self::new(self.provider, self.id.to_ascii_lowercase())
            }
            Provider::OpenCode => self.clone(),
        }
    }
}

/// The Linux display server the person's desktop session runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum DisplayServer {
    X11,
    /// Generic window raising is not assumed on Wayland.
    Wayland,
}

/// The operating system the adapter runs on. Always known to the adapter;
/// only the Linux display server can be unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Platform {
    MacOs,
    Linux { session: Fact<DisplayServer> },
    Windows,
}

impl Platform {
    pub fn is_unix(self) -> bool {
        !matches!(self, Self::Windows)
    }
}

/// Where a session executes: its provider store and its processes. A launch
/// must run in the same runtime (a WSL session resumed natively would open a
/// different store, and OpenCode would silently create the session there).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Runtime {
    /// The adapter's own operating system.
    Native,
    /// A WSL distribution on Windows. Launches run through `wsl.exe -d`.
    Wsl { distro: String },
    /// A container, a remote host or anything else this machine cannot
    /// launch into. Only ever refused.
    Foreign { label: String },
}

/// Terminal applications and the platforms each exists on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TerminalKind {
    Ghostty,
    AppleTerminal,
    ITerm2,
    Kitty,
    WezTerm,
    Alacritty,
    GnomeTerminal,
    Konsole,
    Xterm,
    WindowsTerminal,
    /// The classic console host (`conhost.exe` / `OpenConsole.exe`).
    ConsoleHost,
}

impl TerminalKind {
    pub const ALL: [TerminalKind; 11] = [
        TerminalKind::Ghostty,
        TerminalKind::AppleTerminal,
        TerminalKind::ITerm2,
        TerminalKind::Kitty,
        TerminalKind::WezTerm,
        TerminalKind::Alacritty,
        TerminalKind::GnomeTerminal,
        TerminalKind::Konsole,
        TerminalKind::Xterm,
        TerminalKind::WindowsTerminal,
        TerminalKind::ConsoleHost,
    ];

    pub fn app(self) -> App {
        match self {
            Self::Ghostty => App::Ghostty,
            Self::AppleTerminal => App::AppleTerminal,
            Self::ITerm2 => App::ITerm2,
            Self::Kitty => App::Kitty,
            Self::WezTerm => App::WezTerm,
            Self::Alacritty => App::Alacritty,
            Self::GnomeTerminal => App::GnomeTerminal,
            Self::Konsole => App::Konsole,
            Self::Xterm => App::Xterm,
            Self::WindowsTerminal => App::WindowsTerminal,
            Self::ConsoleHost => App::ConsoleHost,
        }
    }

    /// Whether this terminal exists on `platform` at all.
    pub fn exists_on(self, platform: Platform) -> bool {
        self.app().identifier(platform).is_some()
    }
}

/// Applications this crate knows how to talk to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum App {
    ClaudeDesktop,
    CodexDesktop,
    OpenCodeDesktop,
    Ghostty,
    AppleTerminal,
    ITerm2,
    Kitty,
    WezTerm,
    Alacritty,
    GnomeTerminal,
    Konsole,
    Xterm,
    WindowsTerminal,
    ConsoleHost,
}

/// How a platform identifies an application. Data only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AppIdentifier {
    /// macOS bundle identifier.
    Bundle(&'static str),
    /// Linux desktop-file id (without `.desktop`) and the executable name.
    Linux {
        desktop_file: &'static str,
        executable: &'static str,
    },
    /// Windows executable name, plus the AppUserModelID of packaged apps.
    Windows {
        executable: &'static str,
        aumid: Option<&'static str>,
    },
}

impl AppIdentifier {
    /// The string a scheme handler or `Step::OpenUrl` names this app by:
    /// the bundle id, the desktop-file id or the AUMID (else the executable).
    pub fn handler(self) -> &'static str {
        match self {
            Self::Bundle(bundle) => bundle,
            Self::Linux { desktop_file, .. } => desktop_file,
            Self::Windows { executable, aumid } => aumid.unwrap_or(executable),
        }
    }
}

impl App {
    pub const ALL: [App; 14] = [
        App::ClaudeDesktop,
        App::CodexDesktop,
        App::OpenCodeDesktop,
        App::Ghostty,
        App::AppleTerminal,
        App::ITerm2,
        App::Kitty,
        App::WezTerm,
        App::Alacritty,
        App::GnomeTerminal,
        App::Konsole,
        App::Xterm,
        App::WindowsTerminal,
        App::ConsoleHost,
    ];

    /// macOS bundle identifiers. The desktop apps' and Ghostty's were
    /// VERIFIED with `plutil` (facts Claude E, Codex F, OpenCode A);
    /// Terminal's is the system one; the others are the vendors' published
    /// ids (not checked natively).
    fn mac_bundle(self) -> Option<&'static str> {
        Some(match self {
            Self::ClaudeDesktop => "com.anthropic.claudefordesktop",
            Self::CodexDesktop => "com.openai.codex",
            Self::OpenCodeDesktop => "ai.opencode.desktop",
            Self::Ghostty => "com.mitchellh.ghostty",
            Self::AppleTerminal => "com.apple.Terminal",
            Self::ITerm2 => "com.googlecode.iterm2",
            Self::Kitty => "net.kovidgoyal.kitty",
            Self::WezTerm => "com.github.wez.wezterm",
            Self::Alacritty => "org.alacritty",
            _ => return None,
        })
    }

    /// Linux desktop-file ids and executables (the old hosts table,
    /// `desktop-core/src/session_locations/hosts.rs`). No desktop app of a
    /// provider is known on Linux.
    fn linux(self) -> Option<(&'static str, &'static str)> {
        Some(match self {
            Self::Ghostty => ("com.mitchellh.ghostty", "ghostty"),
            Self::Kitty => ("kitty", "kitty"),
            Self::WezTerm => ("org.wezfurlong.wezterm", "wezterm-gui"),
            Self::Alacritty => ("Alacritty", "alacritty"),
            Self::GnomeTerminal => ("org.gnome.Terminal", "gnome-terminal-server"),
            Self::Konsole => ("org.kde.konsole", "konsole"),
            Self::Xterm => ("xterm", "xterm"),
            _ => return None,
        })
    }

    /// Windows executables and AUMIDs. No desktop app of a provider is
    /// modelled on Windows yet.
    fn windows(self) -> Option<(&'static str, Option<&'static str>)> {
        Some(match self {
            Self::WindowsTerminal => (
                "WindowsTerminal.exe",
                Some("Microsoft.WindowsTerminal_8wekyb3d8bbwe!App"),
            ),
            Self::ConsoleHost => ("conhost.exe", None),
            Self::WezTerm => ("wezterm-gui.exe", None),
            Self::Alacritty => ("alacritty.exe", None),
            _ => return None,
        })
    }

    /// How `platform` identifies this app; `None` when it does not exist
    /// there (as far as this crate knows).
    pub fn identifier(self, platform: Platform) -> Option<AppIdentifier> {
        match platform {
            Platform::MacOs => self.mac_bundle().map(AppIdentifier::Bundle),
            Platform::Linux { .. } => {
                self.linux()
                    .map(|(desktop_file, executable)| AppIdentifier::Linux {
                        desktop_file,
                        executable,
                    })
            }
            Platform::Windows => self
                .windows()
                .map(|(executable, aumid)| AppIdentifier::Windows { executable, aumid }),
        }
    }

    /// The handler string on `platform` (see [`AppIdentifier::handler`]).
    pub fn handler(self, platform: Platform) -> Option<&'static str> {
        self.identifier(platform).map(AppIdentifier::handler)
    }

    /// The macOS bundle id, or `""` for an app that has none.
    pub fn bundle_id(self) -> &'static str {
        self.mac_bundle().unwrap_or("")
    }

    pub fn from_bundle_id(bundle: &str) -> Option<App> {
        Self::from_handler(Platform::MacOs, bundle)
    }

    pub fn from_handler(platform: Platform, handler: &str) -> Option<App> {
        if handler.is_empty() {
            return None;
        }
        Self::ALL
            .into_iter()
            .find(|app| app.handler(platform) == Some(handler))
    }

    pub fn terminal(self) -> Option<TerminalKind> {
        TerminalKind::ALL.into_iter().find(|t| t.app() == self)
    }
}

/// Command-line tools whose version or path matters to a plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Tool {
    ClaudeCli,
    CodexCli,
    OpenCodeCli,
    GrokCli,
    /// The tmux client, for attaching to a detached session.
    Tmux,
}

impl Tool {
    pub const ALL: [Tool; 5] = [
        Tool::ClaudeCli,
        Tool::CodexCli,
        Tool::OpenCodeCli,
        Tool::GrokCli,
        Tool::Tmux,
    ];

    pub fn program(self) -> &'static str {
        match self {
            Self::ClaudeCli => "claude",
            Self::CodexCli => "codex",
            Self::OpenCodeCli => "opencode",
            Self::GrokCli => "grok",
            Self::Tmux => "tmux",
        }
    }

    pub fn from_program(program: &str) -> Option<Tool> {
        Self::ALL.into_iter().find(|tool| tool.program() == program)
    }

    /// The tool a program name or absolute executable path runs, on any
    /// platform (`/`, `\`, a `.exe` suffix). Grok installs itself as
    /// `grok-<version>` too (relay `grok.rs` `is_grok_process`).
    pub fn from_executable(path: &str) -> Option<Tool> {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let lower = name.to_ascii_lowercase();
        let name = lower.strip_suffix(".exe").unwrap_or(&lower);
        if name.starts_with("grok-") {
            return Some(Tool::GrokCli);
        }
        Self::from_program(name)
    }

    /// Environment keys that select the provider's home or database, and
    /// the only keys a launch may carry (Codex facts 21/29, Claude facts D,
    /// OpenCode facts A "Database location", Grok `GROK_HOME`). Nothing else
    /// is ever copied.
    pub fn environment_keys(self) -> &'static [&'static str] {
        match self {
            Self::ClaudeCli => &["CLAUDE_CONFIG_DIR"],
            Self::CodexCli => &["CODEX_HOME", "CODEX_SQLITE_HOME"],
            Self::OpenCodeCli => &[
                "OPENCODE_DB",
                "OPENCODE_DISABLE_CHANNEL_DB",
                "XDG_DATA_HOME",
                "XDG_STATE_HOME",
            ],
            Self::GrokCli => &["GROK_HOME"],
            Self::Tmux => &[],
        }
    }
}

/// Something with a version that a plan depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Component {
    App(App),
    Tool(Tool),
}

/// Where a session presents itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Host {
    Desktop(App),
    Terminal(TerminalKind),
    /// A tmux pane, inside `terminal` when a client was attached when it
    /// was seen.
    Tmux(Option<TerminalKind>),
    Headless,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BootId(pub u64);

/// A process incarnation. A pid alone is never an identity: pids recycle,
/// within a boot and across reboots. On Windows `start` is the creation time
/// and `boot` the system boot time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// Kernel start time in whole seconds (the Claude registry's `procStart`
    /// precision).
    pub start: u64,
    pub boot: BootId,
}

/// How precisely a plan returns the person. Ordered from weakest to strongest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Precision {
    /// The right application comes forward; nothing more is claimed.
    AppOnly,
    /// The window or pane that hosted the session comes forward, but it may
    /// show a different conversation now.
    RightWindow,
    /// The exact conversation is selected in the claimed host.
    ExactConversation,
}

/// Why a plan is reduced, refused or failed. Logic branches on these, never on
/// text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Reason {
    InvalidSessionId,
    EvidenceIncomplete,
    SessionNotFound,
    Archived,
    ChildSession,
    CwdUnknown,
    CwdMissing,
    AppNotInstalled,
    ToolNotInstalled,
    SchemeHandlerMismatch,
    UnverifiedVersion,
    ServerVersionMismatch,
    LaunchPending,
    LiveNotLocatable,
    RunningHeadless,
    AmbiguousIdentity,
    PriorIdentityOnly,
    PossiblyOpenElsewhere,
    DesktopRecordMissing,
    NoSessionDeepLink,
    SelectionGone,
    PermissionDenied,
    TakeOverRaced,
    ReservationStoreFailed,
    TargetGone,
    WrongDestination,
    DispatchFailed,
    /// The provider home or database the session lives in is not known, so a
    /// launch could open (or silently create) it in the wrong store.
    EnvironmentUnknown,
    /// The session's own recorded directory disagrees with other evidence.
    CwdConflict,
    /// The desktop record belongs to another account or organisation.
    OtherAccount,
    /// The server reports a turn running on this session right now.
    SessionBusy,
    /// The conversation a desktop record would resume when revived is
    /// running in another process.
    RevivalCollides,
    /// A label, not a refusal: the plan opens another client of a session
    /// whose single writer (the Codex daemon, the OpenCode server) may
    /// already be serving it to a window that cannot be located (native N10,
    /// N14). The person may end up with two views of one session.
    PossiblySecondView,
    /// The provider, app or terminal does not exist on this platform.
    PlatformUnsupported,
    /// The adapter cannot drive this terminal (no pane API, not implemented).
    HostUnsupported,
    /// The terminal can do it, but a setting is off (Kitty
    /// `allow_remote_control` and `listen_on`, the WezTerm mux socket).
    HostSettingDisabled,
    /// A permission the person has not granted is needed (macOS Automation).
    PermissionRequired,
    /// Wayland: no generic, verified window activation.
    WaylandNoRaise,
    /// Several windows of the host could be the session's and none is
    /// identified: the person has to pick it outside Relay.
    WindowAmbiguous,
    /// A label: the right window or app comes forward, but its exact pane or
    /// tab cannot be selected.
    PaneSelectionUnavailable,
    /// The session runs in a runtime this machine cannot launch into (a
    /// remote host, a container, WSL on a non-Windows machine).
    ForeignRuntime,
    /// A launch needs the default terminal, and neither the person's
    /// preference nor the operating system names one Relay can use.
    NoDefaultTerminal,
}
