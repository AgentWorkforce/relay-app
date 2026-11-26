//! tmux servers on macOS: every socket under `<TMUX_TMPDIR>/tmux-<uid>`,
//! its server process (by pid from `list-sessions -F '#{pid}'`, identity from
//! the kernel table), panes and clients.
//!
//! UNVERIFIED (tmux not installed): the socket directory, the formats and
//! the "no server running" error text are tmux's documented defaults.
use session_navigation::evidence::{Exe, Fact, ProcessInfo};
use session_navigation::hosts::TmuxServer;
use session_navigation::model::ProcessIdentity;

use super::super::config::MacConfig;
use crate::common::tmux as api;
use crate::system::{join, FsError, Output, Run, System};

/// One tmux run.
pub fn run(program: &str, args: Vec<String>, cfg: &MacConfig) -> Run {
    Run::with_args(program, args)
        .timeout(cfg.common.timeouts.probe)
        .max_output(4 * 1024 * 1024)
}

/// The socket directory for this person.
pub fn socket_dir(cfg: &MacConfig) -> String {
    api::socket_dir(&cfg.hosts.tmux_tmpdir, cfg.uid)
}

/// What one tmux query gave.
pub enum Answer {
    Ok(Output),
    /// No server listens: a stale socket.
    Gone,
    Failed,
}

pub fn ask<S: System>(sys: &S, run: &Run) -> Answer {
    match sys.run(run) {
        Ok(output) if output.success() => Answer::Ok(output),
        Ok(output) => match api::classify_error(&output.stderr) {
            api::TmuxError::Gone => Answer::Gone,
            _ => Answer::Failed,
        },
        Err(_) => Answer::Failed,
    }
}

/// The server process with `pid`: a tmux of this person.
pub fn server_identity(table: &[ProcessInfo], pid: u32, uid: u32) -> Option<ProcessIdentity> {
    table
        .iter()
        .find(|p| p.identity.pid == pid && p.exe == Exe::Tmux && p.ruid == uid)
        .map(|p| p.identity)
}

/// One server, or `Err(true)` for a stale socket (skip it), `Err(false)` for
/// anything unreadable.
fn server<S: System>(
    sys: &S,
    cfg: &MacConfig,
    table: &[ProcessInfo],
    program: &str,
    socket: &str,
) -> Result<TmuxServer, bool> {
    let pid = match ask(sys, &run(program, api::server_pid_args(socket), cfg)) {
        Answer::Ok(out) => api::parse_server_pid(&out.stdout).ok_or(false)?,
        Answer::Gone => return Err(true),
        Answer::Failed => return Err(false),
    };
    let server = server_identity(table, pid, cfg.uid).ok_or(false)?;
    let panes = match ask(sys, &run(program, api::list_panes_args(socket), cfg)) {
        Answer::Ok(out) => api::parse_panes(&out.stdout).ok_or(false)?,
        Answer::Gone => return Err(true),
        Answer::Failed => return Err(false),
    };
    let clients = match ask(sys, &run(program, api::list_clients_args(socket), cfg)) {
        Answer::Ok(out) => api::parse_clients(&out.stdout).ok_or(false)?,
        Answer::Gone => return Err(true),
        Answer::Failed => return Err(false),
    };
    Ok(TmuxServer {
        socket: socket.to_owned(),
        server,
        panes,
        clients,
    })
}

/// Every server reachable by socket. No socket directory: `Known([])`. A
/// socket that exists but cannot be read (no tmux program, no table, a
/// failed query, a server pid not in the table): `Unknown`.
pub fn observe<S: System>(
    sys: &S,
    cfg: &MacConfig,
    table: Option<&[ProcessInfo]>,
    program: Option<&str>,
) -> Fact<Vec<TmuxServer>> {
    let dir = socket_dir(cfg);
    let entries = match sys.list_dir(&dir) {
        Ok(entries) => entries,
        Err(FsError::NotFound) => return Fact::Known(Vec::new()),
        Err(_) => return Fact::Unknown,
    };
    let sockets: Vec<String> = entries
        .iter()
        .filter(|e| !e.starts_with('.'))
        .map(|e| join(&dir, e))
        .filter(|s| api::is_socket_path(s))
        .collect();
    if sockets.is_empty() {
        return Fact::Known(Vec::new());
    }
    let (Some(table), Some(program)) = (table, program) else {
        return Fact::Unknown;
    };
    let mut servers = Vec::new();
    for socket in sockets {
        match server(sys, cfg, table, program, &socket) {
            Ok(server) => servers.push(server),
            Err(true) => {}
            Err(false) => return Fact::Unknown,
        }
    }
    Fact::Known(servers)
}
