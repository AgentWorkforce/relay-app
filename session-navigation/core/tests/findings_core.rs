//! Regression tests for the review findings that `tests/adversarial_*.rs` do
//! not pin on their own. Numbers refer to the findings list (1–19, a–e).
#[path = "findings/builders.rs"]
mod builders;

use std::collections::{BTreeMap, VecDeque};

use builders::*;
use session_navigation::decide::decide;
use session_navigation::engine;
use session_navigation::evidence::*;
use session_navigation::model::*;
use session_navigation::plan::*;
use session_navigation::ports::{Execute, Observe, Reservations, StoreError};

fn req() -> OpenRequest {
    OpenRequest {
        // An explicit terminal preference: there is no built-in default.
        prefs: session_navigation::plan::Preferences {
            default_terminal: Some(session_navigation::model::TerminalKind::Ghostty),
            ..Default::default()
        },
        ..OpenRequest::default()
    }
}

fn oc(table: Vec<ProcessInfo>, ev: OpenCodeEvidence) -> Snapshot {
    snap(
        Provider::OpenCode,
        OC_ID,
        table,
        ProviderEvidence::OpenCode(ev),
    )
}

// ------------------------------------------------------------- OpenCode ---

/// Finding 1: A TUI started on another session (`-s <other>`) may have navigated to
/// this one; `opencode -m run` is a TUI whose model is named "run";
/// `opencode serve` is not a viewer. A viewer is a client of the server, not
/// a writer (native N13-N14): it no longer refuses the resume, it labels it.
#[test]
fn f1_opencode_viewer_classification() {
    let note = |table: Vec<ProcessInfo>| {
        let d = decide(&oc(table, opencode_ev()), &req());
        assert!(launch_of(&d).is_some(), "{d:?}");
        d.action().and_then(|a| a.note)
    };
    let other = in_ghostty(
        "/dev/ttys004",
        710,
        Exe::OpenCodeCli,
        &["opencode", "-s", OC_OTHER],
    );
    assert_eq!(note(other), Some(Reason::PossiblySecondView));
    let valued = in_ghostty(
        "/dev/ttys004",
        710,
        Exe::OpenCodeCli,
        &["opencode", "-m", "run"],
    );
    assert_eq!(note(valued), Some(Reason::PossiblySecondView));
    let serve = vec![
        launchd(),
        proc_(710, 1, Exe::OpenCodeCli, None, &["opencode", "serve"]),
    ];
    assert_eq!(note(serve), None);
}

/// Finding 2: Unknown busy-ness is not "idle".
#[test]
fn f2_opencode_unknown_busy_is_not_launched() {
    let ev = OpenCodeEvidence {
        busy: Fact::Unknown,
        ..opencode_ev()
    };
    let d = decide(&oc(vec![launchd()], ev), &req());
    assert_eq!(d.plan, Plan::Unavailable(Reason::EvidenceIncomplete));
}

fn desktop_origin(table: Vec<ProcessInfo>, ev: OpenCodeEvidence) -> Decision {
    let mut s = oc(table, ev);
    s.last_seen_host = Fact::Known(Host::Desktop(App::OpenCodeDesktop));
    decide(&s, &req())
}

/// Finding 4: Launching the desktop restores its saved tabs: not onto a session a
/// TUI or one of our own pending launches may hold, and not blind.
#[test]
fn f4_opencode_desktop_is_not_launched_onto_a_session_another_client_holds() {
    let tabbed = OpenCodeEvidence {
        desktop_tabs: Fact::Known(vec![OC_OTHER.into()]),
        ..opencode_ev()
    };
    let headless = vec![
        launchd(),
        proc_(
            710,
            1,
            Exe::OpenCodeCli,
            None,
            &["opencode", "run", "-s", OC_OTHER],
        ),
    ];
    let d = desktop_origin(headless, tabbed.clone());
    assert_eq!(d.plan, Plan::Unavailable(Reason::PossiblyOpenElsewhere));
    let unknown = OpenCodeEvidence {
        desktop_tabs: Fact::Unknown,
        ..opencode_ev()
    };
    let d = desktop_origin(vec![launchd()], unknown);
    assert_eq!(d.plan, Plan::Unavailable(Reason::EvidenceIncomplete));
    let pending = OpenCodeEvidence {
        reserved: Fact::Known(vec![OC_OTHER.into()]),
        ..tabbed.clone()
    };
    let d = desktop_origin(vec![launchd()], pending);
    assert_eq!(d.plan, Plan::Unavailable(Reason::LaunchPending));
    let d = desktop_origin(vec![launchd()], tabbed);
    assert_eq!(d.action().map(|a| a.kind), Some(ActionKind::BringForward));
}

/// Finding c: With the server down, a TUI starts its own server; the desktop later
/// demands its bundled version. Different or unknown → refuse.
#[test]
fn fc_opencode_server_down_checks_the_desktop_version() {
    let down = OpenCodeEvidence {
        server: Fact::Known(ServerState::Down),
        ..opencode_ev()
    };
    let mut s = oc(vec![launchd()], down);
    s.apps = Fact::Known(apps(&[], Some("2.2.7")));
    assert_eq!(
        decide(&s, &req()).plan,
        Plan::Unavailable(Reason::ServerVersionMismatch)
    );
    s.apps = Fact::Known(apps(&[], Some("2.2.6")));
    for app in s
        .apps
        .known_mut()
        .into_iter()
        .flatten()
        .filter(|a| a.app == App::OpenCodeDesktop)
    {
        app.version = None;
    }
    assert_eq!(
        decide(&s, &req()).plan,
        Plan::Unavailable(Reason::ServerVersionMismatch)
    );
    s.apps = Fact::Known(apps(&[], None));
    assert!(
        launch_of(&decide(&s, &req())).is_some(),
        "no desktop, no conflict"
    );
}

// ---------------------------------------------------------------- Codex ---

/// Finding 12: `-c key=value` and `--config=…` before the subcommand.
#[test]
fn f12_codex_config_options_before_resume() {
    for argv in [
        &["codex", "-c", "model=x", "resume", CODEX_ID][..],
        &["codex", "--config=model=x", "resume", CODEX_ID][..],
    ] {
        let table = in_ghostty("/dev/ttys000", 4242, Exe::CodexCli, argv);
        let ev = ProviderEvidence::Codex(codex_ev(CodexOrigin::Tui, vec![4242]));
        let d = decide(&snap(Provider::Codex, CODEX_ID, table, ev), &req());
        let a = d.action().expect("acts");
        assert_eq!(
            (a.kind, a.precision),
            (ActionKind::FocusPane, Precision::ExactConversation)
        );
    }
}

// ------------------------------------------------------------- generic ---

/// Finding 16: No resolved executable → ToolNotInstalled; a plan never runs a bare name.
#[test]
fn f16_launch_needs_a_resolved_absolute_executable() {
    let ev = || ProviderEvidence::Codex(codex_ev(CodexOrigin::Tui, vec![]));
    let s = snap(Provider::Codex, CODEX_ID, vec![launchd()], ev());
    let cmd = launch_of(&decide(&s, &req())).cloned().expect("launch");
    assert_eq!(cmd.program, "/opt/homebrew/bin/codex");
    let mut s = snap(Provider::Codex, CODEX_ID, vec![launchd()], ev());
    for t in s.tools.iter_mut() {
        t.path = None;
    }
    assert_eq!(
        decide(&s, &req()).plan,
        Plan::Unavailable(Reason::ToolNotInstalled)
    );
}

/// Finding a: The environment is carried from the tool's allowlist only, and an
/// unknown environment is refused.
#[test]
fn fa_launch_environment_is_allowlisted_and_required() {
    let ev = || ProviderEvidence::Codex(codex_ev(CodexOrigin::Tui, vec![]));
    let mut s = snap(Provider::Codex, CODEX_ID, vec![launchd()], ev());
    s.environment = Fact::Known(BTreeMap::from([
        (
            "CODEX_HOME".to_owned(),
            "/Users/example/.codex-work".to_owned(),
        ),
        ("CLAUDE_CONFIG_DIR".to_owned(), "/x".to_owned()),
        ("SECRET_TOKEN".to_owned(), "nope".to_owned()),
    ]));
    let cmd = launch_of(&decide(&s, &req())).cloned().expect("launch");
    assert_eq!(
        cmd.env,
        BTreeMap::from([(
            "CODEX_HOME".to_owned(),
            "/Users/example/.codex-work".to_owned()
        )])
    );
    s.environment = Fact::Unknown;
    assert_eq!(
        decide(&s, &req()).plan,
        Plan::Unavailable(Reason::EnvironmentUnknown)
    );
}

/// Finding b: A reservation from an earlier boot cannot still start anything.
#[test]
fn fb_reservation_from_an_earlier_boot_resolves() {
    let ev = ProviderEvidence::Codex(codex_ev(CodexOrigin::Tui, vec![]));
    let mut s = snap(Provider::Codex, CODEX_ID, vec![launchd()], ev);
    let reservation = |boot| Reservation {
        session: s.session.clone(),
        made_at: 1,
        tty: Some("/dev/ttys009".into()),
        seen: true,
        boot: Some(boot),
        via: Pending::Launch,
    };
    s.processes = Fact::Known(in_ghostty("/dev/ttys009", 5, Exe::Shell, &["zsh"]));
    s.reservation = Fact::Known(Some(reservation(BOOT)));
    assert_eq!(
        decide(&s, &req()).plan,
        Plan::Unavailable(Reason::LaunchPending)
    );
    s.reservation = Fact::Known(Some(reservation(BootId(0))));
    let d = decide(&s, &req());
    assert!(launch_of(&d).is_some() && d.release_reservation, "{d:?}");
}

/// Finding 9: A picked desktop that is gone is SelectionGone, not something else.
#[test]
fn f9_selection_of_a_desktop_that_is_gone() {
    let ev = ProviderEvidence::Codex(codex_ev(CodexOrigin::Tui, vec![]));
    let s = snap(Provider::Codex, CODEX_ID, vec![launchd()], ev);
    let request = OpenRequest {
        selected: Some(Target::Desktop(App::CodexDesktop)),
        ..req()
    };
    assert_eq!(
        decide(&s, &request).plan,
        Plan::Unavailable(Reason::SelectionGone)
    );
}

// ------------------------------------------------------------- engine ---

struct Fake {
    snapshots: VecDeque<Snapshot>,
    reservation: Option<Reservation>,
    executed: Vec<Step>,
}

impl Observe for Fake {
    fn observe(&mut self, _: &SessionRef) -> Snapshot {
        let mut s = if self.snapshots.len() > 1 {
            self.snapshots.pop_front()
        } else {
            self.snapshots.front().cloned()
        }
        .expect("snapshots");
        s.reservation = Fact::Known(self.reservation.clone());
        s
    }
}

impl Execute for Fake {
    fn execute(&mut self, step: &Step) -> Dispatch {
        self.executed.push(step.clone());
        Dispatch::Dispatched
    }
}

impl Reservations for Fake {
    fn reserve(&mut self, r: Reservation) -> Result<(), StoreError> {
        self.reservation = Some(r);
        Ok(())
    }
    fn release(&mut self, _: &SessionRef) -> Result<(), StoreError> {
        self.reservation = None;
        Ok(())
    }
}

fn headless_codex(alive: bool) -> Snapshot {
    let mut table = vec![launchd()];
    let mut holders = vec![];
    if alive {
        table.push(proc_(
            4242,
            1,
            Exe::CodexCli,
            None,
            &["codex", "exec", "resume", CODEX_ID],
        ));
        holders.push(4242);
    }
    let ev = ProviderEvidence::Codex(codex_ev(CodexOrigin::Exec, holders));
    snap(Provider::Codex, CODEX_ID, table, ev)
}

fn take_over(alive_observations: usize) -> Fake {
    let mut snapshots = VecDeque::from(vec![headless_codex(true)]);
    snapshots.extend((0..alive_observations).map(|_| headless_codex(true)));
    snapshots.push_back(headless_codex(false));
    Fake {
        snapshots,
        reservation: None,
        executed: vec![],
    }
}

/// Finding e: Take-over waits across several observations for a slow exit, and
/// never resumes when the exit is not proven.
#[test]
fn fe_take_over_waits_a_bounded_number_of_observations() {
    let request = OpenRequest {
        take_over: true,
        ..req()
    };
    let session = SessionRef::new(Provider::Codex, CODEX_ID);
    let mut slow = take_over(2);
    let report = engine::open(&mut slow, &session, &request);
    assert!(
        slow.executed
            .iter()
            .any(|s| matches!(s, Step::Launch { .. })),
        "{report:?}"
    );
    let mut never = take_over(engine::MAX_EXIT_OBSERVATIONS + 5);
    let report = engine::open(&mut never, &session, &request);
    assert_eq!(report.outcome, Outcome::Blocked(Reason::TakeOverRaced));
    assert!(!never
        .executed
        .iter()
        .any(|s| matches!(s, Step::Launch { .. })));
}

/// Ownership seen in another window does not resolve a pending launch: that
/// launch may still start (regression seed lost when the generator changed).
#[test]
fn reservation_is_not_released_by_ownership_elsewhere() {
    let mut table = in_ghostty(
        "/dev/ttys000",
        4242,
        Exe::CodexCli,
        &["codex", "resume", CODEX_ID],
    );
    table.extend(
        in_ghostty("/dev/ttys009", 5, Exe::Shell, &["zsh"])
            .into_iter()
            .skip(2),
    );
    let ev = ProviderEvidence::Codex(codex_ev(CodexOrigin::Tui, vec![4242]));
    let mut s = snap(Provider::Codex, CODEX_ID, table, ev);
    s.reservation = Fact::Known(Some(Reservation {
        session: s.session.clone(),
        made_at: 1,
        tty: Some("/dev/ttys009".into()),
        seen: true,
        boot: Some(BOOT),
        via: Pending::Launch,
    }));
    let d = decide(&s, &req());
    assert_eq!(
        d.action().map(|a| a.kind),
        Some(ActionKind::FocusPane),
        "{d:?}"
    );
    assert!(!d.release_reservation, "{d:?}");
}
