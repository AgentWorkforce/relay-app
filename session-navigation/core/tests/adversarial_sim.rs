//! Adversarial review of the simulator and the oracle.
//!
//! Every test states the defect it probes and the fact-file evidence. A
//! failing test is a finding; a passing test is a confirmed strength.
//! Nothing here touches the real machine: everything runs in `sim::World`.
#[path = "adversarial_sim/fidelity.rs"]
mod fidelity;
#[path = "adversarial_sim/floor.rs"]
mod floor;
#[path = "adversarial_sim/oracle_blind_spots.rs"]
mod oracle_blind_spots;
#[path = "adversarial_sim/probe.rs"]
mod probe;
#[path = "adversarial_sim/vacuity.rs"]
mod vacuity;

use std::collections::{BTreeMap, BTreeSet};

use probe::{aggregate, long_run_bits, probe_run};
use session_navigation::engine::{self, verify, OpenReport};
use session_navigation::evidence::{CodexOrigin, Fact};
use session_navigation::model::{App, Precision, Provider, Reason, SessionRef, TerminalKind, Tool};
use session_navigation::plan::{
    Action, ActionKind, Command, Decision, OpenRequest, Outcome, Plan, PostCondition, SessionState,
    Step,
};
use session_navigation::ports::{Execute, Observe};
use session_navigation::sim::agents::StartHost;
use session_navigation::sim::events::dir;
use session_navigation::sim::gen::generate;
use session_navigation::sim::oracle::{self, Violation};
use session_navigation::sim::runner::{run, run_traced};
use session_navigation::sim::truth::Exit;
use session_navigation::sim::world::{Role, Thread};
use session_navigation::sim::{Event, World, WorldVars};

// ------------------------------------------------------------- helpers ---

/// A machine with every app installed, Automation granted and no launch
/// delay: the "clean common case".
fn clean(vars: WorldVars) -> World {
    let mut w = World::new(11, vars);
    for app in App::ALL {
        if let Some(a) = w.apps.get_mut(&app) {
            a.installed = true;
        }
        w.automation.insert(app, true);
    }
    w.launch_delay = 0;
    w.codex.update_available = false;
    w
}

fn req() -> OpenRequest {
    OpenRequest {
        // An explicit terminal preference: there is no built-in default.
        prefs: session_navigation::plan::Preferences {
            default_terminal: Some(session_navigation::model::TerminalKind::Ghostty),
            ..Default::default()
        },
        ..OpenRequest::default()
    }
}

/// Open through the engine with attribution, exactly as the runner does, and
/// return the report plus the oracle verdict.
fn open(
    w: &mut World,
    s: &SessionRef,
    request: &OpenRequest,
) -> (OpenReport, Result<(), Violation>) {
    let id = 5_000 + w.log.len() as u64;
    let from = w.log.len();
    w.engine_open = Some(id);
    let report = engine::open(w, s, request);
    w.engine_open = None;
    let imports: BTreeSet<u64> = if report.decision.action().is_some_and(|a| a.import) {
        [id].into()
    } else {
        BTreeSet::new()
    };
    let verdict = oracle::check_open(w, id, request, &report)
        .and_then(|()| oracle::check_log(w, from, &imports));
    (report, verdict)
}

/// Dispatch a hand-written ("rogue") action as if the engine had planned it,
/// then ask the oracle. Used to find harms the oracle cannot see: a planner
/// regression producing this action would pass the whole simulation suite.
fn rogue(w: &mut World, s: &SessionRef, action: Action) -> Result<(), Violation> {
    let request = req();
    let id = 9_000 + w.log.len() as u64;
    let from = w.log.len();
    let before = w.observe(s);
    w.engine_open = Some(id);
    let mut executed = Vec::new();
    for step in &action.steps {
        let d = w.execute(step);
        executed.push((step.clone(), d));
    }
    w.engine_open = None;
    let after = w.observe(s);
    let outcome = verify::check(&action.post, &after, &request);
    let report = OpenReport {
        session: s.clone(),
        before,
        decision: Decision::new(SessionState::Stopped, Plan::Act(action)),
        executed,
        after: Some(after),
        outcome,
    };
    oracle::check_open(w, id, &request, &report)?;
    oracle::check_log(w, from, &BTreeSet::new())
}

fn launch_action(terminal: TerminalKind, program: &str, args: &[&str], cwd: &str) -> Action {
    Action {
        kind: ActionKind::Resume,
        steps: vec![Step::Launch {
            terminal,
            command: Command {
                program: program.into(),
                args: args.iter().map(|a| a.to_string()).collect(),
                cwd: cwd.into(),
                env: Default::default(),
            },
            context: session_navigation::plan::LaunchContext::native(),
        }],
        precision: Precision::ExactConversation,
        post: PostCondition::RunningIn { terminal },
        note: None,
        import: false,
        target: None,
    }
}

fn kind(d: &Decision) -> Option<ActionKind> {
    d.action().map(|a| a.kind)
}

fn useful(o: &Outcome) -> bool {
    matches!(o, Outcome::Verified | Outcome::DispatchedUnverified)
}

fn pane_of(w: &World, s: &SessionRef) -> u64 {
    let tty = w
        .procs
        .values()
        .find(|p| p.hosts.as_ref() == Some(s) && p.tty.is_some())
        .and_then(|p| p.tty.clone())
        .expect("terminal owner");
    w.panes
        .values()
        .find(|p| p.tty == tty)
        .map(|p| p.id)
        .expect("pane")
}

/// Run `scenario` under every world-variable combination selected by
/// `filter`, collecting failures (bits + message). Asserts none.
fn all_combos(
    name: &str,
    filter: impl Fn(WorldVars) -> bool,
    scenario: impl Fn(WorldVars) -> Result<(), String>,
) {
    let mut failures = Vec::new();
    let mut ran = 0;
    for bits in 0..1u32 << WorldVars::LEGACY {
        let vars = WorldVars::from_bits(bits);
        if !filter(vars) {
            continue;
        }
        ran += 1;
        if let Err(e) = scenario(vars) {
            failures.push(format!("vars={bits:#x}: {e}"));
        }
    }
    assert!(ran > 0, "{name}: no combination selected");
    assert!(
        failures.is_empty(),
        "{name}: {} of {ran} combinations fail; first: {}",
        failures.len(),
        failures
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(" | ")
    );
}
