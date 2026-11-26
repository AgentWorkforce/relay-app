//! tmux focus and attach, the `open`-based launches (Kitty, WezTerm,
//! Alacritty), and the decision the core takes on what the mac observer
//! reports for a Grok session inside tmux. Documentation-derived fixtures;
//! tmux and these terminals were never run.
#![cfg(feature = "macos")]
mod common;
mod macos_world;

use std::collections::BTreeMap;

use common::*;
use macos_world::*;
use session_navigation::decide::decide;
use session_navigation::hosts::PaneRef;
use session_navigation::model::{Provider, Reason, SessionRef, TerminalKind};
use session_navigation::plan::{Command, Dispatch, LaunchContext, OpenRequest, Plan, Step};
use session_navigation::ports::{Execute, Observe};
use session_navigation_native::common::tmux;
use session_navigation_native::macos::execute::OPEN;
use session_navigation_native::macos::{machine, scripts};
use session_navigation_native::system::{FakeSystem, Output};
use session_navigation_native::{MacExecutor, MacObserver};

fn exec(fake: &FakeSystem, step: &Step) -> Dispatch {
    MacExecutor::new(fake, cfg()).execute(step)
}

fn tmux_ref(outer: PaneRef) -> PaneRef {
    PaneRef::TmuxPane {
        socket: TMUX_SOCK.into(),
        server: ident(4000),
        pane: "%0".into(),
        client_tty: "/dev/ttys000".into(),
        outer: Box::new(outer),
    }
}

fn outer() -> PaneRef {
    PaneRef::Tty {
        tty: "/dev/ttys000".into(),
    }
}

fn step(pane: PaneRef) -> Step {
    Step::FocusPane {
        terminal: TerminalKind::Ghostty,
        pane,
        process: ident(4003),
    }
}

fn ghostty_focus() -> session_navigation_native::system::Run {
    machine::script_run(
        scripts::GHOSTTY_FOCUS,
        &["/dev/ttys000".to_owned()],
        cfg().common.timeouts.script_act,
    )
}

fn tmux_runs(fake: &FakeSystem) -> Vec<Vec<String>> {
    fake.runs()
        .into_iter()
        .filter(|(p, _)| p == TMUX)
        .map(|(_, a)| a)
        .filter(|a| a.get(2).map(String::as_str) != Some("-V"))
        .collect()
}

fn select_args() -> Vec<String> {
    tmux::focus_args(TMUX_SOCK, "/dev/ttys000", "$0", "@0", "%0").unwrap_or_default()
}

#[test]
fn tmux_focus_checks_server_pane_and_client_then_focuses_the_outer_pane() {
    let mut m = host_machine();
    m.run(tmux_run(select_args()), Ok(Output::ok("")));
    m.run(ghostty_focus(), Ok(Output::ok("focused\n")));
    let fake = m.build();
    assert_eq!(exec(&fake, &step(tmux_ref(outer()))), Dispatch::Dispatched);
    assert_eq!(
        tmux_runs(&fake),
        vec![
            tmux::server_pid_args(TMUX_SOCK),
            tmux::list_panes_args(TMUX_SOCK),
            tmux::list_clients_args(TMUX_SOCK),
            select_args(),
        ]
    );
    assert_eq!(
        select_args(),
        [
            "-S",
            TMUX_SOCK,
            "switch-client",
            "-c",
            "/dev/ttys000",
            "-t",
            "$0",
            ";",
            "select-window",
            "-t",
            "@0",
            ";",
            "select-pane",
            "-t",
            "%0",
        ]
        .map(String::from)
        .to_vec()
    );
    let g = ghostty_focus();
    assert_eq!(fake.runs().last().cloned(), Some((g.program, g.args)));
}

#[test]
fn tmux_focus_refuses_every_stale_identity_before_selecting() {
    let gone = Dispatch::Failed(Reason::TargetGone);
    // The server restarted (another start time): nothing is asked.
    let fake = host_machine().build();
    let mut restarted = tmux_ref(outer());
    if let PaneRef::TmuxPane { server, .. } = &mut restarted {
        server.start += 3;
    }
    assert_eq!(exec(&fake, &step(restarted)), gone);
    assert!(tmux_runs(&fake).is_empty());

    // The socket now answers for another server.
    let mut m = host_machine();
    m.run(
        tmux_run(tmux::server_pid_args(TMUX_SOCK)),
        Ok(Output::ok("4002\n")),
    );
    let fake = m.build();
    assert_eq!(exec(&fake, &step(tmux_ref(outer()))), gone);
    assert!(!tmux_runs(&fake).contains(&select_args()));

    // The pane id was recycled onto another tty.
    let mut m = host_machine();
    m.run(
        tmux_run(tmux::list_panes_args(TMUX_SOCK)),
        Ok(Output::ok("%0\t/dev/ttys008\t$0\t@0\n")),
    );
    assert_eq!(exec(&m.build(), &step(tmux_ref(outer()))), gone);

    // The client detached, or now shows another session.
    for clients in ["", "/dev/ttys000\t4001\t$1\t%2\n"] {
        let mut m = host_machine();
        m.run(
            tmux_run(tmux::list_clients_args(TMUX_SOCK)),
            Ok(Output::ok(clients)),
        );
        let fake = m.build();
        assert_eq!(exec(&fake, &step(tmux_ref(outer()))), gone, "{clients:?}");
        assert!(!tmux_runs(&fake).contains(&select_args()));
    }

    // The server went away under the socket.
    let mut m = host_machine();
    m.run(
        tmux_run(tmux::server_pid_args(TMUX_SOCK)),
        Ok(Output::exit(1, "", TMUX_GONE)),
    );
    assert_eq!(exec(&m.build(), &step(tmux_ref(outer()))), gone);

    // Nested tmux is never acted on.
    let fake = host_machine().build();
    assert_eq!(
        exec(&fake, &step(tmux_ref(tmux_ref(outer())))),
        Dispatch::Failed(Reason::HostUnsupported)
    );
    assert!(fake.runs().is_empty());
}

fn launch(terminal: TerminalKind, command: Command) -> Step {
    Step::Launch {
        terminal,
        command,
        context: LaunchContext::native(),
    }
}

#[test]
fn tmux_launches_only_attach() {
    let attach = tmux::attach_args(TMUX_SOCK, "$0", "@0", "%0").unwrap_or_default();
    assert!(tmux::is_attach_args(&attach));
    let mut m = host_machine();
    m.dir(PROJECT);
    let cmd = |args: Vec<String>| Command {
        program: TMUX.into(),
        args,
        cwd: PROJECT.into(),
        env: BTreeMap::new(),
    };
    let line =
        session_navigation_native::common::quote::command_line(TMUX, &attach).unwrap_or_default();
    let script = machine::script_run(
        scripts::GHOSTTY_LAUNCH,
        &[PROJECT.to_owned(), line],
        cfg().common.timeouts.script_act,
    );
    m.run(script, Ok(Output::ok("tty:/dev/ttys013\n")));
    let fake = m.build();
    assert_eq!(
        exec(&fake, &launch(TerminalKind::Ghostty, cmd(attach.clone()))),
        Dispatch::Opened {
            tty: "/dev/ttys013".into()
        }
    );
    // Anything else tmux could do (type, run, kill) is refused unrun.
    for args in [
        vec![
            "-S".into(),
            TMUX_SOCK.into(),
            "send-keys".into(),
            "-t".into(),
            "%0".into(),
            "rm -rf ~".into(),
            "Enter".into(),
        ],
        vec!["new-session".into(), "sh".into()],
        {
            let mut a = attach.clone();
            a[12] = "$0;kill-server".into();
            a
        },
    ] {
        let fake = m.build();
        assert_eq!(
            exec(&fake, &launch(TerminalKind::Ghostty, cmd(args))),
            Dispatch::Failed(Reason::DispatchFailed)
        );
        assert!(fake.runs().is_empty());
    }
}

#[test]
fn kitty_wezterm_and_alacritty_launch_through_open_with_argv() {
    let mut m = Machine::new();
    m.dir(PROJECT);
    let command = Command {
        program: "/opt/homebrew/bin/codex".into(),
        args: vec!["resume".into(), CODEX_ID.into(), "a b'c".into()],
        cwd: PROJECT.into(),
        env: BTreeMap::from([(
            "CODEX_HOME".to_owned(),
            "/Users/example/.codex-alt".to_owned(),
        )]),
    };
    let tail = || {
        vec![
            "/opt/homebrew/bin/codex".to_owned(),
            "resume".into(),
            CODEX_ID.into(),
            "a b'c".into(),
        ]
    };
    let env_tail = || {
        let mut v = vec![
            "/usr/bin/env".to_owned(),
            "CODEX_HOME=/Users/example/.codex-alt".into(),
        ];
        v.extend(tail());
        v
    };
    let head = |bundle: &str| vec!["-n".to_owned(), "-b".into(), bundle.into(), "--args".into()];
    let mut kitty = head("net.kovidgoyal.kitty");
    kitty.extend([
        "--directory".into(),
        PROJECT.into(),
        "-o".into(),
        "env=CODEX_HOME=/Users/example/.codex-alt".into(),
    ]);
    kitty.extend(tail());
    let mut wez = head("com.github.wez.wezterm");
    wez.extend(["start".into(), "--cwd".into(), PROJECT.into(), "--".into()]);
    wez.extend(env_tail());
    let mut ala = head("org.alacritty");
    ala.extend(["--working-directory".into(), PROJECT.into(), "-e".into()]);
    ala.extend(env_tail());
    for (terminal, args) in [
        (TerminalKind::Kitty, kitty),
        (TerminalKind::WezTerm, wez),
        (TerminalKind::Alacritty, ala),
    ] {
        let fake = m.build();
        fake.on_run_owned(OPEN, args.clone(), Ok(Output::ok("")));
        assert_eq!(
            exec(&fake, &launch(terminal, command.clone())),
            Dispatch::Dispatched
        );
        assert_eq!(fake.runs(), vec![(OPEN.to_owned(), args)], "{terminal:?}");
    }
    // An environment key outside the tool's allowlist: nothing runs.
    let mut bad = command;
    bad.env
        .insert("DYLD_INSERT_LIBRARIES".into(), "/tmp/x".into());
    let fake = m.build();
    assert_eq!(
        exec(&fake, &launch(TerminalKind::Kitty, bad)),
        Dispatch::Failed(Reason::DispatchFailed)
    );
    assert!(fake.runs().is_empty());
}

#[test]
fn a_grok_session_in_tmux_is_focused_through_its_client() {
    let mut m = host_machine();
    m.dir(PROJECT);
    m.file(
        &format!("{HOME}/.grok/active_sessions.json"),
        include_str!("fixtures/grok/active_sessions.json"),
    );
    m.file(
        &format!(
            "{HOME}/.grok/sessions/%2FUsers%2Fexample%2Fcode%2Fproject/{GROK_ID}/summary.json"
        ),
        include_str!("fixtures/grok/summary.json"),
    );
    let fake = m.build();
    let session = SessionRef::new(Provider::Grok, GROK_ID);
    let snap = MacObserver::new(&fake, cfg()).observe(&session);
    let decision = decide(&snap, &OpenRequest::default());
    let Plan::Act(action) = &decision.plan else {
        panic!("{decision:?}");
    };
    assert_eq!(action.steps, vec![step(tmux_ref(outer()))], "{decision:?}");
}
