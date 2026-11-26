//! `FocusPane`: tty-scripted terminals (Ghostty, Terminal, iTerm2), Kitty
//! and WezTerm through their APIs, and tmux panes (in `tmux.rs`). Every
//! path re-reads the process table and re-checks the identity it was given
//! before touching the terminal.
//!
//! UNVERIFIED natively: the iTerm2 script, `kitty @ focus-window` and
//! `wezterm cli activate-pane` (none of those terminals was installed).
use session_navigation::hosts::PaneRef;
use session_navigation::model::{ProcessIdentity, Reason, TerminalKind};
use session_navigation::plan::Dispatch;

use super::super::hosts::{kitty as kitty_host, wezterm as wezterm_host};
use super::super::machine::{self, is_tty};
use super::super::scripts;
use super::{script_outcome, MacExecutor};
use crate::common::{kitty, wezterm};
use crate::system::{FsError, System};

/// Whether the socket file behind an address is gone. Anything other than
/// a positive "not found" is not proof.
fn socket_gone<S: System>(sys: &S, path: Option<&str>) -> bool {
    path.is_some_and(|p| matches!(sys.metadata(p), Err(FsError::NotFound)))
}

impl<S: System> MacExecutor<S> {
    pub(super) fn focus(
        &self,
        terminal: TerminalKind,
        pane: &PaneRef,
        process: &ProcessIdentity,
    ) -> Dispatch {
        match (terminal, pane) {
            (_, PaneRef::Tty { tty }) => self.focus_tty(terminal, tty, process),
            (TerminalKind::Kitty, PaneRef::KittyWindow { socket, id }) => {
                self.focus_kitty(socket, *id, process)
            }
            (TerminalKind::WezTerm, PaneRef::WezTermPane { socket, id }) => {
                self.focus_wezterm(socket, *id, process)
            }
            (
                _,
                PaneRef::TmuxPane {
                    socket,
                    server,
                    pane,
                    client_tty,
                    outer,
                },
            ) => self.focus_tmux(
                terminal,
                super::tmux::Target {
                    socket,
                    server,
                    pane,
                    client_tty,
                    outer,
                },
                process,
            ),
            _ => Dispatch::Failed(Reason::HostUnsupported),
        }
    }

    /// A tty pane of a tty-scripted terminal; `process` must still be on it.
    fn focus_tty(&self, terminal: TerminalKind, tty: &str, process: &ProcessIdentity) -> Dispatch {
        let script = match terminal {
            TerminalKind::Ghostty => scripts::GHOSTTY_FOCUS,
            TerminalKind::AppleTerminal => scripts::TERMINAL_FOCUS,
            TerminalKind::ITerm2 => scripts::ITERM_FOCUS,
            _ => return Dispatch::Failed(Reason::HostUnsupported),
        };
        if !is_tty(tty) {
            return Dispatch::Failed(Reason::TargetGone);
        }
        match self.find(process) {
            Ok(p) if p.tty.as_deref() == Some(tty) => {}
            Ok(_) => return Dispatch::Failed(Reason::TargetGone),
            Err(reason) => return Dispatch::Failed(reason),
        }
        let run = machine::script_run(
            script,
            &[tty.to_owned()],
            self.cfg.common.timeouts.script_act,
        );
        match script_outcome(self.sys.run(&run)) {
            Ok(out) if out == "focused" => Dispatch::Dispatched,
            Ok(_) => Dispatch::Failed(Reason::DispatchFailed),
            Err(dispatch) => dispatch,
        }
    }

    /// Map a failed `kitty @` run.
    fn kitty_failure(stderr: &[u8]) -> Dispatch {
        match kitty::classify_error(stderr) {
            kitty::RemoteError::Gone => Dispatch::Failed(Reason::TargetGone),
            kitty::RemoteError::Refused => Dispatch::Refused,
            kitty::RemoteError::Other => Dispatch::Failed(Reason::DispatchFailed),
        }
    }

    /// `ls` must list window `id` still holding `process` (its own pid or an
    /// ancestor below Kitty), then `focus-window --match id:<id>`.
    fn focus_kitty(&self, address: &str, id: u64, process: &ProcessIdentity) -> Dispatch {
        if !kitty::is_valid_address(address) || socket_gone(&self.sys, kitty::socket_path(address))
        {
            return Dispatch::Failed(Reason::TargetGone);
        }
        let (table, proc) = match self.fresh(process) {
            Ok(found) => found,
            Err(reason) => return Dispatch::Failed(reason),
        };
        let report = machine::apps(&self.sys, &self.cfg);
        let Some(program) = kitty_host::program(report.as_ref()) else {
            return Dispatch::Failed(Reason::DispatchFailed);
        };
        let ls = kitty_host::run(&program, kitty::ls_args(address), &self.cfg);
        let listed = match self.sys.run(&ls) {
            Ok(out) if out.success() => kitty::parse_ls(&out.stdout),
            Ok(out) => return Self::kitty_failure(&out.stderr),
            Err(_) => return Dispatch::Failed(Reason::DispatchFailed),
        };
        let Some(listed) = listed else {
            return Dispatch::Failed(Reason::DispatchFailed);
        };
        let holds = listed
            .iter()
            .find(|p| p.id == id)
            .is_some_and(|p| kitty::pane_holds(&table, &proc, None, p.pid));
        if !holds {
            return Dispatch::Failed(Reason::TargetGone);
        }
        let focus = kitty_host::run(&program, kitty::focus_args(address, id), &self.cfg)
            .timeout(self.cfg.common.timeouts.dispatch);
        match self.sys.run(&focus) {
            Ok(out) if out.success() => Dispatch::Dispatched,
            Ok(out) => Self::kitty_failure(&out.stderr),
            Err(_) => Dispatch::Failed(Reason::DispatchFailed),
        }
    }

    fn wezterm_failure(stderr: &[u8]) -> Dispatch {
        match wezterm::classify_error(stderr) {
            wezterm::CliError::Gone | wezterm::CliError::NoSuchPane => {
                Dispatch::Failed(Reason::TargetGone)
            }
            wezterm::CliError::Other => Dispatch::Failed(Reason::DispatchFailed),
        }
    }

    /// `list` on `socket` must show pane `id` with `process`'s tty, then
    /// `activate-pane --pane-id <id>`.
    fn focus_wezterm(&self, socket: &str, id: u64, process: &ProcessIdentity) -> Dispatch {
        if !wezterm::is_valid_socket(socket) || socket_gone(&self.sys, Some(socket)) {
            return Dispatch::Failed(Reason::TargetGone);
        }
        let (table, proc) = match self.fresh(process) {
            Ok(found) => found,
            Err(reason) => return Dispatch::Failed(reason),
        };
        let report = machine::apps(&self.sys, &self.cfg);
        let Some(program) = wezterm_host::program(report.as_ref()) else {
            return Dispatch::Failed(Reason::DispatchFailed);
        };
        let list = wezterm_host::run(&program, wezterm::list_args(), socket, &self.cfg);
        let listed = match self.sys.run(&list) {
            Ok(out) if out.success() => wezterm::parse_list(&out.stdout),
            Ok(out) => return Self::wezterm_failure(&out.stderr),
            Err(_) => return Dispatch::Failed(Reason::DispatchFailed),
        };
        let Some(listed) = listed else {
            return Dispatch::Failed(Reason::DispatchFailed);
        };
        let holds = listed
            .iter()
            .find(|p| p.id == id)
            .is_some_and(|p| wezterm::pane_holds(&table, &proc, p.tty.as_deref(), None));
        if !holds {
            return Dispatch::Failed(Reason::TargetGone);
        }
        let activate = wezterm_host::run(&program, wezterm::activate_args(id), socket, &self.cfg)
            .timeout(self.cfg.common.timeouts.dispatch);
        match self.sys.run(&activate) {
            Ok(out) if out.success() => Dispatch::Dispatched,
            Ok(out) => Self::wezterm_failure(&out.stderr),
            Err(_) => Dispatch::Failed(Reason::DispatchFailed),
        }
    }
}
