//! The terminal preference: a stopped session the plan would open in its
//! desktop app resumes in a terminal instead, when it can, in the terminal
//! any resume would use. Never for a live session, never over a known
//! last-seen terminal, never for an unarchived Claude desktop record.
use std::collections::BTreeSet;

use session_navigation::decide::decide;
use session_navigation::engine::{self, OpenReport};
use session_navigation::evidence::Fact;
use session_navigation::model::{App, Host, Reason, TerminalKind, Tool};
use session_navigation::plan::{ActionKind, OpenRequest, Outcome, Plan, Step};
use session_navigation::ports::Observe;
use session_navigation::sim::agents::StartHost;
use session_navigation::sim::{oracle, preference, Event};

use crate::common::*;

fn preferring() -> OpenRequest {
    let mut r = request();
    r.prefs.prefer_terminal = true;
    r
}

/// Through the engine, judged by the oracle and the preference check.
fn open_checked(t: &mut T, request: &OpenRequest) -> OpenReport {
    let id = t.w.log.len() as u64 + 1000;
    let from = t.w.log.len();
    let was_live = t.w.writers(&t.s) > 0;
    t.w.engine_open = Some(id);
    let report = engine::open(&mut t.w, &t.s, request);
    t.w.engine_open = None;
    oracle::check_open(&t.w, id, request, &report).expect("oracle");
    oracle::check_log(&t.w, from, &BTreeSet::new()).expect("oracle log");
    preference::check(request, &report, was_live).expect("preference");
    report
}

fn kind(d: &session_navigation::plan::Decision) -> Option<ActionKind> {
    d.action().map(|a| a.kind)
}

fn stopped_codex_desktop() -> T {
    let mut t = start(StartHost::CodexDesktop);
    t.apply(Event::QuitApp {
        app: App::CodexDesktop,
    });
    t
}

#[test]
fn a_stopped_codex_desktop_thread_resumes_in_a_terminal_when_preferred() {
    let mut t = stopped_codex_desktop();
    assert_eq!(kind(&t.plan()), Some(ActionKind::OpenExisting));
    let d = t.plan_with(&preferring());
    let id = t.s.id.clone();
    let cwd = t.cwd();
    expect_launch(&d, TerminalKind::Ghostty, "codex", &["resume", &id], &cwd);
    let report = open_checked(&mut t, &preferring());
    assert_eq!(report.outcome, Outcome::Verified);
    assert!(
        !t.w.codex.app_loaded.contains(&t.s.id),
        "Codex was not used"
    );
    // It converges: the next look returns to the new terminal.
    assert_eq!(
        kind(&t.plan_with(&preferring())),
        Some(ActionKind::FocusPane)
    );
}

#[test]
fn a_known_last_terminal_is_kept_and_the_default_is_not_forced() {
    let mut t = stopped_codex_desktop();
    let mut snap = t.w.observe(&t.s);
    snap.last_seen_host = Fact::Known(Host::Terminal(TerminalKind::AppleTerminal));
    let d = decide(&snap, &preferring());
    let Some(Step::Launch { terminal, .. }) = d.action().and_then(|a| a.steps.first()) else {
        panic!("{d:?}");
    };
    assert_eq!(
        *terminal,
        TerminalKind::AppleTerminal,
        "not the default Ghostty"
    );
    // Unlike `use_default_terminal`, which does use the default.
    let mut asked = preferring();
    asked.use_default_terminal = true;
    let d = decide(&snap, &asked);
    let Some(Step::Launch { terminal, .. }) = d.action().and_then(|a| a.steps.first()) else {
        panic!("{d:?}");
    };
    assert_eq!(*terminal, TerminalKind::Ghostty);
}

#[test]
fn without_a_possible_resume_the_thread_still_opens_in_codex() {
    let mut t = stopped_codex_desktop();
    t.apply(Event::UninstallTool {
        tool: Tool::CodexCli,
    });
    assert_eq!(
        kind(&t.plan_with(&preferring())),
        Some(ActionKind::OpenExisting)
    );
}

#[test]
fn an_archived_codex_thread_still_opens_in_codex() {
    let mut t = stopped_codex_desktop();
    let pick = t.pick();
    t.apply(Event::Archive { pick });
    let without = t.plan();
    let with = t.plan_with(&preferring());
    assert_eq!(
        kind(&without),
        Some(ActionKind::OpenExisting),
        "{without:?}"
    );
    assert_eq!(with, without);
}

#[test]
fn a_live_session_is_never_changed() {
    for host in [
        StartHost::CodexDesktop,
        StartHost::ClaudeDesktop,
        StartHost::CodexGhostty,
        StartHost::ClaudeGhostty,
    ] {
        let mut t = start(host);
        let without = t.plan();
        assert!(without.state.is_live(), "{host:?}: {without:?}");
        assert_eq!(t.plan_with(&preferring()), without, "{host:?}");
        // Whatever runs is what runs without the preference (the oracle and
        // the preference check judge it).
        let report = open_checked(&mut t, &preferring());
        assert_eq!(report.decision, without, "{host:?}");
    }
}

#[test]
fn an_unarchived_claude_desktop_record_still_opens_in_claude() {
    let mut t = start(StartHost::ClaudeDesktop);
    t.apply(Event::QuitApp {
        app: App::ClaudeDesktop,
    });
    let without = t.plan();
    assert_eq!(kind(&without), Some(ActionKind::OpenExisting));
    assert_eq!(t.plan_with(&preferring()), without);
}

#[test]
fn an_archived_claude_record_resumes_in_a_terminal_when_preferred() {
    let mut t = start(StartHost::ClaudeDesktop);
    let pick = t.pick();
    t.apply(Event::Archive { pick });
    assert_eq!(kind(&t.plan()), Some(ActionKind::Reopen));
    let d = t.plan_with(&preferring());
    let id = t.s.id.clone();
    let cwd = t.cwd();
    expect_launch(
        &d,
        TerminalKind::Ghostty,
        "claude",
        &["--resume", &id],
        &cwd,
    );
    let report = open_checked(&mut t, &preferring());
    assert_eq!(report.outcome, Outcome::Verified, "{report:?}");
}

#[test]
fn a_refusal_stays_a_refusal() {
    let mut t = stopped_codex_desktop();
    t.apply(Event::Uninstall {
        app: App::CodexDesktop,
    });
    t.apply(Event::UninstallTool {
        tool: Tool::CodexCli,
    });
    let without = t.plan();
    assert!(matches!(without.plan, Plan::Unavailable(_)), "{without:?}");
    assert_eq!(t.plan_with(&preferring()), without);
}

#[test]
fn the_check_rejects_a_preference_that_reaches_a_live_session() {
    let mut t = stopped_codex_desktop();
    let request = preferring();
    let mut report = open_checked(&mut t, &request);
    // The same change, claimed for a session that was running.
    let v = preference::check(&request, &report, true).expect_err("live");
    assert_eq!(v.invariant, oracle::Invariant::PreferenceScoped);
    // A change that is not a resume (a refusal in its place).
    report.decision.plan = Plan::Unavailable(Reason::EvidenceIncomplete);
    let v = preference::check(&request, &report, false).expect_err("refusal");
    assert_eq!(v.invariant, oracle::Invariant::PreferenceScoped);
}

#[test]
fn a_resume_the_version_gate_refuses_leaves_the_thread_in_codex() {
    let mut t = start(StartHost::CodexDesktop);
    t.apply(Event::Upgrade {
        app: None,
        tool: Some(Tool::CodexCli),
    });
    t.apply(Event::QuitApp {
        app: App::CodexDesktop,
    });
    let without = t.plan();
    assert_eq!(
        kind(&without),
        Some(ActionKind::OpenExisting),
        "{without:?}"
    );
    assert_eq!(
        t.plan_with(&preferring()),
        without,
        "never a refusal instead"
    );
}
