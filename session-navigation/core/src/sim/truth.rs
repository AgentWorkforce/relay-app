//! Truth queries and process lifecycle. The oracle relies on these, never on
//! `decide`.
use super::world::{LogEntry, Pane, Proc, Role, World};
use crate::evidence::Exe;
use crate::model::{Provider, SessionRef, TerminalKind};
use crate::plan::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// Normal exit: exit handlers run (the Claude registry file is deleted).
    Normal,
    /// SIGKILL or crash: files are left behind.
    Crash,
}

/// The session a command will resume, if it is a resume command.
pub fn command_target(command: &Command) -> Option<SessionRef> {
    let args: Vec<&str> = command.args.iter().map(String::as_str).collect();
    let tool = crate::model::Tool::from_executable(&command.program)?;
    let (provider, id) = match (tool.program(), args.as_slice()) {
        ("claude", ["--resume", id]) => (Provider::Claude, *id),
        ("codex", ["resume", id]) => (Provider::Codex, *id),
        ("opencode", ["-s", id]) => (Provider::OpenCode, *id),
        ("grok", ["--resume", id]) => (Provider::Grok, *id),
        _ => return None,
    };
    Some(SessionRef::new(provider, id))
}

impl World {
    /// Live writers and presentations of `session`, plus launches in flight.
    pub fn writers(&self, session: &SessionRef) -> usize {
        // A daemon client is not a writer; the daemon serving it is one,
        // once per thread (VERIFIED, Codex 0.167.0, native N8-N10).
        let procs = self
            .procs
            .values()
            .filter(|p| p.is_writer() && p.hosts.as_ref() == Some(session))
            .filter(|p| !self.is_codex_client(p.pid))
            // An OpenCode TUI is a client of the server, the only writer
            // (VERIFIED, 2.2.6, native N13-N14); a running turn is a writer.
            .filter(|p| !(p.role == Role::Interactive && session.provider == Provider::OpenCode))
            .count();
        let loaded = match session.provider {
            Provider::Codex => {
                usize::from(self.codex.app_loaded.contains(&session.id))
                    + usize::from(self.codex_daemon_serves(&session.id))
            }
            // The desktop showing the session is a client too.
            Provider::OpenCode | Provider::Claude | Provider::Grok => 0,
        };
        let pending = self
            .pending
            .iter()
            .filter(|p| command_target(&p.command).as_ref() == Some(session))
            .count();
        procs + loaded + pending
    }

    /// What a truthful person answers to "is this session open elsewhere?":
    /// no writer for it exists or is about to start, and no conversation a
    /// desktop record claiming it would revive is running outside that record.
    pub fn truly_not_open_elsewhere(&self, session: &SessionRef) -> bool {
        if self.writers(session) > 0 {
            return false;
        }
        if session.provider != Provider::Claude {
            return true;
        }
        self.claude
            .desktop
            .values()
            .filter(|d| {
                d.holds_live(&session.id)
                    || d.local.strip_prefix("local_") == Some(session.id.as_str())
                    || d.prior.contains(&session.id)
            })
            .all(|d| {
                let Some(current) = d.current.as_ref().or(d.unarchived.as_ref()) else {
                    return true;
                };
                let conversation = SessionRef::new(Provider::Claude, current.clone());
                !self.procs.values().any(|p| {
                    p.is_writer()
                        && p.hosts.as_ref() == Some(&conversation)
                        && p.role != Role::DesktopChild(d.local.clone())
                }) && !self
                    .pending
                    .iter()
                    .any(|p| command_target(&p.command).as_ref() == Some(&conversation))
            })
    }

    /// Non-archived desktop sessions (other than `except`) holding this CLI id
    /// live. The desktop may revive their process at any moment.
    pub fn desktop_holders(&self, session: &SessionRef, except: Option<&str>) -> usize {
        if session.provider != Provider::Claude {
            return 0;
        }
        self.claude
            .desktop
            .values()
            .filter(|d| {
                !d.archived && d.holds_live(&session.id) && Some(d.local.as_str()) != except
            })
            .count()
    }

    /// Record that a writer for `session` just started, with how many others
    /// were already present.
    pub(crate) fn note_writer(
        &mut self,
        session: &SessionRef,
        by_engine: Option<u64>,
        except_desktop: Option<&str>,
        immediate: bool,
    ) {
        let others = self
            .writers(session)
            .saturating_sub(1)
            .saturating_add(self.desktop_holders(session, except_desktop));
        self.log.push(LogEntry::WriterStarted {
            open: by_engine,
            session: session.clone(),
            others,
            immediate,
        });
    }

    /// The agent process in the foreground of a pane.
    pub fn pane_agent(&self, pane: &Pane) -> Option<&Proc> {
        self.procs
            .values()
            .filter(|p| p.tty.as_deref() == Some(pane.tty.as_str()))
            .find(|p| matches!(p.role, Role::Interactive | Role::Stray))
    }

    pub fn pane_by_tty(&self, terminal: TerminalKind, tty: &str) -> Option<&Pane> {
        self.panes
            .values()
            .find(|p| p.terminal == terminal && p.tty == tty && p.tmux.is_none())
    }

    fn descendants(&self, pid: u32) -> Vec<u32> {
        let mut out = vec![pid];
        let mut index = 0;
        while let Some(&current) = out.get(index) {
            for p in self.procs.values() {
                if p.ppid == current && p.pid != current && !out.contains(&p.pid) {
                    out.push(p.pid);
                }
            }
            index += 1;
        }
        out
    }

    /// Kill a process and its descendants.
    pub fn kill(&mut self, pid: u32, exit: Exit) {
        if pid <= 1 {
            return;
        }
        let mut restarts = Vec::new();
        let mut servers = Vec::new();
        for victim in self.descendants(pid) {
            self.stopping.remove(&victim);
            let Some(proc) = self.procs.remove(&victim) else {
                continue;
            };
            if exit == Exit::Normal && proc.exe == Exe::ClaudeCli {
                self.claude.registry.remove(&victim);
            }
            if exit == Exit::Normal && proc.exe == Exe::GrokCli {
                self.grok.active.remove(&victim);
            }
            self.on_exit(&proc);
            self.codex_exited(victim);
            self.wsl_procs.remove(&victim);
            if proc.role == Role::TmuxServer {
                servers.push(victim);
            }
            if exit == Exit::Normal && proc.supervised {
                restarts.push(proc);
            }
        }
        for server in servers {
            self.tmux_server_gone(server);
        }
        for proc in restarts {
            self.restart_supervised(proc);
        }
    }

    fn on_exit(&mut self, proc: &Proc) {
        match &proc.role {
            Role::Server => self.opencode.server = None,
            Role::AppServer => self.codex.app_loaded.clear(),
            Role::App(app) => {
                self.views.remove(app);
                if self.front_app == Some(*app) {
                    self.front_app = None;
                    self.front_pane = None;
                }
            }
            Role::DesktopChild(_) => {
                if let Some(session) = &proc.hosts {
                    self.claude.last_stopped = Some(session.id.clone());
                }
            }
            _ => {}
        }
    }

    /// A supervisor restarts a stopped background owner (a hazard for
    /// take-over).
    fn restart_supervised(&mut self, proc: Proc) {
        if !self.procs.contains_key(&proc.ppid) {
            return;
        }
        let session = proc.hosts.clone();
        let pid = self.spawn(Proc { pid: 0, ..proc });
        if let (Some(session), Some(p)) = (session, self.procs.get(&pid)) {
            if p.exe == Exe::ClaudeCli {
                self.write_registry(pid, &session.id, None, "sdk-cli", false, true);
            }
            self.note_writer(&session, None, None, true);
        }
    }

    pub fn close_pane(&mut self, id: u64) {
        let Some(pane) = self.panes.remove(&id) else {
            return;
        };
        // A launch that had not started yet dies with its window.
        self.pending.retain(|p| p.pane != id);
        self.kill(pane.login, Exit::Normal);
        self.pane_closed(&pane);
        if self.front_pane == Some(id) {
            self.front_pane = None;
        }
    }
}
