//! The default terminal: the person's explicit preference, else the
//! operating system's default as observed, else a refusal. Never a built-in
//! terminal, never a silent swap.
use std::collections::BTreeSet;

use session_navigation::decide::decide;
use session_navigation::engine;
use session_navigation::evidence::{Fact, Snapshot};
use session_navigation::model::{App, Host, Reason, TerminalKind};
use session_navigation::plan::{
    Decision, OpenRequest, Outcome, Plan, PostCondition, Preferences, Step,
};
use session_navigation::ports::Observe;
use session_navigation::sim::agents::StartHost;
use session_navigation::sim::defaults::SystemDefault;
use session_navigation::sim::world::LogEntry;
use session_navigation::sim::{oracle, Event};

use crate::common::{start, T};

/// A Codex TUI session whose Ghostty pane was closed: stopped, resumable.
fn stopped() -> T {
    let mut t = start(StartHost::CodexGhostty);
    t.apply(Event::ClosePane { pick: 0 });
    t
}

fn with_pref(pref: Option<TerminalKind>, use_default: bool) -> OpenRequest {
    OpenRequest {
        prefs: Preferences {
            default_terminal: pref,
            ..Preferences::default()
        },
        use_default_terminal: use_default,
        ..OpenRequest::default()
    }
}

/// The stopped session's snapshot with no host on record (seen by nobody).
fn hostless(t: &mut T, system: Fact<Option<TerminalKind>>) -> Snapshot {
    let mut snap = t.w.observe(&t.s);
    snap.last_seen_host = Fact::Unknown;
    snap.original_host = Fact::Unknown;
    snap.default_terminal = system;
    snap
}

fn launched_in(d: &Decision) -> Option<TerminalKind> {
    let action = d.action()?;
    let terminal = action.steps.iter().find_map(|s| match s {
        Step::Launch { terminal, .. } => Some(*terminal),
        _ => None,
    })?;
    assert_eq!(action.post, PostCondition::RunningIn { terminal });
    Some(terminal)
}

fn uninstall(snap: &mut Snapshot, app: App) {
    if let Fact::Known(apps) = &mut snap.apps {
        for row in apps.iter_mut().filter(|a| a.app == app) {
            row.installed = false;
        }
    }
}

#[test]
fn no_preference_uses_the_system_default() {
    let mut t = stopped();
    let snap = hostless(&mut t, Fact::Known(Some(TerminalKind::AppleTerminal)));
    let d = decide(&snap, &with_pref(None, false));
    assert_eq!(launched_in(&d), Some(TerminalKind::AppleTerminal), "{d:?}");
    let d = decide(&snap, &with_pref(None, true));
    assert_eq!(launched_in(&d), Some(TerminalKind::AppleTerminal), "{d:?}");
}

#[test]
fn no_preference_and_no_system_default_refuses() {
    let mut t = stopped();
    let snap = hostless(&mut t, Fact::Known(None));
    for use_default in [false, true] {
        assert_eq!(
            decide(&snap, &with_pref(None, use_default)).plan,
            Plan::Unavailable(Reason::NoDefaultTerminal)
        );
    }
}

#[test]
fn no_preference_and_unreadable_system_default_is_incomplete() {
    let mut t = stopped();
    let snap = hostless(&mut t, Fact::Unknown);
    for use_default in [false, true] {
        assert_eq!(
            decide(&snap, &with_pref(None, use_default)).plan,
            Plan::Unavailable(Reason::EvidenceIncomplete)
        );
    }
}

#[test]
fn the_preference_wins_over_the_system_default() {
    let mut t = stopped();
    for system in [
        Fact::Known(Some(TerminalKind::Ghostty)),
        Fact::Known(None),
        Fact::Unknown,
    ] {
        let snap = hostless(&mut t, system);
        for use_default in [false, true] {
            let d = decide(
                &snap,
                &with_pref(Some(TerminalKind::AppleTerminal), use_default),
            );
            assert_eq!(launched_in(&d), Some(TerminalKind::AppleTerminal), "{d:?}");
        }
    }
}

#[test]
fn a_known_host_wins_unless_the_default_is_asked_for() {
    let mut t = stopped();
    let mut snap = hostless(&mut t, Fact::Known(Some(TerminalKind::ITerm2)));
    snap.last_seen_host = Fact::Known(Host::Terminal(TerminalKind::Ghostty));
    for pref in [None, Some(TerminalKind::AppleTerminal)] {
        let d = decide(&snap, &with_pref(pref, false));
        assert_eq!(launched_in(&d), Some(TerminalKind::Ghostty), "{d:?}");
    }
    let d = decide(&snap, &with_pref(Some(TerminalKind::AppleTerminal), true));
    assert_eq!(launched_in(&d), Some(TerminalKind::AppleTerminal), "{d:?}");
    let d = decide(&snap, &with_pref(None, true));
    assert_eq!(launched_in(&d), Some(TerminalKind::ITerm2), "{d:?}");
    // The host needs no default at all.
    snap.default_terminal = Fact::Unknown;
    let d = decide(&snap, &with_pref(None, false));
    assert_eq!(launched_in(&d), Some(TerminalKind::Ghostty), "{d:?}");
    // The original host stands in for an unknown last one.
    snap.last_seen_host = Fact::Unknown;
    snap.original_host = Fact::Known(Host::Tmux(Some(TerminalKind::AppleTerminal)));
    let d = decide(&snap, &with_pref(None, false));
    assert_eq!(launched_in(&d), Some(TerminalKind::AppleTerminal), "{d:?}");
}

#[test]
fn a_default_that_is_not_installed_is_refused_not_swapped() {
    let mut t = stopped();
    let mut snap = hostless(&mut t, Fact::Known(Some(TerminalKind::ITerm2)));
    uninstall(&mut snap, App::ITerm2);
    assert_eq!(
        decide(&snap, &with_pref(None, false)).plan,
        Plan::Unavailable(Reason::AppNotInstalled)
    );
    // An uninstalled preference is not swapped for the installed system
    // default either.
    snap.default_terminal = Fact::Known(Some(TerminalKind::Ghostty));
    assert_eq!(
        decide(&snap, &with_pref(Some(TerminalKind::ITerm2), false)).plan,
        Plan::Unavailable(Reason::AppNotInstalled)
    );
    // A default that does not exist on this platform is not swapped.
    snap.default_terminal = Fact::Known(Some(TerminalKind::WindowsTerminal));
    assert_eq!(
        decide(&snap, &with_pref(None, false)).plan,
        Plan::Unavailable(Reason::PlatformUnsupported)
    );
}

/// Through the engine against the simulator: a refusal launches nothing,
/// reserves nothing, and the oracle agrees.
fn engine_refusal(system: SystemDefault, draw: u64, reason: Reason) {
    let mut t = stopped();
    t.w.hosts.default_draw = draw;
    assert_eq!(t.w.system_default(), system);
    let request = with_pref(None, true);
    let id = t.w.log.len() as u64 + 1000;
    let from = t.w.log.len();
    t.w.engine_open = Some(id);
    let report = engine::open(&mut t.w, &t.s, &request);
    t.w.engine_open = None;
    assert_eq!(report.decision.plan, Plan::Unavailable(reason));
    assert_eq!(report.outcome, Outcome::Blocked(reason));
    assert!(report.executed.is_empty(), "{:?}", report.executed);
    assert!(!t.w.reservations.contains_key(&t.s), "nothing reserved");
    assert!(!t.w.log[from..]
        .iter()
        .any(|e| matches!(e, LogEntry::LaunchDispatched { .. })));
    oracle::check_open(&t.w, id, &request, &report).expect("oracle");
    oracle::check_log(&t.w, from, &BTreeSet::new()).expect("oracle log");
}

#[test]
fn engine_refuses_without_a_default_and_reserves_nothing() {
    engine_refusal(SystemDefault::NoneSet, 75, Reason::NoDefaultTerminal);
    engine_refusal(SystemDefault::Unreadable, 90, Reason::EvidenceIncomplete);
}

#[test]
fn engine_uses_the_system_default_and_the_oracle_agrees() {
    let mut t = stopped();
    // 40..=54: the platform's alternative (Terminal on macOS).
    t.w.hosts.default_draw = 40;
    assert_eq!(
        t.w.system_default(),
        SystemDefault::Set(TerminalKind::AppleTerminal)
    );
    let request = with_pref(None, true);
    let id = t.w.log.len() as u64 + 1000;
    t.w.engine_open = Some(id);
    let report = engine::open(&mut t.w, &t.s, &request);
    t.w.engine_open = None;
    assert_eq!(
        launched_in(&report.decision),
        Some(TerminalKind::AppleTerminal)
    );
    assert_eq!(report.outcome, Outcome::Verified);
    oracle::check_open(&t.w, id, &request, &report).expect("oracle");
}

#[test]
fn the_oracle_rejects_a_built_in_fallback_and_a_false_refusal() {
    let mut t = stopped();
    t.w.hosts.default_draw = 75;
    let request = with_pref(None, true);
    let id = t.w.log.len() as u64 + 1000;
    t.w.engine_open = Some(id);
    let mut report = engine::open(&mut t.w, &t.s, &request);
    t.w.engine_open = None;
    // Claiming "unreadable" would be fine for the oracle only if it were;
    // claiming "none" when the default is unreadable is not.
    t.w.hosts.default_draw = 90;
    let v = oracle::check_open(&t.w, id, &request, &report).expect_err("false refusal");
    assert_eq!(v.invariant, oracle::Invariant::RightTerminal);
    // A launch in Ghostty with no default anywhere and no host on record.
    t.w.hosts.default_draw = 75;
    let mut snap = hostless(&mut t, Fact::Known(Some(TerminalKind::Ghostty)));
    let d = decide(&snap, &with_pref(None, true));
    let Some(Step::Launch { .. }) = d.action().and_then(|a| a.steps.first()).cloned() else {
        panic!("{d:?}");
    };
    snap.default_terminal = Fact::Known(None);
    report.before = snap;
    report.decision = d.clone();
    report.executed = d
        .action()
        .map(|a| {
            a.steps
                .iter()
                .map(|s| (s.clone(), session_navigation::plan::Dispatch::Dispatched))
                .collect()
        })
        .unwrap_or_default();
    t.w.history.remove(&t.s);
    t.w.first_host.remove(&t.s);
    let v = oracle::check_open(&t.w, id, &request, &report).expect_err("built-in fallback");
    assert_eq!(v.invariant, oracle::Invariant::RightTerminal);
}
