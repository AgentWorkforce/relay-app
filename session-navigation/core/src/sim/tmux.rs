//! tmux in the fake machine: a server per socket owning sessions and panes
//! (each pane its own pty, its shell a child of the server), and clients
//! (`tmux attach`) running in an outer terminal pane. A session with no
//! client is live but on no screen. A restarted server numbers panes from
//! `%0` again.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::truth::Exit;
use super::world::{Pane, Proc, Role, World, USER};
use crate::evidence::Exe;
use crate::hosts::{TmuxClientInfo, TmuxPaneInfo, TmuxServer};
use crate::model::{BootId, Platform, ProcessIdentity, TerminalKind};

pub const SOCKET: &str = "/tmp/tmux-501/default";

/// Where a pane lives in tmux.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TmuxLoc {
    pub socket: String,
    /// `%N`.
    pub pane: u32,
    /// `$N` (each session has one window, `@N`).
    pub session: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxSrv {
    pub pid: u32,
    pub next_pane: u32,
    pub next_session: u32,
    /// Session → its active pane (a `World::panes` id).
    pub sessions: BTreeMap<u32, u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TmuxTruth {
    pub servers: BTreeMap<String, TmuxSrv>,
}

/// A client process: its socket and session.
pub fn client_of(proc: &Proc) -> Option<(&str, u32)> {
    match &proc.role {
        Role::TmuxClient { socket, session } => Some((socket.as_str(), *session)),
        _ => None,
    }
}

impl World {
    fn tmux_server(&mut self) -> Option<u32> {
        if self.platform == Platform::Windows {
            return None;
        }
        if let Some(srv) = self.tmux.servers.get(SOCKET) {
            return Some(srv.pid);
        }
        let pid = self.spawn(Proc {
            pid: 0,
            start: 0,
            ppid: 1,
            euid: USER,
            ruid: USER,
            exe: Exe::Tmux,
            tty: None,
            argv: vec!["tmux".to_owned(), "new-session".to_owned()],
            cwd: Some("/".to_owned()),
            role: Role::TmuxServer,
            hosts: None,
            by_engine: None,
            supervised: false,
        });
        self.tmux.servers.insert(
            SOCKET.to_owned(),
            TmuxSrv {
                pid,
                next_pane: 0,
                next_session: 0,
                sessions: BTreeMap::new(),
            },
        );
        Some(pid)
    }

    /// A new tmux session with one pane (a shell in `cwd`), attached from a
    /// new window of `outer` if given. Returns the tmux pane.
    pub fn tmux_new_session(&mut self, outer: Option<TerminalKind>, cwd: &str) -> Option<u64> {
        let server = self.tmux_server()?;
        let srv = self.tmux.servers.get_mut(SOCKET)?;
        let session = srv.next_session;
        srv.next_session += 1;
        let pane = self.tmux_pane(server, session, cwd)?;
        if let Some(srv) = self.tmux.servers.get_mut(SOCKET) {
            srv.sessions.insert(session, pane);
        }
        if let Some(terminal) = outer {
            self.tmux_attach_client(terminal, session, cwd);
        }
        Some(pane)
    }

    fn tmux_pane(&mut self, server: u32, session: u32, cwd: &str) -> Option<u64> {
        let tty = self.alloc_tty();
        let shell = self.spawn(Proc {
            pid: 0,
            start: 0,
            ppid: server,
            euid: USER,
            ruid: USER,
            exe: Exe::Shell,
            tty: Some(tty.clone()),
            argv: vec!["-zsh".to_owned()],
            cwd: Some(cwd.to_owned()),
            role: Role::Shell,
            hosts: None,
            by_engine: None,
            supervised: false,
        });
        let srv = self.tmux.servers.get_mut(SOCKET)?;
        let number = srv.next_pane;
        srv.next_pane += 1;
        let id = self.next_pane;
        self.next_pane += 1;
        self.labels.new_pane(id);
        self.panes.insert(
            id,
            Pane {
                id,
                terminal: TerminalKind::Ghostty,
                tty,
                login: shell,
                shell,
                window: 0,
                api: 0,
                tmux: Some(TmuxLoc {
                    socket: SOCKET.to_owned(),
                    pane: number,
                    session,
                }),
            },
        );
        Some(id)
    }

    /// A person runs `tmux attach -t $session` in a new window of `terminal`.
    pub fn tmux_attach_client(&mut self, terminal: TerminalKind, session: u32, cwd: &str) {
        let Some(outer) = self.open_pane(terminal, cwd) else {
            return;
        };
        self.tmux_client_in(outer, session);
    }

    pub(crate) fn tmux_client_in(&mut self, outer: u64, session: u32) -> Option<u32> {
        let pane = self.panes.get(&outer)?.clone();
        let known = self
            .tmux
            .servers
            .get(SOCKET)
            .is_some_and(|s| s.sessions.contains_key(&session));
        if !known {
            return None;
        }
        Some(self.spawn(Proc {
            pid: 0,
            start: 0,
            ppid: pane.shell,
            euid: USER,
            ruid: USER,
            exe: Exe::Tmux,
            tty: Some(pane.tty.clone()),
            argv: vec!["tmux".to_owned(), "attach".to_owned()],
            cwd: Some("/".to_owned()),
            role: Role::TmuxClient {
                socket: SOCKET.to_owned(),
                session,
            },
            hosts: None,
            by_engine: None,
            supervised: false,
        }))
    }

    /// What `tmux -S <socket> select-window -t @W ; select-pane -t %P ;
    /// attach-session -t $S` does in a new window's `pane`.
    pub(crate) fn tmux_attach_command(&mut self, pane: u64, args: &[String]) {
        let value = |flag_at: usize| args.get(flag_at + 2).map(String::as_str);
        let socket = args.get(1).map(String::as_str);
        let mut session = None;
        let mut select = None;
        for (i, word) in args.iter().enumerate() {
            match word.as_str() {
                "attach-session" => session = value(i).and_then(|s| s.strip_prefix('$')),
                "select-pane" => select = value(i).and_then(|p| p.strip_prefix('%')),
                _ => {}
            }
        }
        let (Some(socket), Some(Ok(session))) = (socket, session.map(str::parse::<u32>)) else {
            return;
        };
        if socket != SOCKET || !self.tmux.servers.contains_key(SOCKET) {
            return; // "no server running"
        }
        if let Some(Ok(number)) = select.map(str::parse::<u32>) {
            let target = self
                .panes
                .values()
                .find(|p| {
                    p.tmux
                        .as_ref()
                        .is_some_and(|l| l.pane == number && l.session == session)
                })
                .map(|p| p.id);
            if let (Some(target), Some(srv)) = (target, self.tmux.servers.get_mut(SOCKET)) {
                srv.sessions.insert(session, target);
            }
        }
        self.tmux_client_in(pane, session);
    }

    pub(crate) fn tmux_pane_closed(&mut self, loc: &TmuxLoc) {
        let remaining = self
            .panes
            .values()
            .find(|p| p.tmux.as_ref().is_some_and(|l| l.session == loc.session))
            .map(|p| p.id);
        let Some(srv) = self.tmux.servers.get_mut(&loc.socket) else {
            return;
        };
        match remaining {
            Some(other) => {
                srv.sessions.insert(loc.session, other);
            }
            None => {
                srv.sessions.remove(&loc.session);
            }
        }
    }

    /// The server process exited: its panes and clients go with it.
    pub(crate) fn tmux_server_gone(&mut self, pid: u32) {
        let Some(socket) = self
            .tmux
            .servers
            .iter()
            .find(|(_, s)| s.pid == pid)
            .map(|(k, _)| k.clone())
        else {
            return;
        };
        self.tmux.servers.remove(&socket);
        let panes: Vec<u64> = self
            .panes
            .values()
            .filter(|p| p.tmux.as_ref().is_some_and(|l| l.socket == socket))
            .map(|p| p.id)
            .collect();
        for id in panes {
            self.panes.remove(&id);
        }
        let clients: Vec<u32> = self
            .procs
            .values()
            .filter(|p| client_of(p).is_some_and(|(s, _)| s == socket))
            .map(|p| p.pid)
            .collect();
        for pid in clients {
            self.kill(pid, Exit::Normal);
        }
    }

    /// The server restarts with one fresh session: pane ids recycle.
    pub fn tmux_restart(&mut self) {
        let Some(pid) = self.tmux.servers.get(SOCKET).map(|s| s.pid) else {
            return;
        };
        self.kill(pid, Exit::Normal);
        self.tmux_new_session(None, "/");
    }

    /// The listing an adapter reads through `list-panes` / `list-clients`.
    pub fn tmux_listing(&self) -> Vec<TmuxServer> {
        self.tmux
            .servers
            .iter()
            .filter_map(|(socket, srv)| {
                let server = self.procs.get(&srv.pid)?;
                let panes = self
                    .panes
                    .values()
                    .filter_map(|p| {
                        let loc = p.tmux.as_ref().filter(|l| l.socket == *socket)?;
                        Some(TmuxPaneInfo {
                            pane: format!("%{}", loc.pane),
                            tty: p.tty.clone(),
                            session: format!("${}", loc.session),
                            window: format!("@{}", loc.session),
                        })
                    })
                    .collect();
                let clients = self
                    .procs
                    .values()
                    .filter_map(|p| {
                        let (s, session) = client_of(p).filter(|(s, _)| *s == socket)?;
                        let active = srv.sessions.get(&session)?;
                        let number = self.panes.get(active)?.tmux.as_ref()?.pane;
                        let _ = s;
                        Some(TmuxClientInfo {
                            tty: p.tty.clone()?,
                            pid: p.pid,
                            session: format!("${session}"),
                            active_pane: format!("%{number}"),
                        })
                    })
                    .collect();
                Some(TmuxServer {
                    socket: socket.clone(),
                    server: ProcessIdentity {
                        pid: server.pid,
                        start: server.start,
                        boot: BootId(self.boot),
                    },
                    panes,
                    clients,
                })
            })
            .collect()
    }
}

impl World {
    /// Detach one client.
    pub(crate) fn tmux_detach(&mut self, index: u32) {
        let clients: Vec<u32> = self
            .procs
            .values()
            .filter(|p| client_of(p).is_some())
            .map(|p| p.pid)
            .collect();
        if let Some(pid) = self.pick_proc(&clients, index) {
            self.kill(pid, Exit::Normal);
        }
    }

    /// Attach a client to one session.
    pub(crate) fn tmux_attach(&mut self, index: u32, terminal: u8) {
        let sessions: Vec<(u32, u64)> = self
            .tmux
            .servers
            .get(SOCKET)
            .map(|s| s.sessions.iter().map(|(k, v)| (*k, *v)).collect())
            .unwrap_or_default();
        let candidates: Vec<(u32, u64)> = sessions
            .iter()
            .map(|(session, pane)| (*session, self.labels.pane(*pane)))
            .collect();
        let Some(session) = super::labels::resolve(&candidates, index) else {
            return;
        };
        let choices = self.terminal_choices();
        let Some(t) = choices
            .get(usize::from(terminal) % choices.len().max(1))
            .copied()
        else {
            return;
        };
        self.tmux_attach_client(t, session, "/");
    }
}

impl World {
    /// The terminal a client of `pane`'s session runs in, if one is attached.
    pub fn tmux_outer(&self, pane: &Pane) -> Option<TerminalKind> {
        let loc = pane.tmux.as_ref()?;
        let client = self
            .procs
            .values()
            .find(|p| client_of(p) == Some((loc.socket.as_str(), loc.session)))?;
        let tty = client.tty.as_ref()?;
        self.panes
            .values()
            .find(|p| p.tmux.is_none() && p.tty == *tty)
            .map(|p| p.terminal)
    }
}
