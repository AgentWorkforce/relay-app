//! The Win32 fast path against the PowerShell path: the same snapshot from
//! the same machine, a fallback to the script whenever a Win32 call fails
//! before acting, refusals at the moment of acting, the launch line it
//! builds, and which facts it reuses for two seconds.
#![cfg(feature = "windows")]
mod windows_fast_support;
mod windows_support;

use std::collections::BTreeMap;

use session_navigation::evidence::Fact;
use session_navigation::hosts::{PaneRef, WindowId};
use session_navigation::model::{ProcessIdentity, Reason, Runtime, TerminalKind};
use session_navigation::plan::{Command, Dispatch, LaunchContext, Step};
use session_navigation::ports::{Execute, Observe};
use session_navigation_native::system::win32::{SpawnConsole, WinDelegation};
use session_navigation_native::system::{FakeSystem, Win32Call};
use session_navigation_native::windows::execute::{focus_run, identity_run, launch_run};
use session_navigation_native::windows::launch::LaunchSpec;
use session_navigation_native::windows::observe::observe_run;
use session_navigation_native::windows::{wsl, WindowsExecutor, WindowsObserver};
use windows_fast_support::*;
use windows_support::*;

const HWND: i64 = 0x10;

fn console() -> (Machine, ProcessIdentity, ProcessIdentity) {
    let mut procs = base_procs();
    procs.extend(console_chain());
    let mut state = State::new(procs);
    let cmd = state.proc(3000).clone();
    state.windows = vec![Win::of(HWND, &cmd)];
    state.foreground = Some(Win::of(HWND, &cmd));
    let claude = state.proc(3002).id();
    let mut m = Machine::new(state);
    m.focus = Some("focused");
    m.launch = Some("dispatched");
    (m, cmd.id(), claude)
}

fn focus(owner: ProcessIdentity, process: ProcessIdentity) -> Step {
    Step::FocusPane {
        terminal: TerminalKind::ConsoleHost,
        pane: PaneRef::WindowsWindow {
            hwnd: WindowId(HWND as u64),
            owner,
        },
        process,
    }
}

fn any_focus(m: &Machine) -> session_navigation_native::system::Run {
    focus_run(&m.cfg, WindowId(0), &identity(0, 0), &identity(0, 0))
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

fn snapshot(m: &Machine, fake: &FakeSystem) -> session_navigation::evidence::Snapshot {
    WindowsObserver::new(fake, m.cfg.clone()).observe(&claude())
}

#[test]
fn both_paths_observe_the_same_machine_alike() {
    let (m, _, _) = console();
    let slow = snapshot(&m, &m.build());
    let fake = fast(&m);
    let quick = snapshot(&m, &fake);
    assert_eq!(powershell_runs(&fake), 0);
    assert_eq!(quick.boot, slow.boot);
    assert_eq!(quick.processes, slow.processes);
    assert_eq!(quick.surfaces, slow.surfaces);
    assert_eq!(quick.front, slow.front);
    assert_eq!(quick.apps, slow.apps);
    assert_eq!(quick.provider, slow.provider);
    // The fixture's report has no delegation section; the registry read
    // answers "let Windows decide".
    assert_eq!(slow.default_terminal, Fact::Unknown);
    assert_eq!(quick.default_terminal, Fact::Known(None));
}

#[test]
fn any_failed_read_falls_back_to_the_whole_script() {
    let (m, _, _) = console();
    let slow = snapshot(&m, &m.build());
    for broken in 0..5 {
        let fake = fast(&m);
        let w = win(&fake);
        let mut world = world(m.states.first().expect("a state"));
        match broken {
            0 => world.boot = Err(failed()),
            1 => world.processes = Err(failed()),
            2 => world.windows = Err(failed()),
            3 => world.foreground = Err(failed()),
            _ => w.set_terminal(Err(failed())),
        }
        w.set_worlds(vec![world]);
        let snap = snapshot(&m, &fake);
        assert_eq!(count(&fake, &observe_run(&m.cfg)), 1, "case {broken}");
        assert_eq!(snap.processes, slow.processes, "case {broken}");
        assert_eq!(snap.surfaces, slow.surfaces, "case {broken}");
    }
    // A table without a boot id or time cannot hold identities either.
    let fake = fast(&m);
    let mut world = world(m.states.first().expect("a state"));
    world.boot = Ok(Default::default());
    win(&fake).set_worlds(vec![world]);
    let _ = snapshot(&m, &fake);
    assert_eq!(count(&fake, &observe_run(&m.cfg)), 1);
}

#[test]
fn a_failed_focus_call_uses_the_script_but_a_refusal_does_not() {
    let (m, cmd, claude) = console();
    let fake = fast(&m);
    win(&fake).set_focus(Err(failed()));
    let mut ex = WindowsExecutor::new(&fake, m.cfg.clone());
    assert_eq!(ex.execute(&focus(cmd, claude)), Dispatch::Dispatched);
    // The identity re-check stayed on Win32; only the act fell back.
    assert_eq!(count(&fake, &identity_run(&m.cfg)), 0);
    assert_eq!(count(&fake, &any_focus(&m)), 1);

    let fake = fast(&m);
    win(&fake).set_focus(Ok(
        session_navigation_native::system::win32::FocusResult::Refused,
    ));
    let mut ex = WindowsExecutor::new(&fake, m.cfg.clone());
    assert_eq!(
        ex.execute(&focus(cmd, claude)),
        Dispatch::Failed(Reason::DispatchFailed)
    );
    assert_eq!(powershell_runs(&fake), 0);
}

#[test]
fn stale_identities_and_a_vanished_window_are_refused() {
    let (m, cmd, claude) = console();
    let fake = fast(&m);
    let mut ex = WindowsExecutor::new(&fake, m.cfg.clone());
    let older = ProcessIdentity {
        start: claude.start - 1,
        ..claude
    };
    assert_eq!(
        ex.execute(&focus(cmd, older)),
        Dispatch::Failed(Reason::TargetGone)
    );
    assert!(win(&fake).focuses().is_empty());
    // Both processes live, but the window closed before the raise.
    let mut world = world(m.states.first().expect("a state"));
    world.windows = Ok(Vec::new());
    win(&fake).set_worlds(vec![world]);
    assert_eq!(
        ex.execute(&focus(cmd, claude)),
        Dispatch::Failed(Reason::TargetGone)
    );
    assert_eq!(win(&fake).focuses().len(), 1);
    assert_eq!(powershell_runs(&fake), 0);
}

fn launch(program: &str, env: &[(&str, &str)]) -> Step {
    Step::Launch {
        terminal: TerminalKind::ConsoleHost,
        command: Command {
            program: program.into(),
            args: vec!["--resume".into(), CLAUDE_ID.into()],
            cwd: PROJECT.into(),
            env: env
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        },
        context: LaunchContext {
            runtime: Runtime::Native,
            profile: None,
        },
    }
}

#[test]
fn a_console_launch_is_one_create_process_with_the_persons_environment() {
    let (m, _, _) = console();
    let fake = fast(&m);
    let mut ex = WindowsExecutor::new(&fake, m.cfg.clone());
    let config = r"C:\Users\dev\claude config";
    let step = launch(CLAUDE_EXE, &[("CLAUDE_CONFIG_DIR", config)]);
    assert_eq!(ex.execute(&step), Dispatch::Dispatched);
    let spawns = win(&fake).spawns();
    let spawn = spawns.first().expect("one spawn");
    assert_eq!(spawn.application, CLAUDE_EXE);
    assert_eq!(
        spawn.command_line,
        format!("{CLAUDE_EXE} --resume {CLAUDE_ID}")
    );
    assert_eq!(spawn.cwd, PROJECT);
    assert_eq!(spawn.console, SpawnConsole::New);
    let mut want = user_env();
    want.push(("CLAUDE_CONFIG_DIR".to_owned(), config.to_owned()));
    assert_eq!(spawn.environment, want);
    assert_eq!(powershell_runs(&fake), 0);
}

#[test]
fn an_npm_shim_runs_through_cmd_and_keeps_its_refusals() {
    let (mut m, _, _) = console();
    let shim = format!(r"{APPDATA}\npm\claude.cmd");
    m.file(&shim, b"@echo off".to_vec());
    let fake = fast(&m);
    let mut ex = WindowsExecutor::new(&fake, m.cfg.clone());
    assert_eq!(ex.execute(&launch(&shim, &[])), Dispatch::Dispatched);
    let spawns = win(&fake).spawns();
    let spawn = spawns.first().expect("one spawn");
    assert_eq!(spawn.application, m.cfg.system32("cmd.exe"));
    assert_eq!(
        spawn.command_line,
        format!(
            r#"{} /d /s /c "{shim} --resume {CLAUDE_ID}""#,
            m.cfg.system32("cmd.exe")
        )
    );
    // A word cmd.exe would reinterpret is refused before anything runs.
    let Step::Launch {
        mut command,
        context,
        terminal,
    } = launch(&shim, &[])
    else {
        return;
    };
    command.args.push("100%".into());
    let bad = Step::Launch {
        terminal,
        command,
        context,
    };
    assert_eq!(ex.execute(&bad), Dispatch::Failed(Reason::DispatchFailed));
    assert_eq!(win(&fake).spawns().len(), 1);
    assert_eq!(powershell_runs(&fake), 0);
}

#[test]
fn a_launch_that_cannot_start_directly_uses_the_script() {
    for broken in [0, 1] {
        let (m, _, _) = console();
        let fake = fast(&m);
        if broken == 0 {
            win(&fake).set_environment(Err(failed()));
        } else {
            win(&fake).set_spawn(Err(failed()));
        }
        let mut ex = WindowsExecutor::new(&fake, m.cfg.clone());
        assert_eq!(ex.execute(&launch(CLAUDE_EXE, &[])), Dispatch::Dispatched);
        assert_eq!(count(&fake, &any_launch(&m)), 1, "case {broken}");
    }
}

#[test]
fn registry_and_package_facts_are_reused_for_one_click_only() {
    let (m, _, _) = console();
    let fake = fast(&m);
    win(&fake).set_delegation(Ok(WinDelegation {
        console: "{B23D10C0-E52E-411E-9D5B-C09FDF709C7D}".into(),
        terminal: "{B23D10C0-E52E-411E-9D5B-C09FDF709C7D}".into(),
    }));
    let mut observer = WindowsObserver::new(&fake, m.cfg.clone());
    let first = observer.observe(&claude());
    let second = observer.observe(&claude());
    assert_eq!(
        second.default_terminal,
        Fact::Known(Some(TerminalKind::ConsoleHost))
    );
    assert_eq!(first.default_terminal, second.default_terminal);
    let calls = win(&fake).calls();
    let n = |call: Win32Call| calls.iter().filter(|c| **c == call).count();
    // The table, windows and foreground are read every time.
    assert_eq!(n(Win32Call::Processes), 2);
    assert_eq!(n(Win32Call::Windows), 2);
    assert_eq!(n(Win32Call::Foreground), 2);
    assert_eq!(n(Win32Call::Delegation), 1);
    assert_eq!(n(Win32Call::TerminalPackage), 1);
    // A new ports instance reads them again.
    let mut other = WindowsObserver::new(&fake, m.cfg.clone());
    let _ = other.observe(&claude());
    let calls = win(&fake).calls();
    assert_eq!(
        calls
            .iter()
            .filter(|c| **c == Win32Call::Delegation)
            .count(),
        2
    );
}

#[test]
fn a_wsl_session_probes_its_distribution_beside_the_win32_reads() {
    let mut m = Machine::new(State::new(base_procs()));
    m.cfg = wsl_cfg();
    let mut fake = fast(&m);
    // Really on threads, as on the machine.
    fake.set_concurrent(true);
    let table = wsl_table(&[wsl_proc(1, 0, "/init", &["/init"], 0, 5)]);
    let tools = String::from_utf8(fixture("wsl-tools.txt")).unwrap_or_default();
    script_wsl(&fake, &table, &tools);
    let mut observer = WindowsObserver::new(&fake, m.cfg.clone());
    let first = observer.observe(&claude());
    let second = observer.observe(&claude());
    assert!(first.processes.is_known());
    assert_eq!(first.tools, second.tools);
    assert!(first.surfaces.windows.is_known());
    let tools_run = wsl::tools_run(&m.cfg, DISTRO).expect("a tools run");
    // The table is fresh each time; the tools probe is reused.
    assert_eq!(count(&fake, &wsl::table_run(&m.cfg, DISTRO)), 2);
    assert_eq!(count(&fake, &tools_run), 1);
    assert_eq!(powershell_runs(&fake), 0);
    // A native session never starts wsl.exe.
    let native = Machine::new(State::new(base_procs()));
    let fake = fast(&native);
    let _ = snapshot(&native, &fake);
    assert!(fake.runs().iter().all(|(p, _)| !p.ends_with("wsl.exe")));
}
