//! Each `Step` through `WindowsExecutor`: what runs, what is re-checked
//! first, and what is refused without running anything.
#![cfg(feature = "windows")]
mod windows_support;

use std::collections::BTreeMap;

use session_navigation::hosts::{PaneRef, WindowId};
use session_navigation::model::{App, BootId, ProcessIdentity, Reason, Runtime, TerminalKind};
use session_navigation::plan::{Command, Dispatch, LaunchContext, Step};
use session_navigation::ports::Execute;
use session_navigation_native::system::{Output, RunError};
use session_navigation_native::windows::execute::{focus_run, identity_run, launch_run};
use session_navigation_native::windows::launch::LaunchSpec;
use session_navigation_native::windows::{wsl, WindowsExecutor};
use windows_support::*;

fn console() -> (Machine, ProcessIdentity, ProcessIdentity) {
    let mut procs = base_procs();
    procs.extend(console_chain());
    let state = State::new(procs);
    let (cmd, claude) = (state.proc(3000).id(), state.proc(3002).id());
    (Machine::new(state), cmd, claude)
}

fn focus(owner: ProcessIdentity, process: ProcessIdentity) -> Step {
    Step::FocusPane {
        terminal: TerminalKind::ConsoleHost,
        pane: PaneRef::WindowsWindow {
            hwnd: WindowId(0x10),
            owner,
        },
        process,
    }
}

fn any_focus(m: &Machine) -> session_navigation_native::system::Run {
    focus_run(&m.cfg, WindowId(0), &identity(0, 0), &identity(0, 0))
}

#[test]
fn the_focus_script_answer_maps_to_a_dispatch() {
    for (status, want) in [
        ("focused", Dispatch::Dispatched),
        ("gone", Dispatch::Failed(Reason::TargetGone)),
        ("refused", Dispatch::Failed(Reason::DispatchFailed)),
        ("unconfirmed", Dispatch::Failed(Reason::DispatchFailed)),
        ("???", Dispatch::Failed(Reason::DispatchFailed)),
    ] {
        let (mut m, cmd, claude) = console();
        m.focus = Some(status);
        let fake = m.build();
        let mut ex = WindowsExecutor::new(&fake, m.cfg.clone());
        assert_eq!(ex.execute(&focus(cmd, claude)), want, "{status}");
    }
}

#[test]
fn stale_identities_are_refused_before_the_window_is_touched() {
    let (mut m, cmd, claude) = console();
    m.focus = Some("focused");
    let fake = m.build();
    let mut ex = WindowsExecutor::new(&fake, m.cfg.clone());
    let older = ProcessIdentity {
        start: claude.start - 1,
        ..claude
    };
    let other_boot = ProcessIdentity {
        boot: BootId(BOOT_ID + 1),
        ..cmd
    };
    for step in [focus(cmd, older), focus(other_boot, claude)] {
        assert_eq!(ex.execute(&step), Dispatch::Failed(Reason::TargetGone));
    }
    assert_eq!(count(&fake, &any_focus(&m)), 0);
    // An unreadable table refuses too.
    fake.on(&identity_run(&m.cfg), vec![Err(RunError::TimedOut)]);
    assert_eq!(
        ex.execute(&focus(cmd, claude)),
        Dispatch::Failed(Reason::EvidenceIncomplete)
    );
    assert_eq!(count(&fake, &any_focus(&m)), 0);
}

#[test]
fn only_windows_of_raisable_terminals_are_raised() {
    let (mut m, cmd, claude) = console();
    m.focus = Some("focused");
    let fake = m.build();
    let mut ex = WindowsExecutor::new(&fake, m.cfg.clone());
    let tty = Step::FocusPane {
        terminal: TerminalKind::ConsoleHost,
        pane: PaneRef::Tty {
            tty: "/dev/pts/1".into(),
        },
        process: claude,
    };
    let ghostty = Step::FocusPane {
        terminal: TerminalKind::Ghostty,
        pane: PaneRef::WindowsWindow {
            hwnd: WindowId(0x10),
            owner: cmd,
        },
        process: claude,
    };
    for step in [tty, ghostty] {
        assert_eq!(ex.execute(&step), Dispatch::Failed(Reason::HostUnsupported));
    }
    let mut wsl_ex = WindowsExecutor::new(&fake, wsl_cfg());
    assert_eq!(
        wsl_ex.execute(&focus(cmd, claude)),
        Dispatch::Failed(Reason::HostUnsupported)
    );
    assert!(fake.runs().is_empty());
}

#[test]
fn links_apps_and_native_stops_run_nothing() {
    let (m, _, claude) = console();
    let fake = m.build();
    let mut ex = WindowsExecutor::new(&fake, m.cfg.clone());
    let url = Step::OpenUrl {
        url: format!("claude://resume?session={CLAUDE_ID}"),
        bundle: "claude.exe".into(),
    };
    assert_eq!(
        ex.execute(&url),
        Dispatch::Failed(Reason::PlatformUnsupported)
    );
    assert_eq!(
        ex.execute(&Step::ActivateApp {
            app: App::WindowsTerminal
        }),
        Dispatch::Failed(Reason::HostUnsupported)
    );
    // No graceful stop exists natively.
    assert_eq!(
        ex.execute(&Step::Stop { process: claude }),
        Dispatch::Failed(Reason::HostUnsupported)
    );
    assert_eq!(
        ex.execute(&Step::AwaitExit { process: claude }),
        Dispatch::Dispatched
    );
    assert!(fake.runs().is_empty());
}

fn launch_step(terminal: TerminalKind, runtime: Runtime) -> Step {
    Step::Launch {
        terminal,
        command: Command {
            program: CLAUDE_EXE.into(),
            args: vec!["--resume".into(), CLAUDE_ID.into()],
            cwd: PROJECT.into(),
            env: BTreeMap::new(),
        },
        context: LaunchContext {
            runtime,
            profile: None,
        },
    }
}

fn any_launch(m: &Machine) -> session_navigation_native::system::Run {
    let spec = LaunchSpec {
        file: String::new(),
        line: String::new(),
        cwd: String::new(),
        env: BTreeMap::new(),
    };
    launch_run(&m.cfg, &spec)
}

#[test]
fn launches_are_validated_before_anything_runs() {
    let (mut m, _, _) = console();
    m.launch = Some("dispatched");
    let fake = m.build();
    let mut ex = WindowsExecutor::new(&fake, m.cfg.clone());
    assert_eq!(
        ex.execute(&launch_step(TerminalKind::ConsoleHost, Runtime::Native)),
        Dispatch::Dispatched
    );
    let wsl = Runtime::Wsl {
        distro: DISTRO.into(),
    };
    assert_eq!(
        ex.execute(&launch_step(TerminalKind::ConsoleHost, wsl)),
        Dispatch::Failed(Reason::PlatformUnsupported)
    );
    assert_eq!(
        ex.execute(&launch_step(TerminalKind::WezTerm, Runtime::Native)),
        Dispatch::Failed(Reason::HostUnsupported)
    );
    let Step::Launch {
        mut command,
        context,
        ..
    } = launch_step(TerminalKind::ConsoleHost, Runtime::Native)
    else {
        return;
    };
    command.args.push("bad\u{7}".into());
    let bad = Step::Launch {
        terminal: TerminalKind::ConsoleHost,
        command,
        context,
    };
    assert_eq!(ex.execute(&bad), Dispatch::Failed(Reason::DispatchFailed));
    assert_eq!(count(&fake, &any_launch(&m)), 1);
    // A launcher that fails or answers anything else is not a dispatch.
    fake.on(
        &any_launch(&m),
        vec![Ok(Output::exit(1, b"".to_vec(), b"".to_vec()))],
    );
    assert_eq!(
        ex.execute(&launch_step(TerminalKind::ConsoleHost, Runtime::Native)),
        Dispatch::Failed(Reason::DispatchFailed)
    );
}

fn wsl_machine() -> (Machine, ProcessIdentity, ProcessIdentity, ProcessIdentity) {
    let mut m = Machine::new(State::new(base_procs()));
    m.cfg = wsl_cfg();
    let boot = BootId(0x3f1b_9c2e_8d4a_4b6f);
    let id = |pid: u32, ticks: u64| ProcessIdentity {
        pid,
        start: 1_794_400_000 + ticks / 100,
        boot,
    };
    (m, id(311, 12_500), id(310, 12_000), id(312, 13_000))
}

#[test]
fn a_wsl_stop_signals_only_the_exact_agent_cli() {
    let (m, claude, bash, _) = wsl_machine();
    let fake = m.build();
    let table = String::from_utf8(fixture("wsl-table.txt")).unwrap_or_default();
    script_wsl(&fake, &table, "");
    let timeout = m.cfg.common.timeouts.dispatch;
    let kill = wsl::kill_run(&m.cfg, DISTRO, claude.pid, timeout);
    fake.on(&kill, vec![Ok(Output::ok(Vec::new()))]);
    let mut ex = WindowsExecutor::new(&fake, m.cfg.clone());
    let stale = ProcessIdentity {
        start: claude.start + 1,
        ..claude
    };
    for target in [stale, bash] {
        assert_eq!(
            ex.execute(&Step::Stop { process: target }),
            Dispatch::Failed(Reason::TargetGone)
        );
    }
    assert_eq!(count(&fake, &kill), 0);
    assert_eq!(
        ex.execute(&Step::Stop { process: claude }),
        Dispatch::Dispatched
    );
    assert_eq!(count(&fake, &kill), 1);
}

#[test]
fn a_wsl_stop_refuses_another_users_process() {
    let (m, _, _, node) = wsl_machine();
    let fake = m.build();
    let table = String::from_utf8(fixture("wsl-table.txt"))
        .unwrap_or_default()
        .replace("uid 1000", "uid 1001");
    script_wsl(&fake, &table, "");
    let mut ex = WindowsExecutor::new(&fake, m.cfg.clone());
    assert_eq!(
        ex.execute(&Step::Stop { process: node }),
        Dispatch::Failed(Reason::TargetGone)
    );
    let kills = fake
        .runs()
        .into_iter()
        .filter(|(_, a)| a.iter().any(|w| w == "/bin/kill"))
        .count();
    assert_eq!(kills, 0);
}
