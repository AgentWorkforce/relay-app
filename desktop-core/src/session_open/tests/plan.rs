//! The button: decisions as the apps draw them, requests checked before
//! the engine, and a plan look that can never prompt.
use session_navigation::evidence::Fact;
use session_navigation::hosts::PaneRef;
use session_navigation::model::{
    App, BootId, Precision as CorePrecision, ProcessIdentity, Reason, TerminalKind,
};
use session_navigation::plan::{
    Action as CoreAction, ActionKind, Command, Confirmation, Decision, Elsewhere, LaunchContext,
    Plan, PostCondition, SessionState, Step, Target,
};
use session_navigation_native::FakeSystem;

use super::support::*;
use crate::session_open::contract::{Action, Precision, State};
use crate::session_open::view;
use crate::session_open::wording::{self, Local};

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

fn focus(terminal: TerminalKind, tty: &str) -> Step {
    Step::FocusPane {
        terminal,
        pane: PaneRef::Tty { tty: tty.into() },
        process: PID,
    }
}

fn launch(terminal: TerminalKind) -> Step {
    Step::Launch {
        terminal,
        command: Command {
            program: "/opt/homebrew/bin/codex".into(),
            args: vec!["resume".into(), CODEX_ID.into()],
            cwd: "/Users/example/code".into(),
            env: Default::default(),
        },
        context: LaunchContext::native(),
    }
}

fn view_of(decision: Decision) -> crate::session_open::contract::PlanView {
    view::plan(&decision, &Fact::Known(false), None)
}

#[test]
fn each_action_has_the_apps_label() {
    let cases = [
        (
            act(
                ActionKind::FocusPane,
                vec![focus(TerminalKind::Ghostty, "/dev/ttys004")],
            ),
            Action::Focus,
            "Open in Ghostty",
        ),
        (
            act(ActionKind::Resume, vec![launch(TerminalKind::ITerm2)]),
            Action::Resume,
            "Resume in iTerm2",
        ),
        (
            act(
                ActionKind::TakeOver,
                vec![
                    Step::Stop { process: PID },
                    Step::AwaitExit { process: PID },
                    launch(TerminalKind::Kitty),
                ],
            ),
            Action::TakeOver,
            "Stop and resume in Kitty",
        ),
        (
            act(ActionKind::Attach, vec![launch(TerminalKind::WezTerm)]),
            Action::Attach,
            "Attach in WezTerm",
        ),
        (
            act(
                ActionKind::OpenExisting,
                vec![Step::OpenUrl {
                    url: "claude://code/continue?session=local_x".into(),
                    bundle: App::ClaudeDesktop.bundle_id().into(),
                }],
            ),
            Action::OpenLink,
            "Open in Claude",
        ),
        (
            act(
                ActionKind::Reopen,
                vec![Step::OpenUrl {
                    url: "codex://threads/x".into(),
                    bundle: App::CodexDesktop.bundle_id().into(),
                }],
            ),
            Action::OpenLink,
            "Open in Codex",
        ),
        (
            act(
                ActionKind::BringForward,
                vec![Step::ActivateApp {
                    app: App::Alacritty,
                }],
            ),
            Action::ShowApplication,
            "Show Alacritty",
        ),
        (
            act(
                ActionKind::OpenExisting,
                vec![Step::ActivateApp {
                    app: App::OpenCodeDesktop,
                }],
            ),
            Action::ShowApplication,
            "Show OpenCode",
        ),
    ];
    for (action, expected, title) in cases {
        let view = view_of(Decision::new(SessionState::Stopped, Plan::Act(action)));
        assert_eq!((view.action, view.title.as_str()), (expected, title));
        assert!(view.enabled);
        assert_eq!(view.precision, Some(Precision::RightWindow));
        assert_eq!(view.reason_code, None);
    }
}

#[test]
fn a_reduced_action_says_why() {
    let mut action = act(
        ActionKind::BringForward,
        vec![Step::ActivateApp { app: App::ITerm2 }],
    );
    action.note = Some(Reason::PaneSelectionUnavailable);
    let view = view_of(Decision::new(
        SessionState::LiveLocatable,
        Plan::Act(action),
    ));
    assert_eq!(view.state, State::Live);
    assert_eq!(
        view.reason_code.as_deref(),
        Some("pane_selection_unavailable")
    );
    assert_eq!(
        view.message.as_deref(),
        Some(wording::reason(Reason::PaneSelectionUnavailable).1)
    );
}

#[test]
fn several_places_are_choices_whose_ids_come_back() {
    let pane = act(
        ActionKind::FocusPane,
        vec![focus(TerminalKind::Ghostty, "/dev/ttys004")],
    );
    let mut desktop = act(
        ActionKind::OpenExisting,
        vec![Step::OpenUrl {
            url: "claude://code/continue?session=local_x".into(),
            bundle: App::ClaudeDesktop.bundle_id().into(),
        }],
    );
    desktop.target = Some(Target::Desktop(App::ClaudeDesktop));
    let view = view_of(Decision::new(
        SessionState::LiveMultiple,
        Plan::Choose(vec![pane, desktop]),
    ));
    assert_eq!(view.action, Action::Choose);
    assert_eq!(view.title, "Open running session");
    assert_eq!(view.reason_code.as_deref(), Some("several_locations"));
    let labels: Vec<_> = view.choices.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(labels, ["Ghostty (/dev/ttys004)", "Claude"]);
    assert_eq!(view::target(&view.choices[0].id), Some(Target::Pane(PID)));
    assert_eq!(
        view::target(&view.choices[1].id),
        Some(Target::Desktop(App::ClaudeDesktop))
    );
    for bad in [
        "pane:1:2",
        "pane:1:2:3:4",
        "desktop:ghostty",
        "x",
        "pane:a:b:c",
    ] {
        assert_eq!(view::target(bad), None, "{bad}");
    }
}

#[test]
fn a_confirmation_names_where_the_session_may_be_open() {
    let then = act(ActionKind::Resume, vec![launch(TerminalKind::Ghostty)]);
    let plan = Plan::Confirm(Confirmation {
        reason: Reason::PossiblyOpenElsewhere,
        elsewhere: vec![
            Elsewhere {
                process: PID,
                terminal: Some(TerminalKind::AppleTerminal),
                tty: Some("/dev/ttys009".into()),
            },
            Elsewhere {
                process: PID,
                terminal: None,
                tty: None,
            },
        ],
        then,
    });
    let view = view_of(Decision::new(SessionState::Stopped, plan));
    assert_eq!(view.action, Action::Confirm);
    assert_eq!(view.title, "Resume in Ghostty");
    assert!(view.enabled);
    assert_eq!(view.reason_code.as_deref(), Some("possibly_open_elsewhere"));
    assert_eq!(
        view.elsewhere,
        ["Terminal (/dev/ttys009)", "Another process"]
    );
}

#[test]
fn a_pending_launch_disables_resume_and_offers_to_clear_it() {
    let decision = Decision::unavailable(SessionState::LaunchPending, Reason::LaunchPending);
    let view = view::plan(&decision, &Fact::Known(true), None);
    assert_eq!((view.action, view.enabled), (Action::Unavailable, false));
    assert_eq!(view.title, "Session opening unavailable");
    assert!(view.pending_launch);
    assert_eq!(view.reason_code.as_deref(), Some("launch_pending"));
    // An unreadable store blocks, but no confirmation can clear it.
    assert!(!view::plan(&decision, &Fact::Unknown, None).pending_launch);
    let mut released = decision.clone();
    released.release_reservation = true;
    assert!(!view::plan(&released, &Fact::Known(true), None).pending_launch);
}

#[test]
fn the_default_terminal_is_offered_only_when_it_would_resume_differently() {
    let refused = Decision::unavailable(SessionState::Stopped, Reason::HostUnsupported);
    let resume = Decision::new(
        SessionState::Stopped,
        Plan::Act(act(
            ActionKind::Resume,
            vec![launch(TerminalKind::AppleTerminal)],
        )),
    );
    assert!(view::plan(&refused, &Fact::Known(false), Some(&resume)).can_resume_default);
    assert!(!view::plan(&resume, &Fact::Known(false), Some(&resume)).can_resume_default);
    assert!(!view::plan(&refused, &Fact::Known(false), Some(&refused)).can_resume_default);
    assert!(!view::plan(&refused, &Fact::Known(false), None).can_resume_default);
}

#[test]
fn requests_are_checked_before_the_engine() {
    let paths = mac_cfg("/tmp/app").common.paths;
    let fake = FakeSystem::new();
    let check = |r: crate::session_open::contract::Request| {
        crate::session_open::prepare(&fake, &paths, &r).err()
    };
    assert_eq!(
        check(request("claude", CLAUDE_ID, "relative/dir")),
        Some(Local::InvalidRequest)
    );
    assert_eq!(
        check(request("cursor", CLAUDE_ID, "/tmp/app")),
        Some(Local::UnsupportedSource)
    );
    let mut r = request("claude", CLAUDE_ID, "/tmp/app");
    r.default_terminal = Some("hyper".into());
    assert_eq!(check(r), Some(Local::UnknownTerminal));
    let mut r = request("claude", CLAUDE_ID, "/tmp/app");
    r.selected = Some("pane:nonsense".into());
    assert_eq!(check(r), Some(Local::InvalidRequest));
    assert_eq!(
        check(request(
            "claude",
            "local_11111111-2222-4333-8444-555555555555",
            "/tmp/app"
        )),
        Some(Local::DesktopIdUnmapped)
    );
    // An invalid id is the engine's to refuse, with its own reason.
    assert_eq!(check(request("codex", "not a thread", "/tmp/app")), None);
    let view = view::refused(Local::UnsupportedSource);
    assert_eq!(
        (view.state, view.action),
        (State::Unsupported, Action::Unavailable)
    );
    assert!(!view.enabled);
}

#[test]
fn a_desktop_id_maps_to_the_cli_id_its_one_record_names() {
    let paths = mac_cfg("/tmp/app").common.paths;
    let fake = FakeSystem::new();
    let local = "local_11111111-2222-4333-8444-555555555555";
    let dir = format!("{}/account/org", paths.claude_desktop_sessions_dir());
    let record = format!(
        r#"{{"sessionId":"{local}","cliSessionId":"{}","isArchived":false}}"#,
        CLAUDE_ID.to_ascii_uppercase()
    );
    fake.add_file(&format!("{dir}/{local}.json"), record, 1);
    let prepared =
        crate::session_open::prepare(&fake, &paths, &request("claude", local, "/tmp/app")).unwrap();
    assert_eq!(prepared.session, claude());
    assert!(prepared.keys.contains(&format!("claude:{local}")));
    assert!(prepared.keys.contains(&format!("claude:{CLAUDE_ID}")));
    // A second record naming another conversation makes it ambiguous.
    let other =
        format!(r#"{{"sessionId":"{local}","cliSessionId":"{CODEX_ID}","isArchived":false}}"#);
    fake.add_file(
        &format!(
            "{}/account/org2/{local}.json",
            paths.claude_desktop_sessions_dir()
        ),
        other,
        1,
    );
    assert_eq!(
        crate::session_open::prepare(&fake, &paths, &request("claude", local, "/tmp/app")).err(),
        Some(Local::DesktopIdUnmapped)
    );
}

/// The registry query with Ghostty in front, from the adapter's fixture.
#[cfg(target_os = "macos")]
fn ghostty_in_front(fake: &FakeSystem, cfg: &session_navigation_native::MacConfig) {
    use session_navigation_native::macos::{machine, scripts};
    use session_navigation_native::system::Output;
    let fixture =
        include_str!("../../../../session-navigation/native/tests/fixtures/apps-jxa.json");
    let mut apps: serde_json::Value = serde_json::from_str(fixture).unwrap();
    apps["front"] = "com.mitchellh.ghostty".into();
    apps["running"] = serde_json::json!(["com.mitchellh.ghostty"]);
    fake.on(
        &machine::apps_run(cfg),
        vec![Ok(Output::ok(apps.to_string()))],
    );
    let front = machine::script_run(
        scripts::GHOSTTY_FRONT_TTY,
        &[],
        cfg.common.timeouts.script_read,
    );
    fake.on(&front, vec![Ok(Output::ok("/dev/ttys000\n"))]);
}

#[cfg(target_os = "macos")]
#[test]
fn a_plan_look_never_scripts_a_terminal() {
    use crate::session_open::native::{ports, PLAN_IS_PASSIVE};
    let app = tempfile::tempdir().unwrap();
    let app = app.path().to_string_lossy().into_owned();
    let cfg = mac_cfg(&app);
    for (passive, expected) in [(PLAN_IS_PASSIVE, 0), (false, 1)] {
        let fake = FakeSystem::new();
        ghostty_in_front(&fake, &cfg);
        let paths = cfg.common.paths.clone();
        let view = crate::session_open::plan_with(
            &fake,
            &paths,
            None,
            &request("codex", CODEX_ID, &app),
            |h| ports(&fake, cfg.clone(), h, passive),
        );
        assert_eq!(
            applescript_runs(&fake),
            expected,
            "passive {passive}: {view:?}"
        );
        assert!(actions(&fake).is_empty());
        assert!(fake.files_under(&app).is_empty(), "a plan writes nothing");
    }
}
