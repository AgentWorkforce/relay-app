//! Grok: the `grok` CLI in a terminal. No desktop app, URL scheme or deep
//! link is known, so every plan is a terminal focus or a resume
//! (facts-grok 5).
//!
//! A running TUI records itself in `<grok home>/active_sessions.json`
//! (`session_id`, `pid`, `cwd`, `opened_at`) and removes the record when it
//! exits (relay `grok.rs`). A record is live only when its pid runs a Grok
//! executable that started no later than `opened_at` (desktop-core
//! `providers.rs`; a recycled pid starts after it). A process whose argv
//! names the session (`grok --resume <id>`) is an owner too, record or not.
//!
//! UNVERIFIED (Grok was not installed; each has a simulator variable):
//! whether the record follows the TUI to another session, whether `grok
//! --resume <id>` resumes in place, creates a missing id, or needs the
//! session's own directory, and whether a subagent id can be resumed. So
//! focus is `RightWindow`; a resume needs positive existence, runs in the
//! session's own `cwd`, refuses subagents, and asks first when another Grok
//! process in that directory may already host the session. Relay offers no
//! Grok route on Windows (its leader socket is Unix-only), so Grok is
//! unavailable there.
use super::{mismatched, ProviderPolicy};
use crate::decide::common::{self, from_result, live_decision};
use crate::decide::interpret::{self, Place};
use crate::decide::Context;
use crate::evidence::{
    Capability, Exe, Fact, GrokActive, GrokEvidence, GrokSession, ProcessInfo, ProviderEvidence,
};
use crate::model::{Component, Platform, Precision, Provider, Reason, Tool};
use crate::plan::{ActionKind, Confirmation, Decision, Elsewhere, Plan, SessionState};

pub struct GrokPolicy;

/// No Grok version was exercised natively.
const VERIFIED: &[(Component, &str)] = &[];

impl ProviderPolicy for GrokPolicy {
    fn provider(&self) -> Provider {
        Provider::Grok
    }

    fn verified_versions(&self) -> &'static [(Component, &'static str)] {
        VERIFIED
    }

    fn decide(&self, cx: &Context<'_>) -> Decision {
        let ProviderEvidence::Grok(ev) = &cx.snap.provider else {
            return mismatched();
        };
        if cx.snap.platform == Platform::Windows {
            return Decision::unavailable(SessionState::Unknown, Reason::PlatformUnsupported);
        }
        let Fact::Known(active) = &ev.active else {
            return Decision::unavailable(SessionState::Unknown, Reason::EvidenceIncomplete);
        };
        let owners = match owners(cx, active) {
            Ok(owners) => owners,
            Err(reason) => return Decision::unavailable(SessionState::Unknown, reason),
        };
        if !owners.is_empty() {
            return live(cx, ev, active, &owners);
        }
        confirm_if_elsewhere(cx, active, &[], stopped(cx, ev))
    }
}

enum Claim {
    Live,
    Dead,
    Unknown,
}

/// A record is live only for a Grok process of this boot that started no
/// later than the record was written.
fn check(cx: &Context<'_>, record: &GrokActive) -> Claim {
    let Some(process) = cx.table.iter().find(|p| p.identity.pid == record.pid) else {
        return Claim::Dead;
    };
    if process.exe != Exe::GrokCli || process.identity.boot != cx.boot {
        return Claim::Dead;
    }
    match record.opened_at {
        Some(opened) if process.identity.start <= opened => Claim::Live,
        Some(_) => Claim::Dead,
        None => Claim::Unknown,
    }
}

/// `grok --resume <id>`, `grok -r <id>` or `grok resume <id>`.
fn names(process: &ProcessInfo, id: &str) -> bool {
    interpret::args(process)
        .windows(2)
        .any(|w| matches!(w, [flag, value] if matches!(flag.as_str(), "--resume" | "-r" | "resume") && value.eq_ignore_ascii_case(id)))
}

/// `grok agent leader` serves clients and never itself hosts a session.
fn is_leader(process: &ProcessInfo) -> bool {
    let args = interpret::args(process);
    args.iter().any(|a| a == "agent") && args.iter().any(|a| a == "leader")
}

fn owners<'a>(cx: &Context<'a>, active: &[GrokActive]) -> Result<Vec<&'a ProcessInfo>, Reason> {
    let id = cx.snap.session.id.as_str();
    let mut out: Vec<&ProcessInfo> = Vec::new();
    for record in active
        .iter()
        .filter(|r| r.session_id.eq_ignore_ascii_case(id))
    {
        match check(cx, record) {
            Claim::Live => {
                if let Some(p) = cx.table.iter().find(|p| p.identity.pid == record.pid) {
                    out.push(p);
                }
            }
            Claim::Dead => {}
            Claim::Unknown => return Err(Reason::EvidenceIncomplete),
        }
    }
    for process in cx.table.iter().filter(|p| p.exe == Exe::GrokCli) {
        if names(process, id) && !out.iter().any(|o| o.identity == process.identity) {
            out.push(process);
        }
    }
    Ok(out)
}

fn live(
    cx: &Context<'_>,
    ev: &GrokEvidence,
    active: &[GrokActive],
    owners: &[&ProcessInfo],
) -> Decision {
    let mut actions = Vec::new();
    let mut headless = Vec::new();
    let mut blocked = None;
    for process in owners {
        match interpret::place(cx, process) {
            // The TUI may have moved to another session since (UNVERIFIED,
            // `grok_registry_follows_session`).
            Place::Pane(ret) => match common::return_to(cx, ret, process, Precision::RightWindow) {
                Ok(action) => actions.push(action),
                Err(reason) => common::block(&mut blocked, reason),
            },
            Place::Headless => headless.push(*process),
            Place::Unlocatable(reason) => common::block(&mut blocked, reason),
            Place::Desktop(_) => common::block(&mut blocked, Reason::LiveNotLocatable),
        }
    }
    let pids: Vec<u32> = owners.iter().map(|p| p.identity.pid).collect();
    live_decision(cx, actions, &headless, blocked, || {
        let unconfirmed = !cx.request.confirm_not_open_elsewhere;
        if unconfirmed && !possibly_elsewhere(cx, active, &pids).is_empty() {
            return Err(Reason::PossiblyOpenElsewhere);
        }
        stopped(cx, ev)
            .action()
            .filter(|a| a.kind == ActionKind::Resume)
            .cloned()
            .ok_or(Reason::RunningHeadless)
    })
}

/// Whether the record is authoritative: it follows the TUI to another
/// session. Only a version-matched capability says so.
fn registry_authoritative(cx: &Context<'_>) -> bool {
    let cli = cx
        .snap
        .tools
        .iter()
        .find(|t| t.tool == Tool::GrokCli)
        .and_then(|t| t.version.known().cloned().flatten());
    cx.snap.capabilities.iter().any(|c| {
        matches!(c, Capability::GrokRegistryFollowsSession { cli_version }
            if cli.as_deref() == Some(cli_version.as_str()))
    })
}

/// Other Grok processes that may host the session: TUIs that may have
/// moved to it, and `grok agent stdio` clients, which register nowhere.
/// With an authoritative registry, a registered TUI is excluded. Same or
/// unknown cwd counts.
fn possibly_elsewhere(cx: &Context<'_>, active: &[GrokActive], owners: &[u32]) -> Vec<Elsewhere> {
    let cwd = cx.snap.cwd.known();
    let authoritative = registry_authoritative(cx);
    cx.table
        .iter()
        .filter(|p| p.exe == Exe::GrokCli && !owners.contains(&p.identity.pid))
        .filter(|p| !is_leader(p))
        .filter(|p| !(authoritative && active.iter().any(|r| r.pid == p.identity.pid)))
        .filter(|p| interpret::cwd_may_be(p, cwd))
        .map(|p| Elsewhere {
            process: p.identity,
            terminal: match interpret::place(cx, p) {
                Place::Pane(ret) => interpret::return_terminal(&ret),
                _ => None,
            },
            tty: p.tty.clone(),
        })
        .collect()
}

fn confirm_if_elsewhere(
    cx: &Context<'_>,
    active: &[GrokActive],
    owners: &[u32],
    decision: Decision,
) -> Decision {
    if cx.request.confirm_not_open_elsewhere {
        return decision;
    }
    let elsewhere = possibly_elsewhere(cx, active, owners);
    match decision.plan {
        Plan::Act(action) if !elsewhere.is_empty() => Decision::new(
            decision.state,
            Plan::Confirm(Confirmation {
                reason: Reason::PossiblyOpenElsewhere,
                elsewhere,
                then: action,
            }),
        ),
        plan => Decision::new(decision.state, plan),
    }
}

fn stopped(cx: &Context<'_>, ev: &GrokEvidence) -> Decision {
    let session: &GrokSession = match &ev.session {
        Fact::Unknown => {
            return Decision::unavailable(SessionState::Unknown, Reason::EvidenceIncomplete)
        }
        Fact::Known(None) => {
            return Decision::unavailable(SessionState::NotFound, Reason::SessionNotFound)
        }
        Fact::Known(Some(session)) => session,
    };
    if session.subagent {
        return Decision::unavailable(SessionState::Stopped, Reason::ChildSession);
    }
    // Sessions are stored per directory; resume only in the session's own.
    if cx.snap.cwd.known().is_some_and(|c| *c != session.cwd) {
        return Decision::unavailable(SessionState::Stopped, Reason::CwdConflict);
    }
    let result = common::resume(
        cx,
        Tool::GrokCli,
        vec!["--resume".to_owned(), cx.snap.session.id.clone()],
    )
    .map(|action| crate::plan::Action {
        precision: Precision::RightWindow,
        ..action
    });
    from_result(SessionState::Stopped, result)
}
