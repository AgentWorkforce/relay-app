//! `FocusPane` for iTerm2 (tty script), Kitty and WezTerm (their APIs) on a
//! fake Mac. Responses are documentation-derived fixtures; none of these
//! terminals was ever driven for real.
#![cfg(feature = "macos")]
mod common;
mod macos_world;

use std::collections::BTreeMap;

use common::*;
use macos_world::*;
use session_navigation::hosts::PaneRef;
use session_navigation::model::{ProcessIdentity, Reason, TerminalKind};
use session_navigation::plan::{Command, Dispatch, LaunchContext, Step};
use session_navigation::ports::Execute;
use session_navigation_native::common::quote::unquote_reference;
use session_navigation_native::common::{kitty, wezterm};
use session_navigation_native::macos::execute::iterm_command;
use session_navigation_native::macos::{machine, scripts};
use session_navigation_native::system::{Call, FakeSystem, Output};
use session_navigation_native::MacExecutor;

fn exec(fake: &FakeSystem, step: &Step) -> Dispatch {
    MacExecutor::new(fake, cfg()).execute(step)
}

fn focus(terminal: TerminalKind, pane: PaneRef, process: ProcessIdentity) -> Step {
    Step::FocusPane {
        terminal,
        pane,
        process,
    }
}

fn ran(fake: &FakeSystem, program: &str) -> Vec<Vec<String>> {
    fake.runs()
        .into_iter()
        .filter(|(p, _)| p == program)
        .map(|(_, a)| a)
        .collect()
}

fn refused(app: &str) -> Output {
    Output::exit(
        1,
        "",
        format!("0:120: execution error: Not authorized to send Apple events to {app}. (-1743)\n"),
    )
}

#[test]
fn iterm_focus_by_tty_with_identity_check() {
    let tty = "/dev/ttys000".to_owned();
    let step = focus(
        TerminalKind::ITerm2,
        PaneRef::Tty { tty: tty.clone() },
        ident(998),
    );
    let script = machine::script_run(
        scripts::ITERM_FOCUS,
        std::slice::from_ref(&tty),
        cfg().common.timeouts.script_act,
    );
    for (response, expected) in [
        (Output::ok("focused\n"), Dispatch::Dispatched),
        (refused("iTerm2"), Dispatch::Refused),
        (
            Output::exit(1, "", "execution error: session unavailable (7001)\n"),
            Dispatch::Failed(Reason::TargetGone),
        ),
        (
            Output::exit(1, "", "execution error: session not confirmed (7002)\n"),
            Dispatch::Failed(Reason::WrongDestination),
        ),
    ] {
        let mut m = Machine::new();
        m.run(script.clone(), Ok(response));
        let fake = m.build();
        assert_eq!(exec(&fake, &step), expected);
        assert!(fake
            .runs()
            .contains(&(script.program.clone(), script.args.clone())));
    }
    // A recycled pid (another start time) never reaches iTerm2.
    let mut stale = ident(998);
    stale.start += 1;
    let fake = Machine::new().build();
    let step = focus(TerminalKind::ITerm2, PaneRef::Tty { tty }, stale);
    assert_eq!(exec(&fake, &step), Dispatch::Failed(Reason::TargetGone));
    assert!(ran(&fake, machine::OSASCRIPT)
        .iter()
        .all(|a| !a.contains(&scripts::ITERM_FOCUS.to_owned())));
}

fn command() -> Command {
    Command {
        program: "/opt/homebrew/bin/codex".into(),
        args: vec!["resume".into(), CODEX_ID.into(), "it's $HOME `x`".into()],
        cwd: PROJECT.into(),
        env: BTreeMap::from([("CODEX_HOME".to_owned(), format!("{HOME}/.codex-alt"))]),
    }
}

#[test]
fn iterm_launch_passes_argv_as_positional_parameters() {
    let line = iterm_command(&command(), "/usr/bin/env").unwrap_or_default();
    assert_eq!(
        unquote_reference(&line),
        Some(vec![
            "/bin/sh".into(),
            "-c".into(),
            scripts::ITERM_SH.into(),
            "sh".into(),
            PROJECT.into(),
            "/usr/bin/env".into(),
            format!("CODEX_HOME={HOME}/.codex-alt"),
            "/opt/homebrew/bin/codex".into(),
            "resume".into(),
            CODEX_ID.into(),
            "it's $HOME `x`".into(),
        ])
    );
    let mut m = Machine::new();
    m.dir(PROJECT);
    let script = machine::script_run(
        scripts::ITERM_LAUNCH,
        &[line],
        cfg().common.timeouts.script_act,
    );
    m.run(script, Ok(Output::ok("tty:/dev/ttys012\n")));
    let fake = m.build();
    let step = Step::Launch {
        terminal: TerminalKind::ITerm2,
        command: command(),
        context: LaunchContext::native(),
    };
    assert_eq!(
        exec(&fake, &step),
        Dispatch::Opened {
            tty: "/dev/ttys012".into()
        }
    );

    // A backslash could be read as an escape by iTerm2's splitter: refused.
    let mut bad = command();
    bad.args.push("a\\b".into());
    let fake = m.build();
    let step = Step::Launch {
        terminal: TerminalKind::ITerm2,
        command: bad,
        context: LaunchContext::native(),
    };
    assert_eq!(exec(&fake, &step), Dispatch::Failed(Reason::DispatchFailed));
    assert!(ran(&fake, machine::OSASCRIPT)
        .iter()
        .all(|a| !a.contains(&scripts::ITERM_LAUNCH.to_owned())));
}

fn kitty_step(id: u64, process: ProcessIdentity) -> Step {
    focus(
        TerminalKind::Kitty,
        PaneRef::KittyWindow {
            socket: KITTY_ADDR.into(),
            id,
        },
        process,
    )
}

#[test]
fn kitty_focus_rechecks_the_pane_then_focuses_it() {
    let mut m = host_machine();
    m.run(
        kitty_run(kitty::focus_args(KITTY_ADDR, 1)),
        Ok(Output::ok("")),
    );
    let fake = m.build();
    // Window 1's shell (2001) is the parent of codex (2002).
    assert_eq!(
        exec(&fake, &kitty_step(1, ident(2002))),
        Dispatch::Dispatched
    );
    assert_eq!(
        ran(&fake, KITTY),
        vec![
            kitty::ls_args(KITTY_ADDR),
            vec![
                "@".into(),
                "--to".into(),
                KITTY_ADDR.into(),
                "focus-window".into(),
                "--match".into(),
                "id:1".into()
            ]
        ]
    );

    // Window 2 does not hold codex: nothing is focused.
    let fake = m.build();
    assert_eq!(
        exec(&fake, &kitty_step(2, ident(2002))),
        Dispatch::Failed(Reason::TargetGone)
    );
    assert_eq!(ran(&fake, KITTY), vec![kitty::ls_args(KITTY_ADDR)]);

    // A window id that is no longer listed.
    let fake = m.build();
    assert_eq!(
        exec(&fake, &kitty_step(9, ident(2002))),
        Dispatch::Failed(Reason::TargetGone)
    );
}

#[test]
fn kitty_stale_socket_refusal_and_identity() {
    // The socket file is gone: stale, nothing runs.
    let mut m = host_machine();
    m.files.retain(|(p, _, _)| p != KITTY_SOCK);
    let fake = m.build();
    assert_eq!(
        exec(&fake, &kitty_step(1, ident(2002))),
        Dispatch::Failed(Reason::TargetGone)
    );
    assert!(ran(&fake, KITTY).is_empty());

    // Nobody listens, or remote control is disabled.
    for (stderr, expected) in [
        (KITTY_GONE, Dispatch::Failed(Reason::TargetGone)),
        (KITTY_DISABLED, Dispatch::Refused),
    ] {
        let mut m = host_machine();
        m.run(
            kitty_run(kitty::ls_args(KITTY_ADDR)),
            Ok(Output::exit(1, "", stderr)),
        );
        assert_eq!(exec(&m.build(), &kitty_step(1, ident(2002))), expected);
    }

    // The focus itself refused (e.g. a declined `ask` prompt).
    let mut m = host_machine();
    m.run(
        kitty_run(kitty::focus_args(KITTY_ADDR, 1)),
        Ok(Output::exit(
            1,
            "",
            "Error: Remote control request denied by user\n",
        )),
    );
    assert_eq!(
        exec(&m.build(), &kitty_step(1, ident(2002))),
        Dispatch::Refused
    );

    // A recycled pid: refused before Kitty is asked.
    let mut stale = ident(2002);
    stale.start += 5;
    let fake = host_machine().build();
    assert_eq!(
        exec(&fake, &kitty_step(1, stale)),
        Dispatch::Failed(Reason::TargetGone)
    );
    assert!(ran(&fake, KITTY).is_empty());

    // A hostile address never reaches argv.
    let fake = host_machine().build();
    let step = focus(
        TerminalKind::Kitty,
        PaneRef::KittyWindow {
            socket: "--to=tcp:evil".into(),
            id: 1,
        },
        ident(2002),
    );
    assert_eq!(exec(&fake, &step), Dispatch::Failed(Reason::TargetGone));
    assert!(ran(&fake, KITTY).is_empty());
}

fn wez_step(id: u64, process: ProcessIdentity) -> Step {
    focus(
        TerminalKind::WezTerm,
        PaneRef::WezTermPane {
            socket: WEZ_SOCK.into(),
            id,
        },
        process,
    )
}

#[test]
fn wezterm_focus_by_tty_on_its_socket() {
    let mut m = host_machine();
    m.run(wez_run(wezterm::activate_args(0)), Ok(Output::ok("")));
    let fake = m.build();
    assert_eq!(exec(&fake, &wez_step(0, ident(3001))), Dispatch::Dispatched);
    assert_eq!(
        ran(&fake, WEZ),
        vec![
            wezterm::list_args(),
            vec![
                "cli".into(),
                "--no-auto-start".into(),
                "activate-pane".into(),
                "--pane-id".into(),
                "0".into()
            ]
        ]
    );
    assert!(fake.calls().iter().all(|c| match c {
        Call::Run { program, env, .. } if program == WEZ =>
            env == &vec![("WEZTERM_UNIX_SOCKET".to_owned(), WEZ_SOCK.to_owned())],
        _ => true,
    }));

    // Pane 1 is on another tty.
    let fake = m.build();
    assert_eq!(
        exec(&fake, &wez_step(1, ident(3001))),
        Dispatch::Failed(Reason::TargetGone)
    );
    assert_eq!(ran(&fake, WEZ), vec![wezterm::list_args()]);

    // The GUI went away under the socket.
    let mut gone = host_machine();
    gone.run(
        wez_run(wezterm::list_args()),
        Ok(Output::exit(1, "", WEZ_GONE)),
    );
    assert_eq!(
        exec(&gone.build(), &wez_step(0, ident(3001))),
        Dispatch::Failed(Reason::TargetGone)
    );

    // A WezTerm pane given to another terminal is refused untouched.
    let fake = host_machine().build();
    let step = focus(
        TerminalKind::Ghostty,
        PaneRef::WezTermPane {
            socket: WEZ_SOCK.into(),
            id: 0,
        },
        ident(3001),
    );
    assert_eq!(
        exec(&fake, &step),
        Dispatch::Failed(Reason::HostUnsupported)
    );
    assert!(fake.runs().is_empty());
}
