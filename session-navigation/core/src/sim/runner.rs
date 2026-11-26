//! Runs event sequences against a World, checks every invariant after every
//! step, and shrinks failures.
use std::collections::BTreeSet;
use std::panic::{catch_unwind, AssertUnwindSafe};

use serde::{Deserialize, Serialize};

use super::events::{Event, OpenEvent};
use super::oracle::{self, Invariant, Violation};
use super::truth::command_target;
use super::vars::WorldVars;
use super::world::World;
use crate::engine::{self, OpenReport};
use crate::model::{Provider, Reason, SessionRef};
use crate::plan::{ActionKind, OpenRequest, Outcome, Plan, Preferences};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStats {
    pub opens: usize,
    pub verified: usize,
    pub verified_resumes: usize,
    pub blocked: usize,
    pub failed: usize,
    pub unverified: usize,
    /// (provider, action kind) pairs that were verified.
    pub verified_kinds: BTreeSet<(Provider, String)>,
    /// How often each refusal reason occurred.
    pub blocked_reasons: std::collections::BTreeMap<String, usize>,
    /// Opens answered with a request for confirmation (`Plan::Confirm`).
    pub confirmations: usize,
    /// Outcome classes per provider: Verified, DispatchedUnverified,
    /// NeedsConfirmation, NeedsChoice, Failed, Refused.
    pub by_provider: std::collections::BTreeMap<String, std::collections::BTreeMap<String, usize>>,
    /// The same classes per `Provider/ActionKind` of the planned action (the
    /// guarded action for a confirmation).
    pub by_action: std::collections::BTreeMap<String, std::collections::BTreeMap<String, usize>>,
    /// The same classes per `Provider/origin host` of the opened session.
    pub by_host: std::collections::BTreeMap<String, std::collections::BTreeMap<String, usize>>,
    /// Failure and refusal reasons per platform (`MacOs`, `Linux/X11`, …).
    pub by_platform: std::collections::BTreeMap<String, std::collections::BTreeMap<String, usize>>,
    /// Refusal reasons per provider.
    pub refusals: std::collections::BTreeMap<String, std::collections::BTreeMap<String, usize>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub seed: u64,
    pub vars: u32,
    pub step: usize,
    pub violation: Violation,
}

struct Runner {
    world: World,
    opens: u64,
    imports: BTreeSet<u64>,
    stats: RunStats,
    trace: Option<Vec<String>>,
}

fn violation(invariant: Invariant, detail: impl Into<String>) -> Violation {
    Violation {
        invariant,
        cause: format!("{invariant:?}/runner"),
        detail: detail.into(),
    }
}

impl Runner {
    /// A truthful person only confirms a launch is gone when it is.
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

    fn open_once(
        &mut self,
        session: &SessionRef,
        request: &OpenRequest,
    ) -> Result<OpenReport, Violation> {
        self.opens += 1;
        let id = self.opens;
        let from = self.world.log.len();
        let was_live = self.world.writers(session) > 0;
        self.world.engine_open = Some(id);
        let result = catch_unwind(AssertUnwindSafe(|| {
            engine::open(&mut self.world, session, request)
        }));
        self.world.engine_open = None;
        let report =
            result.map_err(|_| violation(Invariant::Total, "decide or engine panicked"))?;
        if report.decision.action().is_some_and(|a| a.import) && request.prefs.allow_import {
            self.imports.insert(id);
        }
        if self.trace.is_some() {
            let line = describe_open(&self.world, id, session, request, &report);
            if let Some(trace) = &mut self.trace {
                trace.push(line);
            }
        }
        oracle::check_open(&self.world, id, request, &report)?;
        oracle::check_log(&self.world, from, &self.imports)?;
        super::preference::check(request, &report, was_live)?;
        self.record(&report);
        Ok(report)
    }

    fn record(&mut self, report: &OpenReport) {
        let s = &mut self.stats;
        s.opens += 1;
        let class = match (&report.outcome, &report.decision.plan) {
            (_, Plan::Confirm(_)) => "NeedsConfirmation",
            (Outcome::Verified, _) => "Verified",
            (Outcome::DispatchedUnverified, _) => "DispatchedUnverified",
            (Outcome::NeedsChoice, _) => "NeedsChoice",
            (Outcome::Failed(_), _) => "Failed",
            (Outcome::Blocked(_), _) => "Refused",
        };
        *s.by_provider
            .entry(format!("{:?}", report.session.provider))
            .or_default()
            .entry(class.to_owned())
            .or_default() += 1;
        let origin = self
            .world
            .origins
            .get(&report.session)
            .cloned()
            .unwrap_or_else(|| "Other".to_owned());
        *s.by_host
            .entry(format!("{:?}/{origin}", report.session.provider))
            .or_default()
            .entry(class.to_owned())
            .or_default() += 1;
        let planned = match &report.decision.plan {
            Plan::Act(action) => Some(action.kind),
            Plan::Confirm(confirm) => Some(confirm.then.kind),
            _ => None,
        };
        if let Some(kind) = planned {
            *s.by_action
                .entry(format!("{:?}/{kind:?}", report.session.provider))
                .or_default()
                .entry(class.to_owned())
                .or_default() += 1;
        }
        let platform = match report.before.platform {
            crate::model::Platform::Linux { session } => format!("Linux/{session:?}"),
            other => format!("{other:?}"),
        };
        let reason = match &report.outcome {
            Outcome::Blocked(reason) => Some(format!("Refused:{reason:?}")),
            Outcome::Failed(reason) => Some(format!("Failed:{reason:?}")),
            _ => None,
        };
        let row = s.by_platform.entry(platform).or_default();
        *row.entry(class.to_owned()).or_default() += 1;
        if let Some(reason) = reason {
            *row.entry(reason).or_default() += 1;
        }
        if let Outcome::Blocked(reason) = &report.outcome {
            *s.refusals
                .entry(format!("{:?}", report.session.provider))
                .or_default()
                .entry(format!("{reason:?}"))
                .or_default() += 1;
        }
        if matches!(report.decision.plan, Plan::Confirm(_)) {
            s.confirmations += 1;
        }
        match &report.outcome {
            Outcome::Verified => {
                s.verified += 1;
                if let Some(action) = report.decision.action() {
                    if action.starts_process() {
                        s.verified_resumes += 1;
                    }
                    s.verified_kinds
                        .insert((report.session.provider, format!("{:?}", action.kind)));
                }
            }
            Outcome::DispatchedUnverified => s.unverified += 1,
            Outcome::Failed(_) => s.failed += 1,
            Outcome::Blocked(reason) => {
                s.blocked += 1;
                *s.blocked_reasons.entry(format!("{reason:?}")).or_default() += 1;
            }
            Outcome::NeedsChoice => s.blocked += 1,
        }
    }

    /// After a verified resume the next decision returns to it, not resumes.
    fn converges(&mut self, report: &OpenReport, prefs: Preferences) -> Result<(), Violation> {
        let resumed = report.outcome == Outcome::Verified
            && report.decision.action().is_some_and(|a| a.starts_process());
        if !resumed {
            return Ok(());
        }
        let request = OpenRequest {
            prefs,
            ..OpenRequest::default()
        };
        let (_, next) = engine::plan(&mut self.world, &report.session, &request);
        // A confirmation that guards a return (not another resume) also
        // converges: the person is asked again, nothing starts.
        let kind = match &next.plan {
            Plan::Confirm(confirm) => Some(confirm.then.kind),
            _ => next.action().map(|a| a.kind),
        };
        // Honest uncertainty is acceptable when a channel is down (the
        // snapshot is then worse than the one that verified the resume);
        // planning another resume never is.
        let honest_unknown = next.plan == Plan::Unavailable(Reason::EvidenceIncomplete);
        match kind {
            Some(ActionKind::FocusPane | ActionKind::OpenExisting | ActionKind::BringForward) => {
                Ok(())
            }
            None if honest_unknown => Ok(()),
            _ => Err(violation(
                Invariant::Converges,
                format!("after a verified resume the next plan is {:?}", next.plan),
            )),
        }
    }

    fn open(&mut self, event: &OpenEvent) -> Result<(), Violation> {
        let session = self.world.open_target(event.pick);
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
        let mut report = self.open_once(&session, &request)?;
        self.converges(&report, prefs)?;
        if let (true, Plan::Choose(choices)) = (event.choose_first, &report.decision.plan) {
            request.selected = choices.first().and_then(|a| a.target);
            report = self.open_once(&session, &request)?;
            self.converges(&report, prefs)?;
        }
        // A truthful person confirms only when the session is not open in
        // the other terminals (nor about to start anywhere).
        let confirm = matches!(report.decision.plan, Plan::Confirm(_));
        if confirm && event.answer_confirm && self.world.truly_not_open_elsewhere(&session) {
            request.confirm_not_open_elsewhere = true;
            let confirmed = self.open_once(&session, &request)?;
            self.converges(&confirmed, prefs)?;
        }
        if event.twice {
            request.confirm_launch_gone = event.confirm_gone && self.launch_gone(&session);
            let second = self.open_once(&session, &request)?;
            self.converges(&second, prefs)?;
            let ids: BTreeSet<u64> = (first_id..=self.opens).collect();
            let starts = oracle::engine_starts(&self.world, &ids, &session);
            // A person who truthfully confirmed the first launch is gone (it
            // never started a writer) asked for a second one.
            if starts > 1 && !request.confirm_launch_gone {
                return Err(violation(
                    Invariant::Idempotent,
                    format!("{starts} starts across a double open"),
                ));
            }
        }
        // A hazard strikes only between the open's decision and its steps.
        self.world.race = None;
        Ok(())
    }

    fn racy(&self, event: &Event) -> bool {
        self.world.races_engine_launch(event)
    }

    fn step(&mut self, event: &Event) -> Result<(), Violation> {
        let from = self.world.log.len();
        if let Some(trace) = &mut self.trace {
            trace.push(format!("{event:?}"));
        }
        match event {
            Event::Open(open) => self.open(open)?,
            _ if self.racy(event) => {}
            _ => self.world.apply(event),
        }
        oracle::check_log(&self.world, from, &self.imports)
    }
}

fn describe_open(
    world: &World,
    id: u64,
    session: &SessionRef,
    request: &OpenRequest,
    report: &OpenReport,
) -> String {
    let registry: Vec<(u32, u64, String)> = world
        .claude
        .registry
        .iter()
        .map(|(pid, f)| {
            (
                *pid,
                f.mtime,
                String::from_utf8_lossy(&f.bytes).chars().take(80).collect(),
            )
        })
        .collect();
    let hosts: Vec<(u32, u64, String, String)> = world
        .procs
        .values()
        .filter_map(|p| {
            p.hosts
                .as_ref()
                .map(|h| (p.pid, p.start, h.id.clone(), format!("{:?}", p.role)))
        })
        .collect();
    format!(
        "  open #{id} {session:?} {request:?}\n    decision: {:?}\n    executed: {:?}\n    outcome: {:?}\n    registry after: {registry:?}\n    hosts after: {hosts:?}",
        report.decision, report.executed, report.outcome
    )
}

/// Run `events` in a fresh world. Every invariant is checked after every step.
pub fn run(seed: u64, vars: WorldVars, events: &[Event]) -> Result<RunStats, Failure> {
    execute_run(seed, vars, events, None).0
}

/// Like [`run`], also returning a line per event and open for debugging.
pub fn run_traced(
    seed: u64,
    vars: WorldVars,
    events: &[Event],
) -> (Result<RunStats, Failure>, Vec<String>) {
    let (result, trace) = execute_run(seed, vars, events, Some(Vec::new()));
    (result, trace.unwrap_or_default())
}

fn execute_run(
    seed: u64,
    vars: WorldVars,
    events: &[Event],
    trace: Option<Vec<String>>,
) -> (Result<RunStats, Failure>, Option<Vec<String>>) {
    let mut runner = Runner {
        world: World::new_on(seed, vars, super::hosts::platform_for(seed)),
        opens: 0,
        imports: BTreeSet::new(),
        stats: RunStats::default(),
        trace,
    };
    for (step, event) in events.iter().enumerate() {
        if let Err(violation) = runner.step(event) {
            let failure = Failure {
                seed,
                vars: vars.bits(),
                step,
                violation,
            };
            return (Err(failure), runner.trace);
        }
    }
    (Ok(runner.stats), runner.trace)
}

/// Greedily drop events while the run still fails for the same cause (the
/// invariant, the check and the provider). Picks follow identities, so
/// dropping an event does not retarget the others.
pub fn shrink(seed: u64, vars: WorldVars, events: &[Event], violation: &Violation) -> Vec<Event> {
    let mut current = events.to_vec();
    let fails = |candidate: &[Event]| {
        matches!(run(seed, vars, candidate), Err(f)
            if f.violation.invariant == violation.invariant && f.violation.cause == violation.cause)
    };
    loop {
        let mut changed = false;
        let mut index = 0;
        while index < current.len() {
            let candidate: Vec<Event> = current
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != index)
                .map(|(_, e)| *e)
                .collect();
            if fails(&candidate) {
                current = candidate;
                changed = true;
            } else {
                index += 1;
            }
        }
        if !changed {
            return current;
        }
    }
}

/// A human-readable failure report with the minimal prefix and a replay line.
pub fn explain(failure: &Failure, minimal: &[Event]) -> String {
    let mut out = format!(
        "session-navigation simulation failed: seed={} vars={:#x} invariant={:?} at event #{}\n  cause: {}\n  detail: {}\n  minimal sequence ({} events):\n",
        failure.seed, failure.vars, failure.violation.invariant, failure.step, failure.violation.cause, failure.violation.detail, minimal.len()
    );
    for event in minimal {
        out.push_str(&format!("    {event:?}\n"));
    }
    out.push_str(&format!(
        "  replay: SESSION_NAVIGATION_REPLAY={}:{} cargo test --test simulation replay -- --nocapture\n",
        failure.seed, failure.vars
    ));
    out
}
