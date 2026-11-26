//! The executor: the exact program and argv of every step, refusals before
//! any side effect, Automation refusal, and the environment allowlist.
#![cfg(feature = "macos")]
mod common;

use std::collections::BTreeMap;

use common::*;
use session_navigation::hosts::PaneRef;
use session_navigation::model::{App, BootId, ProcessIdentity, Reason, TerminalKind};
use session_navigation::plan::{Command, Dispatch, LaunchContext, Step};
use session_navigation::ports::Execute;
use session_navigation_native::macos::execute::{MacExecutor, KILL, OPEN};
use session_navigation_native::macos::machine::{self, OSASCRIPT};
use session_navigation_native::macos::scripts;
use session_navigation_native::system::{FakeSystem, Output, Run, RunError};

const CODEX: &str = "/opt/homebrew/bin/codex";

fn tui() -> Proc {
    Proc::new(
        4242,
        998,
        "ttys000",
        "codex",
        &format!("-{CODEX}"),
        &format!("-{CODEX} resume {CODEX_ID}"),
    )
}

fn tui_identity() -> ProcessIdentity {
    ProcessIdentity {
        pid: 4242,
        start: tui().start,
        boot: BootId(BOOT),
    }
}

fn machine_with_tui() -> Machine {
    let mut m = Machine::new();
    m.procs.push(tui());
    m.dir(PROJECT);
    m
}

fn exec(fake: &FakeSystem, step: &Step) -> Dispatch {
    MacExecutor::new(fake, cfg()).execute(step)
}

fn osascript_runs(fake: &FakeSystem) -> Vec<Vec<String>> {
    fake.runs()
        .into_iter()
        .filter(|(p, _)| p == OSASCRIPT)
        .map(|(_, a)| a)
        .collect()
}

fn kills(fake: &FakeSystem) -> usize {
    fake.runs().iter().filter(|(p, _)| p == KILL).count()
}

fn refused() -> Output {
    Output::exit(
        1,
        "",
        "0:120: execution error: Not authorized to send Apple events to Ghostty. (-1743)\n",
    )
}

#[test]
fn open_url_and_activate_run_exactly_open_b() {
    let fake = FakeSystem::new();
    let url = format!("claude://resume?session={CLAUDE_ID}");
    fake.on(
        &Run::new(OPEN, &["-b", "com.anthropic.claudefordesktop", &url]),
        vec![Ok(Output::ok(""))],
    );
    let step = Step::OpenUrl {
        url: url.clone(),
        bundle: "com.anthropic.claudefordesktop".into(),
    };
    assert_eq!(exec(&fake, &step), Dispatch::Dispatched);
    assert_eq!(
        fake.runs(),
        vec![(
            OPEN.to_owned(),
            vec![
                "-b".into(),
                "com.anthropic.claudefordesktop".into(),
                url.clone()
            ]
        )]
    );
    fake.on(
        &Run::new(OPEN, &["-b", "com.anthropic.claudefordesktop", &url]),
        vec![Ok(Output::exit(1, "", "LSOpen failed"))],
    );
    assert_eq!(exec(&fake, &step), Dispatch::Failed(Reason::DispatchFailed));

    let fake = FakeSystem::new();
    fake.on(
        &Run::new(OPEN, &["-b", "com.mitchellh.ghostty"]),
        vec![Ok(Output::ok(""))],
    );
    assert_eq!(
        exec(&fake, &Step::ActivateApp { app: App::Ghostty }),
        Dispatch::Dispatched
    );
    assert_eq!(
        fake.runs(),
        vec![(
            OPEN.to_owned(),
            vec!["-b".into(), "com.mitchellh.ghostty".into()]
        )]
    );
}

#[test]
fn hostile_urls_never_reach_open() {
    let fake = FakeSystem::new();
    for (url, bundle) in [
        (
            "claude://resume?session=a b",
            "com.anthropic.claudefordesktop",
        ),
        (
            "claude://resume?session=a\nb",
            "com.anthropic.claudefordesktop",
        ),
        ("opencode://x", "ai.opencode.desktop"),
        ("https://evil.example", "com.openai.codex"),
        ("-a/Applications/Calculator.app", "com.openai.codex"),
        ("codex://threads/x", "com.anthropic.claudefordesktop"),
        ("claude://x\"y", "com.anthropic.claudefordesktop"),
    ] {
        let step = Step::OpenUrl {
            url: url.into(),
            bundle: bundle.into(),
        };
        assert_eq!(
            exec(&fake, &step),
            Dispatch::Failed(Reason::DispatchFailed),
            "{url}"
        );
    }
    assert!(fake.runs().is_empty());
}

fn focus(terminal: TerminalKind, process: ProcessIdentity) -> Step {
    Step::FocusPane {
        terminal,
        pane: PaneRef::Tty {
            tty: "/dev/ttys000".into(),
        },
        process,
    }
}

#[test]
fn focus_checks_identity_then_runs_the_script_with_the_tty_as_argv() {
    for (terminal, script) in [
        (TerminalKind::Ghostty, scripts::GHOSTTY_FOCUS),
        (TerminalKind::AppleTerminal, scripts::TERMINAL_FOCUS),
    ] {
        let m = machine_with_tui();
        let fake = m.build();
        let run = machine::script_run(
            script,
            &["/dev/ttys000".into()],
            cfg().common.timeouts.script_act,
        );
        fake.on(&run, vec![Ok(Output::ok("focused\n"))]);
        assert_eq!(
            exec(&fake, &focus(terminal, tui_identity())),
            Dispatch::Dispatched
        );
        assert_eq!(
            osascript_runs(&fake),
            vec![vec![
                "-e".to_owned(),
                script.to_owned(),
                "/dev/ttys000".to_owned()
            ]]
        );
        // The script text is a constant: the tty is never inside it.
        assert!(!script.contains("ttys000"));
        for (output, expected) in [
            (refused(), Dispatch::Refused),
            (
                Output::exit(1, "", "execution error: pane unavailable (7001)"),
                Dispatch::Failed(Reason::TargetGone),
            ),
            (
                Output::exit(1, "", "execution error: pane not confirmed (7002)"),
                Dispatch::Failed(Reason::WrongDestination),
            ),
            (
                Output::exit(1, "", "execution error: other (-1728)"),
                Dispatch::Failed(Reason::DispatchFailed),
            ),
            (
                Output::ok("something else"),
                Dispatch::Failed(Reason::DispatchFailed),
            ),
        ] {
            let fake = m.build();
            fake.on(&run, vec![Ok(output)]);
            assert_eq!(exec(&fake, &focus(terminal, tui_identity())), expected);
        }
    }
}

#[test]
fn focus_on_a_gone_target_touches_no_terminal() {
    let m = machine_with_tui();
    let other_start = ProcessIdentity {
        start: tui().start + 1,
        ..tui_identity()
    };
    let other_boot = ProcessIdentity {
        boot: BootId(BOOT + 1),
        ..tui_identity()
    };
    let gone = ProcessIdentity {
        pid: 9999,
        ..tui_identity()
    };
    for identity in [other_start, other_boot, gone] {
        let fake = m.build();
        assert_eq!(
            exec(&fake, &focus(TerminalKind::Ghostty, identity)),
            Dispatch::Failed(Reason::TargetGone)
        );
        assert!(osascript_runs(&fake).is_empty());
    }
    // Same process, different tty.
    let fake = m.build();
    let step = Step::FocusPane {
        terminal: TerminalKind::Ghostty,
        pane: PaneRef::Tty {
            tty: "/dev/ttys009".into(),
        },
        process: tui_identity(),
    };
    assert_eq!(exec(&fake, &step), Dispatch::Failed(Reason::TargetGone));
    assert!(osascript_runs(&fake).is_empty());
    // Unreadable table.
    let mut broken = machine_with_tui();
    broken.ps_broken = true;
    let fake = broken.build();
    assert!(matches!(
        exec(&fake, &focus(TerminalKind::Ghostty, tui_identity())),
        Dispatch::Failed(_)
    ));
    assert!(osascript_runs(&fake).is_empty());
}

fn command(env: &[(&str, &str)], args: &[&str]) -> Command {
    Command {
        program: CODEX.into(),
        args: args.iter().map(|a| (*a).to_owned()).collect(),
        cwd: PROJECT.into(),
        env: env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
    }
}

fn launch_run(script: &str, argv: Vec<String>) -> Run {
    machine::script_run(script, &argv, cfg().common.timeouts.script_act)
}

#[test]
fn ghostty_launch_passes_cwd_command_and_env_as_argv() {
    let m = machine_with_tui();
    let quoted = format!("'{CODEX}' 'resume' '{CODEX_ID}'");
    let argv = vec![
        PROJECT.to_owned(),
        quoted.clone(),
        "CODEX_HOME=/Users/example/.codex-alt".to_owned(),
    ];
    let fake = m.build();
    fake.on(
        &launch_run(scripts::GHOSTTY_LAUNCH, argv.clone()),
        vec![Ok(Output::ok("tty:/dev/ttys004\n"))],
    );
    let step = Step::Launch {
        terminal: TerminalKind::Ghostty,
        command: command(
            &[("CODEX_HOME", "/Users/example/.codex-alt")],
            &["resume", CODEX_ID],
        ),
        context: LaunchContext::native(),
    };
    assert_eq!(
        exec(&fake, &step),
        Dispatch::Opened {
            tty: "/dev/ttys004".into()
        }
    );
    let mut expected = vec!["-e".to_owned(), scripts::GHOSTTY_LAUNCH.to_owned()];
    expected.extend(argv);
    assert_eq!(osascript_runs(&fake), vec![expected]);

    let plain = vec![PROJECT.to_owned(), quoted];
    for (output, dispatch) in [
        (Output::ok("dispatched\n"), Dispatch::Dispatched),
        (Output::ok("tty:/etc/passwd"), Dispatch::Dispatched),
        (refused(), Dispatch::Refused),
        (
            Output::exit(1, "", "execution error: boom (-2700)"),
            Dispatch::Failed(Reason::DispatchFailed),
        ),
    ] {
        let fake = m.build();
        fake.on(
            &launch_run(scripts::GHOSTTY_LAUNCH, plain.clone()),
            vec![Ok(output)],
        );
        let step = Step::Launch {
            terminal: TerminalKind::Ghostty,
            command: command(&[], &["resume", CODEX_ID]),
            context: LaunchContext::native(),
        };
        assert_eq!(exec(&fake, &step), dispatch);
    }
    let fake = m.build();
    fake.on(
        &launch_run(scripts::GHOSTTY_LAUNCH, plain),
        vec![Err(RunError::TimedOut)],
    );
    let step = Step::Launch {
        terminal: TerminalKind::Ghostty,
        command: command(&[], &["resume", CODEX_ID]),
        context: LaunchContext::native(),
    };
    assert_eq!(exec(&fake, &step), Dispatch::Failed(Reason::DispatchFailed));
}

#[test]
fn terminal_launch_quotes_cd_and_exec() {
    let m = machine_with_tui();
    let line = format!(
        "cd -- '{PROJECT}' && exec '/usr/bin/env' 'CODEX_HOME=/a b' '{CODEX}' 'resume' 'it'\\''s'"
    );
    let fake = m.build();
    fake.on(
        &launch_run(scripts::TERMINAL_LAUNCH, vec![line.clone()]),
        vec![Ok(Output::ok("tty:/dev/ttys007"))],
    );
    let step = Step::Launch {
        terminal: TerminalKind::AppleTerminal,
        command: command(&[("CODEX_HOME", "/a b")], &["resume", "it's"]),
        context: LaunchContext::native(),
    };
    assert_eq!(
        exec(&fake, &step),
        Dispatch::Opened {
            tty: "/dev/ttys007".into()
        }
    );
    assert_eq!(
        osascript_runs(&fake),
        vec![vec![
            "-e".to_owned(),
            scripts::TERMINAL_LAUNCH.to_owned(),
            line
        ]]
    );
}

#[test]
fn bad_launches_are_refused_before_any_script() {
    let m = machine_with_tui();
    let mut cases = vec![
        (
            TerminalKind::Ghostty,
            command(&[("HOME", "/tmp")], &["resume", CODEX_ID]),
        ),
        (
            TerminalKind::Ghostty,
            command(&[("CLAUDE_CONFIG_DIR", "/x")], &["resume", CODEX_ID]),
        ),
        (
            TerminalKind::Ghostty,
            command(&[("CODEX_HOME", "/x\ny")], &[]),
        ),
        (TerminalKind::Ghostty, command(&[], &["resume\nrm -rf ~"])),
        (TerminalKind::Ghostty, command(&[], &["a\0b"])),
        (TerminalKind::AppleTerminal, command(&[], &["back\\slash"])),
        (TerminalKind::AppleTerminal, command(&[], &["bang!"])),
    ];
    let mut relative = command(&[], &[]);
    relative.program = "codex".into();
    let mut not_a_tool = command(&[], &[]);
    not_a_tool.program = "/bin/sh".into();
    let mut missing = command(&[], &[]);
    missing.program = "/usr/local/bin/codex".into();
    let mut no_cwd = command(&[], &[]);
    no_cwd.cwd = "/Users/example/gone".into();
    let mut relative_cwd = command(&[], &[]);
    relative_cwd.cwd = "code".into();
    for c in [relative, not_a_tool, missing, no_cwd, relative_cwd] {
        cases.push((TerminalKind::Ghostty, c));
    }
    for (terminal, command) in cases {
        let fake = m.build();
        let dispatch = exec(
            &fake,
            &Step::Launch {
                terminal,
                command: command.clone(),
                context: LaunchContext::native(),
            },
        );
        assert_eq!(
            dispatch,
            Dispatch::Failed(Reason::DispatchFailed),
            "{command:?}"
        );
        assert!(osascript_runs(&fake).is_empty(), "{command:?}");
    }
}

#[test]
fn stop_signals_only_an_identified_agent_cli_of_this_person() {
    let m = machine_with_tui();
    let fake = m.build();
    fake.on(
        &Run::new(KILL, &["-TERM", "4242"]),
        vec![Ok(Output::ok(""))],
    );
    assert_eq!(
        exec(
            &fake,
            &Step::Stop {
                process: tui_identity()
            }
        ),
        Dispatch::Dispatched
    );
    assert_eq!(
        fake.runs().last().cloned(),
        Some((KILL.to_owned(), vec!["-TERM".into(), "4242".into()]))
    );

    let zsh = ProcessIdentity {
        pid: 998,
        start: m.procs[3].start,
        boot: BootId(BOOT),
    };
    let mut someone_else = machine_with_tui();
    if let Some(p) = someone_else.procs.last_mut() {
        p.ruid = 502;
    }
    let mut pid_one = Machine::new();
    pid_one.procs[0].ucomm = "codex".into();
    let launchd = ProcessIdentity {
        pid: 1,
        start: pid_one.procs[0].start,
        boot: BootId(BOOT),
    };
    let stale = ProcessIdentity {
        start: tui().start - 1,
        ..tui_identity()
    };
    for (machine, identity) in [
        (&m, zsh),
        (&someone_else, tui_identity()),
        (&pid_one, launchd),
        (&m, stale),
    ] {
        let fake = machine.build();
        assert_eq!(
            exec(&fake, &Step::Stop { process: identity }),
            Dispatch::Failed(Reason::TargetGone)
        );
        assert_eq!(kills(&fake), 0);
    }
    let fake = m.build();
    assert_eq!(
        exec(
            &fake,
            &Step::AwaitExit {
                process: tui_identity()
            }
        ),
        Dispatch::Dispatched
    );
    assert!(fake.runs().is_empty());
}
