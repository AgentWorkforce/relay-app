//! What an adapter can observe about terminal hosts in the fake machine:
//! the process table of a runtime (no ttys on Windows, the distribution's
//! own table for WSL), each terminal's abilities on this platform, pane-API
//! listings, tmux listings, OS windows and the front.
use super::world::World;
use crate::evidence::{Fact, ProcessInfo};
use crate::hosts::{
    Ability, PaneInfo, PaneRef, Setting, Surfaces, TerminalSupport, Unsupported, WindowId,
    WindowInfo,
};
use crate::model::{App, BootId, DisplayServer, Platform, ProcessIdentity, TerminalKind};

/// (pane tty, API pane, OS window) of the front.
pub(crate) type FrontPanes = (
    Fact<Option<String>>,
    Fact<Option<PaneRef>>,
    Fact<Option<WindowId>>,
);

/// Terminals that export `WINDOWID` to their children on X11.
fn exports_window_id(terminal: TerminalKind) -> bool {
    matches!(
        terminal,
        TerminalKind::Kitty | TerminalKind::Alacritty | TerminalKind::Xterm | TerminalKind::Konsole
    )
}

fn api_terminal(terminal: TerminalKind) -> bool {
    matches!(terminal, TerminalKind::Kitty | TerminalKind::WezTerm)
}

impl World {
    fn x11(&self) -> bool {
        self.platform
            == Platform::Linux {
                session: Fact::Known(DisplayServer::X11),
            }
    }

    fn windowed(&self) -> bool {
        self.x11() || self.platform == Platform::Windows
    }

    pub fn identity(&self, pid: u32) -> Option<ProcessIdentity> {
        self.procs.get(&pid).map(|p| ProcessIdentity {
            pid,
            start: p.start,
            boot: BootId(self.boot),
        })
    }

    /// The process table of the native runtime, or of the WSL distribution.
    pub(crate) fn table_for(&self, wsl: bool) -> Vec<ProcessInfo> {
        let ttys = self.platform != Platform::Windows || wsl;
        self.procs
            .values()
            .filter(|p| self.wsl_procs.contains(&p.pid) == wsl)
            .filter(|p| {
                p.tty
                    .as_ref()
                    .is_none_or(|t| !self.hosts.hidden_ttys.contains(t))
            })
            .map(|p| {
                let pane = p
                    .tty
                    .as_ref()
                    .and_then(|t| self.panes.values().find(|pane| pane.tty == *t));
                let window = pane
                    .filter(|pane| self.x11() && pane.tmux.is_none())
                    .filter(|pane| exports_window_id(pane.terminal))
                    .map(|pane| WindowId(pane.window));
                ProcessInfo {
                    identity: ProcessIdentity {
                        pid: p.pid,
                        start: p.start,
                        boot: BootId(self.boot),
                    },
                    ppid: p.ppid,
                    euid: p.euid,
                    ruid: p.ruid,
                    exe: p.exe,
                    tty: if ttys { p.tty.clone() } else { None },
                    argv: p.argv.clone(),
                    // A root-owned process's cwd is not readable.
                    cwd: match (&p.cwd, p.euid) {
                        (Some(cwd), euid) if euid != 0 => Fact::Known(cwd.clone()),
                        _ => Fact::Unknown,
                    },
                    window,
                }
            })
            .collect()
    }

    /// What the adapter reports each terminal can do here.
    pub fn support(&self, terminal: TerminalKind) -> TerminalSupport {
        let no = |why| Ability::Unsupported(why);
        if !terminal.exists_on(self.platform) {
            return TerminalSupport {
                terminal,
                focus_pane: no(Unsupported::NotOnThisPlatform),
                raise_window: no(Unsupported::NotOnThisPlatform),
                activate_app: no(Unsupported::NotOnThisPlatform),
                launch: no(Unsupported::NotOnThisPlatform),
                tabs_share_window: true,
            };
        }
        let scripted = self.platform == Platform::MacOs
            && matches!(
                terminal,
                TerminalKind::Ghostty | TerminalKind::AppleTerminal | TerminalKind::ITerm2
            );
        let focus_pane = if scripted {
            Ability::Available
        } else if api_terminal(terminal) {
            match (self.api_on(terminal), terminal) {
                (true, _) => Ability::Available,
                (false, TerminalKind::Kitty) => Ability::NeedsSetting(Setting::KittyRemoteControl),
                (false, _) => Ability::NeedsSetting(Setting::WezTermMuxSocket),
            }
        } else {
            no(Unsupported::NoPaneApi)
        };
        let (raise_window, activate_app) = match self.platform {
            Platform::MacOs => (no(Unsupported::NotImplemented), Ability::Available),
            Platform::Linux {
                session: Fact::Known(DisplayServer::X11),
            } => (Ability::Available, no(Unsupported::NotImplemented)),
            Platform::Linux { .. } => (no(Unsupported::Wayland), no(Unsupported::Wayland)),
            Platform::Windows => (Ability::Available, no(Unsupported::NotImplemented)),
        };
        TerminalSupport {
            terminal,
            focus_pane,
            raise_window,
            activate_app,
            launch: Ability::Available,
            tabs_share_window: super::hosts::tabbed(terminal),
        }
    }

    pub(crate) fn surfaces(&self) -> Surfaces {
        let terminals = TerminalKind::ALL
            .into_iter()
            .filter(|t| t.exists_on(self.platform))
            .map(|t| self.support(t))
            .collect();
        let panes = self
            .panes
            .values()
            .filter(|p| p.tmux.is_none() && api_terminal(p.terminal) && self.api_on(p.terminal))
            .filter_map(|p| {
                let socket = self.api_socket(p.terminal)?;
                let pane = match p.terminal {
                    TerminalKind::Kitty => PaneRef::KittyWindow { socket, id: p.api },
                    _ => PaneRef::WezTermPane { socket, id: p.api },
                };
                Some(PaneInfo {
                    terminal: p.terminal,
                    pane,
                    // WezTerm reports `tty_name`; Kitty does not.
                    tty: (p.terminal == TerminalKind::WezTerm).then(|| p.tty.clone()),
                    pid: Some(p.shell),
                })
            })
            .collect();
        let tmux = if self.channels.tmux {
            Fact::Known(self.tmux_listing())
        } else {
            Fact::Unknown
        };
        let windows = if self.windowed() {
            self.window_ids()
                .into_iter()
                .filter_map(|id| {
                    let owner = self.identity(self.window_owner(id)?)?;
                    Some(WindowInfo {
                        id: WindowId(id),
                        owner,
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        Surfaces {
            terminals: Fact::Known(terminals),
            panes: Fact::Known(panes),
            tmux,
            windows: Fact::Known(windows),
        }
    }

    /// (pane tty, API pane, OS window) of the front.
    pub(crate) fn front_panes(&self) -> FrontPanes {
        let Some(terminal) = self.front_app.and_then(App::terminal) else {
            return (Fact::Known(None), Fact::Known(None), Fact::Known(None));
        };
        let pane = self.front_pane.and_then(|id| self.panes.get(&id));
        let tty = pane.map(|p| p.tty.clone());
        let scripted = matches!(
            terminal,
            TerminalKind::Ghostty | TerminalKind::AppleTerminal | TerminalKind::ITerm2
        );
        let pane_tty = match self.platform {
            Platform::Windows => Fact::Known(None),
            Platform::MacOs if scripted => {
                if self
                    .automation
                    .get(&terminal.app())
                    .copied()
                    .unwrap_or(false)
                {
                    Fact::Known(tty.clone())
                } else {
                    Fact::Unknown
                }
            }
            _ if api_terminal(terminal) && self.api_on(terminal) => Fact::Known(tty.clone()),
            _ => Fact::Unknown,
        };
        let api = if !api_terminal(terminal) {
            Fact::Known(None)
        } else if !self.api_on(terminal) {
            Fact::Unknown
        } else {
            Fact::Known(pane.and_then(|p| {
                let socket = self.api_socket(terminal)?;
                Some(match terminal {
                    TerminalKind::Kitty => PaneRef::KittyWindow { socket, id: p.api },
                    _ => PaneRef::WezTermPane { socket, id: p.api },
                })
            }))
        };
        let window = if self.windowed() {
            Fact::Known(pane.map(|p| WindowId(p.window)))
        } else if self.platform == Platform::MacOs {
            Fact::Known(None)
        } else {
            Fact::Unknown
        };
        (pane_tty, api, window)
    }
}
