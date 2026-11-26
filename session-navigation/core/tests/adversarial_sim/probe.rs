//! An instrumented replica of `sim::runner` (whose `Runner` is private). It
//! drives the same generator, the same engine and the same oracle, in the same
//! order, and additionally counts which situations each open actually met.
//! `replica_matches_the_official_runner` checks that it does not drift.
use std::collections::{BTreeMap, BTreeSet};

#[path = "classify.rs"]
mod classify;

#[path = "triggers.rs"]
mod triggers;

use classify::TruthBefore;

use session_navigation::engine::{self, OpenReport};
use session_navigation::evidence::{Exe, Fact, ProviderEvidence};
use session_navigation::model::{App, Provider, Reason, SessionRef};
use session_navigation::plan::{
    ActionKind, OpenRequest, Outcome, Plan, PostCondition, Preferences, SessionState, Step,
};
use session_navigation::sim::agents::StartHost;
use session_navigation::sim::hosts::platform_for;
use session_navigation::sim::oracle;
use session_navigation::sim::truth::command_target;
use session_navigation::sim::world::Role;
use session_navigation::sim::{Event, OpenEvent, World, WorldVars};

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct Totals {
    pub opens: usize,
    pub verified: usize,
    pub blocked: usize,
    pub failed: usize,
    pub unverified: usize,
}

pub struct Probe {
    pub world: World,
    pub vars: WorldVars,
    opens: u64,
    imports: BTreeSet<u64>,
    pub counts: BTreeMap<String, usize>,
    pub origin: BTreeMap<SessionRef, &'static str>,
    pub violation: Option<String>,
    pub totals: Totals,
}

fn start_tag(host: StartHost) -> &'static str {
    match host {
        StartHost::ClaudeDesktop => "claude-desktop",
        StartHost::CodexDesktop => "codex-desktop",
        StartHost::ClaudeGhostty => "claude-ghostty",
        StartHost::CodexGhostty => "codex-ghostty",
        StartHost::OpenCodeDesktop => "opencode-desktop",
        StartHost::OpenCodeTerminal { .. } => "opencode-terminal",
        StartHost::Headless { provider, .. } => match provider {
            Provider::Claude => "claude-headless",
            Provider::Codex => "codex-headless",
            Provider::OpenCode => "opencode-headless",
            Provider::Grok => "grok-headless",
        },
        StartHost::Terminal { provider, .. } => match provider {
            Provider::Claude => "claude-terminal",
            Provider::Codex => "codex-terminal",
            Provider::OpenCode => "opencode-terminal-any",
            Provider::Grok => "grok-terminal",
        },
    }
}

impl Probe {
    pub fn new(seed: u64, vars: WorldVars) -> Self {
        Self {
            world: World::new_on(seed, vars, platform_for(seed)),
            vars,
            opens: 0,
            imports: BTreeSet::new(),
            counts: BTreeMap::new(),
            origin: BTreeMap::new(),
            violation: None,
            totals: Totals::default(),
        }
    }

    fn bump(&mut self, key: String) {
        *self.counts.entry(key).or_default() += 1;
    }

    pub(crate) fn var_trigger(&mut self, name: &str, value: bool) {
        self.bump(format!("var:{name}={}", u8::from(value)));
    }

    /// Tag catalog entries that no Start event tagged (/clear successors,
    /// subagents, engine-created ids) from truth.
    fn tag_untagged(&mut self) {
        let catalog = self.world.catalog.clone();
        for s in catalog {
            if self.origin.contains_key(&s) {
                continue;
            }
            let tag = match s.provider {
                Provider::Claude => match self.world.claude.transcripts.get(&s.id) {
                    Some(t) => match t.entrypoint {
                        session_navigation::evidence::Entrypoint::Desktop => "claude-desktop",
                        session_navigation::evidence::Entrypoint::Cli => "claude-ghostty",
                        _ => "claude-headless",
                    },
                    None => "claude-other",
                },
                Provider::Codex => match self.world.codex.threads.get(&s.id) {
                    Some(t) if t.subagent => "codex-child",
                    _ => "codex-other",
                },
                Provider::OpenCode => match self.world.opencode.sessions.get(&s.id) {
                    Some(o) if o.parent.is_some() => "opencode-child",
                    _ => "opencode-other",
                },
                Provider::Grok => "grok-other",
            };
            self.origin.insert(s, tag);
        }
    }

    fn host_of(&self, s: &SessionRef) -> &'static str {
        self.origin.get(s).copied().unwrap_or(match s.provider {
            Provider::Claude => "claude-nonexistent",
            Provider::Codex => "codex-nonexistent",
            Provider::OpenCode => "opencode-nonexistent",
            Provider::Grok => "grok-nonexistent",
        })
    }

    // ---- replica of Runner ------------------------------------------------

    // Kept in step with the runner: opens resolve their target by identity
    // (`World::open_target`) since stable picks were added.
    fn session_for(&mut self, pick: u32) -> SessionRef {
        self.world.open_target(pick)
    }

    // Kept in step with the runner: our own launched client counts too.
    fn launch_gone(&self, session: &SessionRef) -> bool {
        self.world.writers(session) == 0
            && !self
                .world
                .procs
                .values()
                .any(|p| p.hosts.as_ref() == Some(session) && p.by_engine.is_some())
            && !self
                .world
                .pending
                .iter()
                .any(|p| command_target(&p.command).as_ref() == Some(session))
    }

    // Kept in step with the runner: the races the generator leaves out are
    // now defined in one place, `World::races_engine_launch`.
    fn racy(&self, event: &Event) -> bool {
        self.world.races_engine_launch(event)
    }

    pub fn step(&mut self, event: &Event) {
        if self.violation.is_some() {
            return;
        }
        let from = self.world.log.len();
        match event {
            Event::Open(open) => self.open(open),
            _ if self.racy(event) => {}
            _ => {
                self.pre_event(event);
                let before = self.world.catalog.len();
                self.world.apply(event);
                if let Event::Start { host, .. } = event {
                    let tag = start_tag(*host);
                    let new: Vec<SessionRef> =
                        self.world.catalog.iter().skip(before).cloned().collect();
                    for s in new {
                        self.origin.entry(s).or_insert(tag);
                    }
                }
            }
        }
        self.tag_untagged();
        if self.violation.is_none() {
            if let Err(v) = oracle::check_log(&self.world, from, &self.imports) {
                self.violation = Some(format!("{v:?}"));
            }
        }
    }

    /// Var triggers that only person events can reach.
    fn pre_event(&mut self, event: &Event) {
        match event {
            Event::PersonResume { pick } => {
                let Some(s) = self.world.pick_session(*pick) else {
                    return;
                };
                match s.provider {
                    Provider::Codex => {
                        if self.world.writers(&s) > 0 {
                            self.var_trigger(
                                "codex_second_writer_refused",
                                self.vars.codex_second_writer_refused,
                            );
                        }
                        if self
                            .world
                            .codex
                            .threads
                            .get(&s.id)
                            .is_some_and(|t| t.archived)
                        {
                            self.var_trigger(
                                "codex_resume_archived_works",
                                self.vars.codex_resume_archived_works,
                            );
                        }
                    }
                    Provider::OpenCode => {
                        if self
                            .world
                            .opencode
                            .sessions
                            .get(&s.id)
                            .is_some_and(|o| o.archived || o.parent.is_some())
                        {
                            self.var_trigger(
                                "opencode_tui_opens_archived_or_child",
                                self.vars.opencode_tui_opens_archived_or_child,
                            );
                        }
                    }
                    Provider::Grok => self.grok_person_triggers(&s),
                    Provider::Claude => {}
                }
            }
            Event::Clear { .. }
                if self.world.procs.values().any(|p| {
                    p.exe == Exe::GrokCli && p.role == Role::Interactive && p.hosts.is_some()
                }) =>
            {
                self.var_trigger(
                    "grok_registry_follows_session",
                    self.vars.grok_registry_follows_session,
                );
            }
            Event::QuitApp {
                app: App::OpenCodeDesktop,
            } if self.world.opencode.server.is_some()
                && self.world.running(App::OpenCodeDesktop) =>
            {
                self.var_trigger(
                    "opencode_server_survives_quit",
                    self.vars.opencode_server_survives_quit,
                );
            }
            _ => {}
        }
    }

    fn open_once(
        &mut self,
        session: &SessionRef,
        request: &OpenRequest,
        second_after_start: bool,
    ) -> Option<OpenReport> {
        self.opens += 1;
        let id = self.opens;
        let from = self.world.log.len();
        let pending_before = self
            .world
            .pending
            .iter()
            .any(|p| command_target(&p.command).as_ref() == Some(session));
        let truth = TruthBefore::capture(&self.world, session);
        self.world.engine_open = Some(id);
        let report = engine::open(&mut self.world, session, request);
        self.world.engine_open = None;
        self.host_var_triggers(&report);
        if report.decision.action().is_some_and(|a| a.import) && request.prefs.allow_import {
            self.imports.insert(id);
        }
        if let Err(v) = oracle::check_open(&self.world, id, request, &report) {
            self.violation = Some(format!("{v:?}"));
            return None;
        }
        if let Err(v) = oracle::check_log(&self.world, from, &self.imports) {
            self.violation = Some(format!("{v:?}"));
            return None;
        }
        self.record(&report);
        self.classify(session, &report, pending_before, second_after_start, &truth);
        Some(report)
    }

    fn record(&mut self, report: &OpenReport) {
        let t = &mut self.totals;
        t.opens += 1;
        match &report.outcome {
            Outcome::Verified => t.verified += 1,
            Outcome::DispatchedUnverified => t.unverified += 1,
            Outcome::Failed(_) => t.failed += 1,
            Outcome::Blocked(_) | Outcome::NeedsChoice => t.blocked += 1,
        }
    }

    fn converges(&mut self, report: &OpenReport, prefs: Preferences) {
        let resumed = report.outcome == Outcome::Verified
            && report.decision.action().is_some_and(|a| a.starts_process());
        if !resumed {
            return;
        }
        let request = OpenRequest {
            prefs,
            ..OpenRequest::default()
        };
        let (_, next) = engine::plan(&mut self.world, &report.session, &request);
        let host = self.host_of(&report.session);
        self.bump(format!("{host}|converges_checked"));
        // A confirmation guarding a return (not another resume) converges too.
        let kind = match &next.plan {
            Plan::Confirm(confirm) => Some(confirm.then.kind),
            _ => next.action().map(|a| a.kind),
        };
        let honest_unknown = next.plan == Plan::Unavailable(Reason::EvidenceIncomplete);
        let ok = matches!(
            kind,
            Some(ActionKind::FocusPane | ActionKind::OpenExisting | ActionKind::BringForward)
        ) || (kind.is_none() && honest_unknown);
        if !ok && self.violation.is_none() {
            self.violation = Some(format!("Converges: {:?}", next.plan));
        }
    }

    fn started(report: &OpenReport) -> bool {
        report.executed.iter().any(|(s, _)| match s {
            Step::Launch { .. } => true,
            Step::OpenUrl { .. } => report
                .decision
                .action()
                .is_some_and(|a| a.kind == ActionKind::Reopen),
            _ => false,
        })
    }

    fn open(&mut self, event: &OpenEvent) {
        let session = self.session_for(event.pick);
        // Kept in step with the runner: the platform's default terminals.
        let [first, second] = self.world.default_terminals();
        let prefs = Preferences {
            default_terminal: (!event.no_preference).then_some(if event.second_terminal {
                second
            } else {
                first
            }),
            allow_import: event.allow_import,
            prefer_terminal: event.prefer_terminal,
        };
        let mut request = OpenRequest {
            prefs,
            take_over: event.take_over,
            use_default_terminal: event.default_terminal,
            confirm_launch_gone: event.confirm_gone && self.launch_gone(&session),
            selected: None,
            confirm_not_open_elsewhere: false,
        };
        let first_id = self.opens + 1;
        let Some(mut report) = self.open_once(&session, &request, false) else {
            return;
        };
        self.converges(&report, prefs);
        let mut any_started = Self::started(&report);
        if let (true, Plan::Choose(choices)) = (event.choose_first, &report.decision.plan) {
            request.selected = choices.first().and_then(|a| a.target);
            let Some(r) = self.open_once(&session, &request, false) else {
                return;
            };
            report = r;
            any_started |= Self::started(&report);
            self.converges(&report, prefs);
        }
        // Kept in step with the runner: a truthful person answers a
        // confirmation request and opens again.
        let confirm = matches!(report.decision.plan, Plan::Confirm(_));
        if confirm && event.answer_confirm && self.world.truly_not_open_elsewhere(&session) {
            request.confirm_not_open_elsewhere = true;
            let Some(confirmed) = self.open_once(&session, &request, false) else {
                return;
            };
            any_started |= Self::started(&confirmed);
            self.converges(&confirmed, prefs);
        }
        if event.twice {
            request.confirm_launch_gone = event.confirm_gone && self.launch_gone(&session);
            let Some(second) = self.open_once(&session, &request, any_started) else {
                return;
            };
            self.converges(&second, prefs);
            let ids: BTreeSet<u64> = (first_id..=self.opens).collect();
            let starts = oracle::engine_starts(&self.world, &ids, &session);
            if starts > 1 && !request.confirm_launch_gone && self.violation.is_none() {
                self.violation = Some(format!("Idempotent: {starts}"));
            }
        }
        // Kept in step with the runner: an armed hazard lasts one open.
        self.world.race = None;
    }
}

/// Run one generated sequence through the probe.
pub fn probe_run(seed: u64, vars: WorldVars, events: &[Event]) -> Probe {
    let mut p = Probe::new(seed, vars);
    for e in events {
        p.step(e);
        if p.violation.is_some() {
            break;
        }
    }
    p
}

pub const COMBINATIONS: u32 = 1 << WorldVars::COUNT;

/// The same seed → vars mapping as `tests/simulation.rs::long_random_runs`.
pub fn long_run_bits(seed: u64) -> u32 {
    u32::try_from(seed.wrapping_mul(2_654_435_761) % u64::from(COMBINATIONS)).unwrap_or(0)
}

/// Aggregate counts over many seeded runs.
pub fn aggregate(
    seeds: std::ops::Range<u64>,
    len: usize,
) -> (BTreeMap<String, usize>, Vec<String>) {
    let mut total: BTreeMap<String, usize> = BTreeMap::new();
    let mut violations = Vec::new();
    for seed in seeds {
        let bits = long_run_bits(seed);
        let events = session_navigation::sim::gen::generate(seed, len);
        let p = probe_run(seed, WorldVars::from_bits(bits), &events);
        if let Some(v) = p.violation {
            violations.push(format!("seed {seed} bits {bits:#x}: {v}"));
        }
        for (k, n) in p.counts {
            *total.entry(k).or_default() += n;
        }
    }
    (total, violations)
}
