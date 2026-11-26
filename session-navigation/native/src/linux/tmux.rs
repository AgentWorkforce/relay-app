//! tmux servers on Linux, over the shared formats in
//! [`crate::common::tmux`]. Sockets come from `<TMUX_TMPDIR>/tmux-<uid>/`
//! and from the `TMUX` variable of every process of the person (a server
//! started with `-S` elsewhere). The server process is resolved to its full
//! identity in the table: pane ids are only meaningful for that
//! incarnation.
use std::collections::BTreeSet;

use session_navigation::evidence::{Exe, Fact, ProcessInfo};
use session_navigation::hosts::TmuxServer;

use super::config::LinuxConfig;
use super::procfs;
use super::table::Table;
use crate::common::tmux::{self, TmuxError};
use crate::system::{join, FsError, Run, System};

pub fn run(cfg: &LinuxConfig, program: &str, args: Vec<String>) -> Run {
    Run::with_args(program, args).timeout(cfg.common.timeouts.probe)
}

/// stdout of a tmux run, or why it failed.
pub fn call<S: System>(sys: &S, run: &Run) -> Result<Vec<u8>, TmuxError> {
    match sys.run(run) {
        Ok(output) if output.success() => Ok(output.stdout),
        Ok(output) => Err(tmux::classify_error(&output.stderr)),
        Err(_) => Err(TmuxError::Other),
    }
}

/// The server on `socket`, its panes and clients.
pub fn server<S: System>(
    sys: &S,
    cfg: &LinuxConfig,
    program: &str,
    socket: &str,
    table: &[ProcessInfo],
) -> Result<TmuxServer, TmuxError> {
    if !tmux::is_socket_path(socket) {
        return Err(TmuxError::Other);
    }
    let pid_out = call(sys, &run(cfg, program, tmux::server_pid_args(socket)))?;
    let pid = tmux::parse_server_pid(&pid_out).ok_or(TmuxError::Other)?;
    let server = table
        .iter()
        .find(|p| p.identity.pid == pid && p.exe == Exe::Tmux)
        .ok_or(TmuxError::Other)?;
    let panes = call(sys, &run(cfg, program, tmux::list_panes_args(socket)))?;
    let clients = call(sys, &run(cfg, program, tmux::list_clients_args(socket)))?;
    Ok(TmuxServer {
        socket: socket.to_owned(),
        server: server.identity,
        panes: tmux::parse_panes(&panes).ok_or(TmuxError::Other)?,
        clients: tmux::parse_clients(&clients).ok_or(TmuxError::Other)?,
    })
}

/// Every candidate socket: the person's socket directory and every
/// `TMUX` value in the table. `None` when the directory exists but cannot
/// be listed.
pub fn sockets<S: System>(sys: &S, cfg: &LinuxConfig, table: &Table) -> Option<BTreeSet<String>> {
    let mut out: BTreeSet<String> = table
        .identities
        .values()
        .filter_map(|i| i.tmux.as_deref().and_then(procfs::tmux_socket))
        .map(str::to_owned)
        .collect();
    let dir = tmux::socket_dir(&cfg.tmux_tmpdir, cfg.uid);
    match sys.list_dir(&dir) {
        Ok(names) => out.extend(names.iter().map(|n| join(&dir, n))),
        Err(FsError::NotFound) => {}
        Err(_) => return None,
    }
    Some(out)
}

/// Every reachable server. A socket nobody listens on is skipped; one
/// that cannot be read makes the list `Unknown`, as does a socket with no
/// tmux to ask.
pub fn servers<S: System>(
    sys: &S,
    cfg: &LinuxConfig,
    program: Option<&str>,
    table: &Table,
) -> Fact<Vec<TmuxServer>> {
    let Some(sockets) = sockets(sys, cfg, table) else {
        return Fact::Unknown;
    };
    if sockets.is_empty() {
        return Fact::Known(Vec::new());
    }
    let Some(program) = program else {
        return Fact::Unknown;
    };
    let mut out = Vec::new();
    for socket in sockets {
        match server(sys, cfg, program, &socket, &table.processes) {
            Ok(srv) => out.push(srv),
            Err(TmuxError::Gone) => {}
            Err(_) => return Fact::Unknown,
        }
    }
    Fact::Known(out)
}
