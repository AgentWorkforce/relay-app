//! Post-condition checks against a fresh snapshot. Anything not positively
//! observed is `DispatchedUnverified`; anything observed wrong is `Failed`.
use super::panes;
use crate::evidence::{
    ClaimKind, ClaudeEvidence, DesktopClaim, Exe, Fact, ProviderEvidence, RegistryClaim, Snapshot,
};
use crate::model::Reason;

/// The Claude registry, when it was readable.
fn claude_registry(snap: &Snapshot) -> Option<&[RegistryClaim]> {
    match &snap.provider {
        ProviderEvidence::Claude(ev) => ev.registry.known().map(Vec::as_slice),
        _ => None,
    }
}
use crate::plan::{OpenRequest, Outcome, PostCondition};

pub(super) fn wrong() -> Outcome {
    Outcome::Failed(Reason::WrongDestination)
}

/// Check `post` against a fresh snapshot alone. Without the snapshot the
/// plan was made from, a Claude desktop link is at best unverified.
pub fn check(post: &PostCondition, after: &Snapshot, request: &OpenRequest) -> Outcome {
    check_inner(post, None, after, request)
}

/// Check `post` against a fresh snapshot, given the one the plan was made
/// from (so a process that appeared since can count as proof).
pub fn check_against(
    post: &PostCondition,
    before: &Snapshot,
    after: &Snapshot,
    request: &OpenRequest,
) -> Outcome {
    check_inner(post, Some(before), after, request)
}

fn check_inner(
    post: &PostCondition,
    before: Option<&Snapshot>,
    after: &Snapshot,
    request: &OpenRequest,
) -> Outcome {
    match post {
        PostCondition::PaneFocused {
            terminal,
            pane,
            process,
        } => panes::pane_focused(after, *terminal, pane, process),
        PostCondition::ConversationShown { app } => {
            match &after.front.app {
                Fact::Unknown => return Outcome::DispatchedUnverified,
                Fact::Known(front) if *front != Some(*app) => return wrong(),
                Fact::Known(_) => {}
            }
            if let ProviderEvidence::Claude(ev) = &after.provider {
                return claude_shown(ev, before, after);
            }
            match &after.front.view {
                Fact::Unknown => Outcome::DispatchedUnverified,
                Fact::Known(None) => wrong(),
                Fact::Known(Some(view)) if *view == after.session.id => Outcome::Verified,
                Fact::Known(Some(_)) => wrong(),
            }
        }
        PostCondition::RunningIn { terminal } => {
            if !codex_presented(before, after) {
                return Outcome::DispatchedUnverified;
            }
            panes::running_in(after, *terminal, request)
        }
        PostCondition::AppFrontmost { app } => {
            if after.front.app.is(&Some(*app)) {
                Outcome::Verified
            } else {
                Outcome::DispatchedUnverified
            }
        }
        PostCondition::TmuxAttached {
            terminal,
            socket,
            server,
            pane,
            process,
        } => panes::tmux_attached(after, *terminal, socket, server, pane, process),
    }
}

/// Which conversation Claude.app shows is not observable, but the record it
/// focuses is (VERIFIED for 5.1231.2, native N1, N3, N5). What can be
/// observed:
/// - every record holding the session as its current transcript is still
///   archived: no link can be showing it (`continue` on an archived record
///   is a silent no-op, N3), so the destination is wrong;
/// - a non-archived record holding the session as its current transcript
///   has a `lastFocusedAt` newer than in the plan's snapshot: the app
///   focused exactly that record (N1 for `continue`, N5 for `resume`);
/// - a registry claim that was not there before now names, for a live
///   process, this session and such a record: the app started exactly this
///   conversation.
///
/// Anything else is unverified, including an unchanged `lastFocusedAt` on a
/// non-archived record: re-focusing the record already on screen and a slow
/// app launch were not observed natively, so a missing signal is not proof
/// of a wrong destination (the engine observes a bounded number of times
/// first). A stem-only or prior claim never proves it.
fn claude_shown(ev: &ClaudeEvidence, before: Option<&Snapshot>, after: &Snapshot) -> Outcome {
    let id = after.session.id.as_str();
    let Fact::Known(claims) = &ev.desktop else {
        return Outcome::DispatchedUnverified;
    };
    let live: Vec<_> = claims
        .iter()
        .filter(|c| c.kind == ClaimKind::Live)
        .collect();
    if !live.is_empty() && live.iter().all(|c| c.archived) {
        return wrong();
    }
    if before.is_some_and(|before| focused_since(&live, before)) {
        return Outcome::Verified;
    }
    let (Some(before), Fact::Known(table), Fact::Known(boot), Fact::Known(registry)) =
        (before, &after.processes, &after.boot, &ev.registry)
    else {
        return Outcome::DispatchedUnverified;
    };
    // A claim that was not there before: a pre-warmed spare claimed for the
    // session keeps its pid, so the process alone is not the signal.
    let existed = |claim: &RegistryClaim| {
        claude_registry(before).is_some_and(|r| {
            r.iter().any(|c| {
                c.pid == claim.pid
                    && c.proc_start == claim.proc_start
                    && c.session_id.eq_ignore_ascii_case(&claim.session_id)
                    && c.host_session == claim.host_session
            })
        })
    };
    let appeared = registry.iter().any(|claim| {
        let alive = table.iter().any(|p| {
            p.identity.pid == claim.pid
                && p.identity.start == claim.proc_start
                && p.identity.boot == *boot
                && p.exe == Exe::ClaudeCli
        });
        claim.session_id.eq_ignore_ascii_case(id)
            && !claim.spare
            && claim.written_this_boot == Fact::Known(true)
            && alive
            && !existed(claim)
            && claim
                .host_session
                .as_deref()
                .is_some_and(|host| live.iter().any(|c| c.local_id == host && !c.archived))
    });
    if appeared {
        Outcome::Verified
    } else {
        Outcome::DispatchedUnverified
    }
}

/// A live, non-archived record whose `lastFocusedAt` moved forward since
/// `before`. A record absent from `before` was never focused then.
fn focused_since(live: &[&DesktopClaim], before: &Snapshot) -> bool {
    let ProviderEvidence::Claude(ev) = &before.provider else {
        return false;
    };
    let Fact::Known(earlier) = &ev.desktop else {
        return false;
    };
    live.iter().filter(|c| !c.archived).any(|claim| {
        let then = earlier
            .iter()
            .find(|c| c.local_id == claim.local_id)
            .map_or(Fact::Known(None), |c| c.last_focused);
        match (then, &claim.last_focused) {
            (Fact::Known(None), Fact::Known(Some(_))) => true,
            (Fact::Known(Some(then)), Fact::Known(Some(now))) => *now > then,
            _ => false,
        }
    })
}

/// A Codex TUI presents its thread only once a writer lock is held for it:
/// by the daemon it is a client of (VERIFIED, 0.167.0, native N8) or by
/// itself. A TUI waiting at the "Update available" menu holds none (N12). A
/// holder that was not one before is the proof; with the same holders as
/// before (another client of the daemon's writer, N10) the new window cannot
/// be told apart from the old one, so the resume stays unverified. Other
/// providers are not affected.
fn codex_presented(before: Option<&Snapshot>, after: &Snapshot) -> bool {
    let holders = |snap: &Snapshot| match &snap.provider {
        ProviderEvidence::Codex(ev) => ev.lock_holders.known().cloned(),
        _ => None,
    };
    if !matches!(after.provider, ProviderEvidence::Codex(_)) {
        return true;
    }
    match (before.and_then(holders), holders(after)) {
        (Some(then), Some(now)) => now.iter().any(|pid| !then.contains(pid)),
        _ => false,
    }
}
