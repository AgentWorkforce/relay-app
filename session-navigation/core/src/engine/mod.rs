//! observe → decide → reserve → dispatch → observe → verify.
//!
//! A failed focus or link is reported, never turned into a launch. A second
//! open while a reservation is unresolved cannot launch again because the
//! reservation is part of the snapshot `decide` reads.
mod panes;
pub mod verify;

use serde::{Deserialize, Serialize};

use crate::decide::decide;
use crate::evidence::{Fact, Pending, Reservation, Snapshot};
use crate::model::{ProcessIdentity, Provider, Reason, SessionRef};
use crate::plan::{
    Action, ActionKind, Decision, Dispatch, OpenRequest, Outcome, Plan, PostCondition,
    SessionState, Step,
};
use crate::ports::{Execute, Observe, Reservations};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenReport {
    pub session: SessionRef,
    pub before: Snapshot,
    pub decision: Decision,
    pub executed: Vec<(Step, Dispatch)>,
    pub after: Option<Snapshot>,
    pub outcome: Outcome,
}

/// Observe and decide without acting.
pub fn plan<P: Observe>(
    ports: &mut P,
    session: &SessionRef,
    request: &OpenRequest,
) -> (Snapshot, Decision) {
    let snapshot = ports.observe(&session.normalized());
    let decision = decide(&snapshot, request);
    (snapshot, decision)
}

pub fn open<P>(ports: &mut P, session: &SessionRef, request: &OpenRequest) -> OpenReport
where
    P: Observe + Execute + Reservations,
{
    let session = &session.normalized();
    let (before, decision) = plan(ports, session, request);
    let had_reservation = matches!(before.reservation, Fact::Known(Some(_)));
    let mut report = OpenReport {
        session: session.clone(),
        before,
        decision: decision.clone(),
        executed: Vec::new(),
        after: None,
        outcome: Outcome::NeedsChoice,
    };
    if had_reservation && decision.release_reservation && ports.release(session).is_err() {
        report.outcome = Outcome::Blocked(Reason::ReservationStoreFailed);
        return report;
    }
    if !decision.release_reservation {
        mark_seen(ports, &report.before);
    }
    report.outcome = match &decision.plan {
        Plan::Unavailable(reason) => Outcome::Blocked(*reason),
        // A confirmation is a choice the person makes before anything runs.
        Plan::Choose(_) | Plan::Confirm(_) => Outcome::NeedsChoice,
        Plan::Act(action) => run(ports, &mut report, action, request),
    };
    report
}

fn may_start(action: &Action, step: &Step) -> bool {
    match step {
        Step::Launch { .. } => action.kind != ActionKind::Attach,
        Step::OpenUrl { .. } => action.kind == ActionKind::Reopen,
        _ => false,
    }
}

/// How many observations a take-over waits for the stopped owner to exit.
pub const MAX_EXIT_OBSERVATIONS: usize = 4;

/// How many observations a Claude desktop link gets to show its signal
/// (`lastFocusedAt` moving forward, native N1).
pub const MAX_LINK_OBSERVATIONS: usize = 3;

fn exited(snapshot: &Snapshot, process: &ProcessIdentity) -> bool {
    match &snapshot.processes {
        Fact::Known(table) => !table.iter().any(|p| p.identity == *process),
        Fact::Unknown => false,
    }
}

/// After a stop, observe again, up to [`MAX_EXIT_OBSERVATIONS`] times: the
/// owner must be gone and nothing may have replaced it (a supervisor can
/// restart a stopped process). An exit that is never proven never resumes.
fn await_exit<P: Observe>(
    ports: &mut P,
    session: &SessionRef,
    process: &ProcessIdentity,
    request: &OpenRequest,
) -> bool {
    let plain = OpenRequest {
        take_over: false,
        selected: None,
        ..*request
    };
    for _ in 0..MAX_EXIT_OBSERVATIONS {
        let snapshot = ports.observe(session);
        if !exited(&snapshot, process) {
            continue;
        }
        // Gone: the session must now look stopped, not taken by a successor.
        return decide(&snapshot, &plain).state == SessionState::Stopped;
    }
    false
}

/// Record that a process was observed on the tty of a reservation's window:
/// from now on an empty tty means the window closed.
fn mark_seen<P: Reservations>(ports: &mut P, snapshot: &Snapshot) {
    let (Fact::Known(Some(r)), Fact::Known(table)) = (&snapshot.reservation, &snapshot.processes)
    else {
        return;
    };
    let Some(tty) = &r.tty else {
        return;
    };
    if !r.seen && table.iter().any(|p| p.tty.as_ref() == Some(tty)) {
        // A failed write only keeps the reservation blocking.
        let _ = ports.reserve(Reservation {
            seen: true,
            ..r.clone()
        });
    }
}

fn reservation(report: &OpenReport, action: &Action, tty: Option<String>) -> Reservation {
    Reservation {
        session: report.session.clone(),
        made_at: report.before.observed_at,
        tty,
        seen: false,
        boot: report.before.boot.known().copied(),
        via: if action.kind == ActionKind::Reopen {
            Pending::Link
        } else {
            Pending::Launch
        },
    }
}

fn run<P>(ports: &mut P, report: &mut OpenReport, action: &Action, request: &OpenRequest) -> Outcome
where
    P: Observe + Execute + Reservations,
{
    let mut reserved = false;
    for step in &action.steps {
        if let Step::AwaitExit { process } = step {
            let ok = await_exit(ports, &report.session, process, request);
            report.executed.push((step.clone(), Dispatch::Dispatched));
            if !ok {
                return Outcome::Blocked(Reason::TakeOverRaced);
            }
            continue;
        }
        if !reserved && may_start(action, step) {
            if ports.reserve(reservation(report, action, None)).is_err() {
                return Outcome::Blocked(Reason::ReservationStoreFailed);
            }
            reserved = true;
        }
        let dispatch = ports.execute(step);
        report.executed.push((step.clone(), dispatch.clone()));
        match dispatch {
            Dispatch::Dispatched => {}
            Dispatch::Opened { tty } if reserved => {
                // Without the handle the reservation still blocks relaunching.
                let _ = ports.reserve(reservation(report, action, Some(tty)));
            }
            Dispatch::Opened { .. } => {}
            Dispatch::Refused => {
                // The script never ran (Automation refused, -1743): nothing
                // can start, so the reservation must not block a retry.
                if reserved {
                    let _ = ports.release(&report.session);
                }
                return Outcome::Blocked(Reason::PermissionDenied);
            }
            Dispatch::Failed(reason) => return Outcome::Failed(reason),
        }
    }
    let mut after = ports.observe(&report.session);
    let mut outcome = verify::check_against(&action.post, &report.before, &after, request);
    // Claude.app handles a link asynchronously (it may still be coming to
    // the front): look for its signal a bounded number of times; the last
    // observation decides.
    let claude_link = report.session.provider == Provider::Claude
        && matches!(action.post, PostCondition::ConversationShown { .. });
    for _ in 1..MAX_LINK_OBSERVATIONS {
        if !claude_link || outcome == Outcome::Verified {
            break;
        }
        after = ports.observe(&report.session);
        outcome = verify::check_against(&action.post, &report.before, &after, request);
    }
    if outcome == Outcome::Verified && reserved {
        // A failed release only leaves a reservation that blocks relaunching.
        let _ = ports.release(&report.session);
    } else if reserved {
        mark_seen(ports, &after);
    }
    report.after = Some(after);
    outcome
}
