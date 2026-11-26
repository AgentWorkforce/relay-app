//! A metamorphic check of the terminal preference
//! ([`crate::plan::Preferences::prefer_terminal`]): the same snapshot is
//! decided again without it, and whatever the preference changed must be
//! what it is allowed to change. Unlike the [`super::oracle`], this calls
//! `decide` (twice on one snapshot, comparing the answers); the truth it
//! judges against (whether the session was running) comes from the world.
use super::oracle::{fail, Invariant, Violation};
use crate::decide::decide;
use crate::engine::OpenReport;
use crate::model::{App, Host, TerminalKind};
use crate::plan::{Action, ActionKind, OpenRequest, Plan, SessionState, Step};

/// The action a plan would run, a confirmation's included.
fn acting(plan: &Plan) -> Option<&Action> {
    match plan {
        Plan::Act(action) => Some(action),
        Plan::Confirm(confirmation) => Some(&confirmation.then),
        Plan::Choose(_) | Plan::Unavailable(_) => None,
    }
}

/// Whether `action` ends in a provider's desktop app.
fn in_desktop_app(action: &Action) -> bool {
    action.steps.iter().any(|step| match step {
        Step::OpenUrl { bundle, .. } => App::from_bundle_id(bundle).is_some_and(|a| {
            matches!(
                a,
                App::ClaudeDesktop | App::CodexDesktop | App::OpenCodeDesktop
            )
        }),
        Step::ActivateApp { app } => app.terminal().is_none(),
        _ => false,
    })
}

fn launched(action: &Action) -> Option<TerminalKind> {
    match action.steps.as_slice() {
        [Step::Launch { terminal, .. }] => Some(*terminal),
        _ => None,
    }
}

fn host_terminal(host: Option<&Host>) -> Option<TerminalKind> {
    match host {
        Some(Host::Terminal(t) | Host::Tmux(Some(t))) => Some(*t),
        _ => None,
    }
}

/// `report` was decided with `request`; `was_live` is the world's truth
/// before the open (a writer of the session existed or was starting).
pub fn check(request: &OpenRequest, report: &OpenReport, was_live: bool) -> Result<(), Violation> {
    if !request.prefs.prefer_terminal {
        return Ok(());
    }
    let provider = Some(report.session.provider);
    let mut without = *request;
    without.prefs.prefer_terminal = false;
    let base = decide(&report.before, &without);
    if base == report.decision {
        return Ok(());
    }
    let why = |cause: &str| {
        fail(
            Invariant::PreferenceScoped,
            cause,
            provider,
            format!("without {:?}\nwith {:?}", base, report.decision),
        )
    };
    if was_live || base.state.is_live() {
        return why("changed a live session's plan");
    }
    if !matches!(base.state, SessionState::Stopped | SessionState::Archived) {
        return why("changed a plan that was not for a stopped session");
    }
    // What it replaces is a desktop plan, or the refusal of one (a link
    // whose handler or account is wrong): never a choice, never a plan
    // that already used a terminal.
    let Some(to) = acting(&report.decision.plan) else {
        return why("turned a plan into a refusal or a choice");
    };
    match (&base.plan, acting(&base.plan)) {
        (Plan::Unavailable(_), _) => {}
        (_, Some(from)) if in_desktop_app(from) => {}
        _ => return why("replaced a plan that was not for a desktop app"),
    }
    if to.kind != ActionKind::Resume {
        return why("replaced a desktop plan with something other than a resume");
    }
    let Some(terminal) = launched(to) else {
        return why("a preferred resume that is not one launch");
    };
    let last = host_terminal(report.before.last_seen_host.known());
    if last.is_some_and(|known| known != terminal) {
        return why("a preferred resume overrode the last-seen terminal");
    }
    Ok(())
}
