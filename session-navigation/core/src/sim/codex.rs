//! The Codex CLI under its shared daemon, as observed on 0.167.0 (native
//! N8-N12): a TUI is a client; the daemon holds the thread's writer lock and
//! its rollout and is the only writer, however many TUIs show the thread. A
//! TUI started under another version may hold its own lock instead
//! (`codex_tui_holds_lock`, UNVERIFIED: `--no-daemon` and older versions).
use super::world::{Role, World};
use crate::model::{Provider, SessionRef, Tool};

impl World {
    /// Whether a TUI started now is a client of the shared daemon.
    pub fn codex_daemon_mode(&self) -> bool {
        let verified = self
            .tools
            .get(&Tool::CodexCli)
            .cloned()
            .flatten()
            .as_deref()
            == Some(super::world::verified_tool_version(Tool::CodexCli));
        verified || !self.vars.codex_tui_holds_lock
    }

    /// A daemon client: a Codex TUI without its own lock.
    pub fn is_codex_client(&self, pid: u32) -> bool {
        self.procs.get(&pid).is_some_and(|p| {
            p.role == Role::Interactive
                && p.hosts
                    .as_ref()
                    .is_some_and(|h| h.provider == Provider::Codex)
                && !self.codex.own_lock.contains(&pid)
        })
    }

    /// Whether the daemon writes thread `id`: it is alive and serves the
    /// thread to at least one client.
    pub fn codex_daemon_serves(&self, id: &str) -> bool {
        self.daemon_pid().is_some()
            && self.procs.values().any(|p| {
                self.is_codex_client(p.pid) && p.hosts.as_ref().is_some_and(|h| h.id == id)
            })
    }

    /// Writers of a Codex thread other than the daemon a new client would
    /// join: a second client of the daemon is not a second writer (N10).
    pub fn codex_writers_beside_daemon(&self, session: &SessionRef) -> usize {
        let daemon = usize::from(self.codex_daemon_serves(&session.id));
        self.writers(session).saturating_sub(daemon)
    }

    /// Start the TUI process of `codex resume <id>` in `pane`. Returns
    /// whether a writer started for the thread (a client joining the
    /// daemon's writer starts none).
    pub(crate) fn codex_tui_start(
        &mut self,
        pane: u64,
        command: &crate::plan::Command,
        session: &SessionRef,
        by_engine: Option<u64>,
    ) -> bool {
        let daemon_mode = self.codex_daemon_mode();
        let others = if daemon_mode {
            self.codex_writers_beside_daemon(session)
        } else {
            self.writers(session)
        };
        // Another writer refusing a second one is UNVERIFIED either way; a
        // client of the daemon's own writer is accepted (VERIFIED, N10).
        if others > 0 && self.vars.codex_second_writer_refused {
            return false;
        }
        let mut argv = vec![command.program.clone()];
        argv.extend(command.args.iter().cloned());
        let stalls = self.codex.update_available;
        let hosts = (!stalls).then(|| session.clone());
        let Some(pid) = self.spawn_in_pane(
            pane,
            crate::evidence::Exe::CodexCli,
            argv,
            &command.cwd,
            hosts,
            by_engine,
        ) else {
            return false;
        };
        if stalls {
            // The menu blocks before the TUI resumes anything (N12).
            self.codex.stalled.insert(pid, session.clone());
            return false;
        }
        self.codex_attach(pid, session, daemon_mode)
    }

    /// The TUI `pid` now shows `session`. Returns whether a writer started.
    fn codex_attach(&mut self, pid: u32, session: &SessionRef, daemon_mode: bool) -> bool {
        if !daemon_mode {
            self.codex.own_lock.insert(pid);
            return true;
        }
        let served = self.procs.values().any(|p| {
            p.pid != pid && self.is_codex_client(p.pid) && p.hosts.as_ref() == Some(session)
        }) && self.daemon_pid().is_some();
        self.ensure_daemon();
        !served
    }

    /// The person dismisses the update menu in one stalled TUI (Esc): it
    /// goes on to resume its thread.
    pub(crate) fn dismiss_update(&mut self, index: u32) {
        let pids: Vec<u32> = self.codex.stalled.keys().copied().collect();
        let Some(pid) = self.pick_proc(&pids, index) else {
            return;
        };
        let Some(session) = self.codex.stalled.remove(&pid) else {
            return;
        };
        if !self.codex.threads.contains_key(&session.id) {
            return;
        }
        let daemon_mode = self.codex_daemon_mode();
        let by_engine = self.procs.get(&pid).and_then(|p| p.by_engine);
        if let Some(p) = self.procs.get_mut(&pid) {
            p.hosts = Some(session.clone());
        }
        if self.codex_attach(pid, &session, daemon_mode) {
            self.note_writer(&session, by_engine, None, false);
        }
    }

    /// Forget per-process Codex state of an exited process.
    pub(crate) fn codex_exited(&mut self, pid: u32) {
        self.codex.own_lock.remove(&pid);
        self.codex.stalled.remove(&pid);
    }
}
