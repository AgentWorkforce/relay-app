//! `FocusPane { TmuxPane }`, as the adapter contract says: the server
//! process must still be `server`, `%pane`'s tty must be `process`'s tty, a
//! client on `client_tty` must show the pane's session; then one tmux run
//! `switch-client -c … ; select-window … ; select-pane …`, then the client's
//! own pane (`outer`) is focused in `terminal` with the client as process.
//!
//! UNVERIFIED natively (tmux not installed).
use session_navigation::evidence::Exe;
use session_navigation::hosts::PaneRef;
use session_navigation::model::{ProcessIdentity, Reason, TerminalKind, Tool};
use session_navigation::plan::Dispatch;

use super::super::hosts::tmux::{self as host, Answer};
use super::MacExecutor;
use crate::common::tmux;
use crate::system::{join, System};

/// The fields of a [`PaneRef::TmuxPane`].
pub(super) struct Target<'a> {
    pub socket: &'a str,
    pub server: &'a ProcessIdentity,
    pub pane: &'a str,
    pub client_tty: &'a str,
    pub outer: &'a PaneRef,
}

impl<S: System> MacExecutor<S> {
    /// The first executable `tmux` on the configured search path.
    fn tmux_program(&self) -> Option<String> {
        self.cfg
            .common
            .paths
            .tool_search_path
            .iter()
            .map(|dir| join(dir, Tool::Tmux.program()))
            .find(|path| {
                self.sys
                    .metadata(path)
                    .is_ok_and(|m| m.is_file && m.executable)
            })
    }

    pub(super) fn focus_tmux(
        &self,
        terminal: TerminalKind,
        t: Target<'_>,
        process: &ProcessIdentity,
    ) -> Dispatch {
        let gone = Dispatch::Failed(Reason::TargetGone);
        if matches!(t.outer, PaneRef::TmuxPane { .. }) {
            // Nested tmux is not located by the core; never act on it.
            return Dispatch::Failed(Reason::HostUnsupported);
        }
        let shaped = tmux::is_socket_path(t.socket)
            && tmux::is_pane_id(t.pane)
            && tmux::is_tty_path(t.client_tty);
        if !shaped {
            return gone;
        }
        let (table, proc) = match self.fresh(process) {
            Ok(found) => found,
            Err(reason) => return Dispatch::Failed(reason),
        };
        let server_live = table
            .iter()
            .any(|p| p.identity == *t.server && p.exe == Exe::Tmux && p.ruid == self.cfg.uid);
        if !server_live {
            return gone;
        }
        let Some(program) = self.tmux_program() else {
            return Dispatch::Failed(Reason::DispatchFailed);
        };
        let query = |args: Vec<String>| host::ask(&self.sys, &host::run(&program, args, &self.cfg));
        let failed = |answer: Answer| match answer {
            Answer::Gone => Dispatch::Failed(Reason::TargetGone),
            _ => Dispatch::Failed(Reason::DispatchFailed),
        };
        let pid = match query(tmux::server_pid_args(t.socket)) {
            Answer::Ok(out) => tmux::parse_server_pid(&out.stdout),
            other => return failed(other),
        };
        if pid != Some(t.server.pid) {
            return gone;
        }
        let panes = match query(tmux::list_panes_args(t.socket)) {
            Answer::Ok(out) => tmux::parse_panes(&out.stdout),
            other => return failed(other),
        };
        let Some(target) = panes
            .unwrap_or_default()
            .into_iter()
            .find(|p| p.pane == t.pane && Some(p.tty.as_str()) == proc.tty.as_deref())
        else {
            return gone;
        };
        let clients = match query(tmux::list_clients_args(t.socket)) {
            Answer::Ok(out) => tmux::parse_clients(&out.stdout),
            other => return failed(other),
        };
        let Some(client) = clients
            .unwrap_or_default()
            .into_iter()
            .find(|c| c.tty == t.client_tty && c.session == target.session)
        else {
            return gone;
        };
        let Some(client_proc) = table.iter().find(|p| {
            p.identity.pid == client.pid
                && p.exe == Exe::Tmux
                && p.tty.as_deref() == Some(t.client_tty)
        }) else {
            return gone;
        };
        let Some(args) = tmux::focus_args(
            t.socket,
            t.client_tty,
            &target.session,
            &target.window,
            &target.pane,
        ) else {
            return gone;
        };
        let select =
            host::run(&program, args, &self.cfg).timeout(self.cfg.common.timeouts.dispatch);
        match self.sys.run(&select) {
            Ok(out) if out.success() => {}
            Ok(out) => {
                return match tmux::classify_error(&out.stderr) {
                    tmux::TmuxError::Gone | tmux::TmuxError::NotFound => gone,
                    tmux::TmuxError::Other => Dispatch::Failed(Reason::DispatchFailed),
                }
            }
            Err(_) => return Dispatch::Failed(Reason::DispatchFailed),
        }
        self.focus(terminal, t.outer, &client_proc.identity)
    }
}
