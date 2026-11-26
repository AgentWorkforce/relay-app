//! The invariant oracle. It judges World truth and the engine's report; it
//! never calls `decide` or reuses its helpers.
use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::truth::command_target;
use super::world::{LogEntry, World};
use crate::engine::OpenReport;
use crate::evidence::{Fact, ProviderEvidence, Snapshot};
use crate::model::{App, Precision, Provider, SessionRef};
use crate::plan::{Action, ActionKind, Dispatch, OpenRequest, Outcome, Plan, PostCondition, Step};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Invariant {
    NoDuplicates,
    NoSilentCreation,
    NoStaleHandles,
    UncertaintyPreserved,
    HonestPrecision,
    Idempotent,
    Converges,
    Total,
    NoInputSideEffects,
    /// A launch runs in the session's own directory.
    RightDirectory,
    /// The engine never replaces the shared OpenCode server.
    SharedServerIntact,
    /// A launch runs in the session's own runtime (WSL or native).
    SameRuntime,
    /// A launch opens the default terminal or one the session was seen in;
    /// "no default terminal" only when none exists; a known last-seen
    /// terminal is kept unless the default was asked for.
    RightTerminal,
    /// The terminal preference changes only a stopped session's desktop
    /// plan, into a resume in the terminal any resume would use
    /// ([`super::preference`]).
    PreferenceScoped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Violation {
    pub invariant: Invariant,
    /// A stable description of what broke (check, provider), free of ids and
    /// counters: shrinking keeps a candidate only if its cause is the same.
    pub cause: String,
    pub detail: String,
}

pub(super) fn fail(
    invariant: Invariant,
    cause: &str,
    provider: Option<Provider>,
    detail: impl Into<String>,
) -> Result<(), Violation> {
    Err(Violation {
        invariant,
        cause: format!("{invariant:?}/{cause}/{provider:?}"),
        detail: detail.into(),
    })
}

/// Effects logged since `from`: engine-caused writers next to existing ones,
/// and engine-caused creations not covered by an allowed import.
/// Effects logged since `from`: engine-caused writers next to existing ones
/// (immediate or after a launch delay), engine-caused creations not covered
/// by an allowed import, and engine-caused server replacement.
pub fn check_log(world: &World, from: usize, imports: &BTreeSet<u64>) -> Result<(), Violation> {
    for entry in world.log.iter().skip(from) {
        match entry {
            LogEntry::LaunchDispatched {
                open: Some(o),
                session,
                located,
                ..
            } if *located > 0 => {
                return fail(
                    Invariant::NoDuplicates,
                    "needless second client",
                    session.as_ref().map(|s| s.provider),
                    format!("open {o} launched {session:?} next to {located} locatable window(s)"),
                );
            }
            LogEntry::LaunchDispatched {
                open: Some(o),
                session,
                others,
                ..
            } if *others > 0 => {
                return fail(
                    Invariant::NoDuplicates,
                    "launch next to owner",
                    session.as_ref().map(|s| s.provider),
                    format!("open {o} launched {session:?} next to {others} live owner(s)"),
                );
            }
            LogEntry::WriterStarted {
                open: Some(o),
                session,
                others,
                immediate,
            } if *others > 0 => {
                return fail(
                    Invariant::NoDuplicates,
                    if *immediate {
                        "writer next to owner"
                    } else {
                        "delayed writer next to owner"
                    },
                    Some(session.provider),
                    format!("open {o} started a writer for {session:?} next to {others}"),
                );
            }
            LogEntry::Created {
                open: Some(o),
                session,
            } if !imports.contains(o) => {
                return fail(
                    Invariant::NoSilentCreation,
                    "created",
                    Some(session.provider),
                    format!("open {o} created {session:?}"),
                );
            }
            LogEntry::ServerReplaced { open: Some(o) } => {
                return fail(
                    Invariant::SharedServerIntact,
                    "server replaced",
                    Some(Provider::OpenCode),
                    format!("open {o} replaced the shared OpenCode server"),
                );
            }
            _ => {}
        }
    }
    Ok(())
}

/// The oracle's own notion of evidence complete enough to start a process.
pub fn complete_for_start(snapshot: &Snapshot) -> bool {
    let common = snapshot.boot.is_known()
        && snapshot.processes.is_known()
        && snapshot.reservation.is_known();
    let provider = match &snapshot.provider {
        ProviderEvidence::Claude(ev) => {
            ev.registry.is_known() && ev.desktop.is_known() && ev.transcript.is_known()
        }
        ProviderEvidence::Codex(ev) => ev.thread.is_known() && ev.lock_holders.is_known(),
        ProviderEvidence::OpenCode(ev) => ev.session.is_known() && ev.server.is_known(),
        ProviderEvidence::Grok(ev) => ev.active.is_known() && ev.session.is_known(),
    };
    common && provider
}

fn executed(report: &OpenReport) -> impl Iterator<Item = &Step> {
    report.executed.iter().map(|(step, _)| step)
}

fn check_handles(world: &World, open: u64, report: &OpenReport) -> Result<(), Violation> {
    for entry in &world.log {
        let LogEntry::Exec {
            open: Some(o),
            step,
            target_hosted,
            acted: true,
        } = entry
        else {
            continue;
        };
        if *o != open {
            continue;
        }
        let (identity, tty) = match step {
            Step::FocusPane { process, pane, .. } => (
                process,
                match pane {
                    crate::hosts::PaneRef::Tty { tty } => Some(tty),
                    _ => None,
                },
            ),
            Step::Stop { process } => (process, None),
            _ => continue,
        };
        let in_snapshot = report.before.processes.known().is_some_and(|table| {
            table
                .iter()
                .any(|p| p.identity == *identity && (tty.is_none() || p.tty.as_ref() == tty))
        });
        let provider = Some(report.session.provider);
        if !in_snapshot {
            return fail(
                Invariant::NoStaleHandles,
                "absent from snapshot",
                provider,
                format!("{step:?} targets a process absent from the decision snapshot"),
            );
        }
        if !target_hosted.contains(&report.session) {
            return fail(
                Invariant::NoStaleHandles,
                "never hosted",
                provider,
                format!(
                    "{step:?} targets a process that never hosted {:?} (hosted {target_hosted:?})",
                    report.session
                ),
            );
        }
    }
    Ok(())
}

/// A step that may start an agent: a launch of a resume command (a tmux
/// attach starts none), or a reopening link.
fn starts(action: &Action, step: &Step) -> bool {
    matches!(step, Step::Launch { command, .. } if command_target(command).is_some())
        || (matches!(step, Step::OpenUrl { .. }) && action.kind == ActionKind::Reopen)
}

fn check_uncertainty(report: &OpenReport, action: &Action) -> Result<(), Violation> {
    let provider = Some(report.session.provider);
    for step in executed(report) {
        if !starts(action, step) {
            continue;
        }
        if !complete_for_start(&report.before) {
            return fail(
                Invariant::UncertaintyPreserved,
                "incomplete evidence",
                provider,
                format!("{step:?} dispatched on incomplete evidence"),
            );
        }
        if matches!(step, Step::Launch { .. }) && report.before.cwd_exists != Fact::Known(true) {
            return fail(
                Invariant::UncertaintyPreserved,
                "unconfirmed cwd",
                provider,
                "launch without a confirmed working directory",
            );
        }
        if matches!(step, Step::Launch { .. }) && !report.before.environment.is_known() {
            return fail(
                Invariant::UncertaintyPreserved,
                "unknown environment",
                provider,
                "launch without the session's provider environment",
            );
        }
    }
    Ok(())
}

/// The directory the session truly lives in.
fn true_dir(world: &World, session: &SessionRef) -> Option<String> {
    match session.provider {
        Provider::Claude => world
            .claude
            .transcripts
            .get(&session.id)
            .map(|t| t.cwd.clone()),
        Provider::Codex => world.codex.threads.get(&session.id).map(|t| t.cwd.clone()),
        Provider::OpenCode => world
            .opencode
            .sessions
            .get(&session.id)
            .map(|s| s.dir.clone()),
        Provider::Grok => world.grok.sessions.get(&session.id).map(|s| s.cwd.clone()),
    }
}

/// A resume launched outside the session's directory acts on the wrong tree
/// (and Claude cannot even find the transcript there, Claude facts F).
fn check_directory(world: &World, report: &OpenReport) -> Result<(), Violation> {
    for step in executed(report) {
        let Step::Launch { command, .. } = step else {
            continue;
        };
        let target = command_target(command);
        let truth = target.as_ref().and_then(|s| true_dir(world, s));
        if truth.as_ref().is_some_and(|dir| *dir != command.cwd) {
            return fail(
                Invariant::RightDirectory,
                "wrong cwd",
                target.map(|s| s.provider),
                format!("{command:?} runs outside the session's directory {truth:?}"),
            );
        }
    }
    Ok(())
}

/// Whether the front pane's agent hosts `session` now, or (when `past` is
/// allowed) hosted it before.
fn front_pane_hosts(world: &World, session: &SessionRef, past: bool) -> bool {
    let agent = super::oracle_hosts::front_agent(world);
    let Some(agent) = agent else {
        return false;
    };
    if past {
        world.hosted_or_waiting(agent.pid).contains(session)
    } else {
        world.hosted_by(agent.pid).first() == Some(session)
    }
}

fn desktop_shows(world: &World, app: App, session: &SessionRef) -> bool {
    let Some(Some(view)) = world.views.get(&app) else {
        return false;
    };
    if world.front_app != Some(app) {
        return false;
    }
    match session.provider {
        Provider::Claude => world
            .claude
            .desktop
            .get(view)
            .is_some_and(|d| d.holds_live(&session.id)),
        _ => *view == session.id,
    }
}

fn check_precision(world: &World, report: &OpenReport, action: &Action) -> Result<(), Violation> {
    if report.outcome != Outcome::Verified {
        return Ok(());
    }
    let truthful = match (&action.post, action.precision) {
        (PostCondition::AppFrontmost { app }, _) => world.front_app == Some(*app),
        (_, Precision::AppOnly) => true,
        (PostCondition::ConversationShown { app }, _) => {
            desktop_shows(world, *app, &report.session)
        }
        (
            PostCondition::PaneFocused { .. }
            | PostCondition::RunningIn { .. }
            | PostCondition::TmuxAttached { .. },
            precision,
        ) => front_pane_hosts(world, &report.session, precision == Precision::RightWindow),
    };
    if truthful {
        Ok(())
    } else {
        fail(
            Invariant::HonestPrecision,
            "verified wrong destination",
            Some(report.session.provider),
            format!(
                "verified {:?} at {:?} but truth disagrees",
                action.kind, action.precision
            ),
        )
    }
}

fn check_input(report: &OpenReport, request: &OpenRequest) -> Result<(), Violation> {
    let provider = Some(report.session.provider);
    let mut awaited = BTreeSet::new();
    let mut stopped = BTreeSet::new();
    for step in executed(report) {
        match step {
            Step::OpenUrl { url, .. } => {
                let bad = url.contains("prompt=") || url.contains("q=") || url.contains('#');
                if bad {
                    return fail(
                        Invariant::NoInputSideEffects,
                        "url input",
                        provider,
                        format!("url carries input: {url}"),
                    );
                }
            }
            Step::Launch { command, .. } => {
                let program = crate::model::Tool::from_executable(&command.program)
                    .map(crate::model::Tool::program)
                    .unwrap_or_default();
                let ok = matches!(
                    (program, command.args.as_slice()),
                    ("claude", [flag, id]) | ("codex", [flag, id]) | ("opencode", [flag, id]) | ("grok", [flag, id])
                        if matches!(flag.as_str(), "--resume" | "resume" | "-s") && *id == report.session.id
                ) || (program == "tmux"
                    && super::oracle_hosts::tmux_attach_only(&command.args));
                if !ok {
                    return fail(
                        Invariant::NoInputSideEffects,
                        "argv",
                        provider,
                        format!("unexpected argv {command:?}"),
                    );
                }
                if stopped.iter().any(|p| !awaited.contains(p)) {
                    return fail(
                        Invariant::NoInputSideEffects,
                        "launch before stop",
                        provider,
                        "launch before the stop was verified",
                    );
                }
            }
            Step::Stop { process } => {
                if !request.take_over {
                    return fail(
                        Invariant::NoInputSideEffects,
                        "unrequested stop",
                        provider,
                        "stop without an explicit take-over request",
                    );
                }
                stopped.insert(*process);
            }
            Step::AwaitExit { process } => {
                awaited.insert(*process);
            }
            Step::FocusPane { .. } | Step::ActivateApp { .. } => {}
        }
    }
    Ok(())
}

fn check_shape(report: &OpenReport) -> Result<(), Violation> {
    let ok = match &report.decision.plan {
        Plan::Act(a) => {
            !a.steps.is_empty()
                && (a.kind != ActionKind::BringForward || a.precision == Precision::AppOnly)
        }
        Plan::Choose(list) => list.len() > 1 && list.iter().all(|a| !a.starts_process()),
        Plan::Confirm(c) => !c.then.steps.is_empty() && report.executed.is_empty(),
        Plan::Unavailable(_) => report.executed.is_empty(),
    };
    let dispatch_ok = report
        .executed
        .iter()
        .all(|(step, d)| !matches!(step, Step::AwaitExit { .. }) || *d == Dispatch::Dispatched);
    if ok && dispatch_ok {
        Ok(())
    } else {
        fail(
            Invariant::Total,
            "malformed",
            Some(report.session.provider),
            format!("malformed decision {:?}", report.decision),
        )
    }
}

/// Everything checkable from one open's report and the world right after it.
pub fn check_open(
    world: &World,
    open: u64,
    request: &OpenRequest,
    report: &OpenReport,
) -> Result<(), Violation> {
    check_shape(report)?;
    check_handles(world, open, report)?;
    check_directory(world, report)?;
    super::oracle_hosts::check_runtime(world, report)?;
    check_input(report, request)?;
    if let Plan::Act(action) = &report.decision.plan {
        check_uncertainty(report, action)?;
        check_precision(world, report, action)?;
    }
    super::defaults::check_terminal(world, request, report)
}

/// Engine-caused launches or writer starts for `session` across a set of opens.
pub fn engine_starts(world: &World, opens: &BTreeSet<u64>, session: &SessionRef) -> usize {
    world
        .log
        .iter()
        .filter(|e| match e {
            LogEntry::LaunchDispatched {
                open: Some(o),
                session: Some(s),
                ..
            } => opens.contains(o) && s == session,
            LogEntry::WriterStarted {
                open: Some(o),
                session: s,
                immediate: true,
                ..
            } => opens.contains(o) && s == session,
            _ => false,
        })
        .count()
}
