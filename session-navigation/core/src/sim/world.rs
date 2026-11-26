//! The fake machine's truth. Evidence is derived from it only through the
//! lossy channels in `observe.rs`.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::rng::Rng;
use super::vars::WorldVars;
use crate::evidence::{Exe, Reservation};
use crate::model::{App, Provider, SessionRef, TerminalKind, Tool};
use crate::plan::{Command, Step};

pub use super::stores::{
    ClaudeTranscript, ClaudeTruth, CodexTruth, DesktopSession, OcSession, OcTruth, RegFile, Thread,
};

/// The apps of the first round, whose draws keep their old random stream.
pub const LEGACY_APPS: [App; 5] = [
    App::ClaudeDesktop,
    App::CodexDesktop,
    App::OpenCodeDesktop,
    App::Ghostty,
    App::AppleTerminal,
];

pub const EPOCH: u64 = 1_790_000_000;
pub const FIRST_PID: u32 = 400;
pub const USER: u32 = 501;
pub const DIRS: [&str; 4] = [
    "/Users/example/code/alpha",
    "/Users/example/code/beta",
    "/Users/example/code/with space",
    "/Users/example/code/alpha/.claude/worktrees/wt",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Launchd,
    App(App),
    AppServer,
    Login,
    Shell,
    Interactive,
    DesktopChild(String),
    Spare,
    Headless,
    Daemon,
    Server,
    Supervisor,
    Stray,
    TmuxServer,
    TmuxClient {
        socket: String,
        session: u32,
    },
    /// Another program (it may own a window).
    OtherApp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proc {
    pub pid: u32,
    pub start: u64,
    pub ppid: u32,
    pub euid: u32,
    pub ruid: u32,
    pub exe: Exe,
    pub tty: Option<String>,
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    pub role: Role,
    /// The session this process truly writes or presents.
    pub hosts: Option<SessionRef>,
    pub by_engine: Option<u64>,
    pub supervised: bool,
}

impl Proc {
    pub fn is_writer(&self) -> bool {
        matches!(
            self.role,
            Role::Interactive | Role::DesktopChild(_) | Role::Headless
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pane {
    pub id: u64,
    pub terminal: TerminalKind,
    /// The pane's tty. Internal on Windows, which shows no ttys.
    pub tty: String,
    pub login: u32,
    pub shell: u32,
    /// The OS window holding the pane (tabs share one).
    pub window: u64,
    /// The pane's id in its terminal's pane API (Kitty window id, WezTerm
    /// pane id).
    pub api: u64,
    /// Set for a pane of a tmux server (then `terminal` is meaningless).
    pub tmux: Option<super::tmux::TmuxLoc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppTruth {
    pub installed: bool,
    pub version: String,
}

/// Which observation channels currently work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Channels {
    pub proc_table: bool,
    pub claude_registry: bool,
    pub claude_desktop: bool,
    pub codex_db: bool,
    pub opencode_db: bool,
    pub opencode_hung: bool,
    pub fs: bool,
    pub reservations: bool,
    /// The provider-home selectors a session was recorded under.
    pub environment: bool,
    /// The tmux server socket (`list-panes`, `list-clients`).
    pub tmux: bool,
}

impl Default for Channels {
    fn default() -> Self {
        Self {
            proc_table: true,
            claude_registry: true,
            claude_desktop: true,
            codex_db: true,
            opencode_db: true,
            opencode_hung: false,
            fs: true,
            reservations: true,
            environment: true,
            tmux: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingLaunch {
    pub due: u64,
    pub pane: u64,
    pub command: Command,
    pub by_engine: Option<u64>,
}

/// Effects recorded for the oracle, with truth captured at the moment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogEntry {
    Exec {
        open: Option<u64>,
        step: Step,
        /// The session the targeted process truly hosted at dispatch.
        /// Sessions the targeted process hosts now (first) or hosted before.
        target_hosted: Vec<SessionRef>,
        /// The executor acted (it did not refuse after re-checking).
        acted: bool,
    },
    LaunchDispatched {
        open: Option<u64>,
        session: Option<SessionRef>,
        /// Writers of the session the launch would run next to.
        others: usize,
        /// Windows already showing the session that a planner can locate
        /// from argv: a launch next to one opens a needless second client.
        located: usize,
    },
    WriterStarted {
        open: Option<u64>,
        session: SessionRef,
        others: usize,
        immediate: bool,
    },
    Created {
        open: Option<u64>,
        session: SessionRef,
    },
    /// A client stopped the shared OpenCode server and started its own
    /// (version mismatch, OpenCode facts G).
    ServerReplaced { open: Option<u64> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct World {
    pub vars: WorldVars,
    pub clock: u64,
    pub boot: u64,
    pub boot_time: u64,
    pub next_pid: u32,
    pub procs: BTreeMap<u32, Proc>,
    pub panes: BTreeMap<u64, Pane>,
    pub next_pane: u64,
    pub apps: BTreeMap<App, AppTruth>,
    /// Installed CLI versions (`None`: not installed, not on PATH).
    pub tools: BTreeMap<Tool, Option<String>>,
    /// Where each CLI is installed.
    pub tool_paths: BTreeMap<Tool, String>,
    pub schemes: BTreeMap<String, String>,
    pub automation: BTreeMap<App, bool>,
    pub front_app: Option<App>,
    pub front_pane: Option<u64>,
    pub views: BTreeMap<App, Option<String>>,
    pub dirs: BTreeSet<String>,
    pub claude: ClaudeTruth,
    pub codex: CodexTruth,
    pub opencode: OcTruth,
    pub grok: super::grok::GrokTruth,
    pub platform: crate::model::Platform,
    pub hosts: super::hosts::HostTruth,
    pub tmux: super::tmux::TmuxTruth,
    /// Processes that run inside the WSL distribution.
    pub wsl_procs: BTreeSet<u32>,
    /// Sessions that live in the WSL distribution.
    pub wsl_sessions: BTreeSet<SessionRef>,
    /// A hazard armed to strike right before the next dispatched step.
    pub race: Option<super::hosts::Hazard>,
    pub pending: Vec<PendingLaunch>,
    pub history: BTreeMap<SessionRef, crate::model::Host>,
    /// Where background discovery first saw each session.
    pub first_host: BTreeMap<SessionRef, crate::model::Host>,
    /// The Windows Terminal profile a session's window was opened with.
    pub profiles: BTreeMap<SessionRef, String>,
    pub reservations: BTreeMap<SessionRef, Reservation>,
    pub channels: Channels,
    pub catalog: Vec<SessionRef>,
    /// The host each session was started in (statistics only).
    pub origins: BTreeMap<SessionRef, String>,
    pub log: Vec<LogEntry>,
    pub engine_open: Option<u64>,
    pub launch_delay: u64,
    /// Sessions a process (pid, start) hosted before a /clear.
    pub past_hosts: BTreeMap<(u32, u64), Vec<SessionRef>>,
    /// Processes asked to stop that exit after this many more observations.
    pub stopping: BTreeMap<u32, u64>,
    /// How many observations a stopped process takes to exit.
    pub exit_delay: u64,
    /// Stable labels of sessions, panes and processes for shrinking.
    pub labels: super::labels::Labels,
    pub rng: Rng,
}

pub fn tool_path(tool: Tool) -> &'static str {
    match tool {
        Tool::ClaudeCli => "/Users/example/.local/bin/claude",
        Tool::CodexCli => "/opt/homebrew/bin/codex",
        Tool::OpenCodeCli => "/Users/example/.opencode/bin/opencode",
        Tool::GrokCli => "/Users/example/.grok/bin/grok",
        Tool::Tmux => "/opt/homebrew/bin/tmux",
    }
}

pub fn verified_app_version(app: App) -> &'static str {
    match app {
        App::ClaudeDesktop => "5.1231.2",
        App::CodexDesktop => "26.1103.60933",
        App::OpenCodeDesktop => "2.2.6",
        App::Ghostty => "1.4.0",
        App::AppleTerminal => "2.15",
        // Not version-gated (no native check); any value works.
        _ => "1.0.0",
    }
}

pub fn verified_tool_version(tool: Tool) -> &'static str {
    match tool {
        Tool::ClaudeCli => "2.1.321",
        Tool::CodexCli => "0.167.0",
        Tool::OpenCodeCli => "2.2.6",
        // relay's `MIN_VERSION`; never run natively.
        Tool::GrokCli => "1.0.46",
        Tool::Tmux => "3.5a",
    }
}

impl World {
    /// A machine with every app installed at its verified version, all
    /// channels working, and Automation granted; the seed picks the launch
    /// delay and a few missing apps or permissions.
    pub fn new(seed: u64, vars: WorldVars) -> Self {
        let mut rng = Rng::new(seed);
        let mut apps = BTreeMap::new();
        let mut automation = BTreeMap::new();
        // The first round's apps draw from the main stream (unchanged
        // worlds); the others from their own.
        let mut extra = Rng::new(seed ^ 0x7E57_A995);
        for app in App::ALL {
            let legacy = LEGACY_APPS.contains(&app);
            let stream = if legacy { &mut rng } else { &mut extra };
            let exists = app.identifier(crate::model::Platform::MacOs).is_some();
            let installed = !stream.chance(4) && exists;
            let version = verified_app_version(app).to_owned();
            apps.insert(app, AppTruth { installed, version });
            automation.insert(app, !stream.chance(6));
        }
        let tools = Tool::ALL
            .into_iter()
            .map(|t| (t, Some(verified_tool_version(t).to_owned())))
            .collect();
        let schemes = [
            ("claude", App::ClaudeDesktop),
            ("codex", App::CodexDesktop),
            ("opencode", App::OpenCodeDesktop),
        ]
        .into_iter()
        .map(|(s, a)| (s.to_owned(), a.bundle_id().to_owned()))
        .collect();
        let launch_delay = rng.below(3);
        // Drawn from a separate stream so the main one is unchanged.
        let exit_delay = match Rng::new(seed ^ 0xE817_DE1A).below(4) {
            3 => 9,
            n => n,
        };
        let mut world = Self {
            vars,
            clock: EPOCH,
            boot: 1,
            boot_time: EPOCH - 3600,
            next_pid: FIRST_PID,
            procs: BTreeMap::new(),
            panes: BTreeMap::new(),
            next_pane: 1,
            apps,
            tools,
            tool_paths: Tool::ALL
                .into_iter()
                .map(|t| (t, tool_path(t).to_owned()))
                .collect(),
            schemes,
            automation,
            front_app: None,
            front_pane: None,
            views: BTreeMap::new(),
            dirs: DIRS.iter().map(|d| (*d).to_owned()).collect(),
            claude: ClaudeTruth::default(),
            codex: CodexTruth::default(),
            opencode: OcTruth {
                server_version: verified_tool_version(Tool::OpenCodeCli).to_owned(),
                ..OcTruth::default()
            },
            grok: super::grok::GrokTruth::default(),
            platform: crate::model::Platform::MacOs,
            hosts: super::hosts::HostTruth::new(seed),
            tmux: super::tmux::TmuxTruth::default(),
            wsl_procs: BTreeSet::new(),
            wsl_sessions: BTreeSet::new(),
            race: None,
            pending: Vec::new(),
            history: BTreeMap::new(),
            first_host: BTreeMap::new(),
            profiles: BTreeMap::new(),
            reservations: BTreeMap::new(),
            channels: Channels::default(),
            catalog: Vec::new(),
            origins: BTreeMap::new(),
            log: Vec::new(),
            engine_open: None,
            launch_delay,
            past_hosts: BTreeMap::new(),
            stopping: BTreeMap::new(),
            exit_delay,
            labels: super::labels::Labels::default(),
            rng,
        };
        // Drawn from a separate stream so the main one is unchanged.
        world.codex.update_available = Rng::new(seed ^ 0x0DA7_E5A1).chance(25);
        world.spawn_launchd();
        world
    }

    pub fn installed(&self, app: App) -> bool {
        self.apps.get(&app).is_some_and(|a| a.installed)
    }

    pub fn app_pid(&self, app: App) -> Option<u32> {
        self.procs
            .values()
            .find(|p| p.role == Role::App(app))
            .map(|p| p.pid)
    }

    pub fn running(&self, app: App) -> bool {
        self.app_pid(app).is_some()
    }

    pub(crate) fn alloc_pid(&mut self) -> u32 {
        for _ in 0..100_000 {
            let pid = self.next_pid;
            self.next_pid = if self.next_pid >= 99_998 {
                FIRST_PID
            } else {
                self.next_pid + 1
            };
            if !self.procs.contains_key(&pid) {
                return pid;
            }
        }
        FIRST_PID
    }

    pub(crate) fn alloc_tty(&self) -> String {
        let used: BTreeSet<&str> = self.panes.values().map(|p| p.tty.as_str()).collect();
        (0..1000)
            .map(|n| format!("/dev/ttys{n:03}"))
            .find(|t| !used.contains(t.as_str()))
            .unwrap_or_else(|| "/dev/ttys999".to_owned())
    }

    fn spawn_launchd(&mut self) {
        self.procs.insert(
            1,
            Proc {
                pid: 1,
                start: self.boot_time,
                ppid: 0,
                euid: 0,
                ruid: 0,
                exe: Exe::Other,
                tty: None,
                argv: vec!["/sbin/launchd".to_owned()],
                cwd: Some("/".to_owned()),
                role: Role::Launchd,
                hosts: None,
                by_engine: None,
                supervised: false,
            },
        );
    }

    /// Insert a process with a fresh pid. Returns the pid.
    pub(crate) fn spawn(&mut self, mut proc: Proc) -> u32 {
        let pid = self.alloc_pid();
        proc.pid = pid;
        proc.start = self.clock;
        self.procs.insert(pid, proc);
        self.labels.new_proc(pid);
        pid
    }

    pub fn new_session_id(&mut self, provider: Provider) -> String {
        match provider {
            Provider::Claude => self.rng.uuid_v4(),
            Provider::Codex => {
                let millis = self.clock.saturating_mul(1000);
                self.rng.uuid_v7(millis)
            }
            Provider::OpenCode => self.rng.opencode_id(),
            Provider::Grok => self.rng.uuid_v4(),
        }
    }

    pub(crate) fn record_session(&mut self, session: SessionRef, by_engine: Option<u64>) {
        if by_engine.is_some() {
            self.log.push(LogEntry::Created {
                open: by_engine,
                session: session.clone(),
            });
        }
        self.labels.new_session(&session);
        if !self.catalog.contains(&session) {
            self.catalog.push(session);
        }
    }
}
