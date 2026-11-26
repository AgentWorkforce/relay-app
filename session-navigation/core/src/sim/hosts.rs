//! Terminal hosts in the fake machine: the platform, which terminals exist
//! there, their windows, tabs and pane-API ids, and the settings that switch
//! their APIs on. Pane shapes follow the old implementation's notes
//! (`desktop-core/src/session_locations/hosts.rs`, `gtk/src/
//! session_navigation/`): a login shell under the terminal on macOS, a shell
//! directly under it on Linux and in Windows Terminal, and a classic console
//! whose `conhost.exe` is a child of its first client.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::rng::Rng;
use super::world::{Pane, Proc, Role, World, DIRS, USER};
use crate::evidence::Exe;
use crate::evidence::Fact;
use crate::model::{App, DisplayServer, Platform, TerminalKind, Tool};
use crate::plan::Command;

/// Something that happens between a decision and its first step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Hazard {
    /// Kitty remote control / the WezTerm socket is switched off.
    RevokeApi,
    /// macOS Automation is revoked for every terminal.
    RevokeAutomation,
    /// The terminal the step targets quits and starts again: its windows,
    /// panes and socket are new.
    RestartTerminal,
    /// The tmux server restarts: its pane ids start again from `%0`.
    RestartTmux,
    /// The window the step targets closes and another program's new window
    /// gets its id.
    ReuseWindow,
    /// Every tmux client detaches.
    DetachTmux,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostTruth {
    pub rng: Rng,
    /// Kitty `allow_remote_control` + `listen_on`, the WezTerm mux socket.
    pub api: BTreeMap<TerminalKind, bool>,
    pub next_window: u64,
    pub next_api: u64,
    /// Windows of other programs: id → owner pid.
    pub foreign_windows: BTreeMap<u64, u32>,
    /// The selected tab of each window.
    pub active: BTreeMap<u64, u64>,
    /// New windows report their tty before any process appears on it.
    pub slow_start: bool,
    /// ttys whose processes have not appeared yet (until time passes).
    pub hidden_ttys: BTreeSet<String>,
    /// Draws the machine's default terminal (`World::system_default`).
    pub default_draw: u64,
}

impl HostTruth {
    pub fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed ^ 0x4057_7E55);
        let api = [TerminalKind::Kitty, TerminalKind::WezTerm]
            .into_iter()
            .map(|t| (t, rng.chance(70)))
            .collect();
        Self {
            rng,
            api,
            next_window: 0x0400_0001,
            next_api: 1,
            foreign_windows: BTreeMap::new(),
            active: BTreeMap::new(),
            slow_start: false,
            hidden_ttys: BTreeSet::new(),
            default_draw: Rng::new(seed ^ 0xDEF7_E2A1).next_u64(),
        }
    }
}

pub const WIN_DIRS: [&str; 4] = [
    "C:\\Users\\example\\code\\alpha",
    "C:\\Users\\example\\code\\beta",
    "C:\\Users\\example\\code\\with space",
    "C:\\Users\\example\\code\\alpha\\.claude\\worktrees\\wt",
];

/// The platform a seeded run uses: half macOS, the rest Linux (X11 or
/// Wayland) and Windows. A separate stream, so other draws are unchanged.
pub fn platform_for(seed: u64) -> Platform {
    match Rng::new(seed ^ 0x9147_F0E5).below(10) {
        0..=4 => Platform::MacOs,
        5 | 6 => Platform::Linux {
            session: Fact::Known(DisplayServer::X11),
        },
        7 => Platform::Linux {
            session: Fact::Known(DisplayServer::Wayland),
        },
        _ => Platform::Windows,
    }
}

pub fn windows_tool_path(tool: Tool) -> String {
    format!("C:\\Users\\example\\.local\\bin\\{}.exe", tool.program())
}

/// Terminals with tabs: a raised window may show another one.
pub fn tabbed(terminal: TerminalKind) -> bool {
    !matches!(
        terminal,
        TerminalKind::Ghostty
            | TerminalKind::Alacritty
            | TerminalKind::Xterm
            | TerminalKind::ConsoleHost
    )
}

impl World {
    /// A world on `platform` (macOS is [`World::new`]).
    pub fn new_on(seed: u64, vars: super::vars::WorldVars, platform: Platform) -> Self {
        let mut world = Self::new(seed, vars);
        if platform != Platform::MacOs {
            world.set_platform(platform);
        }
        world
    }

    fn set_platform(&mut self, platform: Platform) {
        self.platform = platform;
        for app in App::ALL {
            let exists = app.identifier(platform).is_some();
            let mac = app.identifier(Platform::MacOs).is_some();
            let draw = !self.hosts.rng.chance(15);
            if let Some(truth) = self.apps.get_mut(&app) {
                truth.installed = exists && if mac { truth.installed } else { draw };
            }
        }
        // No provider desktop app (and so no scheme handler) is modelled
        // off macOS.
        self.schemes.clear();
        if platform == Platform::Windows {
            for tool in Tool::ALL {
                self.tool_paths.insert(tool, windows_tool_path(tool));
            }
            self.dirs.extend(WIN_DIRS.iter().map(|d| (*d).to_owned()));
        }
    }

    /// The directory for event index `d` on this platform.
    pub fn dir_at(&self, d: u8) -> String {
        let list = if self.platform == Platform::Windows {
            WIN_DIRS
        } else {
            DIRS
        };
        list.get(usize::from(d) % list.len())
            .copied()
            .unwrap_or("/")
            .to_owned()
    }

    /// Terminals that exist here, in a fixed order.
    pub fn terminal_choices(&self) -> Vec<TerminalKind> {
        let mut out: Vec<TerminalKind> = TerminalKind::ALL
            .into_iter()
            .filter(|t| t.exists_on(self.platform))
            .collect();
        if self.platform == Platform::MacOs {
            // Keep the first round's two first.
            out.sort_by_key(|t| !matches!(t, TerminalKind::Ghostty | TerminalKind::AppleTerminal));
        }
        out
    }

    /// `terminal` if it exists here, else the platform's counterpart of the
    /// first round's two (Ghostty → default, Terminal → alternative).
    pub fn local(&self, terminal: TerminalKind) -> TerminalKind {
        if terminal.exists_on(self.platform) {
            return terminal;
        }
        let [first, second] = self.default_terminals();
        if terminal == TerminalKind::AppleTerminal {
            second
        } else {
            first
        }
    }

    /// The person's default terminal and their alternative.
    pub fn default_terminals(&self) -> [TerminalKind; 2] {
        match self.platform {
            Platform::MacOs => [TerminalKind::Ghostty, TerminalKind::AppleTerminal],
            Platform::Linux { .. } => [TerminalKind::Kitty, TerminalKind::GnomeTerminal],
            Platform::Windows => [TerminalKind::WindowsTerminal, TerminalKind::ConsoleHost],
        }
    }

    fn new_window(&mut self) -> u64 {
        let id = self.hosts.next_window;
        self.hosts.next_window = id.saturating_add(2);
        id
    }

    fn proc(ppid: u32, exe: Exe, tty: &str, argv: &[&str], cwd: Option<&str>, role: Role) -> Proc {
        Proc {
            pid: 0,
            start: 0,
            ppid,
            euid: USER,
            ruid: USER,
            exe,
            tty: (!tty.is_empty()).then(|| tty.to_owned()),
            argv: argv.iter().map(|a| (*a).to_owned()).collect(),
            cwd: cwd.map(str::to_owned),
            role,
            hosts: None,
            by_engine: None,
            supervised: false,
        }
    }

    /// Open a new pane in `terminal`: a new window, or (for a tabbed
    /// terminal other than the first round's two) sometimes a new tab of an
    /// existing one. `join` forces the choice.
    pub fn open_pane(&mut self, terminal: TerminalKind, cwd: &str) -> Option<u64> {
        self.open_pane_with(terminal, cwd, None, false)
    }

    pub(crate) fn open_pane_with(
        &mut self,
        terminal: TerminalKind,
        cwd: &str,
        join: Option<bool>,
        wsl: bool,
    ) -> Option<u64> {
        if terminal == TerminalKind::ConsoleHost && !self.installed(App::ConsoleHost) {
            return None;
        }
        let app_pid = if terminal == TerminalKind::ConsoleHost {
            1
        } else {
            self.ensure_app(terminal.app())?
        };
        let tty = self.alloc_tty();
        let (login, shell) = match self.platform {
            Platform::MacOs => {
                // VERIFIED (native N15): `login` has effective uid 0, real
                // uid the person's.
                let login = self.spawn(Proc {
                    euid: 0,
                    ..Self::proc(
                        app_pid,
                        Exe::Login,
                        &tty,
                        &["/usr/bin/login", "-flp"],
                        None,
                        Role::Login,
                    )
                });
                let shell = self.spawn(Self::proc(
                    login,
                    Exe::Shell,
                    &tty,
                    &["-/bin/zsh"],
                    Some(cwd),
                    Role::Shell,
                ));
                (login, shell)
            }
            Platform::Linux { .. } => {
                let shell = self.spawn(Self::proc(
                    app_pid,
                    Exe::Shell,
                    &tty,
                    &["bash"],
                    Some(cwd),
                    Role::Shell,
                ));
                (shell, shell)
            }
            Platform::Windows if terminal == TerminalKind::ConsoleHost => {
                let shell = self.spawn(Self::proc(
                    1,
                    Exe::Shell,
                    &tty,
                    &["cmd.exe"],
                    Some(cwd),
                    Role::Shell,
                ));
                self.spawn(Self::proc(
                    shell,
                    Exe::ConsoleHost,
                    "",
                    &["conhost.exe", "0xffffffff"],
                    None,
                    Role::Login,
                ));
                (shell, shell)
            }
            Platform::Windows => {
                let argv: &[&str] = if wsl { &["wsl.exe"] } else { &["pwsh.exe"] };
                let shell = self.spawn(Self::proc(
                    app_pid,
                    Exe::Shell,
                    &tty,
                    argv,
                    Some(cwd),
                    Role::Shell,
                ));
                if !wsl {
                    (shell, shell)
                } else {
                    let inner = self.spawn(Self::proc(
                        shell,
                        Exe::Shell,
                        &tty,
                        &["-bash"],
                        Some(cwd),
                        Role::Shell,
                    ));
                    self.wsl_procs.insert(inner);
                    (shell, inner)
                }
            }
        };
        let legacy = matches!(
            terminal,
            TerminalKind::Ghostty | TerminalKind::AppleTerminal
        );
        let join = join.unwrap_or_else(|| !legacy && tabbed(terminal) && self.hosts.rng.chance(35));
        let existing = join
            .then(|| {
                self.panes
                    .values()
                    .filter(|p| p.terminal == terminal && p.tmux.is_none())
                    .map(|p| p.window)
                    .next_back()
            })
            .flatten();
        let window = match existing {
            Some(window) => window,
            None => self.new_window(),
        };
        let api = self.hosts.next_api;
        self.hosts.next_api += 1;
        let id = self.next_pane;
        self.next_pane += 1;
        self.labels.new_pane(id);
        self.panes.insert(
            id,
            Pane {
                id,
                terminal,
                tty,
                login,
                shell,
                window,
                api,
                tmux: None,
            },
        );
        self.hosts.active.insert(window, id);
        self.front_app = Some(terminal.app());
        self.front_pane = Some(id);
        Some(id)
    }

    /// The window a launch opens. Ghostty runs a configured command as
    /// `/usr/bin/login -flp <user> /bin/bash --noprofile --norc -c exec -l
    /// <command>`, so the command replaces bash and its parent is `login`;
    /// the terminal's `pid` property is that `login` (VERIFIED, 1.4.0, native
    /// N18). Terminal's `do script` types into a login shell. Windows
    /// Terminal's `new-tab` may join an existing window
    /// (`wt_new_tab_joins_window`).
    pub fn open_launch_pane(
        &mut self,
        terminal: TerminalKind,
        command: &Command,
        wsl: bool,
    ) -> Option<u64> {
        let join = (terminal == TerminalKind::WindowsTerminal)
            .then_some(self.vars.wt_new_tab_joins_window);
        let id = self.open_pane_with(terminal, &command.cwd, join, wsl)?;
        if self.hosts.slow_start {
            if let Some(pane) = self.panes.get(&id) {
                self.hosts.hidden_ttys.insert(pane.tty.clone());
            }
        }
        if terminal != TerminalKind::Ghostty || self.platform != Platform::MacOs {
            return Some(id);
        }
        let pane = self.panes.get(&id)?.clone();
        self.procs.remove(&pane.shell);
        let mut line = vec![command.program.clone()];
        line.extend(command.args.iter().cloned());
        if let Some(login) = self.procs.get_mut(&pane.login) {
            login.argv = [
                "/usr/bin/login",
                "-flp",
                "example",
                "/bin/bash",
                "--noprofile",
                "--norc",
                "-c",
            ]
            .iter()
            .map(|a| (*a).to_owned())
            .chain(std::iter::once(format!("exec -l {}", line.join(" "))))
            .collect();
        }
        if let Some(p) = self.panes.get_mut(&id) {
            p.shell = pane.login;
        }
        Some(id)
    }

    /// Keep tabs and the front consistent after a pane closed.
    pub(crate) fn pane_closed(&mut self, pane: &Pane) {
        self.hosts.hidden_ttys.remove(&pane.tty);
        if self.hosts.active.get(&pane.window) == Some(&pane.id) {
            let next = self
                .panes
                .values()
                .filter(|p| p.window == pane.window && p.tmux.is_none())
                .map(|p| p.id)
                .next_back();
            match next {
                Some(other) => {
                    self.hosts.active.insert(pane.window, other);
                }
                None => {
                    self.hosts.active.remove(&pane.window);
                }
            }
        }
        if let Some(loc) = &pane.tmux {
            self.tmux_pane_closed(loc);
        }
    }

    /// The process that owns `window` as the OS reports it.
    pub fn window_owner(&self, window: u64) -> Option<u32> {
        if let Some(owner) = self.hosts.foreign_windows.get(&window) {
            return Some(*owner);
        }
        let pane = self
            .panes
            .values()
            .find(|p| p.window == window && p.tmux.is_none())?;
        if pane.terminal != TerminalKind::ConsoleHost {
            return self.app_pid(pane.terminal.app());
        }
        if self.vars.console_window_owner_is_client {
            return Some(pane.shell);
        }
        self.procs
            .values()
            .find(|p| p.exe == Exe::ConsoleHost && p.ppid == pane.shell)
            .map(|p| p.pid)
    }

    /// Windows that exist (terminal windows and other programs').
    pub fn window_ids(&self) -> BTreeSet<u64> {
        self.panes
            .values()
            .filter(|p| p.tmux.is_none())
            .map(|p| p.window)
            .chain(self.hosts.foreign_windows.keys().copied())
            .collect()
    }

    /// The socket a pane-API terminal listens on: new after a restart.
    pub fn api_socket(&self, terminal: TerminalKind) -> Option<String> {
        let pid = self.app_pid(terminal.app())?;
        match terminal {
            TerminalKind::Kitty => Some(format!("unix:/tmp/kitty-{pid}")),
            TerminalKind::WezTerm => Some(format!("/run/user/501/wezterm/gui-sock-{pid}")),
            _ => None,
        }
    }

    pub fn api_on(&self, terminal: TerminalKind) -> bool {
        self.hosts.api.get(&terminal).copied().unwrap_or(false)
    }

    /// Whether `program` runs an installed CLI: a bare name through the
    /// shell's PATH, an absolute path only if the CLI is installed there
    /// (inside WSL, at its POSIX path).
    pub fn resolves_in(&self, program: &str, wsl: bool) -> bool {
        let Some(tool) = Tool::from_executable(program) else {
            return false;
        };
        let installed = self.tools.get(&tool).is_some_and(Option::is_some);
        let at = if wsl {
            Some(super::world::tool_path(tool).to_owned())
        } else {
            self.tool_paths.get(&tool).cloned()
        };
        installed && (!program.contains(['/', '\\']) || at.as_deref() == Some(program))
    }
}
