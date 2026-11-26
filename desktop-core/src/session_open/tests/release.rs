//! What this change added to the contract: `release` (only clears a
//! pending launch, one at a time with `open`), the fixed words in every
//! answer and in `shared/session-open-strings.json`, `destination`,
//! `question`, and the terminal preference reaching the engine.
use std::path::Path;

use session_navigation::evidence::{Fact, Pending, Reservation};
use session_navigation::hosts::PaneRef;
use session_navigation::model::{App, BootId, Precision as CorePrecision, ProcessIdentity};
use session_navigation::model::{Reason, TerminalKind};
use session_navigation::plan::{
    Action as CoreAction, ActionKind, Command, Confirmation, Decision, LaunchContext, Plan,
    PostCondition, SessionState, Step, Target,
};
use session_navigation_native::system::join;
use session_navigation_native::{FakeSystem, LinuxConfig, LinuxPorts, MacPorts};

use super::support::*;
use crate::session_open::command::strings_json;
use crate::session_open::contract::{Action, Destination};
use crate::session_open::legacy::{GTK_FILE, MAC_FILE};
use crate::session_open::native::reservation_dir;
use crate::session_open::{prepare, release_with, view, wording};

const PID: ProcessIdentity = ProcessIdentity {
    pid: 30,
    start: 31,
    boot: BootId(7),
};

fn act(kind: ActionKind, steps: Vec<Step>) -> CoreAction {
    CoreAction {
        kind,
        steps,
        precision: CorePrecision::RightWindow,
        post: PostCondition::RunningIn {
            terminal: TerminalKind::Ghostty,
        },
        note: None,
        import: false,
        target: Some(Target::Pane(PID)),
    }
}

fn launch() -> Step {
    Step::Launch {
        terminal: TerminalKind::Ghostty,
        command: Command {
            program: "/opt/homebrew/bin/codex".into(),
            args: vec!["resume".into(), CODEX_ID.into()],
            cwd: "/Users/example/code".into(),
            env: Default::default(),
        },
        context: LaunchContext::native(),
    }
}

fn view_of(plan: Plan) -> crate::session_open::contract::PlanView {
    view::plan(
        &Decision::new(SessionState::Stopped, plan),
        &Fact::Known(false),
        None,
    )
}

fn app_dir() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    (dir, path)
}

#[test]
fn the_checked_in_strings_are_the_table() {
    let file = include_str!("../../../../shared/session-open-strings.json");
    assert_eq!(
        file,
        strings_json().unwrap(),
        "regenerate with `agent-relay-probe session-open strings > shared/session-open-strings.json`"
    );
}

#[test]
fn every_answer_carries_the_strings() {
    let strings = wording::strings();
    assert_eq!(
        strings.already_opening,
        "This session is already being opened."
    );
    assert_eq!(
        view::refused(wording::Local::UnsupportedSource).strings,
        strings
    );
    assert_eq!(
        view_of(Plan::Unavailable(Reason::Archived)).strings,
        strings
    );
}

#[test]
fn the_destination_is_where_a_click_goes() {
    let pane = Step::FocusPane {
        terminal: TerminalKind::Ghostty,
        pane: PaneRef::Tty {
            tty: "/dev/ttys004".into(),
        },
        process: PID,
    };
    let link = Step::OpenUrl {
        url: format!("codex://threads/{CODEX_ID}"),
        bundle: App::CodexDesktop.bundle_id().into(),
    };
    let cases = [
        (
            act(ActionKind::FocusPane, vec![pane]),
            Destination::Terminal,
        ),
        (
            act(ActionKind::Resume, vec![launch()]),
            Destination::Terminal,
        ),
        (
            act(ActionKind::Attach, vec![launch()]),
            Destination::Terminal,
        ),
        (
            act(
                ActionKind::TakeOver,
                vec![
                    Step::Stop { process: PID },
                    Step::AwaitExit { process: PID },
                    launch(),
                ],
            ),
            Destination::Terminal,
        ),
        (
            act(ActionKind::OpenExisting, vec![link.clone()]),
            Destination::DesktopApp,
        ),
        (
            act(
                ActionKind::BringForward,
                vec![Step::ActivateApp {
                    app: App::OpenCodeDesktop,
                }],
            ),
            Destination::DesktopApp,
        ),
        (
            act(
                ActionKind::BringForward,
                vec![Step::ActivateApp { app: App::ITerm2 }],
            ),
            Destination::Terminal,
        ),
    ];
    for (action, expected) in cases {
        assert_eq!(view::destination(&action), expected, "{action:?}");
        assert_eq!(view_of(Plan::Act(action)).destination, expected);
    }
    assert_eq!(
        view_of(Plan::Unavailable(Reason::Archived)).destination,
        Destination::None
    );
    let mixed = Plan::Choose(vec![
        act(ActionKind::OpenExisting, vec![link]),
        act(ActionKind::Resume, vec![launch()]),
    ]);
    assert_eq!(view_of(mixed).destination, Destination::None);
}

#[test]
fn a_confirm_or_take_over_carries_its_question() {
    let confirm = view_of(Plan::Confirm(Confirmation {
        reason: Reason::PossiblyOpenElsewhere,
        elsewhere: vec![session_navigation::plan::Elsewhere {
            process: PID,
            terminal: Some(TerminalKind::AppleTerminal),
            tty: Some("/dev/ttys009".into()),
        }],
        then: act(ActionKind::Resume, vec![launch()]),
    }));
    assert_eq!(confirm.action, Action::Confirm);
    assert_eq!(
        confirm.question.as_deref(),
        Some(
            format!(
                "{}\nTerminal (/dev/ttys009)",
                confirm.message.clone().unwrap()
            )
            .as_str()
        )
    );
    assert_eq!(confirm.destination, Destination::Terminal);
    let take_over = view_of(Plan::Act(act(
        ActionKind::TakeOver,
        vec![
            Step::Stop { process: PID },
            Step::AwaitExit { process: PID },
            launch(),
        ],
    )));
    assert_eq!(take_over.action, Action::TakeOver);
    assert_eq!(
        take_over.question.as_deref(),
        Some(wording::strings().take_over_detail.as_str())
    );
    assert_eq!(
        view_of(Plan::Act(act(ActionKind::Resume, vec![launch()]))).question,
        None
    );
}

#[test]
fn the_terminal_preference_reaches_the_engine() {
    let paths = mac_cfg("/tmp/app").common.paths;
    let fake = FakeSystem::new();
    let mut r = request("codex", CODEX_ID, "/tmp/app");
    let off = prepare(&fake, &paths, &r).unwrap();
    assert!(!off.request.prefs.prefer_terminal);
    r.prefer_terminal = true;
    let on = prepare(&fake, &paths, &r).unwrap();
    assert!(on.request.prefs.prefer_terminal);
    assert!(!on.request.use_default_terminal, "a different request");
    let parsed: crate::session_open::contract::Request = serde_json::from_str(
        r#"{"source":"codex","session_id":"x","app_data_dir":"/a","prefer_terminal":true}"#,
    )
    .unwrap();
    assert!(parsed.prefer_terminal);
}

fn reserved() -> Fixed {
    let mut snap = snapshot(&claude());
    snap.reservation = Fact::Known(Some(Reservation {
        session: claude(),
        made_at: 5,
        tty: Some("/dev/ttys004".into()),
        seen: false,
        boot: None,
        via: Pending::Launch,
    }));
    Fixed::new(snap)
}

#[test]
fn release_clears_the_reservation_and_opens_nothing() {
    let (_dir, app) = app_dir();
    let fake = FakeSystem::new();
    let paths = mac_cfg(&app).common.paths;
    let ports = reserved();
    let (released, executed, made) = (
        ports.released.clone(),
        ports.executed.clone(),
        ports.reserved.clone(),
    );
    let view = release_with(
        &fake,
        &paths,
        None,
        &request("claude", CLAUDE_ID, &app),
        |_| ports,
    );
    assert_eq!(*released.lock().unwrap(), vec![claude()]);
    assert!(executed.lock().unwrap().is_empty(), "nothing was opened");
    assert!(made.lock().unwrap().is_empty(), "nothing was reserved");
    assert!(actions(&fake).is_empty());
    assert_eq!(view.strings, wording::strings());
    // Without a reservation it is only a look.
    let ports = Fixed::new(snapshot(&claude()));
    let released = ports.released.clone();
    release_with(
        &fake,
        &paths,
        None,
        &request("claude", CLAUDE_ID, &app),
        |_| ports,
    );
    assert!(released.lock().unwrap().is_empty());
}

#[test]
fn release_clears_a_legacy_pending_resume_on_every_adapter() {
    for linux in [false, true] {
        let (_dir, app) = app_dir();
        let fake = FakeSystem::new();
        fake.add_file(
            &join(&app, GTK_FILE),
            format!(r#"["claude:{CLAUDE_ID}"]"#),
            1,
        );
        fake.add_file(
            &join(&app, MAC_FILE),
            format!(r#"{{"version":1,"pending":["claude:{CLAUDE_ID}","codex:other"]}}"#),
            1,
        );
        let mut paths = mac_cfg(&app).common.paths;
        paths.reservation_dir = reservation_dir(&app);
        let r = request("claude", CLAUDE_ID, &app);
        let view = if linux {
            release_with(&fake, &paths, None, &r, |h| {
                let mut cfg = LinuxConfig::for_user("/home/example", 1000);
                cfg.common.paths.reservation_dir = reservation_dir(&app);
                LinuxPorts::with_history(&fake, cfg, h)
            })
        } else {
            release_with(&fake, &paths, None, &r, |h| {
                MacPorts::with_history(&fake, mac_cfg(&app), h)
            })
        };
        assert!(!view.pending_launch, "{view:?}");
        let gtk: Vec<String> =
            serde_json::from_slice(&fake.file(&join(&app, GTK_FILE)).unwrap()).unwrap();
        assert!(gtk.is_empty());
        let mac: serde_json::Value =
            serde_json::from_slice(&fake.file(&join(&app, MAC_FILE)).unwrap()).unwrap();
        assert_eq!(
            mac,
            serde_json::json!({"version":1,"pending":["codex:other"]})
        );
        assert!(actions(&fake).is_empty(), "{:?}", actions(&fake));
    }
}

#[test]
fn a_release_while_an_open_runs_is_refused_and_releases_nothing() {
    let (_dir, app) = app_dir();
    let locks = Path::new(&app).join("session-navigation/opening");
    let held = crate::session_open::guard::begin(&locks, &claude()).unwrap();
    let fake = FakeSystem::new();
    let paths = mac_cfg(&app).common.paths;
    let ports = reserved();
    let released = ports.released.clone();
    let view = release_with(
        &fake,
        &paths,
        None,
        &request("claude", CLAUDE_ID, &app),
        |_| ports,
    );
    assert_eq!(view.reason_code.as_deref(), Some("already_opening"));
    assert!(released.lock().unwrap().is_empty());
    drop(held);
    // And an open waits for a release the same way: the lock is shared.
    let held = crate::session_open::guard::begin(&locks, &claude()).unwrap();
    assert_eq!(
        crate::session_open::guard::begin(&locks, &claude()).unwrap_err(),
        crate::session_open::guard::Refusal::Busy
    );
    drop(held);
}
