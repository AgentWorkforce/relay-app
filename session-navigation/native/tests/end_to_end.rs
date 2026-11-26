//! `engine::open` over `MacPorts` on a fake Mac shaped like the real one.
#![cfg(feature = "macos")]
mod common;

use common::*;
use session_navigation::engine;
use session_navigation::evidence::{Fact, ProviderEvidence, ServerState};
use session_navigation::hosts::PaneRef;
use session_navigation::model::{App, BootId, ProcessIdentity, Reason, TerminalKind};
use session_navigation::plan::{ActionKind, Dispatch, OpenRequest, Outcome, Plan, Step};
use session_navigation_native::macos::execute::{KILL, OPEN};
use session_navigation_native::macos::machine::{self, OSASCRIPT};
use session_navigation_native::macos::scripts;
use session_navigation_native::macos::CURL;
use session_navigation_native::system::{Call, Output, Run};
use session_navigation_native::MacPorts;

/// An explicit terminal preference: there is no built-in default.
fn request() -> OpenRequest {
    let mut request = OpenRequest::default();
    request.prefs.default_terminal = Some(session_navigation::model::TerminalKind::Ghostty);
    request
}

#[test]
fn claude_archived_desktop_session_reopens_through_resume_link() {
    let m = claude_machine(true, false);
    let fake = m.build();
    let url = format!("claude://resume?session={CLAUDE_ID}");
    fake.on(
        &Run::new(OPEN, &["-b", "com.anthropic.claudefordesktop", &url]),
        vec![Ok(Output::ok(""))],
    );
    let mut ports = MacPorts::new(&fake, cfg());
    let report = engine::open(&mut ports, &claude(), &request());
    let action = report.decision.action().cloned();
    assert_eq!(
        action.as_ref().map(|a| a.kind),
        Some(ActionKind::Reopen),
        "{:?}",
        report.decision
    );
    let step = Step::OpenUrl {
        url: url.clone(),
        bundle: "com.anthropic.claudefordesktop".into(),
    };
    assert_eq!(report.executed, vec![(step, Dispatch::Dispatched)]);
    let opens: Vec<_> = fake.runs().into_iter().filter(|(p, _)| p == OPEN).collect();
    assert_eq!(
        opens,
        vec![(
            OPEN.to_owned(),
            vec!["-b".into(), "com.anthropic.claudefordesktop".into(), url]
        )]
    );
    // The record did not change on this fake, so nothing verifies it; the
    // link reservation stays until a later observation resolves it.
    assert_ne!(report.outcome, Outcome::Verified);
    assert!(!fake
        .files_under(&cfg().common.paths.reservation_dir)
        .is_empty());
    assert!(fake.runs().iter().all(|(p, _)| p != KILL));
}

#[test]
fn codex_tui_stopped_resumes_in_ghostty_with_the_exact_quoted_command() {
    let m = codex_machine(Some(codex_row(false, "codex-tui")));
    let fake = m.build();
    let argv = vec![
        PROJECT.to_owned(),
        format!("'/opt/homebrew/bin/codex' 'resume' '{CODEX_ID}'"),
    ];
    let launch = machine::script_run(
        scripts::GHOSTTY_LAUNCH,
        &argv,
        cfg().common.timeouts.script_act,
    );
    fake.on(&launch, vec![Ok(Output::ok("tty:/dev/ttys004\n"))]);
    let mut ports = MacPorts::new(&fake, cfg());
    let report = engine::open(&mut ports, &codex(), &request());
    assert_eq!(
        report.decision.action().map(|a| a.kind),
        Some(ActionKind::Resume),
        "{:?}",
        report.decision
    );
    assert_eq!(report.executed.len(), 1);
    assert_eq!(
        report.executed[0].1,
        Dispatch::Opened {
            tty: "/dev/ttys004".into()
        }
    );
    let scripts_run: Vec<_> = fake
        .runs()
        .into_iter()
        .filter(|(p, a)| {
            p == OSASCRIPT && a.get(1).map(String::as_str) == Some(scripts::GHOSTTY_LAUNCH)
        })
        .collect();
    assert_eq!(scripts_run, vec![(launch.program, launch.args)]);
    // The reservation now carries the new window's tty.
    let store = session_navigation_native::FileReservations::new(
        &fake,
        &cfg().common.paths.reservation_dir,
    );
    assert_eq!(
        store
            .read(&codex())
            .known()
            .cloned()
            .flatten()
            .and_then(|r| r.tty)
            .as_deref(),
        Some("/dev/ttys004")
    );
    // A second click while that window still runs (a shell on its tty)
    // launches nothing: the reservation is unresolved.
    let mut later = codex_machine(Some(codex_row(false, "codex-tui")));
    later.procs.push(Proc::new(
        5000,
        811,
        "ttys004",
        "login",
        "/usr/bin/login",
        "/usr/bin/login -flp example",
    ));
    let fake2 = later.build();
    for path in fake.files_under(&cfg().common.paths.reservation_dir) {
        fake2.add_file(&path, fake.file(&path).unwrap_or_default(), NOW);
    }
    let mut ports = MacPorts::new(&fake2, cfg());
    let again = engine::open(&mut ports, &codex(), &request());
    assert!(again.executed.is_empty(), "{:?}", again.decision);
    assert_eq!(again.outcome, Outcome::Blocked(Reason::LaunchPending));
}

#[test]
fn opencode_server_up_looks_up_the_session_through_curl_with_stdin_config() {
    let m = opencode_machine();
    let fake = m.build();
    let argv = vec![
        PROJECT.to_owned(),
        format!("'{HOME}/.opencode/bin/opencode' '-s' '{OPENCODE_ID}'"),
    ];
    fake.on(
        &machine::script_run(
            scripts::GHOSTTY_LAUNCH,
            &argv,
            cfg().common.timeouts.script_act,
        ),
        vec![Ok(Output::ok("dispatched"))],
    );
    let mut ports = MacPorts::new(&fake, cfg());
    let report = engine::open(&mut ports, &opencode(), &request());
    let ProviderEvidence::OpenCode(ev) = &report.before.provider else {
        panic!("opencode evidence")
    };
    assert!(matches!(ev.server, Fact::Known(ServerState::Up { .. })));
    assert!(ev.session.known().is_some_and(Option::is_some));
    let lookups: Vec<_> = fake
        .calls()
        .into_iter()
        .filter_map(|c| match c {
            Call::Run {
                program,
                args,
                stdin,
                ..
            } if program == CURL && args.last().is_some_and(|u| u.ends_with(OPENCODE_ID)) => {
                Some((args, stdin))
            }
            _ => None,
        })
        .collect();
    assert!(!lookups.is_empty());
    for (args, stdin) in &lookups {
        assert!(args.contains(&"--config".to_owned()) && args.contains(&"-".to_owned()));
        assert!(stdin
            .as_ref()
            .is_some_and(|s| s.expose_secret().starts_with(b"user = \"opencode:")));
    }
    assert!(!all_args(&fake).contains(SECRET));
    assert!(!format!("{report:?}").contains(SECRET));
    assert!(!serde_json::to_string(&report)
        .unwrap_or_default()
        .contains(SECRET));
    assert_eq!(
        report.decision.action().map(|a| a.kind),
        Some(ActionKind::Resume),
        "{:?}",
        report.decision
    );
    assert_eq!(
        report.executed.first().map(|e| e.1.clone()),
        Some(Dispatch::Dispatched)
    );
}

#[test]
fn live_codex_tui_in_ghostty_is_focused() {
    let mut m = codex_machine(Some(codex_row(false, "codex-tui")));
    let tui = Proc::new(
        4242,
        998,
        "ttys000",
        "codex",
        "-/opt/homebrew/bin/codex",
        &format!("-/opt/homebrew/bin/codex resume {CODEX_ID}"),
    );
    let identity = ProcessIdentity {
        pid: 4242,
        start: tui.start,
        boot: BootId(BOOT),
    };
    m.procs.push(tui);
    let cfg = cfg();
    m.file(
        &session_navigation_native::common::codex::lock_path(&cfg.common, CODEX_ID),
        Vec::new(),
    );
    m.run(
        session_navigation_native::common::codex::lock_run(&cfg.common, CODEX_ID),
        Ok(Output::ok("p4242\n")),
    );
    m.apps_json = apps_json(Some(App::Ghostty), &[App::Ghostty]);
    let fake = m.build();
    let focus = machine::script_run(
        scripts::GHOSTTY_FOCUS,
        &["/dev/ttys000".into()],
        cfg.common.timeouts.script_act,
    );
    fake.on(&focus, vec![Ok(Output::ok("focused"))]);
    let mut ports = MacPorts::new(&fake, cfg);
    let report = engine::open(&mut ports, &codex(), &request());
    let step = Step::FocusPane {
        terminal: TerminalKind::Ghostty,
        pane: PaneRef::Tty {
            tty: "/dev/ttys000".into(),
        },
        process: identity,
    };
    assert_eq!(
        report.executed,
        vec![(step, Dispatch::Dispatched)],
        "{:?}",
        report.decision
    );
    assert_eq!(report.outcome, Outcome::Verified);
    assert!(fake.runs().contains(&(focus.program, focus.args)));
    // Focus never reserves.
    assert!(fake.files_under(&cfg_dir()).is_empty());
}

fn cfg_dir() -> String {
    cfg().common.paths.reservation_dir
}

#[test]
fn automation_refusal_releases_the_reservation_and_blocks() {
    let m = codex_machine(Some(codex_row(false, "codex-tui")));
    let fake = m.build();
    let argv = vec![
        PROJECT.to_owned(),
        format!("'/opt/homebrew/bin/codex' 'resume' '{CODEX_ID}'"),
    ];
    fake.on(
        &machine::script_run(
            scripts::GHOSTTY_LAUNCH,
            &argv,
            cfg().common.timeouts.script_act,
        ),
        vec![Ok(Output::exit(
            1,
            "",
            "execution error: Not authorized to send Apple events to Ghostty. (-1743)",
        ))],
    );
    let mut ports = MacPorts::new(&fake, cfg());
    let report = engine::open(&mut ports, &codex(), &request());
    assert_eq!(report.outcome, Outcome::Blocked(Reason::PermissionDenied));
    assert!(fake.files_under(&cfg_dir()).is_empty());
    assert!(!matches!(report.decision.plan, Plan::Unavailable(_)));
}
