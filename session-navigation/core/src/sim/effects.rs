//! Event effects on provider stores and the machine: archive, delete,
//! /clear, subagents, person-typed resumes, reboots, pid reuse and background
//! discovery.
use super::truth::Exit;
use super::world::{Role, World};
use crate::model::{App, Host, Provider, SessionRef, TerminalKind};
use crate::plan::Command;

impl World {
    /// A person types a resume command themselves (a person-made duplicate
    /// is allowed; the oracle only judges the engine).
    pub(crate) fn person_resume(&mut self, index: u32) {
        let Some(session) = self.pick_session(index) else {
            return;
        };
        let (program, args, cwd, terminal) = match session.provider {
            Provider::Claude => {
                let cwd = self
                    .claude
                    .transcripts
                    .get(&session.id)
                    .map(|t| t.cwd.clone());
                (
                    "claude",
                    vec!["--resume".to_owned(), session.id.clone()],
                    cwd,
                    TerminalKind::Ghostty,
                )
            }
            Provider::Codex => {
                let cwd = self.codex.threads.get(&session.id).map(|t| t.cwd.clone());
                (
                    "codex",
                    vec!["resume".to_owned(), session.id.clone()],
                    cwd,
                    TerminalKind::Ghostty,
                )
            }
            Provider::OpenCode => {
                let cwd = self
                    .opencode
                    .sessions
                    .get(&session.id)
                    .map(|s| s.dir.clone());
                (
                    "opencode",
                    vec!["-s".to_owned(), session.id.clone()],
                    cwd,
                    TerminalKind::AppleTerminal,
                )
            }
            Provider::Grok => {
                let cwd = self.grok_person_dir(&session);
                (
                    "grok",
                    vec!["--resume".to_owned(), session.id.clone()],
                    cwd,
                    TerminalKind::Ghostty,
                )
            }
        };
        let Some(cwd) = cwd else {
            return;
        };
        let command = Command {
            program: program.to_owned(),
            args,
            cwd,
            env: Default::default(),
        };
        self.person_runs(self.local(terminal), &command);
    }

    pub(crate) fn set_archived(&mut self, index: u32, archived: bool) {
        let Some(session) = self.pick_session(index) else {
            return;
        };
        match session.provider {
            Provider::Claude => {
                let local = self
                    .claude
                    .desktop
                    .values()
                    .find(|d| d.holds_live(&session.id))
                    .map(|d| d.local.clone());
                let Some(local) = local else {
                    return;
                };
                if let Some(d) = self.claude.desktop.get_mut(&local) {
                    if !archived && d.archived {
                        d.unarchived = d.current.clone().or(d.unarchived.clone());
                    }
                    d.archived = archived;
                }
                if archived {
                    // Archive tears the session down (facts A).
                    let child = self
                        .procs
                        .values()
                        .find(|p| p.role == Role::DesktopChild(local.clone()))
                        .map(|p| p.pid);
                    if let Some(pid) = child {
                        self.kill(pid, Exit::Normal);
                    }
                }
            }
            Provider::Codex => {
                if let Some(t) = self.codex.threads.get_mut(&session.id) {
                    t.archived = archived;
                }
                if archived {
                    self.codex.app_loaded.remove(&session.id);
                }
            }
            Provider::OpenCode => {
                if let Some(s) = self.opencode.sessions.get_mut(&session.id) {
                    s.archived = archived;
                }
            }
            // Grok has no archive (facts-grok 2).
            Provider::Grok => {}
        }
    }

    pub(crate) fn delete(&mut self, index: u32) {
        let Some(session) = self.pick_session(index) else {
            return;
        };
        match session.provider {
            Provider::Claude => {
                self.claude.transcripts.remove(&session.id);
                let locals: Vec<String> = self
                    .claude
                    .desktop
                    .values()
                    .filter(|d| d.holds_live(&session.id))
                    .map(|d| d.local.clone())
                    .collect();
                for local in locals {
                    self.claude.desktop.remove(&local);
                    let child = self
                        .procs
                        .values()
                        .find(|p| p.role == Role::DesktopChild(local.clone()))
                        .map(|p| p.pid);
                    if let Some(pid) = child {
                        self.kill(pid, Exit::Normal);
                    }
                }
            }
            Provider::Codex => {
                self.codex.threads.remove(&session.id);
                self.codex.app_loaded.remove(&session.id);
            }
            Provider::OpenCode => {
                self.opencode.sessions.remove(&session.id);
                self.opencode
                    .sessions
                    .retain(|_, s| s.parent.as_deref() != Some(session.id.as_str()));
            }
            Provider::Grok => {
                self.grok.sessions.remove(&session.id);
            }
        }
    }

    /// `/clear` in a Claude process: the conversation gets a new CLI id. In
    /// worlds where a Codex TUI can switch thread after launch (UNVERIFIED,
    /// `codex_tui_switches_thread`), the same event starts a new thread in
    /// a Codex TUI, whose argv still names the old one.
    pub(crate) fn clear(&mut self, index: u32) {
        let switches = self.vars.codex_tui_switches_thread;
        let candidates: Vec<u32> = self
            .procs
            .values()
            .filter(|p| {
                p.is_writer()
                    && p.hosts.as_ref().is_some_and(|h| {
                        h.provider == Provider::Claude
                            || (h.provider == Provider::Grok && p.role == Role::Interactive)
                            || (switches
                                && h.provider == Provider::Codex
                                && p.role == Role::Interactive)
                    })
            })
            .map(|p| p.pid)
            .collect();
        let Some(pid) = self.pick_proc(&candidates, index) else {
            return;
        };
        let Some(proc) = self.procs.get(&pid).cloned() else {
            return;
        };
        let Some(old) = proc.hosts.clone() else {
            return;
        };
        if old.provider == Provider::Codex {
            self.codex_switch(pid, proc.start, &old);
            return;
        }
        if old.provider == Provider::Grok {
            self.grok_switch(pid, proc.start, &old);
            return;
        }
        let new_id = self.new_session_id(Provider::Claude);
        if let Some(t) = self.claude.transcripts.get(&old.id).cloned() {
            self.claude.transcripts.insert(new_id.clone(), t);
        }
        let new_session = SessionRef::new(Provider::Claude, new_id.clone());
        self.past_hosts
            .entry((pid, proc.start))
            .or_default()
            .push(old.clone());
        if let Some(p) = self.procs.get_mut(&pid) {
            p.hosts = Some(new_session.clone());
        }
        let mut host = None;
        if let Role::DesktopChild(local) = &proc.role {
            if let Some(d) = self.claude.desktop.get_mut(local) {
                if let Some(current) = d.current.take() {
                    d.prior.push(current);
                }
                d.current = Some(new_id.clone());
                d.unarchived = None;
            }
            host = Some(local.clone());
        }
        if self.vars.claude_registry_rewritten_on_clear {
            let (entry, bg) = match (&proc.role, host.is_some()) {
                (_, true) => ("claude-desktop", false),
                (Role::Headless, _) => ("sdk-cli", true),
                _ => ("cli", false),
            };
            self.write_registry(pid, &new_id, host.as_deref(), entry, false, bg);
        }
        self.record_session(new_session, None);
    }

    /// A Codex TUI starts a new thread in the same directory.
    fn codex_switch(&mut self, pid: u32, start: u64, old: &SessionRef) {
        let Some(thread) = self.codex.threads.get(&old.id).cloned() else {
            return;
        };
        let id = self.new_session_id(Provider::Codex);
        self.codex.threads.insert(
            id.clone(),
            super::world::Thread {
                origin: crate::evidence::CodexOrigin::Tui,
                archived: false,
                subagent: false,
                ..thread
            },
        );
        let new_session = SessionRef::new(Provider::Codex, id);
        self.past_hosts
            .entry((pid, start))
            .or_default()
            .push(old.clone());
        if let Some(p) = self.procs.get_mut(&pid) {
            p.hosts = Some(new_session.clone());
        }
        self.record_session(new_session, None);
    }

    /// A subagent session under an existing Codex or OpenCode session.
    pub(crate) fn spawn_child(&mut self, index: u32) {
        let Some(parent) = self.pick_session(index) else {
            return;
        };
        match parent.provider {
            Provider::Codex => {
                let Some(t) = self.codex.threads.get(&parent.id).cloned() else {
                    return;
                };
                let id = self.new_session_id(Provider::Codex);
                self.codex.threads.insert(
                    id.clone(),
                    super::world::Thread {
                        subagent: true,
                        ..t
                    },
                );
                self.record_session(SessionRef::new(Provider::Codex, id), None);
            }
            Provider::OpenCode => {
                let Some(s) = self.opencode.sessions.get(&parent.id).cloned() else {
                    return;
                };
                let id = self.new_session_id(Provider::OpenCode);
                let child = super::world::OcSession {
                    parent: Some(parent.id.clone()),
                    ..s
                };
                self.opencode.sessions.insert(id.clone(), child);
                self.record_session(SessionRef::new(Provider::OpenCode, id), None);
            }
            Provider::Grok => {
                let Some(s) = self.grok.sessions.get(&parent.id).cloned() else {
                    return;
                };
                let id = self.new_grok(&s.cwd, true);
                self.record_session(SessionRef::new(Provider::Grok, id), None);
            }
            Provider::Claude => {}
        }
    }

    /// Everything stops without exit handlers; pids recycle from the start.
    pub(crate) fn reboot(&mut self) {
        self.clock = self.clock.saturating_add(60);
        self.procs.retain(|pid, _| *pid == 1);
        self.stopping.clear();
        if let Some(launchd) = self.procs.get_mut(&1) {
            launchd.start = self.clock;
        }
        self.panes.clear();
        self.tmux.servers.clear();
        self.wsl_procs.clear();
        self.hosts.active.clear();
        self.hosts.hidden_ttys.clear();
        self.hosts.foreign_windows.clear();
        self.views.clear();
        self.front_app = None;
        self.front_pane = None;
        self.codex.app_loaded.clear();
        self.codex.own_lock.clear();
        self.codex.stalled.clear();
        self.opencode.server = None;
        self.pending.clear();
        self.boot = self.boot.saturating_add(1);
        self.boot_time = self.clock;
        self.next_pid = super::world::FIRST_PID;
    }

    /// After a reboot, an unrelated Claude process (another config dir, so no
    /// registry file of its own) gets a stale record's pid AND its start
    /// second (clock correction). Only a boot comparison tells them apart.
    pub(crate) fn pid_reuse(&mut self) {
        let has_stale = self
            .claude
            .registry
            .iter()
            .any(|(pid, f)| f.mtime < self.boot_time && !self.procs.contains_key(pid));
        if !has_stale {
            return;
        }
        let cwd = self.dir_at(u8::try_from(self.clock % 4).unwrap_or(0));
        let Some(pane) = self.open_pane(self.local(TerminalKind::Ghostty), &cwd) else {
            return;
        };
        let Some(shell) = self.panes.get(&pane).cloned() else {
            return;
        };
        // Pick the stale pid only now, so the new pane cannot have taken it.
        let stale = self
            .claude
            .registry
            .iter()
            .find(|(pid, f)| f.mtime < self.boot_time && !self.procs.contains_key(pid))
            .map(|(pid, f)| (*pid, f.bytes.clone()));
        let Some((pid, bytes)) = stale else {
            return;
        };
        let Ok(record) = crate::parse::claude::parse_registry(&bytes) else {
            return;
        };
        let alien = SessionRef::new(Provider::Claude, self.rng.uuid_v4());
        self.labels.new_proc(pid);
        self.procs.insert(
            pid,
            super::world::Proc {
                pid,
                start: record.proc_start,
                ppid: shell.shell,
                euid: super::world::USER,
                ruid: super::world::USER,
                exe: crate::evidence::Exe::ClaudeCli,
                tty: Some(shell.tty.clone()),
                argv: vec!["claude".to_owned()],
                cwd: Some(cwd),
                role: Role::Interactive,
                hosts: Some(alien),
                by_engine: None,
                supervised: false,
            },
        );
    }

    /// Background discovery records where identifiable sessions live.
    pub(crate) fn discover(&mut self) {
        let mut seen = Vec::new();
        for p in self.procs.values().filter(|p| p.is_writer()) {
            let Some(session) = p.hosts.clone() else {
                continue;
            };
            let identifiable = match session.provider {
                Provider::Claude => self.claude.registry.contains_key(&p.pid),
                Provider::Codex => {
                    p.argv.iter().any(|a| a == &session.id) || self.codex.own_lock.contains(&p.pid)
                }
                Provider::OpenCode => p.argv.iter().any(|a| a == &session.id),
                Provider::Grok => {
                    self.grok
                        .active
                        .get(&p.pid)
                        .is_some_and(|r| r.session_id == session.id)
                        || p.argv.iter().any(|a| a == &session.id)
                }
            };
            if !identifiable {
                continue;
            }
            let host = match (&p.role, &p.tty) {
                (Role::DesktopChild(_), _) => Host::Desktop(App::ClaudeDesktop),
                (_, Some(tty)) => match self.panes.values().find(|pane| &pane.tty == tty) {
                    Some(pane) if pane.tmux.is_some() => Host::Tmux(self.tmux_outer(pane)),
                    Some(pane) => Host::Terminal(pane.terminal),
                    None => continue,
                },
                _ => Host::Headless,
            };
            seen.push((session, host));
        }
        for id in &self.codex.app_loaded {
            seen.push((
                SessionRef::new(Provider::Codex, id.clone()),
                Host::Desktop(App::CodexDesktop),
            ));
        }
        if let Some(Some(id)) = self.views.get(&App::OpenCodeDesktop) {
            if self.running(App::OpenCodeDesktop) {
                seen.push((
                    SessionRef::new(Provider::OpenCode, id.clone()),
                    Host::Desktop(App::OpenCodeDesktop),
                ));
            }
        }
        for (session, host) in seen {
            self.first_host.entry(session.clone()).or_insert(host);
            self.history.insert(session, host);
        }
    }
}
