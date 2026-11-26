//! Things that happen between opens. Targets are picks resolved against the
//! current candidates at apply time, so any subsequence of events is valid
//! (which is what shrinking needs). Generated picks are stable: they follow
//! the target's identity, not its position (see `labels`).
use serde::{Deserialize, Serialize};

use super::agents::StartHost;
use super::labels::{self, resolve};
use super::truth::Exit;
use super::world::{Role, World, DIRS};
use crate::model::{App, SessionRef, TerminalKind, Tool};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channel {
    ProcTable,
    ClaudeRegistry,
    ClaudeDesktop,
    CodexDb,
    OpenCodeDb,
    OpenCodeHung,
    Fs,
    Reservations,
    Environment,
    Tmux,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenEvent {
    pub pick: u32,
    pub twice: bool,
    pub take_over: bool,
    pub default_terminal: bool,
    pub confirm_gone: bool,
    pub allow_import: bool,
    pub choose_first: bool,
    /// The person's default-terminal preference is the platform's second
    /// terminal rather than its first.
    pub second_terminal: bool,
    /// The person prefers a terminal to the provider's desktop app
    /// ([`crate::plan::Preferences::prefer_terminal`]).
    pub prefer_terminal: bool,
    /// The person set no terminal preference in Relay: the machine's
    /// default applies.
    pub no_preference: bool,
    /// Answer a `Plan::Confirm` truthfully and open again.
    pub answer_confirm: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    Start {
        host: StartHost,
        dir: u8,
        /// Identity of what this start creates (stable across shrinking).
        tag: u32,
    },
    PersonResume {
        pick: u32,
    },
    ClosePane {
        pick: u32,
    },
    QuitApp {
        app: App,
    },
    Crash {
        pick: u32,
    },
    Archive {
        pick: u32,
    },
    Unarchive {
        pick: u32,
    },
    Delete {
        pick: u32,
    },
    Clear {
        pick: u32,
    },
    SpawnChild {
        pick: u32,
    },
    Reboot,
    PidReuse,
    DeleteDir {
        dir: u8,
    },
    RestoreDir {
        dir: u8,
    },
    Tick {
        seconds: u8,
    },
    Permission {
        terminal: TerminalKind,
        granted: bool,
    },
    Upgrade {
        app: Option<App>,
        tool: Option<Tool>,
    },
    Uninstall {
        app: App,
    },
    /// A CLI is removed (or was never on PATH: no `claude` is on PATH on the
    /// surveyed Mac, Claude facts correction 4).
    UninstallTool {
        tool: Tool,
    },
    HijackScheme {
        codex: bool,
    },
    ServerStop {
        crash: bool,
    },
    ServerStart,
    Degrade {
        channel: Channel,
        ok: bool,
    },
    TornRegistry {
        pick: u32,
    },
    Discover,
    /// The person dismisses the Codex "Update available" menu in one TUI
    /// waiting at it (Esc, native N12).
    DismissUpdate {
        pick: u32,
    },
    /// A tmux client detaches (the session stays, on no screen).
    TmuxDetach {
        pick: u32,
    },
    /// The person attaches another client to a tmux session, in a new
    /// window of the terminal at `terminal`.
    TmuxAttach {
        pick: u32,
        terminal: u8,
    },
    /// Arm a hazard that strikes between the next decision and its first
    /// step.
    Race {
        hazard: super::hosts::Hazard,
    },
    /// New windows report their tty before any process appears on it.
    SlowStart {
        on: bool,
    },
    /// Kitty remote control / the WezTerm socket switched on or off.
    HostApi {
        kitty: bool,
        on: bool,
    },
    Open(OpenEvent),
}

pub fn dir(index: u8) -> &'static str {
    DIRS.get(usize::from(index) % DIRS.len())
        .copied()
        .unwrap_or("/")
}

impl World {
    pub fn pick_session(&self, index: u32) -> Option<SessionRef> {
        let candidates: Vec<(SessionRef, u64)> = self
            .catalog
            .iter()
            .map(|s| (s.clone(), self.labels.session(s)))
            .collect();
        resolve(&candidates, index)
    }

    /// The session an open targets: a catalogued one, or (as often as one
    /// more candidate would be) a well-formed id that never existed.
    pub fn open_target(&mut self, pick: u32) -> SessionRef {
        self.labels.begin(labels::mix(u64::from(pick), 0x0BE7));
        let mut candidates: Vec<(Option<SessionRef>, u64)> = self
            .catalog
            .iter()
            .map(|s| (Some(s.clone()), self.labels.session(s)))
            .collect();
        candidates.push((None, labels::NONEXISTENT));
        if let Some(Some(session)) = resolve(&candidates, pick) {
            return session;
        }
        let provider = match pick % 3 {
            0 => crate::model::Provider::Claude,
            1 => crate::model::Provider::Codex,
            _ => crate::model::Provider::OpenCode,
        };
        let id = self.new_session_id(provider);
        SessionRef::new(provider, id)
    }

    pub(crate) fn pick_pane(&self, index: u32) -> Option<u64> {
        let candidates: Vec<(u64, u64)> = self
            .panes
            .keys()
            .map(|id| (*id, self.labels.pane(*id)))
            .collect();
        resolve(&candidates, index)
    }

    pub(crate) fn pick_proc(&self, pids: &[u32], index: u32) -> Option<u32> {
        let candidates: Vec<(u32, u64)> = pids.iter().map(|p| (*p, self.labels.proc(*p))).collect();
        resolve(&candidates, index)
    }

    /// Races no planner can prevent, which the generator leaves out: while
    /// our own launch for a session is still starting, the person deletes
    /// that session, resumes it themselves, starts another client or server
    /// of the same provider, or upgrades or removes its CLI (which the
    /// delayed launch then meets).
    pub fn races_engine_launch(&self, event: &Event) -> bool {
        // A launch waiting at the Codex update menu is still starting.
        let stalled = self.codex.stalled.iter().filter_map(|(pid, session)| {
            self.procs
                .get(pid)
                .filter(|p| p.by_engine.is_some())
                .map(|_| session.clone())
        });
        let pending: Vec<SessionRef> = self
            .pending
            .iter()
            .filter(|p| p.by_engine.is_some())
            .filter_map(|p| super::truth::command_target(&p.command))
            .chain(stalled)
            .collect();
        if pending.is_empty() {
            return false;
        }
        let provider_pending = |provider| pending.iter().any(|s| s.provider == provider);
        match event {
            Event::Delete { pick } | Event::PersonResume { pick } => self
                .pick_session(*pick)
                .is_some_and(|s| pending.contains(&s)),
            Event::Start { host, .. } => provider_pending(host.provider()),
            Event::ServerStart => provider_pending(crate::model::Provider::OpenCode),
            Event::Upgrade {
                tool: Some(tool), ..
            }
            | Event::UninstallTool { tool } => match tool {
                crate::model::Tool::ClaudeCli => provider_pending(crate::model::Provider::Claude),
                crate::model::Tool::CodexCli => provider_pending(crate::model::Provider::Codex),
                crate::model::Tool::OpenCodeCli => {
                    provider_pending(crate::model::Provider::OpenCode)
                }
                crate::model::Tool::GrokCli => provider_pending(crate::model::Provider::Grok),
                crate::model::Tool::Tmux => false,
            },
            _ => false,
        }
    }

    fn agents(&self) -> Vec<u32> {
        self.procs
            .values()
            .filter(|p| p.is_writer() || p.role == Role::Stray)
            .map(|p| p.pid)
            .collect()
    }

    /// Apply one event. `Open` is handled by the runner, not here.
    pub fn apply(&mut self, event: &Event) {
        let hint = serde_json::to_vec(event).map_or(0, |b| labels::digest(&b));
        self.labels.begin(hint);
        // A slow window's first process appears before anything else can
        // happen in it: the gap is only visible to the opens around it.
        if !matches!(event, Event::Race { .. } | Event::SlowStart { .. }) {
            self.hosts.hidden_ttys.clear();
        }
        match *event {
            Event::Start { host, dir: d, .. } => {
                let d = self.dir_at(d);
                self.start_session(host, &d);
            }
            Event::PersonResume { pick: i } => self.person_resume(i),
            Event::ClosePane { pick: i } => {
                if let Some(id) = self.pick_pane(i) {
                    self.close_pane(id);
                }
            }
            Event::QuitApp { app } => self.quit_app(app),
            Event::Crash { pick: i } => {
                if let Some(pid) = self.pick_proc(&self.agents(), i) {
                    self.kill(pid, Exit::Crash);
                }
            }
            Event::Archive { pick: i } => self.set_archived(i, true),
            Event::Unarchive { pick: i } => self.set_archived(i, false),
            Event::Delete { pick: i } => self.delete(i),
            Event::Clear { pick: i } => self.clear(i),
            Event::SpawnChild { pick: i } => self.spawn_child(i),
            Event::Reboot => self.reboot(),
            Event::PidReuse => self.pid_reuse(),
            Event::DeleteDir { dir: d } => {
                let d = self.dir_at(d);
                self.dirs.remove(&d);
            }
            Event::RestoreDir { dir: d } => {
                let d = self.dir_at(d);
                self.dirs.insert(d);
            }
            Event::Tick { seconds } => self.advance(u64::from(seconds)),
            Event::Permission { terminal, granted } => {
                self.automation.insert(terminal.app(), granted);
            }
            Event::Upgrade { app, tool } => self.upgrade(app, tool),
            Event::Uninstall { app } => {
                self.quit_app(app);
                if let Some(a) = self.apps.get_mut(&app) {
                    a.installed = false;
                }
            }
            Event::UninstallTool { tool } => {
                self.tools.insert(tool, None);
            }
            Event::HijackScheme { codex } => {
                let scheme = if codex { "codex" } else { "claude" };
                self.schemes
                    .insert(scheme.to_owned(), "com.example.other".to_owned());
            }
            Event::ServerStop { crash } => {
                if let Some(pid) = self.opencode.server {
                    self.kill(pid, if crash { Exit::Crash } else { Exit::Normal });
                }
            }
            Event::ServerStart => {
                self.ensure_server(None);
            }
            Event::Degrade { channel, ok } => self.set_channel(channel, ok),
            Event::TornRegistry { pick: i } => {
                let pids: Vec<u32> = self.claude.registry.keys().copied().collect();
                if let Some(file) = self
                    .pick_proc(&pids, i)
                    .and_then(|p| self.claude.registry.get_mut(&p))
                {
                    file.torn = true;
                }
            }
            Event::Discover => self.discover(),
            Event::DismissUpdate { pick: i } => self.dismiss_update(i),
            Event::TmuxDetach { pick: i } => self.tmux_detach(i),
            Event::TmuxAttach { pick: i, terminal } => self.tmux_attach(i, terminal),
            Event::Race { hazard } => self.race = Some(hazard),
            Event::SlowStart { on } => self.hosts.slow_start = on,
            Event::HostApi { kitty, on } => {
                let t = if kitty {
                    TerminalKind::Kitty
                } else {
                    TerminalKind::WezTerm
                };
                self.hosts.api.insert(t, on);
            }
            Event::Open(_) => {}
        }
    }

    fn set_channel(&mut self, channel: Channel, ok: bool) {
        let c = &mut self.channels;
        match channel {
            Channel::ProcTable => c.proc_table = ok,
            Channel::ClaudeRegistry => c.claude_registry = ok,
            Channel::ClaudeDesktop => c.claude_desktop = ok,
            Channel::CodexDb => c.codex_db = ok,
            Channel::OpenCodeDb => c.opencode_db = ok,
            Channel::OpenCodeHung => c.opencode_hung = !ok,
            Channel::Fs => c.fs = ok,
            Channel::Reservations => c.reservations = ok,
            Channel::Environment => c.environment = ok,
            Channel::Tmux => c.tmux = ok,
        }
    }

    fn upgrade(&mut self, app: Option<App>, tool: Option<Tool>) {
        if let Some(a) = app.and_then(|a| self.apps.get_mut(&a)) {
            a.version = "99.0.0-unverified".to_owned();
        }
        if let Some(t) = tool.and_then(|t| self.tools.get_mut(&t)) {
            *t = Some("99.0.0-unverified".to_owned());
        }
        if tool == Some(Tool::CodexCli) {
            self.codex.update_available = false;
        }
    }
}
