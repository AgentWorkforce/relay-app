//! End-to-end through the engine for each of the six hosts: start →
//! close/archive/quit → open → verify, including each documented hazard. The
//! oracle checks every open.
use std::collections::BTreeSet;

use crate::common::*;
use session_navigation::engine::{self, verify, OpenReport};
use session_navigation::model::{App, Precision, Reason, TerminalKind};
use session_navigation::plan::{
    ActionKind, Command, Dispatch, OpenRequest, Outcome, PostCondition, Preferences, Step,
};
use session_navigation::ports::Execute;
use session_navigation::sim::agents::StartHost;
use session_navigation::sim::oracle;
use session_navigation::sim::world::{LogEntry, Role};
use session_navigation::sim::{Event, WorldVars};

/// Open through the engine with attribution, then run the oracle.
fn open_checked(t: &mut T, request: &OpenRequest) -> OpenReport {
    let id = t.w.log.len() as u64 + 1000;
    let from = t.w.log.len();
    t.w.engine_open = Some(id);
    let report = engine::open(&mut t.w, &t.s, request);
    t.w.engine_open = None;
    let imports: BTreeSet<u64> = if report.decision.action().is_some_and(|a| a.import) {
        [id].into()
    } else {
        BTreeSet::new()
    };
    oracle::check_open(&t.w, id, request, &report).expect("oracle");
    oracle::check_log(&t.w, from, &imports).expect("oracle log");
    report
}

fn next_kind(t: &mut T) -> Option<ActionKind> {
    t.plan().action().map(|a| a.kind)
}

fn launches(t: &T) -> usize {
    t.w.log
        .iter()
        .filter(|e| matches!(e, LogEntry::LaunchDispatched { .. }))
        .count()
}

#[test]
fn claude_desktop_quit_then_open_restarts_in_app() {
    let mut t = start(StartHost::ClaudeDesktop);
    t.apply(Event::QuitApp {
        app: App::ClaudeDesktop,
    });
    let report = open_checked(&mut t, &request());
    assert_eq!(report.outcome, Outcome::Verified);
    assert_eq!(t.w.writers(&t.s), 1);
    assert_eq!(next_kind(&mut t), Some(ActionKind::OpenExisting));
}

#[test]
fn claude_desktop_archived_reopens_and_converges() {
    let mut t = start(StartHost::ClaudeDesktop);
    let pick = t.pick();
    t.apply(Event::Archive { pick });
    let report = open_checked(&mut t, &request());
    assert_eq!(report.outcome, Outcome::Verified);
    assert!(
        t.w.reservations.is_empty(),
        "verified reopen releases its reservation"
    );
    assert_eq!(next_kind(&mut t), Some(ActionKind::OpenExisting));
}

/// The resume link reviving an archived record is VERIFIED for 5.1231.2
/// (native N5); the old "lands on Code home" answer survives only for an
/// unverified app version, where the planner degrades the link and a link
/// dispatched anyway is caught by the post-condition.
#[test]
fn claude_resume_link_that_does_not_unarchive_fails_visibly() {
    let vars = WorldVars {
        claude_resume_link_unarchives: false,
        ..vars()
    };
    let mut t = start_with(vars, StartHost::ClaudeDesktop);
    let pick = t.pick();
    t.apply(Event::Archive { pick });
    if let Some(app) = t.w.apps.get_mut(&App::ClaudeDesktop) {
        app.version = "99.0.0-unverified".to_owned();
    }
    assert_eq!(
        t.plan().plan,
        session_navigation::plan::Plan::Unavailable(Reason::UnverifiedVersion)
    );
    let step = Step::OpenUrl {
        url: format!("claude://resume?session={}", t.s.id),
        bundle: App::ClaudeDesktop.bundle_id().to_owned(),
    };
    let before = engine::plan(&mut t.w, &t.s.clone(), &request()).0;
    assert_eq!(t.w.execute(&step), Dispatch::Dispatched);
    let after = engine::plan(&mut t.w, &t.s.clone(), &request()).0;
    let post = PostCondition::ConversationShown {
        app: App::ClaudeDesktop,
    };
    assert_eq!(
        verify::check_against(&post, &before, &after, &request()),
        Outcome::Failed(Reason::WrongDestination)
    );
    assert_eq!(t.w.writers(&t.s), 0);
}

#[test]
fn hazard_continue_link_on_archived_lands_on_home_and_is_caught() {
    let mut t = start(StartHost::ClaudeDesktop);
    let local = t.local_id();
    let pick = t.pick();
    t.apply(Event::Archive { pick });
    let url = format!("claude://code/continue?session={local}");
    let step = Step::OpenUrl {
        url,
        bundle: App::ClaudeDesktop.bundle_id().to_owned(),
    };
    assert_eq!(t.w.execute(&step), Dispatch::Dispatched);
    let after = engine::plan(&mut t.w, &t.s.clone(), &request()).0;
    let outcome = verify::check(
        &PostCondition::ConversationShown {
            app: App::ClaudeDesktop,
        },
        &after,
        &request(),
    );
    assert_eq!(outcome, Outcome::Failed(Reason::WrongDestination));
}

/// Claude's view is never readable, but the focused record's
/// `lastFocusedAt` is (native N1): a `continue` link that starts no process
/// is still verified.
#[test]
fn claude_desktop_link_is_verified_by_last_focused_without_a_process() {
    let vars = WorldVars {
        desktop_view_readable: false,
        claude_continue_starts_process: false,
        ..vars()
    };
    let mut t = start_with(vars, StartHost::ClaudeDesktop);
    t.apply(Event::QuitApp {
        app: App::ClaudeDesktop,
    });
    assert_eq!(open_checked(&mut t, &request()).outcome, Outcome::Verified);
    assert_eq!(t.w.writers(&t.s), 0, "the process starts lazily");
}

#[test]
fn claude_prior_only_id_needs_an_explicit_import() {
    let vars = WorldVars {
        claude_resume_prior_imports_duplicate: true,
        ..vars()
    };
    let mut t = start_with(vars, StartHost::ClaudeDesktop);
    t.apply(Event::Clear { pick: 0 });
    t.apply(Event::QuitApp {
        app: App::ClaudeDesktop,
    });
    expect_unavailable(&t.plan(), Reason::PriorIdentityOnly);
    let allow = OpenRequest {
        prefs: Preferences {
            allow_import: true,
            ..request().prefs
        },
        ..request()
    };
    let report = open_checked(&mut t, &allow);
    let action = report.decision.action().unwrap();
    assert!(action.import && action.kind == ActionKind::Reopen);
    let created =
        t.w.log
            .iter()
            .filter(|e| matches!(e, LogEntry::Created { .. }))
            .count();
    assert_eq!(
        created, 1,
        "the labelled import created exactly one desktop session"
    );
}

#[test]
fn spare_registry_naming_a_session_is_never_an_owner() {
    let vars = WorldVars {
        claude_spare_registry_before_claim: true,
        ..vars()
    };
    let mut t = start_with(vars, StartHost::ClaudeDesktop);
    t.apply(Event::QuitApp {
        app: App::ClaudeDesktop,
    });
    // Relaunching the app pre-warms a spare whose registry names this session.
    t.w.ensure_app(App::ClaudeDesktop);
    let spare =
        t.w.procs
            .values()
            .find(|p| p.role == Role::Spare)
            .unwrap()
            .pid;
    let named = String::from_utf8_lossy(&t.w.claude.registry[&spare].bytes).contains(&t.s.id);
    assert!(named, "the spare's registry names the session");
    let url = format!("claude://code/continue?session={}", t.local_id());
    expect_url(
        &t.plan(),
        ActionKind::OpenExisting,
        &url,
        App::ClaudeDesktop,
    );
}

#[test]
fn claude_ghostty_close_then_resume_converges_and_double_open_launches_once() {
    let mut t = start(StartHost::ClaudeGhostty);
    t.apply(Event::ClosePane { pick: 0 });
    let first = open_checked(&mut t, &request());
    assert_eq!(first.outcome, Outcome::Verified);
    let second = open_checked(&mut t, &request());
    assert_eq!(
        second.decision.action().unwrap().kind,
        ActionKind::FocusPane
    );
    assert_eq!(launches(&t), 1);
    assert_eq!(t.w.writers(&t.s), 1);
}

#[test]
fn delayed_launch_blocks_second_open_until_observed() {
    let mut t = start(StartHost::ClaudeGhostty);
    t.w.launch_delay = 2;
    t.apply(Event::ClosePane { pick: 0 });
    assert_eq!(
        open_checked(&mut t, &request()).outcome,
        Outcome::DispatchedUnverified
    );
    let again = open_checked(&mut t, &request());
    assert_eq!(again.outcome, Outcome::Blocked(Reason::LaunchPending));
    t.apply(Event::Tick { seconds: 2 });
    let later = open_checked(&mut t, &request());
    assert_eq!(later.decision.action().unwrap().kind, ActionKind::FocusPane);
    assert!(
        t.w.reservations.is_empty(),
        "observed ownership in the launched window resolves it"
    );
    assert_eq!(launches(&t), 1);
}

#[test]
fn launch_window_closed_before_start_resolves_the_reservation() {
    let mut t = start(StartHost::ClaudeGhostty);
    t.w.launch_delay = 3;
    t.apply(Event::ClosePane { pick: 0 });
    open_checked(&mut t, &request());
    t.apply(Event::ClosePane { pick: 0 });
    let report = open_checked(&mut t, &request());
    assert_eq!(report.decision.action().unwrap().kind, ActionKind::Resume);
}

#[test]
fn permission_denied_never_becomes_a_launch() {
    let mut t = start(StartHost::ClaudeGhostty);
    t.apply(Event::Permission {
        terminal: TerminalKind::Ghostty,
        granted: false,
    });
    let report = open_checked(&mut t, &request());
    assert_eq!(report.outcome, Outcome::Blocked(Reason::PermissionDenied));
    assert_eq!(launches(&t), 0);
}

#[test]
fn crash_leaves_stale_registry_that_is_not_an_owner() {
    let mut t = start(StartHost::ClaudeGhostty);
    let (owner, _) = t.owner();
    t.w.kill(owner.pid, session_navigation::sim::truth::Exit::Crash);
    assert!(t.w.claude.registry.contains_key(&owner.pid));
    // Only the agent died (no exit handler ran); its record is stale and the
    // pane's shell is not a Claude process, so a resume is safe.
    let d = t.plan();
    assert_eq!(d.action().unwrap().kind, ActionKind::Resume);
}

#[test]
fn pid_reuse_after_reboot_is_not_mistaken_for_the_owner() {
    let mut t = start(StartHost::ClaudeGhostty);
    let (owner, _) = t.owner();
    t.apply(Event::Reboot).apply(Event::PidReuse);
    let alien = t.w.procs.get(&owner.pid).expect("pid reused");
    assert_ne!(alien.hosts.as_ref(), Some(&t.s));
    let d = t.plan();
    let focuses_alien = d.action().is_some_and(|a| {
        a.steps
            .iter()
            .any(|s| matches!(s, Step::FocusPane { process, .. } if process.pid == owner.pid))
    });
    assert!(!focuses_alien, "{d:?}");
}

#[test]
fn codex_desktop_quit_then_link_loads_thread() {
    let mut t = start(StartHost::CodexDesktop);
    t.apply(Event::QuitApp {
        app: App::CodexDesktop,
    });
    assert_eq!(open_checked(&mut t, &request()).outcome, Outcome::Verified);
    assert!(t.w.codex.app_loaded.contains(&t.s.id));
}

#[test]
fn codex_ghostty_close_then_resume() {
    let mut t = start(StartHost::CodexGhostty);
    t.apply(Event::ClosePane { pick: 0 });
    assert_eq!(open_checked(&mut t, &request()).outcome, Outcome::Verified);
    assert_eq!(next_kind(&mut t), Some(ActionKind::FocusPane));
}

#[test]
fn codex_live_tui_is_focused_not_duplicated_by_the_desktop() {
    let mut t = start(StartHost::CodexGhostty);
    let pick = t.pick();
    t.apply(Event::ClosePane { pick: 0 })
        .apply(Event::PersonResume { pick });
    t.w.ensure_app(App::CodexDesktop);
    let report = open_checked(&mut t, &request());
    assert_eq!(
        report.decision.action().unwrap().kind,
        ActionKind::FocusPane
    );
    assert!(!t.w.codex.app_loaded.contains(&t.s.id));
}

#[test]
fn opencode_desktop_quit_then_bring_forward() {
    let mut t = start(StartHost::OpenCodeDesktop);
    t.apply(Event::QuitApp {
        app: App::OpenCodeDesktop,
    });
    let report = open_checked(&mut t, &request());
    assert_eq!(report.outcome, Outcome::Verified);
    assert_eq!(
        report.decision.action().unwrap().precision,
        Precision::AppOnly
    );
}

#[test]
fn opencode_tui_close_then_resume() {
    let mut t = start(StartHost::OpenCodeTerminal { with_id: true });
    t.apply(Event::ClosePane { pick: 0 });
    assert_eq!(open_checked(&mut t, &request()).outcome, Outcome::Verified);
    assert_eq!(next_kind(&mut t), Some(ActionKind::FocusPane));
}

#[test]
fn hazard_opencode_missing_id_is_created_by_the_tui_and_never_planned() {
    let mut t = start(StartHost::OpenCodeTerminal { with_id: true });
    let pick = t.pick();
    t.apply(Event::ClosePane { pick: 0 })
        .apply(Event::Delete { pick });
    let before = t.w.opencode.sessions.len();
    open_checked(&mut t, &request());
    assert_eq!(t.w.opencode.sessions.len(), before, "planner never creates");
    // The modelled hazard itself: running the command creates the session.
    let command = Command {
        program: "opencode".into(),
        args: vec!["-s".into(), t.s.id.clone()],
        cwd: t.cwd(),
        env: Default::default(),
    };
    t.w.person_runs(TerminalKind::AppleTerminal, &command);
    assert!(t.w.opencode.sessions.contains_key(&t.s.id));
}

#[test]
fn take_over_requires_request_and_verified_stop() {
    let headless = StartHost::Headless {
        provider: session_navigation::model::Provider::Claude,
        supervised: false,
    };
    let mut t = start(headless);
    expect_unavailable(&t.plan(), Reason::RunningHeadless);
    let take = OpenRequest {
        take_over: true,
        ..request()
    };
    let report = open_checked(&mut t, &take);
    assert_eq!(report.decision.action().unwrap().kind, ActionKind::TakeOver);
    assert_eq!(report.outcome, Outcome::Verified);
}

#[test]
fn take_over_races_a_supervisor_and_does_not_launch() {
    let headless = StartHost::Headless {
        provider: session_navigation::model::Provider::Codex,
        supervised: true,
    };
    let mut t = start(headless);
    let take = OpenRequest {
        take_over: true,
        ..request()
    };
    let report = open_checked(&mut t, &take);
    assert_eq!(report.outcome, Outcome::Blocked(Reason::TakeOverRaced));
    assert_eq!(launches(&t), 0);
}

#[test]
fn scheme_handled_by_another_bundle_is_refused() {
    let mut t = start(StartHost::ClaudeDesktop);
    t.apply(Event::QuitApp {
        app: App::ClaudeDesktop,
    })
    .apply(Event::HijackScheme { codex: false });
    expect_unavailable(&t.plan(), Reason::SchemeHandlerMismatch);
}

#[test]
fn unreadable_reservation_store_blocks_launch() {
    let mut t = start(StartHost::ClaudeGhostty);
    t.apply(Event::ClosePane { pick: 0 }).apply(Event::Degrade {
        channel: session_navigation::sim::Channel::Reservations,
        ok: false,
    });
    expect_unavailable(&t.plan(), Reason::EvidenceIncomplete);
}
