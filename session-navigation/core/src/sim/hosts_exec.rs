//! Focusing panes and raising windows in the fake machine, the way a
//! contract-abiding adapter does it: every identity a [`PaneRef`] carries
//! (socket, window owner, tmux server, the process on the pane) is checked
//! against the machine before anything moves, and hazards armed by
//! [`super::hosts::Hazard`] strike between the decision and the first step.
use super::hosts::Hazard;
use super::truth::Exit;
use super::world::{Role, World};
use crate::evidence::Exe;
use crate::evidence::Fact;
use crate::hosts::PaneRef;
use crate::model::{DisplayServer, Platform, ProcessIdentity, Reason, TerminalKind};
use crate::plan::{Dispatch, Step};

impl World {
    fn identity_ok(&self, process: &ProcessIdentity) -> bool {
        self.procs
            .get(&process.pid)
            .is_some_and(|p| p.start == process.start)
    }

    fn api_pane(&self, terminal: TerminalKind, id: u64) -> Option<u64> {
        self.panes
            .values()
            .find(|p| p.terminal == terminal && p.api == id && p.tmux.is_none())
            .map(|p| p.id)
    }

    fn tmux_pane_by_ref(&self, socket: &str, pane: &str) -> Option<u64> {
        let number: u32 = pane.strip_prefix('%')?.parse().ok()?;
        self.panes
            .values()
            .find(|p| {
                p.tmux
                    .as_ref()
                    .is_some_and(|l| l.socket == socket && l.pane == number)
            })
            .map(|p| p.id)
    }

    /// The pane a ref resolves to by its raw mechanism (no identity check):
    /// what the step actually hits.
    pub fn pane_of_ref(&self, terminal: TerminalKind, pane: &PaneRef) -> Option<u64> {
        match pane {
            PaneRef::Tty { tty } => self.pane_by_tty(terminal, tty).map(|p| p.id),
            PaneRef::KittyWindow { id, .. } | PaneRef::WezTermPane { id, .. } => {
                self.api_pane(terminal, *id)
            }
            PaneRef::X11Window { id, .. } | PaneRef::WindowsWindow { hwnd: id, .. } => {
                self.hosts.active.get(&id.0).copied()
            }
            PaneRef::TmuxPane { socket, pane, .. } => self.tmux_pane_by_ref(socket, pane),
        }
    }

    /// The process the step targets, for the oracle's stale-handle check.
    pub fn target_of(&self, step: &Step) -> Option<u32> {
        match step {
            // A raised window holds the process if any of its tabs does.
            Step::FocusPane {
                pane: PaneRef::X11Window { id, .. } | PaneRef::WindowsWindow { hwnd: id, .. },
                process,
                ..
            } if self
                .panes
                .values()
                .any(|p| p.window == id.0 && p.tmux.is_none() && self.holds(p.id, process)) =>
            {
                Some(process.pid)
            }
            Step::FocusPane { terminal, pane, .. } => self
                .pane_of_ref(*terminal, pane)
                .and_then(|id| self.panes.get(&id))
                .and_then(|p| self.pane_agent(p))
                .map(|p| p.pid),
            Step::Stop { process } => Some(process.pid),
            _ => None,
        }
    }

    fn raise(&mut self, terminal: TerminalKind, pane: u64) {
        self.front_app = Some(terminal.app());
        self.front_pane = Some(pane);
    }

    fn select(&mut self, pane: u64) {
        if let Some(window) = self.panes.get(&pane).map(|p| p.window) {
            self.hosts.active.insert(window, pane);
        }
    }

    /// Focus `pane` of `terminal`, which hosted `process` when observed.
    pub(crate) fn focus_ref(
        &mut self,
        terminal: TerminalKind,
        pane: &PaneRef,
        process: &ProcessIdentity,
    ) -> Dispatch {
        match pane {
            PaneRef::Tty { tty } => {
                let scripted = matches!(
                    terminal,
                    TerminalKind::Ghostty | TerminalKind::AppleTerminal | TerminalKind::ITerm2
                );
                if !scripted || self.platform != Platform::MacOs {
                    return Dispatch::Failed(Reason::HostUnsupported);
                }
                // The adapter re-reads the table first (README, macOS
                // executor): the process must still be on that tty.
                let on_tty = self.procs.get(&process.pid).is_some_and(|p| {
                    p.start == process.start && p.tty.as_deref() == Some(tty.as_str())
                });
                if !on_tty && self.running(terminal.app()) {
                    return Dispatch::Failed(Reason::TargetGone);
                }
                self.focus_pane(terminal, tty)
            }
            PaneRef::KittyWindow { socket, id } | PaneRef::WezTermPane { socket, id } => {
                if self.api_socket(terminal).as_deref() != Some(socket.as_str()) {
                    return Dispatch::Failed(Reason::TargetGone);
                }
                if !self.api_on(terminal) {
                    return Dispatch::Refused;
                }
                let Some(target) = self.api_pane(terminal, *id) else {
                    return Dispatch::Failed(Reason::TargetGone);
                };
                if !self.holds(target, process) {
                    return Dispatch::Failed(Reason::TargetGone);
                }
                self.select(target);
                let raises = match self.platform {
                    Platform::Linux {
                        session: Fact::Known(DisplayServer::Wayland) | Fact::Unknown,
                    } => self.vars.wayland_pane_focus_raises,
                    _ => self.vars.pane_api_focus_raises,
                };
                if raises {
                    self.raise(terminal, target);
                }
                Dispatch::Dispatched
            }
            PaneRef::X11Window { id, owner } | PaneRef::WindowsWindow { hwnd: id, owner } => {
                let owned = self.window_owner(id.0) == Some(owner.pid) && self.identity_ok(owner);
                let Some(target) = self.hosts.active.get(&id.0).copied().filter(|_| owned) else {
                    return Dispatch::Failed(Reason::TargetGone);
                };
                if self.platform == Platform::Windows && !self.vars.windows_foreground_allowed {
                    return Dispatch::Failed(Reason::DispatchFailed);
                }
                if self.platform != Platform::Windows && !self.vars.x11_activation_honored {
                    return Dispatch::Dispatched;
                }
                self.raise(terminal, target);
                Dispatch::Dispatched
            }
            PaneRef::TmuxPane {
                socket,
                server,
                pane,
                client_tty,
                outer,
            } => {
                let alive = self
                    .tmux
                    .servers
                    .get(socket)
                    .is_some_and(|s| s.pid == server.pid)
                    && self.identity_ok(server);
                let target = self.tmux_pane_by_ref(socket, pane).filter(|_| alive);
                let Some(target) = target.filter(|t| self.holds(*t, process)) else {
                    return Dispatch::Failed(Reason::TargetGone);
                };
                let Some(session) = self.panes.get(&target).and_then(|p| p.tmux.clone()) else {
                    return Dispatch::Failed(Reason::TargetGone);
                };
                let client = self.procs.values().find(|p| {
                    p.tty.as_deref() == Some(client_tty.as_str())
                        && p.role
                            == Role::TmuxClient {
                                socket: socket.clone(),
                                session: session.session,
                            }
                });
                let Some(client) = client.map(|c| ProcessIdentity {
                    pid: c.pid,
                    start: c.start,
                    boot: crate::model::BootId(self.boot),
                }) else {
                    return Dispatch::Failed(Reason::TargetGone);
                };
                if let Some(srv) = self.tmux.servers.get_mut(socket) {
                    srv.sessions.insert(session.session, target);
                }
                self.focus_ref(terminal, outer, &client)
            }
        }
    }

    /// Whether `process` (by identity) is on pane `pane` now: on its tty
    /// (internally even on Windows), or is its tmux client.
    fn holds(&self, pane: u64, process: &ProcessIdentity) -> bool {
        let Some(tty) = self.panes.get(&pane).map(|p| p.tty.clone()) else {
            return false;
        };
        self.procs
            .get(&process.pid)
            .is_some_and(|p| p.start == process.start && p.tty.as_deref() == Some(tty.as_str()))
    }

    /// Apply the armed hazard right before `step` runs.
    pub(crate) fn strike(&mut self, step: &Step) {
        let Some(hazard) = self.race.take() else {
            return;
        };
        let terminal = match step {
            Step::FocusPane { terminal, .. } | Step::Launch { terminal, .. } => Some(*terminal),
            _ => None,
        };
        match hazard {
            Hazard::RevokeApi => {
                for on in self.hosts.api.values_mut() {
                    *on = false;
                }
            }
            Hazard::RevokeAutomation => {
                for granted in self.automation.values_mut() {
                    *granted = false;
                }
            }
            Hazard::RestartTerminal => {
                if let Some(t) = terminal.filter(|t| *t != TerminalKind::ConsoleHost) {
                    self.quit_app(t.app());
                    self.ensure_app(t.app());
                }
            }
            Hazard::RestartTmux => self.tmux_restart(),
            Hazard::DetachTmux => {
                let clients: Vec<u32> = self
                    .procs
                    .values()
                    .filter(|p| matches!(p.role, Role::TmuxClient { .. }))
                    .map(|p| p.pid)
                    .collect();
                for pid in clients {
                    self.kill(pid, Exit::Normal);
                }
            }
            Hazard::ReuseWindow => self.reuse_window(step),
        }
    }

    /// The step's window closes; a new window of another process takes its
    /// id: another program's, or (half the time) a new console or a
    /// restarted terminal's, with a shell in it.
    fn reuse_window(&mut self, step: &Step) {
        let Step::FocusPane {
            terminal,
            pane: PaneRef::X11Window { id, .. } | PaneRef::WindowsWindow { hwnd: id, .. },
            ..
        } = step
        else {
            return;
        };
        let panes: Vec<u64> = self
            .panes
            .values()
            .filter(|p| p.window == id.0 && p.tmux.is_none())
            .map(|p| p.id)
            .collect();
        for pane in panes {
            self.close_pane(pane);
        }
        if self.hosts.rng.chance(50) {
            if *terminal != TerminalKind::ConsoleHost {
                self.quit_app(terminal.app());
            }
            if let Some(pane) = self.open_pane_with(*terminal, "/", Some(false), false) {
                let old = self.panes.get(&pane).map(|p| p.window);
                if let Some(p) = self.panes.get_mut(&pane) {
                    p.window = id.0;
                }
                if let Some(old) = old {
                    self.hosts.active.remove(&old);
                }
                self.hosts.active.insert(id.0, pane);
                return;
            }
        }
        let other = self.spawn(super::apps::bare(
            1,
            Exe::Other,
            &["other-app"],
            Role::OtherApp,
        ));
        self.hosts.foreign_windows.insert(id.0, other);
    }
}
