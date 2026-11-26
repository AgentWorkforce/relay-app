//! Claude Code: Claude.app desktop sessions and the `claude` CLI in a terminal.
//!
//! One dedicated process per running session, tied to its CLI id by the
//! per-pid registry (facts C). A desktop session owns a set of CLI ids over
//! time and can be non-archived with no process: not running is not closed.
//!
//! Links, as observed on Claude.app 5.1231.2 (native N1-N6):
//! - `claude://code/continue?session=<local id>` focuses a live record
//!   (`lastFocusedAt` moves) and is a silent no-op on an archived one (N1, N3);
//! - `claude://resume?session=<CLI uuid>` revives an archived record in place,
//!   same record and `cliSessionId`, process started lazily (N5);
//! - `claude://claude.ai/epitaxy/<local id>` shows an archived record without
//!   reviving it (`lastFocusedAt` moves, `isArchived` stays true, N4). No plan
//!   uses it: it returns the person to a read-only view, not the session;
//! - archiving stops the record's process and deletes its registry file (N2);
//!   the app refuses to archive the record on screen (N6).
use super::{mismatched, ProviderPolicy};
use crate::decide::common::{self, from_result, live_decision};
use crate::decide::interpret::{self, check_claim, ClaimStatus, Place};
use crate::decide::Context;
use crate::evidence::{
    Capability, ClaimKind, ClaudeEvidence, DesktopClaim, Entrypoint, Exe, Fact, ProcessInfo,
    ProviderEvidence, RegistryClaim,
};
use crate::model::{App, Component, Precision, Provider, Reason, Tool};
use crate::parse::urls;
use crate::plan::{Action, ActionKind, Confirmation, Decision, Elsewhere, Plan, SessionState};

pub struct ClaudePolicy;

/// Claude.app 5.1231.2 and its bundled CLI 2.1.321 (facts header).
const VERIFIED: &[(Component, &str)] = &[
    (Component::App(App::ClaudeDesktop), "5.1231.2"),
    (Component::Tool(Tool::ClaudeCli), "2.1.321"),
];

impl ProviderPolicy for ClaudePolicy {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    fn verified_versions(&self) -> &'static [(Component, &'static str)] {
        VERIFIED
    }

    fn decide(&self, cx: &Context<'_>) -> Decision {
        let ProviderEvidence::Claude(ev) = &cx.snap.provider else {
            return mismatched();
        };
        let (Fact::Known(registry), Fact::Known(desktop)) = (&ev.registry, &ev.desktop) else {
            return Decision::unavailable(SessionState::Unknown, Reason::EvidenceIncomplete);
        };
        let id = cx.snap.session.id.as_str();
        let mut owners = Vec::new();
        for claim in registry
            .iter()
            .filter(|c| c.session_id.eq_ignore_ascii_case(id) && !c.spare)
        {
            match check_claim(
                cx,
                claim.pid,
                claim.proc_start,
                &claim.written_this_boot,
                Exe::ClaudeCli,
            ) {
                ClaimStatus::Live(process) => owners.push((process, claim)),
                ClaimStatus::Dead => {}
                ClaimStatus::Unknown => {
                    return Decision::unavailable(SessionState::Unknown, Reason::EvidenceIncomplete)
                }
            }
        }
        if !owners.is_empty() {
            return live(cx, ev, &owners, desktop);
        }
        confirm_if_elsewhere(
            cx,
            &[],
            stopped(cx, ev, desktop, cx.request.prefs.prefer_terminal),
        )
    }
}

/// Whether the registry is authoritative: its `sessionId` follows /clear and
/// /resume. Only a natively verified, version-matched capability says so.
pub fn registry_authoritative(cx: &Context<'_>) -> bool {
    let cli = cx
        .snap
        .tools
        .iter()
        .find(|t| t.tool == Tool::ClaudeCli)
        .and_then(|t| t.version.known().cloned().flatten());
    cx.snap.capabilities.iter().any(|c| match c {
        Capability::ClaudeRegistryFollowsSession { cli_version } => {
            cli.as_deref() == Some(cli_version.as_str())
        }
        Capability::GrokRegistryFollowsSession { .. } => false,
    })
}

/// When other Claude CLIs may host the session and that is the only thing
/// standing between the person and `decision`, ask them first (the plan names
/// those processes); with their confirmation, proceed.
fn confirm_if_elsewhere(cx: &Context<'_>, owners: &[u32], decision: Decision) -> Decision {
    if registry_authoritative(cx) || cx.request.confirm_not_open_elsewhere {
        return decision;
    }
    let elsewhere = possibly_hosted_elsewhere(cx, owners);
    if elsewhere.is_empty() {
        return decision;
    }
    match decision.plan {
        Plan::Act(action) => Decision::new(
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

/// The desktop session `host` if it holds this CLI id as its current
/// transcript and is not archived.
fn live_desktop_session<'a>(
    desktop: &'a [DesktopClaim],
    host: Option<&str>,
) -> Option<&'a DesktopClaim> {
    let host = host?;
    desktop
        .iter()
        .find(|c| c.local_id == host && c.kind == ClaimKind::Live && !c.archived)
}

/// Links act on the app's current account only. A record of another
/// account presumably lands on Code home (Claude facts B, UNVERIFIED), so it
/// gets no link.
fn same_account(ev: &ClaudeEvidence, claim: &DesktopClaim) -> Result<(), Reason> {
    match &ev.account {
        Fact::Known(account) if *account == claim.account => Ok(()),
        Fact::Known(_) => Err(Reason::OtherAccount),
        Fact::Unknown => Err(Reason::EvidenceIncomplete),
    }
}

/// A stem-only claim is the right desktop session, but it now holds another
/// conversation (facts A, `localLiveHandles`).
fn claim_precision(action: Action, claim: &DesktopClaim) -> Action {
    if claim.kind == ClaimKind::Live {
        return action;
    }
    Action {
        precision: Precision::RightWindow,
        ..action
    }
}

fn continue_link(
    cx: &Context<'_>,
    ev: &ClaudeEvidence,
    claim: &DesktopClaim,
) -> Result<Action, Reason> {
    same_account(ev, claim)?;
    let action = common::link(
        cx,
        App::ClaudeDesktop,
        urls::claude_continue(&claim.local_id),
        ActionKind::OpenExisting,
        false,
    )?;
    Ok(claim_precision(action, claim))
}

fn live(
    cx: &Context<'_>,
    ev: &ClaudeEvidence,
    owners: &[(&ProcessInfo, &RegistryClaim)],
    desktop: &[DesktopClaim],
) -> Decision {
    let mut actions = Vec::new();
    let mut headless = Vec::new();
    let mut blocked = None;
    for (process, claim) in owners {
        match interpret::place(cx, process) {
            // Whether the registry is rewritten after /clear or /resume is
            // UNVERIFIED (facts C, summary 5): the pane is the one that ran
            // this session, but it may show a later conversation now. (Even
            // with the capability this stays RightWindow: the person can
            // also /resume another conversation in that pane.)
            Place::Pane(ret) => match common::return_to(cx, ret, process, Precision::RightWindow) {
                Ok(action) => actions.push(action),
                Err(reason) => common::block(&mut blocked, reason),
            },
            Place::Desktop(App::ClaudeDesktop) => {
                match live_desktop_session(desktop, claim.host_session.as_deref())
                    .map(|claim| continue_link(cx, ev, claim))
                {
                    Some(Ok(action)) => actions.push(action),
                    _ => common::block(&mut blocked, Reason::LiveNotLocatable),
                }
            }
            Place::Headless => headless.push(*process),
            Place::Unlocatable(reason) => common::block(&mut blocked, reason),
            Place::Desktop(_) => common::block(&mut blocked, Reason::LiveNotLocatable),
        }
    }
    live_decision(cx, actions, &headless, blocked, || {
        let owner_pids: Vec<u32> = owners.iter().map(|(p, _)| p.identity.pid).collect();
        let unconfirmed = !registry_authoritative(cx) && !cx.request.confirm_not_open_elsewhere;
        if unconfirmed && !possibly_hosted_elsewhere(cx, &owner_pids).is_empty() {
            return Err(Reason::PossiblyOpenElsewhere);
        }
        // Preference aside: it never changes what a live session is offered.
        stopped(cx, ev, desktop, false)
            .action()
            .filter(|a| a.kind == ActionKind::Resume)
            .cloned()
            .ok_or(Reason::RunningHeadless)
    })
}

/// Claude CLIs outside the desktop app whose registry does not name this
/// session but which may still be hosting it: whether the registry is
/// rewritten after /clear or /resume is UNVERIFIED (facts C, summary 5).
/// Same or unknown cwd counts.
fn possibly_hosted_elsewhere(cx: &Context<'_>, owners: &[u32]) -> Vec<Elsewhere> {
    let cwd = cx.snap.cwd.known();
    cx.table
        .iter()
        .filter(|p| p.exe == Exe::ClaudeCli && !owners.contains(&p.identity.pid))
        .filter(|p| interpret::cwd_may_be(p, cwd))
        .filter_map(|p| match interpret::place(cx, p) {
            Place::Pane(ret) => Some(Elsewhere {
                process: p.identity,
                terminal: interpret::return_terminal(&ret),
                tty: p.tty.clone(),
            }),
            Place::Headless | Place::Unlocatable(_) => Some(Elsewhere {
                process: p.identity,
                terminal: None,
                tty: p.tty.clone(),
            }),
            Place::Desktop(_) => None,
        })
        .collect()
}

fn reopen(cx: &Context<'_>, import: bool) -> Result<Action, Reason> {
    common::link(
        cx,
        App::ClaudeDesktop,
        urls::claude_resume(&cx.snap.session.id),
        ActionKind::Reopen,
        import,
    )
}

fn import_or(cx: &Context<'_>, state: SessionState, refusal: Reason) -> Decision {
    if cx.request.prefs.allow_import {
        from_result(state, reopen(cx, true))
    } else {
        Decision::unavailable(state, refusal)
    }
}

/// Whether the conversation a desktop record would resume runs in a process
/// outside that record: reviving the record (a link to it, or a `resume` link
/// that may select it) would start a second writer for that conversation.
fn revival_collides(cx: &Context<'_>, ev: &ClaudeEvidence, claim: &DesktopClaim) -> bool {
    let (Some(current), Fact::Known(registry)) = (&claim.current, &ev.registry) else {
        return false;
    };
    registry
        .iter()
        .filter(|c| c.session_id.eq_ignore_ascii_case(current) && !c.spare)
        .filter(|c| c.host_session.as_deref() != Some(claim.local_id.as_str()))
        .any(|c| {
            !matches!(
                check_claim(
                    cx,
                    c.pid,
                    c.proc_start,
                    &c.written_this_boot,
                    Exe::ClaudeCli
                ),
                ClaimStatus::Dead
            )
        })
}

/// The CLI resume a terminal preference asks for: only with the person's
/// preference, a transcript on disk, and every launch precondition met.
fn preferred_terminal(cx: &Context<'_>, ev: &ClaudeEvidence, prefer: bool) -> Option<Action> {
    if !prefer || !matches!(ev.transcript, Fact::Known(Some(_))) {
        return None;
    }
    common::resume(cx, Tool::ClaudeCli, resume_args(cx)).ok()
}

fn resume_args(cx: &Context<'_>) -> Vec<String> {
    vec!["--resume".to_owned(), cx.snap.session.id.clone()]
}

/// `prefer_terminal`: the person's preference, for a session nothing runs.
fn stopped(
    cx: &Context<'_>,
    ev: &ClaudeEvidence,
    desktop: &[DesktopClaim],
    prefer_terminal: bool,
) -> Decision {
    if desktop.iter().any(|c| revival_collides(cx, ev, c)) {
        return Decision::unavailable(SessionState::LiveNotLocatable, Reason::RevivalCollides);
    }
    let strong: Vec<&DesktopClaim> = desktop
        .iter()
        .filter(|c| c.kind != ClaimKind::Prior)
        .collect();
    match strong.as_slice() {
        // An archived id given to `code/continue` silently lands on Code
        // home (VERIFIED, native N3), so archived sessions go through
        // `resume`, which revives the record in place (VERIFIED, N5).
        [one] if one.archived => {
            // An archived record holds nothing the app may revive on its
            // own, so a terminal, when preferred, resumes the conversation
            // with the CLI instead of reviving the record. An unarchived
            // record never is: the app may revive it beside the CLI.
            if let Some(action) = preferred_terminal(cx, ev, prefer_terminal) {
                return Decision::new(SessionState::Archived, Plan::Act(action));
            }
            return from_result(
                SessionState::Archived,
                same_account(ev, one)
                    .and_then(|()| reopen(cx, false))
                    .map(|a| claim_precision(a, one)),
            );
        }
        [one] => return from_result(SessionState::Stopped, continue_link(cx, ev, one)),
        [] => {}
        _ => return Decision::unavailable(SessionState::Stopped, Reason::AmbiguousIdentity),
    }
    if !desktop.is_empty() {
        // Only a prior id matches: `resume` may import a duplicate session
        // (UNVERIFIED, facts B / summary 2).
        return import_or(cx, SessionState::Stopped, Reason::PriorIdentityOnly);
    }
    match &ev.transcript {
        Fact::Unknown => Decision::unavailable(SessionState::Unknown, Reason::EvidenceIncomplete),
        Fact::Known(None) => Decision::unavailable(SessionState::NotFound, Reason::SessionNotFound),
        Fact::Known(Some(t)) if t.entrypoint == Entrypoint::Desktop => {
            import_or(cx, SessionState::Stopped, Reason::DesktopRecordMissing)
        }
        Fact::Known(Some(_)) => from_result(
            SessionState::Stopped,
            common::resume(cx, Tool::ClaudeCli, resume_args(cx)),
        ),
    }
}
