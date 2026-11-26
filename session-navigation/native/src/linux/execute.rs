//! [`Execute`] for Linux. Every step re-reads the boot and the process
//! table and re-checks each identity it carries (pid + start + boot)
//! before acting; a pane, window or tmux id is never trusted on its own.
use session_navigation::evidence::{Fact, ProcessInfo};
use session_navigation::hosts::{PaneRef, WindowId};
use session_navigation::model::{DisplayServer, ProcessIdentity, Reason, Runtime, TerminalKind};
use session_navigation::plan::{Command, Dispatch, LaunchContext, Step};
use session_navigation::ports::Execute;

use super::apps::which;
use super::classify::is_agent_cli;
use super::config::LinuxConfig;
use super::launch;
use super::panes;
use super::support::Resolved;
use super::table::{self, BootClock};
use super::{tmux, x11};
use crate::common::kitty::{self, RemoteError};
use crate::common::tmux::TmuxError;
use crate::common::wezterm::CliError;
use crate::system::{Run, System};

pub struct LinuxExecutor<S> {
    sys: S,
    cfg: LinuxConfig,
    clock: BootClock,
}

fn failed(reason: Reason) -> Dispatch {
    Dispatch::Failed(reason)
}

impl<S: System> LinuxExecutor<S> {
    pub fn new(sys: S, cfg: LinuxConfig) -> Self {
        Self::with_clock(sys, cfg, BootClock::new())
    }

    /// `clock` is shared with the observer so both compute the same start
    /// times.
    pub fn with_clock(sys: S, cfg: LinuxConfig, clock: BootClock) -> Self {
        Self { sys, cfg, clock }
    }

    /// A fresh table of the current boot.
    fn table(&self) -> Result<Vec<ProcessInfo>, Reason> {
        let boot = self
            .clock
            .read(&self.sys)
            .ok_or(Reason::EvidenceIncomplete)?;
        table::read_table(&self.sys, &self.cfg, boot)
            .map(|t| t.processes)
            .ok_or(Reason::EvidenceIncomplete)
    }

    /// The live process with exactly this identity in `table`.
    fn find(table: &[ProcessInfo], identity: &ProcessIdentity) -> Result<ProcessInfo, Reason> {
        table
            .iter()
            .find(|p| p.identity == *identity)
            .cloned()
            .ok_or(Reason::TargetGone)
    }

    fn focus(&self, terminal: TerminalKind, pane: &PaneRef, process: &ProcessIdentity) -> Dispatch {
        let table = match self.table() {
            Ok(table) => table,
            Err(reason) => return failed(reason),
        };
        let target = match Self::find(&table, process) {
            Ok(p) => p,
            Err(reason) => return failed(reason),
        };
        let resolved = Resolved::find(&self.sys, &self.cfg);
        match pane {
            PaneRef::KittyWindow { socket, id } if terminal == TerminalKind::Kitty => {
                self.focus_kitty(&resolved, &table, &target, socket, *id)
            }
            PaneRef::WezTermPane { socket, id } if terminal == TerminalKind::WezTerm => {
                self.focus_wezterm(&resolved, &table, &target, socket, *id)
            }
            PaneRef::X11Window { id, owner } => self.raise(&resolved, &table, *id, owner),
            PaneRef::TmuxPane {
                socket,
                server,
                pane,
                client_tty,
                outer,
            } => self.focus_tmux(
                &table, &target, socket, server, pane, client_tty, terminal, outer,
            ),
            _ => failed(Reason::HostUnsupported),
        }
    }

    fn focus_kitty(
        &self,
        resolved: &Resolved,
        table: &[ProcessInfo],
        target: &ProcessInfo,
        address: &str,
        id: u64,
    ) -> Dispatch {
        let Some(program) = resolved.launcher(TerminalKind::Kitty) else {
            return failed(Reason::HostUnsupported);
        };
        let remote = |error| match error {
            RemoteError::Gone => failed(Reason::TargetGone),
            RemoteError::Refused => Dispatch::Refused,
            RemoteError::Other => failed(Reason::DispatchFailed),
        };
        let listed = match panes::kitty_ls(&self.sys, &self.cfg, program, address) {
            Ok(listed) => listed,
            Err(error) => return remote(error),
        };
        let holds = listed
            .iter()
            .find(|p| p.id == id)
            .is_some_and(|p| kitty::pane_holds(table, target, None, p.pid));
        if !holds {
            return failed(Reason::TargetGone);
        }
        match panes::kitty_focus(&self.sys, &self.cfg, program, address, id) {
            Ok(()) => Dispatch::Dispatched,
            Err(error) => remote(error),
        }
    }

    fn focus_wezterm(
        &self,
        resolved: &Resolved,
        table: &[ProcessInfo],
        target: &ProcessInfo,
        socket: &str,
        id: u64,
    ) -> Dispatch {
        let Some(program) = resolved.launcher(TerminalKind::WezTerm) else {
            return failed(Reason::HostUnsupported);
        };
        let cli = |error| match error {
            CliError::Gone | CliError::NoSuchPane => failed(Reason::TargetGone),
            CliError::Other => failed(Reason::DispatchFailed),
        };
        let listed = match panes::wez_list(&self.sys, &self.cfg, program, socket) {
            Ok(listed) => listed,
            Err(error) => return cli(error),
        };
        let holds = listed
            .iter()
            .find(|p| p.id == id)
            .is_some_and(|p| kitty::pane_holds(table, target, p.tty.as_deref(), None));
        if !holds {
            return failed(Reason::TargetGone);
        }
        match panes::wez_activate(&self.sys, &self.cfg, program, socket, id) {
            Ok(()) => Dispatch::Dispatched,
            Err(error) => cli(error),
        }
    }

    /// `_NET_WM_PID` of `id` must still be `owner`, which must still be
    /// alive with the same start, then `xdotool windowactivate --sync`.
    fn raise(
        &self,
        resolved: &Resolved,
        table: &[ProcessInfo],
        id: WindowId,
        owner: &ProcessIdentity,
    ) -> Dispatch {
        if self.cfg.display_server() != Fact::Known(DisplayServer::X11) {
            return failed(Reason::PlatformUnsupported);
        }
        let Some(xdotool) = resolved.x11.xdotool.as_deref() else {
            return failed(Reason::HostUnsupported);
        };
        if Self::find(table, owner).is_err() {
            return failed(Reason::TargetGone);
        }
        if x11::window_pid(&self.sys, &self.cfg, xdotool, id) != Some(owner.pid) {
            return failed(Reason::TargetGone);
        }
        let mut run = x11::run(xdotool, x11::activate_args(id), &self.cfg);
        run.timeout = self.cfg.common.timeouts.dispatch;
        match self.sys.run(&run) {
            Ok(output) if output.success() => Dispatch::Dispatched,
            _ => failed(Reason::DispatchFailed),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn focus_tmux(
        &self,
        table: &[ProcessInfo],
        target: &ProcessInfo,
        socket: &str,
        server: &ProcessIdentity,
        pane: &str,
        client_tty: &str,
        terminal: TerminalKind,
        outer: &PaneRef,
    ) -> Dispatch {
        if matches!(outer, PaneRef::TmuxPane { .. }) {
            return failed(Reason::HostUnsupported);
        }
        let Some(program) = which(&self.sys, &self.cfg.common.paths.tool_search_path, "tmux")
        else {
            return failed(Reason::HostUnsupported);
        };
        let gone = |error| match error {
            TmuxError::Gone | TmuxError::NotFound => failed(Reason::TargetGone),
            TmuxError::Other => failed(Reason::DispatchFailed),
        };
        let srv = match tmux::server(&self.sys, &self.cfg, &program, socket, table) {
            Ok(srv) => srv,
            Err(error) => return gone(error),
        };
        if srv.server != *server {
            return failed(Reason::TargetGone);
        }
        let Some(listed) = srv.panes.iter().find(|p| p.pane == pane) else {
            return failed(Reason::TargetGone);
        };
        if target.tty.as_deref() != Some(listed.tty.as_str()) {
            return failed(Reason::TargetGone);
        }
        let client = srv
            .clients
            .iter()
            .find(|c| c.tty == client_tty && c.session == listed.session);
        let client_process = client.and_then(|c| {
            table.iter().find(|p| {
                p.identity.pid == c.pid
                    && p.exe == session_navigation::evidence::Exe::Tmux
                    && p.tty.as_deref() == Some(client_tty)
            })
        });
        let Some(client_process) = client_process else {
            return failed(Reason::TargetGone);
        };
        let Some(args) = crate::common::tmux::focus_args(
            socket,
            client_tty,
            &listed.session,
            &listed.window,
            pane,
        ) else {
            return failed(Reason::DispatchFailed);
        };
        let mut run = tmux::run(&self.cfg, &program, args);
        run.timeout = self.cfg.common.timeouts.dispatch;
        if let Err(error) = tmux::call(&self.sys, &run) {
            return gone(error);
        }
        self.focus(terminal, outer, &client_process.identity)
    }

    /// Only native launches, in the terminal's own argv form.
    fn launch(
        &self,
        terminal: TerminalKind,
        command: &Command,
        context: &LaunchContext,
    ) -> Dispatch {
        if context.runtime != Runtime::Native || context.profile.is_some() {
            return failed(Reason::PlatformUnsupported);
        }
        if !self.cfg.display_server().is_known() {
            return failed(Reason::HostUnsupported);
        }
        let resolved = Resolved::find(&self.sys, &self.cfg);
        let Some(program) = resolved.launcher(terminal) else {
            return failed(Reason::HostUnsupported);
        };
        if launch::check(&self.sys, command).is_err() {
            return failed(Reason::DispatchFailed);
        }
        let args = match launch::terminal_args(terminal, command, resolved.env.as_deref()) {
            Ok(args) => args,
            Err(launch::LaunchError::Terminal) => return failed(Reason::HostUnsupported),
            Err(_) => return failed(Reason::DispatchFailed),
        };
        let mut run = Run::with_args(program, args).timeout(self.cfg.common.timeouts.dispatch);
        run.env = self.cfg.session_env.clone();
        match self.sys.spawn_detached(&run) {
            // The new window's tty is not reported by any of these
            // terminals' command lines: the engine observes it.
            Ok(()) => Dispatch::Dispatched,
            Err(_) => failed(Reason::DispatchFailed),
        }
    }

    /// Signal only an agent CLI the person started, by exact identity.
    fn stop(&self, process: &ProcessIdentity) -> Dispatch {
        let target = match self.table().and_then(|t| Self::find(&t, process)) {
            Ok(p) => p,
            Err(_) => return failed(Reason::TargetGone),
        };
        let ours =
            is_agent_cli(target.exe) && target.ruid == self.cfg.uid && target.identity.pid > 1;
        if !ours {
            return failed(Reason::TargetGone);
        }
        let Some(kill) = which(&self.sys, &self.cfg.program_path, super::support::KILL) else {
            return failed(Reason::DispatchFailed);
        };
        let pid = target.identity.pid.to_string();
        let run = Run::new(&kill, &["-TERM", &pid]).timeout(self.cfg.common.timeouts.dispatch);
        match self.sys.run(&run) {
            Ok(output) if output.success() => Dispatch::Dispatched,
            _ => failed(Reason::DispatchFailed),
        }
    }
}

impl<S: System> Execute for LinuxExecutor<S> {
    fn execute(&mut self, step: &Step) -> Dispatch {
        match step {
            // No provider app registers a URL scheme on Linux.
            Step::OpenUrl { .. } => failed(Reason::PlatformUnsupported),
            // No generic app activation on Linux (see `support`).
            Step::ActivateApp { .. } => failed(Reason::HostUnsupported),
            Step::FocusPane {
                terminal,
                pane,
                process,
            } => self.focus(*terminal, pane, process),
            Step::Launch {
                terminal,
                command,
                context,
            } => self.launch(*terminal, command, context),
            Step::Stop { process } => self.stop(process),
            // The engine observes the exit itself.
            Step::AwaitExit { .. } => Dispatch::Dispatched,
        }
    }
}
