//! Agent processes: what a person starts, what a resume command does, and
//! what each provider link does, including the documented hazards.
use serde::{Deserialize, Serialize};

use super::apps::bare;
use super::world::{
    ClaudeTranscript, DesktopSession, LogEntry, OcSession, Proc, Role, Thread, World, USER,
};
use crate::evidence::{CodexOrigin, Entrypoint, Exe};
use crate::model::{App, Provider, SessionRef, TerminalKind};
use crate::plan::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartHost {
    ClaudeDesktop,
    CodexDesktop,
    ClaudeGhostty,
    CodexGhostty,
    OpenCodeDesktop,
    OpenCodeTerminal {
        with_id: bool,
    },
    Headless {
        provider: Provider,
        supervised: bool,
    },
    /// A CLI in the platform's terminal at `terminal` (an index into
    /// `World::terminal_choices`), maybe in tmux or WSL.
    Terminal {
        provider: Provider,
        terminal: u8,
        mux: super::starts::Mux,
        wsl: bool,
        with_id: bool,
    },
}

impl StartHost {
    /// The host family, for usefulness statistics.
    pub fn label(self) -> String {
        match self {
            Self::OpenCodeTerminal { .. } => "OpenCodeTerminal".to_owned(),
            Self::Headless { .. } => "Headless".to_owned(),
            Self::Terminal { mux, .. } => format!(
                "Terminal{}",
                match mux {
                    super::starts::Mux::Attached => "TmuxAttached",
                    super::starts::Mux::Detached => "TmuxDetached",
                    super::starts::Mux::None => "",
                }
            ),
            other => format!("{other:?}"),
        }
    }

    pub fn provider(self) -> Provider {
        match self {
            Self::ClaudeDesktop | Self::ClaudeGhostty => Provider::Claude,
            Self::CodexDesktop | Self::CodexGhostty => Provider::Codex,
            Self::OpenCodeDesktop | Self::OpenCodeTerminal { .. } => Provider::OpenCode,
            Self::Headless { provider, .. } | Self::Terminal { provider, .. } => provider,
        }
    }
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| (*p).to_owned()).collect()
}

impl World {
    fn agent_in_pane(
        &mut self,
        pane: u64,
        exe: Exe,
        argv: Vec<String>,
        cwd: &str,
        hosts: SessionRef,
        by_engine: Option<u64>,
    ) -> Option<u32> {
        self.spawn_in_pane(pane, exe, argv, cwd, Some(hosts), by_engine)
    }

    /// Start an interactive process in `pane` (in the foreground of its
    /// shell, or of `login` directly for a launched command).
    pub(crate) fn spawn_in_pane(
        &mut self,
        pane: u64,
        exe: Exe,
        argv: Vec<String>,
        cwd: &str,
        hosts: Option<SessionRef>,
        by_engine: Option<u64>,
    ) -> Option<u32> {
        let pane = self.panes.get(&pane)?.clone();
        let pid = self.spawn(Proc {
            pid: 0,
            start: 0,
            ppid: pane.shell,
            euid: USER,
            ruid: USER,
            exe,
            tty: Some(pane.tty.clone()),
            argv,
            cwd: Some(cwd.to_owned()),
            role: Role::Interactive,
            hosts,
            by_engine,
            supervised: false,
        });
        if self.wsl_procs.contains(&pane.shell) {
            self.wsl_procs.insert(pid);
        }
        Some(pid)
    }

    /// A person starts a brand-new session in `host`.
    pub fn start_session(&mut self, host: StartHost, dir: &str) -> Option<SessionRef> {
        let session = self.start_in(host, dir)?;
        let label = if self.wsl_sessions.contains(&session) {
            "TerminalWsl".to_owned()
        } else {
            host.label()
        };
        self.origins.insert(session.clone(), label);
        Some(session)
    }

    fn start_in(&mut self, host: StartHost, dir: &str) -> Option<SessionRef> {
        match host {
            StartHost::ClaudeDesktop => {
                self.ensure_app(App::ClaudeDesktop)?;
                let cli = self.new_session_id(Provider::Claude);
                let local = format!("local_{}", self.rng.uuid_v4());
                self.claude
                    .transcripts
                    .insert(cli.clone(), transcript(dir, Entrypoint::Desktop));
                self.claude.desktop.insert(
                    local.clone(),
                    DesktopSession {
                        local: local.clone(),
                        current: Some(cli.clone()),
                        unarchived: None,
                        prior: Vec::new(),
                        archived: false,
                        cwd: dir.to_owned(),
                        last_focused: None,
                    },
                );
                self.start_desktop_child(&local);
                self.focus_claude_record(&local);
                self.front_app = Some(App::ClaudeDesktop);
                Some(self.catalogued(Provider::Claude, cli))
            }
            StartHost::CodexDesktop => {
                self.ensure_app(App::CodexDesktop)?;
                let id = self.new_thread(dir, CodexOrigin::Desktop);
                self.codex.app_loaded.insert(id.clone());
                self.views.insert(App::CodexDesktop, Some(id.clone()));
                self.front_app = Some(App::CodexDesktop);
                Some(self.catalogued(Provider::Codex, id))
            }
            StartHost::ClaudeGhostty => {
                let pane = self.open_pane(self.local(TerminalKind::Ghostty), dir)?;
                let cli = self.new_session_id(Provider::Claude);
                self.claude
                    .transcripts
                    .insert(cli.clone(), transcript(dir, Entrypoint::Cli));
                let session = SessionRef::new(Provider::Claude, cli.clone());
                let pid = self.agent_in_pane(
                    pane,
                    Exe::ClaudeCli,
                    argv(&["claude"]),
                    dir,
                    session,
                    None,
                )?;
                self.write_registry(pid, &cli, None, "cli", false, false);
                Some(self.catalogued(Provider::Claude, cli))
            }
            StartHost::CodexGhostty => {
                let pane = self.open_pane(self.local(TerminalKind::Ghostty), dir)?;
                let id = self.new_thread(dir, CodexOrigin::Tui);
                let session = SessionRef::new(Provider::Codex, id.clone());
                let daemon_mode = self.codex_daemon_mode();
                let pid =
                    self.agent_in_pane(pane, Exe::CodexCli, argv(&["codex"]), dir, session, None)?;
                if daemon_mode {
                    self.ensure_daemon();
                } else {
                    self.codex.own_lock.insert(pid);
                }
                Some(self.catalogued(Provider::Codex, id))
            }
            StartHost::OpenCodeDesktop => {
                self.ensure_app(App::OpenCodeDesktop)?;
                let id = self.new_opencode(dir);
                self.opencode.desktop_tabs.push(id.clone());
                self.views.insert(App::OpenCodeDesktop, Some(id.clone()));
                self.front_app = Some(App::OpenCodeDesktop);
                Some(self.catalogued(Provider::OpenCode, id))
            }
            StartHost::OpenCodeTerminal { with_id } => {
                let pane = self.open_pane(self.local(TerminalKind::AppleTerminal), dir)?;
                self.ensure_server(None);
                let id = self.new_opencode(dir);
                let args = if with_id {
                    argv(&["opencode", "-s", &id])
                } else {
                    argv(&["opencode"])
                };
                let session = SessionRef::new(Provider::OpenCode, id.clone());
                self.agent_in_pane(pane, Exe::OpenCodeCli, args, dir, session, None)?;
                self.opencode.tui_tabs.insert(id.clone());
                Some(self.catalogued(Provider::OpenCode, id))
            }
            StartHost::Headless {
                provider,
                supervised,
            } => self.start_headless(provider, supervised, dir),
            StartHost::Terminal {
                provider,
                terminal,
                mux,
                wsl,
                with_id,
            } => {
                let session = self.start_cli(provider, terminal, mux, wsl, with_id, dir)?;
                Some(session)
            }
        }
    }

    fn catalogued(&mut self, provider: Provider, id: String) -> SessionRef {
        let session = SessionRef::new(provider, id);
        self.record_session(session.clone(), None);
        session
    }

    pub(crate) fn new_thread_at(&mut self, dir: &str, origin: CodexOrigin) -> String {
        self.new_thread(dir, origin)
    }

    pub(crate) fn new_opencode_at(&mut self, dir: &str) -> String {
        self.new_opencode(dir)
    }

    fn new_thread(&mut self, dir: &str, origin: CodexOrigin) -> String {
        let id = self.new_session_id(Provider::Codex);
        let thread = Thread {
            cwd: dir.to_owned(),
            archived: false,
            origin,
            subagent: false,
        };
        self.codex.threads.insert(id.clone(), thread);
        id
    }

    fn new_opencode(&mut self, dir: &str) -> String {
        let id = self.new_session_id(Provider::OpenCode);
        let session = OcSession {
            dir: dir.to_owned(),
            archived: false,
            parent: None,
        };
        self.opencode.sessions.insert(id.clone(), session);
        id
    }

    fn start_headless(
        &mut self,
        provider: Provider,
        supervised: bool,
        dir: &str,
    ) -> Option<SessionRef> {
        let supervisor = self.spawn(bare(1, Exe::Other, &["supervisor"], Role::Supervisor));
        let id = match provider {
            Provider::Claude => {
                let id = self.new_session_id(Provider::Claude);
                self.claude
                    .transcripts
                    .insert(id.clone(), transcript(dir, Entrypoint::Sdk));
                id
            }
            Provider::Codex => self.new_thread(dir, CodexOrigin::Exec),
            Provider::OpenCode => {
                self.ensure_server(None);
                self.new_opencode(dir)
            }
            Provider::Grok => self.new_grok(dir, false),
        };
        let args = match provider {
            Provider::Claude => argv(&["claude", "-p", "--resume", &id]),
            Provider::Codex => argv(&["codex", "exec", "resume", &id]),
            Provider::OpenCode => argv(&["opencode", "run", "-s", &id]),
            // An ACP client (Paseo): it names no session and registers nowhere.
            Provider::Grok => argv(&["grok", "agent", "stdio"]),
        };
        let exe = match provider {
            Provider::Claude => Exe::ClaudeCli,
            Provider::Codex => Exe::CodexCli,
            Provider::OpenCode => Exe::OpenCodeCli,
            Provider::Grok => Exe::GrokCli,
        };
        let session = SessionRef::new(provider, id.clone());
        let pid = self.spawn(Proc {
            argv: args,
            cwd: Some(dir.to_owned()),
            hosts: Some(session),
            supervised,
            ..bare(supervisor, exe, &[], Role::Headless)
        });
        if provider == Provider::Claude {
            self.write_registry(pid, &id, None, "sdk-cli", false, true);
        }
        Some(self.catalogued(provider, id))
    }

    /// What a resume command does once its process starts in `pane`.
    pub(crate) fn run_in_pane(&mut self, pane: u64, command: &Command, by_engine: Option<u64>) {
        if !self.panes.contains_key(&pane) || !self.dirs.contains(&command.cwd) {
            return; // `cd` failed or the window is gone.
        }
        let wsl = self
            .panes
            .get(&pane)
            .is_some_and(|p| self.wsl_procs.contains(&p.shell));
        if !self.resolves_in(&command.program, wsl) {
            return; // "command not found"
        }
        if crate::model::Tool::from_executable(&command.program) == Some(crate::model::Tool::Tmux) {
            self.tmux_attach_command(pane, &command.args);
            return;
        }
        let Some(session) = super::truth::command_target(command) else {
            return;
        };
        if self.wsl_sessions.contains(&session) != wsl {
            // Another runtime's store: the CLI does not find the session.
            return;
        }
        let started = match session.provider {
            Provider::Claude => self.claude_cli_resume(pane, command, &session, by_engine),
            Provider::Codex => self.codex_tui_resume(pane, command, &session, by_engine),
            Provider::OpenCode => self.opencode_tui_resume(pane, command, &session, by_engine),
            Provider::Grok => self.grok_tui_resume(pane, command, &session, by_engine),
        };
        // An OpenCode TUI is a client of the server, not a writer (N14).
        if started && session.provider != Provider::OpenCode {
            self.note_writer(&session, by_engine, None, false);
        }
    }

    /// Whether `program` runs an installed CLI: a bare name through the
    /// shell's PATH, an absolute path only if the CLI is installed there.
    pub fn resolves(&self, program: &str) -> bool {
        self.resolves_in(program, false)
    }

    fn full_argv(command: &Command) -> Vec<String> {
        let mut out = vec![command.program.clone()];
        out.extend(command.args.iter().cloned());
        out
    }

    fn claude_cli_resume(
        &mut self,
        pane: u64,
        command: &Command,
        session: &SessionRef,
        by_engine: Option<u64>,
    ) -> bool {
        // The CLI finds transcripts under `projects/<slug(cwd)>` (Claude facts
        // F): from another directory it reports "No conversation found" and
        // creates nothing.
        let found = self
            .claude
            .transcripts
            .get(&session.id)
            .is_some_and(|t| t.cwd == command.cwd);
        if !found {
            return false;
        }
        let argv = Self::full_argv(command);
        match self.agent_in_pane(
            pane,
            Exe::ClaudeCli,
            argv,
            &command.cwd,
            session.clone(),
            by_engine,
        ) {
            Some(pid) => {
                self.write_registry(pid, &session.id, None, "cli", false, false);
                true
            }
            None => false,
        }
    }

    fn codex_tui_resume(
        &mut self,
        pane: u64,
        command: &Command,
        session: &SessionRef,
        by_engine: Option<u64>,
    ) -> bool {
        let Some(thread) = self.codex.threads.get(&session.id) else {
            return false;
        };
        if thread.archived && !self.vars.codex_resume_archived_works {
            return false;
        }
        self.codex_tui_start(pane, command, session, by_engine)
    }

    fn opencode_tui_resume(
        &mut self,
        pane: u64,
        command: &Command,
        session: &SessionRef,
        by_engine: Option<u64>,
    ) -> bool {
        self.replace_server_on_mismatch(by_engine);
        match self.opencode.sessions.get(&session.id) {
            Some(s)
                if (s.archived || s.parent.is_some())
                    && !self.vars.opencode_tui_opens_archived_or_child =>
            {
                return false;
            }
            Some(_) => {}
            None => {
                // VERIFIED hazard (facts F): a missing id is created silently.
                let created = OcSession {
                    dir: command.cwd.clone(),
                    archived: false,
                    parent: None,
                };
                self.opencode.sessions.insert(session.id.clone(), created);
                self.record_session(session.clone(), by_engine);
            }
        }
        self.opencode.tui_tabs.insert(session.id.clone());
        let argv = Self::full_argv(command);
        self.agent_in_pane(
            pane,
            Exe::OpenCodeCli,
            argv,
            &command.cwd,
            session.clone(),
            by_engine,
        )
        .is_some()
    }

    /// A TUI of another version stops the running service and starts its own
    /// (facts G, `mismatch: "replace"`).
    fn replace_server_on_mismatch(&mut self, by_engine: Option<u64>) {
        let cli = self
            .tools
            .get(&crate::model::Tool::OpenCodeCli)
            .cloned()
            .flatten();
        if let Some(server) = self.opencode.server {
            if cli.as_ref() != Some(&self.opencode.server_version) {
                self.kill(server, super::truth::Exit::Normal);
                self.log.push(LogEntry::ServerReplaced { open: by_engine });
            }
        }
        self.ensure_server(None);
    }

    /// Record that a person typed a resume command in a new window.
    pub fn person_runs(&mut self, terminal: TerminalKind, command: &Command) {
        let wsl =
            super::truth::command_target(command).is_some_and(|s| self.wsl_sessions.contains(&s));
        if let Some(pane) = self.open_pane_with(terminal, &command.cwd, None, wsl) {
            self.run_in_pane(pane, command, None);
        }
    }

    pub(crate) fn log_created(&mut self, session: SessionRef) {
        if self.engine_open.is_some() {
            self.log.push(LogEntry::Created {
                open: self.engine_open,
                session,
            });
        }
    }
}

fn transcript(dir: &str, entrypoint: Entrypoint) -> ClaudeTranscript {
    ClaudeTranscript {
        cwd: dir.to_owned(),
        entrypoint,
    }
}
