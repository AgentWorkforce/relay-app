//! `LinuxExecutor` step by step: every identity re-checked before acting,
//! each terminal's own launch argv, and the steps Linux does not support.
#![cfg(feature = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod linux_common;

use std::collections::BTreeMap;

use linux_common::scenes::*;
use linux_common::*;
use session_navigation::hosts::{PaneRef, WindowId};
use session_navigation::model::{App, ProcessIdentity, Reason, Runtime, TerminalKind};
use session_navigation::plan::{Command, Dispatch, LaunchContext, Step};
use session_navigation::ports::Execute;
use session_navigation_native::common::{kitty, wezterm};
use session_navigation_native::linux::LinuxExecutor;
use session_navigation_native::system::{Call, FakeSystem};

fn exec(fake: &FakeSystem, display: Option<&str>, step: &Step) -> Dispatch {
    LinuxExecutor::new(fake, cfg(display)).execute(step)
}

fn kitty_focus(process: ProcessIdentity) -> Step {
    Step::FocusPane {
        terminal: TerminalKind::Kitty,
        pane: PaneRef::KittyWindow {
            socket: KITTY_ADDRESS.into(),
            id: 1,
        },
        process,
    }
}

fn acted(fake: &FakeSystem) -> bool {
    fake.runs().iter().any(|(p, a)| {
        p == KILL
            || a.iter()
                .any(|w| w == "focus-window" || w == "windowactivate" || w == "activate-pane")
    }) || !fake.spawns().is_empty()
}

#[test]
fn stop_signals_only_the_exact_agent_of_the_person() {
    let mut m = xterm_machine(Some("x11"));
    m.push(Proc::new(3130, 3110, "/usr/bin/vim", &["vim"]).tty(3));
    let mut other = grok_proc(3140, 3110, 3);
    other.ruid = 1001;
    m.push(other);
    let fake = m.build();
    fake.on_run_owned(KILL, vec!["-TERM".into(), "3120".into()], ok(""));
    let stop = |p: ProcessIdentity| exec(&fake, Some("x11"), &Step::Stop { process: p });
    // Another start time (a reused pid), another boot, a non-agent, another uid.
    assert_eq!(
        stop(identity(3120, 9999)),
        Dispatch::Failed(Reason::TargetGone)
    );
    let mut other_boot = identity(3120, 3120);
    other_boot.boot.0 ^= 1;
    assert_eq!(stop(other_boot), Dispatch::Failed(Reason::TargetGone));
    assert_eq!(
        stop(identity(3130, 3130)),
        Dispatch::Failed(Reason::TargetGone)
    );
    assert_eq!(
        stop(identity(3140, 3140)),
        Dispatch::Failed(Reason::TargetGone)
    );
    assert!(!acted(&fake));
    assert_eq!(stop(identity(3120, 3120)), Dispatch::Dispatched);
    assert_eq!(
        fake.runs()
            .into_iter()
            .filter(|(p, _)| p == KILL)
            .collect::<Vec<_>>(),
        vec![(KILL.to_owned(), vec!["-TERM".to_owned(), "3120".to_owned()])]
    );
}

#[test]
fn kitty_focus_rechecks_identity_socket_and_pane() {
    let display = Some("wayland");
    let fake = kitty_machine(display).build();
    fake.on(&kitty_ls_run(), vec![ok(fixture("kitty-ls.json"))]);
    // A reused pid is refused before Kitty is even asked.
    assert_eq!(
        exec(&fake, display, &kitty_focus(identity(2601, 1))),
        Dispatch::Failed(Reason::TargetGone)
    );
    assert!(fake.runs().iter().all(|(p, _)| p != KITTY));
    // The pane listed under id 1 holds bash 2481, the agent's parent: yes.
    let focus = kitty::focus_args(KITTY_ADDRESS, 1);
    fake.on_run_owned(KITTY, focus.clone(), ok(""));
    assert_eq!(
        exec(&fake, display, &kitty_focus(identity(2601, 2601))),
        Dispatch::Dispatched
    );
    // bash 2490 is not in window 1.
    let wrong = exec(&fake, display, &kitty_focus(identity(2490, 2490)));
    assert_eq!(wrong, Dispatch::Failed(Reason::TargetGone));
    let focused: Vec<_> = fake
        .runs()
        .into_iter()
        .filter(|(_, a)| *a == focus)
        .collect();
    assert_eq!(focused.len(), 1);
}

#[test]
fn kitty_stale_socket_is_target_gone_and_refusal_is_refused() {
    let display = Some("wayland");
    let fake = kitty_machine(display).build();
    fake.on(
        &kitty_ls_run(),
        vec![fail("Error: Failed to connect to unix:/run/user/1000/kitty-2470: No such file or directory\n")],
    );
    let step = kitty_focus(identity(2601, 2601));
    assert_eq!(
        exec(&fake, display, &step),
        Dispatch::Failed(Reason::TargetGone)
    );
    fake.on(
        &kitty_ls_run(),
        vec![fail(
            "Error: Remote control is disabled. Add allow_remote_control to your kitty.conf\n",
        )],
    );
    assert_eq!(exec(&fake, display, &step), Dispatch::Refused);
    assert!(!acted(&fake));
}

#[test]
fn wezterm_focus_matches_the_tty_and_names_the_socket() {
    let display = Some("wayland");
    let mut m = Machine::new(display);
    m.push(Proc::new(
        4100,
        1600,
        "/usr/bin/wezterm-gui",
        &["wezterm-gui"],
    ));
    m.push(Proc::new(4110, 4100, "/usr/bin/bash", &["bash"]).tty(4));
    m.push(grok_proc(4120, 4110, 4));
    let fake = m.build();
    fake.on_run_owned(
        WEZTERM,
        wezterm::list_args(),
        ok(fixture("wezterm-list.json")),
    );
    fake.on_run_owned(WEZTERM, wezterm::activate_args(1), ok(""));
    let step = |id| Step::FocusPane {
        terminal: TerminalKind::WezTerm,
        pane: PaneRef::WezTermPane {
            socket: WEZ_SOCKET.into(),
            id,
        },
        process: identity(4120, 4120),
    };
    // Pane 0 is on pts/3, not the agent's pts/4.
    assert_eq!(
        exec(&fake, display, &step(0)),
        Dispatch::Failed(Reason::TargetGone)
    );
    assert_eq!(exec(&fake, display, &step(1)), Dispatch::Dispatched);
    let activate = fake.calls().into_iter().find_map(|c| match c {
        Call::Run { args, env, .. } if args == wezterm::activate_args(1) => Some(env),
        _ => None,
    });
    assert_eq!(
        activate,
        Some(vec![(
            "WEZTERM_UNIX_SOCKET".to_owned(),
            WEZ_SOCKET.to_owned()
        )])
    );
    fake.set_run(
        WEZTERM,
        wezterm::list_args(),
        fail("failed to connect: Connection refused\n"),
    );
    assert_eq!(
        exec(&fake, display, &step(1)),
        Dispatch::Failed(Reason::TargetGone)
    );
}

#[test]
fn x11_window_steps_need_x11_and_the_owner() {
    let step = Step::FocusPane {
        terminal: TerminalKind::Xterm,
        pane: PaneRef::X11Window {
            id: WindowId(XTERM_WINDOW),
            owner: identity(3100, 3100),
        },
        process: identity(3120, 3120),
    };
    let fake = xterm_machine(Some("wayland")).build();
    assert_eq!(
        exec(&fake, Some("wayland"), &step),
        Dispatch::Failed(Reason::PlatformUnsupported)
    );
    let fake = xterm_machine(Some("x11")).build();
    // The owner restarted: same pid, another start.
    let restarted = Step::FocusPane {
        terminal: TerminalKind::Xterm,
        pane: PaneRef::X11Window {
            id: WindowId(XTERM_WINDOW),
            owner: identity(3100, 1),
        },
        process: identity(3120, 3120),
    };
    assert_eq!(
        exec(&fake, Some("x11"), &restarted),
        Dispatch::Failed(Reason::TargetGone)
    );
    // `_NET_WM_PID` unreadable: refused.
    assert_eq!(
        exec(&fake, Some("x11"), &step),
        Dispatch::Failed(Reason::TargetGone)
    );
    assert!(!acted(&fake));
}

#[test]
fn tmux_focus_refuses_a_restarted_server() {
    let display = Some("x11");
    let fake = tmux_machine(true).build();
    script_tmux(&fake);
    let step = Step::FocusPane {
        terminal: TerminalKind::Xterm,
        pane: PaneRef::TmuxPane {
            socket: TMUX_SOCKET.into(),
            server: identity(3200, 1),
            pane: "%1".into(),
            client_tty: "/dev/pts/2".into(),
            outer: Box::new(PaneRef::X11Window {
                id: WindowId(XTERM_WINDOW),
                owner: identity(3100, 3100),
            }),
        },
        process: identity(3220, 3220),
    };
    assert_eq!(
        exec(&fake, display, &step),
        Dispatch::Failed(Reason::TargetGone)
    );
    assert!(fake
        .runs()
        .iter()
        .all(|(_, a)| !a.contains(&"switch-client".to_owned())));
}

fn grok_command() -> Command {
    Command {
        program: GROK_BIN.into(),
        args: vec!["--resume".into(), GROK_ID.into()],
        cwd: PROJECT.into(),
        env: BTreeMap::from([("GROK_HOME".to_owned(), "/home/example/.grok".to_owned())]),
    }
}

fn launch(terminal: TerminalKind, command: Command) -> Step {
    Step::Launch {
        terminal,
        command,
        context: LaunchContext::native(),
    }
}

#[test]
fn each_terminal_gets_its_own_argv_form() {
    let mut m = Machine::new(Some("x11"));
    m.programs
        .extend(["alacritty", "gnome-terminal", "ghostty"]);
    let env_words = |rest: &[&str]| {
        let mut v = vec![
            ENV,
            "GROK_HOME=/home/example/.grok",
            GROK_BIN,
            "--resume",
            GROK_ID,
        ];
        v.splice(0..0, rest.iter().copied());
        v.into_iter().map(String::from).collect::<Vec<_>>()
    };
    let cases: Vec<(TerminalKind, &str, Vec<String>)> = vec![
        (
            TerminalKind::Kitty,
            KITTY,
            [
                "--directory",
                PROJECT,
                "-o",
                "env=GROK_HOME=/home/example/.grok",
                GROK_BIN,
                "--resume",
                GROK_ID,
            ]
            .map(String::from)
            .to_vec(),
        ),
        (
            TerminalKind::WezTerm,
            WEZTERM,
            env_words(&["start", "--cwd", PROJECT, "--"]),
        ),
        (
            TerminalKind::Alacritty,
            "/usr/bin/alacritty",
            env_words(&["--working-directory", PROJECT, "-e"]),
        ),
        (
            TerminalKind::GnomeTerminal,
            "/usr/bin/gnome-terminal",
            env_words(&["--working-directory=/home/example/code/project", "--"]),
        ),
        (
            TerminalKind::Konsole,
            "/usr/bin/konsole",
            env_words(&["--workdir", PROJECT, "-e"]),
        ),
        (
            TerminalKind::Ghostty,
            "/usr/bin/ghostty",
            env_words(&["--working-directory=/home/example/code/project", "-e"]),
        ),
        (
            TerminalKind::Xterm,
            "/usr/bin/xterm",
            [
                "-e",
                ENV,
                "-C",
                PROJECT,
                "GROK_HOME=/home/example/.grok",
                GROK_BIN,
                "--resume",
                GROK_ID,
            ]
            .map(String::from)
            .to_vec(),
        ),
    ];
    for (terminal, program, args) in cases {
        let fake = m.build();
        grok_files(&fake, &[]);
        fake.on_run_owned(program, args.clone(), ok(""));
        let result = exec(&fake, Some("x11"), &launch(terminal, grok_command()));
        assert_eq!(result, Dispatch::Dispatched, "{terminal:?}");
        assert_eq!(
            fake.spawns(),
            vec![(program.to_owned(), args)],
            "{terminal:?}"
        );
        assert!(
            fake.runs().is_empty(),
            "{terminal:?}: a launch never waits on the terminal"
        );
    }
}

#[test]
fn launches_are_rechecked_before_anything_starts() {
    let fake = Machine::new(Some("x11")).build();
    grok_files(&fake, &[]);
    let refused = |step: &Step| exec(&fake, Some("x11"), step);
    let mut bad_env = grok_command();
    bad_env.env.insert("LD_PRELOAD".into(), "/tmp/x.so".into());
    assert_eq!(
        refused(&launch(TerminalKind::Konsole, bad_env)),
        Dispatch::Failed(Reason::DispatchFailed)
    );
    let mut relative = grok_command();
    relative.program = "grok".into();
    assert_eq!(
        refused(&launch(TerminalKind::Konsole, relative)),
        Dispatch::Failed(Reason::DispatchFailed)
    );
    let mut missing_cwd = grok_command();
    missing_cwd.cwd = "/home/example/gone".into();
    assert_eq!(
        refused(&launch(TerminalKind::Konsole, missing_cwd)),
        Dispatch::Failed(Reason::DispatchFailed)
    );
    let mut not_agent = grok_command();
    not_agent.program = "/usr/bin/env".into();
    assert_eq!(
        refused(&launch(TerminalKind::Konsole, not_agent)),
        Dispatch::Failed(Reason::DispatchFailed)
    );
    // tmux only ever attaches.
    fake.add_executable(TMUX, 1);
    let tmux_cmd = Command {
        program: TMUX.into(),
        args: vec![
            "-S".into(),
            TMUX_SOCKET.into(),
            "send-keys".into(),
            "rm -rf ~".into(),
        ],
        cwd: PROJECT.into(),
        env: BTreeMap::new(),
    };
    assert_eq!(
        refused(&launch(TerminalKind::Konsole, tmux_cmd)),
        Dispatch::Failed(Reason::DispatchFailed)
    );
    let wsl = Step::Launch {
        terminal: TerminalKind::Konsole,
        command: grok_command(),
        context: LaunchContext {
            runtime: Runtime::Wsl {
                distro: "Ubuntu".into(),
            },
            profile: None,
        },
    };
    assert_eq!(refused(&wsl), Dispatch::Failed(Reason::PlatformUnsupported));
    // Not installed, or no display: nothing to open a window with.
    assert_eq!(
        refused(&launch(TerminalKind::Alacritty, grok_command())),
        Dispatch::Failed(Reason::HostUnsupported)
    );
    assert_eq!(
        exec(&fake, None, &launch(TerminalKind::Konsole, grok_command())),
        Dispatch::Failed(Reason::HostUnsupported)
    );
    assert!(!acted(&fake));
}

#[test]
fn steps_linux_does_not_support() {
    let fake = Machine::new(Some("x11")).build();
    let run = |step: Step| exec(&fake, Some("x11"), &step);
    assert_eq!(
        run(Step::OpenUrl {
            url: format!("claude://resume?session={GROK_ID}"),
            bundle: "com.anthropic.claudefordesktop".into()
        }),
        Dispatch::Failed(Reason::PlatformUnsupported)
    );
    assert_eq!(
        run(Step::ActivateApp { app: App::Kitty }),
        Dispatch::Failed(Reason::HostUnsupported)
    );
    assert_eq!(
        run(Step::FocusPane {
            terminal: TerminalKind::Konsole,
            pane: PaneRef::Tty {
                tty: "/dev/pts/1".into()
            },
            process: identity(1600, 5),
        }),
        Dispatch::Failed(Reason::HostUnsupported)
    );
    assert_eq!(
        run(Step::AwaitExit {
            process: identity(1600, 5)
        }),
        Dispatch::Dispatched
    );
    assert!(fake.runs().is_empty());
}
