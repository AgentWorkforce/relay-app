//! `engine::open` through `LinuxPorts<&FakeSystem>` on fake Linux machines:
//! the whole cycle (observe, decide, reserve, execute with re-checks,
//! observe again, verify) for each host shape the Linux adapter serves.
#![cfg(feature = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod linux_common;

use linux_common::scenes::*;
use linux_common::*;
use session_navigation::engine;
use session_navigation::hosts::{PaneRef, WindowId};
use session_navigation::model::{Host, Reason, TerminalKind};
use session_navigation::plan::{ActionKind, Dispatch, OpenRequest, Outcome, Plan, Step};
use session_navigation_native::common::{kitty, tmux};
use session_navigation_native::linux::LinuxPorts;
use session_navigation_native::system::{Call, FakeSystem};

fn request(terminal: TerminalKind) -> OpenRequest {
    let mut request = OpenRequest::default();
    request.prefs.default_terminal = Some(terminal);
    request
}

fn focus_runs(fake: &FakeSystem) -> Vec<Vec<String>> {
    fake.runs()
        .into_iter()
        .filter(|(_, a)| {
            a.iter()
                .any(|w| w == "focus-window" || w == "windowactivate" || w == "switch-client")
        })
        .map(|(_, a)| a)
        .collect()
}

#[test]
fn live_session_in_a_kitty_pane_is_focused_and_verified() {
    let display = Some("x11");
    let fake = kitty_machine(display).build();
    grok_files(&fake, &[(2601, OPENED_AT)]);
    script_x11(&fake, display, &[FIREFOX_WINDOW, KITTY_WINDOW]);
    fake.on(
        &kitty_ls_run(),
        vec![
            ok(fixture("kitty-ls.json")),
            ok(fixture("kitty-ls.json")),
            ok(fixture("kitty-ls-focused.json")),
        ],
    );
    let focus = kitty::focus_args(KITTY_ADDRESS, 1);
    fake.on_run_owned(KITTY, focus.clone(), ok(""));
    let mut ports = LinuxPorts::new(&fake, cfg(display));
    let report = engine::open(&mut ports, &grok(), &request(TerminalKind::Xterm));
    let pane = PaneRef::KittyWindow {
        socket: KITTY_ADDRESS.into(),
        id: 1,
    };
    let step = Step::FocusPane {
        terminal: TerminalKind::Kitty,
        pane,
        process: identity(2601, 2601),
    };
    assert_eq!(
        report.executed,
        vec![(step, Dispatch::Dispatched)],
        "{:?}",
        report.decision
    );
    assert_eq!(report.outcome, Outcome::Verified);
    assert_eq!(focus_runs(&fake), vec![focus]);
    assert!(fake.spawns().is_empty());
}

#[test]
fn live_session_under_x11_xterm_raises_its_window_after_the_owner_check() {
    let display = Some("x11");
    let fake = xterm_machine(display).build();
    grok_files(&fake, &[(3120, OPENED_AT)]);
    script_x11(&fake, display, &[FIREFOX_WINDOW, XTERM_WINDOW]);
    fake.on(&windowpid_run(XTERM_WINDOW, display), vec![ok("3100\n")]);
    fake.on(&activate_run(XTERM_WINDOW, display), vec![ok("")]);
    let mut ports = LinuxPorts::new(&fake, cfg(display));
    let report = engine::open(&mut ports, &grok(), &request(TerminalKind::Xterm));
    let step = Step::FocusPane {
        terminal: TerminalKind::Xterm,
        pane: PaneRef::X11Window {
            id: WindowId(XTERM_WINDOW),
            owner: identity(3100, 3100),
        },
        process: identity(3120, 3120),
    };
    assert_eq!(
        report.executed,
        vec![(step, Dispatch::Dispatched)],
        "{:?}",
        report.decision
    );
    assert_eq!(report.outcome, Outcome::Verified);
    // The owner check ran before the activation, and both carry DISPLAY.
    let runs = fake.runs();
    let check = runs
        .iter()
        .position(|(_, a)| a.first().map(String::as_str) == Some("getwindowpid"));
    let act = runs
        .iter()
        .position(|(_, a)| a.first().map(String::as_str) == Some("windowactivate"));
    assert!(check.unwrap() < act.unwrap());
    let env_ok = fake.calls().iter().all(|c| match c {
        Call::Run { program, env, .. } if program == XDOTOOL => {
            env.iter().any(|(k, v)| k == "DISPLAY" && v == ":0")
        }
        _ => true,
    });
    assert!(env_ok);
}

#[test]
fn a_reused_window_owner_is_not_raised() {
    let display = Some("x11");
    let fake = xterm_machine(display).build();
    grok_files(&fake, &[(3120, OPENED_AT)]);
    script_x11(&fake, display, &[FIREFOX_WINDOW]);
    // The window id now belongs to another process.
    fake.on(&windowpid_run(XTERM_WINDOW, display), vec![ok("4444\n")]);
    let mut ports = LinuxPorts::new(&fake, cfg(display));
    let report = engine::open(&mut ports, &grok(), &request(TerminalKind::Xterm));
    assert_eq!(report.executed.len(), 1, "{:?}", report.decision);
    assert_eq!(report.executed[0].1, Dispatch::Failed(Reason::TargetGone));
    assert!(focus_runs(&fake).is_empty());
}

#[test]
fn wayland_refuses_with_an_explicit_reason_and_runs_nothing() {
    let display = Some("wayland");
    let fake = xterm_machine(display).build();
    grok_files(&fake, &[(3120, OPENED_AT)]);
    let mut ports = LinuxPorts::new(&fake, cfg(display));
    let report = engine::open(&mut ports, &grok(), &request(TerminalKind::Xterm));
    assert!(report.executed.is_empty(), "{:?}", report.executed);
    assert_eq!(
        report.decision.plan,
        Plan::Unavailable(Reason::WaylandNoRaise)
    );
    assert!(
        matches!(
            report.outcome,
            Outcome::Blocked(Reason::WaylandNoRaise) | Outcome::Failed(Reason::WaylandNoRaise)
        ),
        "{:?}",
        report.outcome
    );
    // Nothing touched X11 or a terminal.
    assert!(fake.runs().iter().all(|(p, _)| p != XDOTOOL && p != WMCTRL));
    assert!(fake.spawns().is_empty());
}

#[test]
fn attached_tmux_selects_the_pane_then_raises_the_clients_window() {
    let display = Some("x11");
    let fake = tmux_machine(true).build();
    grok_files(&fake, &[(3220, OPENED_AT)]);
    script_tmux(&fake);
    script_x11(&fake, display, &[FIREFOX_WINDOW, XTERM_WINDOW]);
    fake.on(&windowpid_run(XTERM_WINDOW, display), vec![ok("3100\n")]);
    fake.on(&activate_run(XTERM_WINDOW, display), vec![ok("")]);
    let select = tmux::focus_args(TMUX_SOCKET, "/dev/pts/2", "$0", "@1", "%1").unwrap();
    fake.on_run_owned(TMUX, select.clone(), ok(""));
    let mut ports = LinuxPorts::new(&fake, cfg(display));
    let report = engine::open(&mut ports, &grok(), &request(TerminalKind::Xterm));
    let pane = PaneRef::TmuxPane {
        socket: TMUX_SOCKET.into(),
        server: identity(3200, 3200),
        pane: "%1".into(),
        client_tty: "/dev/pts/2".into(),
        outer: Box::new(PaneRef::X11Window {
            id: WindowId(XTERM_WINDOW),
            owner: identity(3100, 3100),
        }),
    };
    let step = Step::FocusPane {
        terminal: TerminalKind::Xterm,
        pane,
        process: identity(3220, 3220),
    };
    assert_eq!(
        report.executed,
        vec![(step, Dispatch::Dispatched)],
        "{:?}",
        report.decision
    );
    assert_eq!(report.outcome, Outcome::Verified);
    assert_eq!(
        focus_runs(&fake),
        vec![
            select,
            vec![
                "windowactivate".into(),
                "--sync".into(),
                XTERM_WINDOW.to_string()
            ]
        ]
    );
}

#[test]
fn detached_tmux_attaches_in_a_new_window_without_starting_an_agent() {
    let display = Some("x11");
    let fake = tmux_machine(false).build();
    grok_files(&fake, &[(3240, OPENED_AT)]);
    script_tmux(&fake);
    script_x11(&fake, display, &[FIREFOX_WINDOW]);
    let attach = tmux::attach_args(TMUX_SOCKET, "$1", "@2", "%2").unwrap();
    let mut xterm_args = vec![
        "-e".to_owned(),
        ENV.into(),
        "-C".into(),
        PROJECT.into(),
        TMUX.into(),
    ];
    xterm_args.extend(attach.iter().cloned());
    fake.on_run_owned("/usr/bin/xterm", xterm_args.clone(), ok(""));
    let mut ports = LinuxPorts::new(&fake, cfg(display));
    let report = engine::open(&mut ports, &grok(), &request(TerminalKind::Xterm));
    assert_eq!(
        report.decision.action().map(|a| a.kind),
        Some(ActionKind::Attach),
        "{:?}",
        report.decision
    );
    assert_eq!(report.executed.len(), 1);
    assert_eq!(
        report.executed[0].1,
        Dispatch::Dispatched,
        "{:?}",
        report.executed
    );
    assert_eq!(
        fake.spawns(),
        vec![("/usr/bin/xterm".to_owned(), xterm_args)]
    );
    // An attach reserves nothing.
    assert!(fake
        .files_under(&cfg(display).common.paths.reservation_dir)
        .is_empty());
}

#[test]
fn stopped_session_resumes_in_its_original_terminal() {
    let display = Some("x11");
    let fake = Machine::new(display).build();
    grok_files(&fake, &[]);
    script_x11(&fake, display, &[FIREFOX_WINDOW]);
    let konsole_args: Vec<String> = ["--workdir", PROJECT, "-e", GROK_BIN, "--resume", GROK_ID]
        .map(String::from)
        .to_vec();
    fake.on_run_owned("/usr/bin/konsole", konsole_args.clone(), ok(""));
    let mut ports = LinuxPorts::with_history(
        &fake,
        cfg(display),
        Seen(Host::Terminal(TerminalKind::Konsole)),
    );
    let report = engine::open(&mut ports, &grok(), &request(TerminalKind::Xterm));
    assert_eq!(
        report.decision.action().map(|a| a.kind),
        Some(ActionKind::Resume),
        "{:?}",
        report.decision
    );
    assert_eq!(report.executed.len(), 1);
    assert!(matches!(
        &report.executed[0].0,
        Step::Launch {
            terminal: TerminalKind::Konsole,
            ..
        }
    ));
    assert_eq!(report.executed[0].1, Dispatch::Dispatched);
    assert_eq!(report.outcome, Outcome::DispatchedUnverified);
    assert_eq!(
        fake.spawns(),
        vec![("/usr/bin/konsole".to_owned(), konsole_args)]
    );
    // The terminal gets the desktop session's variables, nothing else.
    let env = fake
        .calls()
        .into_iter()
        .find_map(|c| match c {
            Call::Spawn { env, .. } => Some(env),
            _ => None,
        })
        .unwrap();
    assert!(env.iter().any(|(k, v)| k == "DISPLAY" && v == ":0"));
    assert!(env.iter().all(|(k, _)| k != "SECRET_TOKEN"));
    // The launch is reserved until a later observation resolves it.
    assert_eq!(
        fake.files_under(&cfg(display).common.paths.reservation_dir)
            .len(),
        1
    );
}

#[test]
fn a_reused_pid_is_never_focused() {
    let display = Some("x11");
    let mut m = kitty_machine(display);
    // Grok 2601 exited; its pid now runs vim, started after the record.
    m.push(
        Proc::new(2601, 2481, "/usr/bin/vim", &["vim"])
            .tty(1)
            .started(9000),
    );
    let fake = m.build();
    grok_files(&fake, &[(2601, OPENED_AT)]);
    script_x11(&fake, display, &[KITTY_WINDOW]);
    fake.on(&kitty_ls_run(), vec![ok(fixture("kitty-ls.json"))]);
    let mut ports = LinuxPorts::new(&fake, cfg(display));
    let report = engine::open(&mut ports, &grok(), &request(TerminalKind::Kitty));
    assert!(
        report
            .executed
            .iter()
            .all(|(s, _)| !matches!(s, Step::FocusPane { .. })),
        "{:?}",
        report.executed
    );
    assert!(focus_runs(&fake).is_empty());
}
