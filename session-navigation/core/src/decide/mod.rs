//! `decide`: pure, total, no I/O, no clock. Shared guards live here; provider
//! policy lives in [`crate::providers`].
pub mod common;
pub mod gate;
pub mod interpret;
pub mod locate;

use crate::evidence::{Fact, Pending, ProcessInfo, Reservation, Snapshot};
use crate::model::{BootId, Platform, Reason};
use crate::parse::ids;
use crate::plan::{Action, ActionKind, Decision, OpenRequest, Plan, SessionState, Step};
use crate::providers;

/// What a provider policy sees: the snapshot plus the facts every policy
/// needs already confirmed as known.
pub struct Context<'a> {
    pub snap: &'a Snapshot,
    pub request: &'a OpenRequest,
    pub table: &'a [ProcessInfo],
    pub boot: BootId,
}

/// Decide what to do for one snapshot. Never panics; every path ends in a plan
/// or a specific reason.
pub fn decide(snap: &Snapshot, request: &OpenRequest) -> Decision {
    let normalized = snap.session.normalized();
    if normalized != snap.session {
        let snap = Snapshot {
            session: normalized,
            ..snap.clone()
        };
        return decide_normalized(&snap, request);
    }
    decide_normalized(snap, request)
}

fn decide_normalized(snap: &Snapshot, request: &OpenRequest) -> Decision {
    if !ids::is_valid_session(&snap.session) {
        return Decision::unavailable(SessionState::InvalidId, Reason::InvalidSessionId);
    }
    let (Fact::Known(table), Fact::Known(boot)) = (&snap.processes, &snap.boot) else {
        return Decision::unavailable(SessionState::Unknown, Reason::EvidenceIncomplete);
    };
    let decision = guarded(snap, request, table, *boot);
    // A terminal preference that cannot be honoured (a gate or guard
    // refuses the resume it chose) leaves the plan as it would have been:
    // the preference never turns a plan into a refusal.
    if request.prefs.prefer_terminal && matches!(decision.plan, Plan::Unavailable(_)) {
        let mut plain = *request;
        plain.prefs.prefer_terminal = false;
        return guarded(snap, &plain, table, *boot);
    }
    decision
}

/// The provider's decision, selected, version-gated and launch-guarded.
fn guarded(
    snap: &Snapshot,
    request: &OpenRequest,
    table: &[ProcessInfo],
    boot: BootId,
) -> Decision {
    let policy = providers::policy_for(snap.session.provider);
    let cx = Context {
        snap,
        request,
        table,
        boot,
    };
    let decision = policy.decide(&cx);
    let decision = select(decision, request);
    let decision = gate::apply(decision, snap, policy.verified_versions());
    guard_start(decision, snap, request)
}

/// Honour a location picked from an earlier choice, by full identity. A pick
/// that no longer exists is `SelectionGone`, never a different action (in
/// particular never a launch).
fn select(decision: Decision, request: &OpenRequest) -> Decision {
    let Some(target) = request.selected else {
        return decision;
    };
    match decision.plan {
        Plan::Choose(actions) => match actions.into_iter().find(|a| a.target == Some(target)) {
            Some(action) => Decision::new(SessionState::LiveLocatable, Plan::Act(action)),
            None => Decision::unavailable(decision.state, Reason::SelectionGone),
        },
        Plan::Act(action) if action.target != Some(target) => {
            Decision::unavailable(decision.state, Reason::SelectionGone)
        }
        Plan::Confirm(_) => Decision::unavailable(decision.state, Reason::SelectionGone),
        plan => Decision::new(decision.state, plan),
    }
}

fn actions(decision: &Decision) -> Vec<&Action> {
    match &decision.plan {
        Plan::Act(action) => vec![action],
        Plan::Choose(actions) => actions.iter().collect(),
        Plan::Confirm(c) => vec![&c.then],
        Plan::Unavailable(_) => Vec::new(),
    }
}

/// The window a reservation's launch opened is gone: a process was seen on
/// its tty since the launch, and none is there now. An observation between
/// the terminal reporting the tty and the first process appearing on it
/// never reads the window as gone.
pub fn reservation_window_gone(reservation: &Reservation, snap: &Snapshot) -> bool {
    let Some(tty) = &reservation.tty else {
        return false;
    };
    reservation.seen
        && snap
            .processes
            .known()
            .is_some_and(|table| !table.iter().any(|p| p.tty.as_ref() == Some(tty)))
}

/// The processes the decision's focus steps return to.
fn focus_targets(decision: &Decision) -> Vec<&crate::model::ProcessIdentity> {
    actions(decision)
        .into_iter()
        .flat_map(|a| a.steps.iter())
        .filter_map(|s| match s {
            Step::FocusPane { process, .. } => Some(process),
            _ => None,
        })
        .collect()
}

/// An earlier launch is resolved when the window it opened is gone (nothing
/// can start there any more) or when the plan returns to an owner in that very
/// window. Ownership seen anywhere else does not resolve it: the launch may
/// still start. Without a tty (Windows), an owner the plan returns to that
/// started after the launch is the launch.
fn resolved(reservation: &Reservation, snap: &Snapshot, decision: &Decision) -> bool {
    let Some(table) = snap.processes.known() else {
        return false;
    };
    let on =
        |identity: &crate::model::ProcessIdentity| table.iter().find(|p| p.identity == *identity);
    match &reservation.tty {
        Some(tty) => {
            let returns_there = focus_targets(decision)
                .into_iter()
                .any(|id| on(id).is_some_and(|p| p.tty.as_ref() == Some(tty)));
            reservation_window_gone(reservation, snap) || returns_there
        }
        None if snap.platform == Platform::Windows => focus_targets(decision)
            .into_iter()
            .any(|id| id.start >= reservation.made_at && on(id).is_some()),
        None => false,
    }
}

/// A link reservation resolves once the desktop record is non-archived and
/// openable: the record is the session, and the app keeps at most one process
/// per record, so a `continue` link cannot duplicate it (Claude facts B, C).
fn resolved_in_desktop(reservation: &Reservation, decision: &Decision) -> bool {
    reservation.via == Pending::Link
        && decision
            .action()
            .is_some_and(|a| a.kind == ActionKind::OpenExisting)
}

/// Nothing dispatched in an earlier boot can still start.
fn from_earlier_boot(reservation: &Reservation, snap: &Snapshot) -> bool {
    match (reservation.boot, &snap.boot) {
        (Some(made), Fact::Known(now)) => made != *now,
        _ => false,
    }
}

/// A process-starting plan needs a resolved reservation: an earlier launch
/// that has not been observed must not be launched again.
fn guard_start(mut decision: Decision, snap: &Snapshot, request: &OpenRequest) -> Decision {
    let pending = match &snap.reservation {
        Fact::Known(Some(r)) => {
            let done = from_earlier_boot(r, snap)
                || resolved(r, snap, &decision)
                || resolved_in_desktop(r, &decision)
                || request.confirm_launch_gone;
            decision.release_reservation = done;
            !done
        }
        Fact::Known(None) => false,
        Fact::Unknown => true,
    };
    let starts = actions(&decision)
        .iter()
        .any(|a| a.may_start_writer() && !matches!(decision.plan, Plan::Choose(_)));
    if !starts || !pending {
        return decision;
    }
    match &snap.reservation {
        Fact::Unknown => Decision::unavailable(SessionState::Unknown, Reason::EvidenceIncomplete),
        _ => Decision::unavailable(SessionState::LaunchPending, Reason::LaunchPending),
    }
}
