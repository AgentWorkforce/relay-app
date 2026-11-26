//! Snapshot fields the Windows observer fills, natively and for WSL.
#![cfg(feature = "windows")]
mod windows_support;

use session_navigation::evidence::{Fact, ProviderEvidence};
use session_navigation::hosts::{Ability, Unsupported};
use session_navigation::model::{App, BootId, Platform, Runtime, TerminalKind, Tool};
use session_navigation::ports::Observe;
use session_navigation_native::system::{Output, RunError};
use session_navigation_native::windows::observe::observe_run;
use session_navigation_native::windows::{wsl, WindowsObserver};
use windows_support::*;

fn console_state() -> State {
    let mut procs = base_procs();
    procs.extend(console_chain());
    let mut state = State::new(procs);
    let cmd = state.proc(3000).clone();
    state.windows = vec![Win::of(0x10, &cmd)];
    state.foreground = Some(Win::of(0x10, &cmd));
    state
}

#[test]
fn a_native_snapshot_has_every_windows_field() {
    let mut m = Machine::new(console_state());
    let codex_shim = format!(r"{APPDATA}\npm\codex.cmd");
    m.file(&codex_shim, b"@echo off".to_vec())
        .version(&codex_shim, "codex-cli 0.167.0\n");
    let fake = m.build();
    let mut observer = WindowsObserver::new(&fake, m.cfg.clone());
    let snap = observer.observe(&claude());
    assert_eq!(snap.platform, Platform::Windows);
    assert_eq!(snap.runtime, Fact::Known(Runtime::Native));
    assert_eq!(snap.boot, Fact::Known(BootId(BOOT_ID)));
    let table = snap.processes.known().expect("table");
    assert!(table.iter().all(|p| p.tty.is_none()));
    assert_eq!(snap.front.pane_tty, Fact::Known(None));
    assert_eq!(snap.front.pane, Fact::Known(None));
    assert_eq!(snap.front.app, Fact::Known(Some(App::ConsoleHost)));
    assert_eq!(
        snap.front.window,
        Fact::Known(Some(session_navigation::hosts::WindowId(0x10)))
    );
    assert_eq!(snap.schemes, Fact::Known(Vec::new()));
    assert_eq!(snap.surfaces.panes, Fact::Known(Vec::new()));
    assert_eq!(snap.surfaces.tmux, Fact::Known(Vec::new()));
    assert_eq!(snap.surfaces.windows.known().map(Vec::len), Some(1));
    // Without history the profile is unknown, never assumed.
    assert_eq!(snap.terminal_profile, Fact::Unknown);
    let support = snap.surfaces.terminals.known().expect("terminals");
    let wt = support
        .iter()
        .find(|s| s.terminal == TerminalKind::WindowsTerminal)
        .expect("wt");
    assert_eq!(wt.focus_pane, Ability::Unsupported(Unsupported::NoPaneApi));
    assert_eq!(wt.raise_window, Ability::Available);
    assert_eq!(wt.launch, Ability::Available);
    assert!(wt.tabs_share_window);
    let console = support
        .iter()
        .find(|s| s.terminal == TerminalKind::ConsoleHost)
        .expect("console");
    assert!(!console.tabs_share_window);
    // Every terminal that exists on Windows is reported, and only those.
    for terminal in TerminalKind::ALL {
        assert_eq!(
            support.iter().any(|s| s.terminal == terminal),
            terminal.exists_on(Platform::Windows),
            "{terminal:?}"
        );
    }
    let apps = snap.apps.known().expect("apps");
    let app = |a: App| apps.iter().find(|i| i.app == a).expect("row");
    assert!(app(App::WindowsTerminal).installed);
    assert_eq!(
        app(App::WindowsTerminal).version.as_deref(),
        Some("1.21.2911.0")
    );
    assert!(!app(App::WindowsTerminal).running);
    assert!(app(App::ConsoleHost).installed && app(App::ConsoleHost).running);
    assert!(!app(App::WezTerm).installed);
    assert_eq!(apps.len(), 4);
    let tool = |t: Tool| snap.tools.iter().find(|i| i.tool == t).expect("tool");
    assert_eq!(tool(Tool::ClaudeCli).path.as_deref(), Some(CLAUDE_EXE));
    assert_eq!(
        tool(Tool::ClaudeCli).version,
        Fact::Known(Some("2.1.321".into()))
    );
    assert_eq!(
        tool(Tool::CodexCli).path.as_deref(),
        Some(codex_shim.as_str())
    );
    assert_eq!(tool(Tool::OpenCodeCli).version, Fact::Known(None));
    // One PowerShell run for boot, table, windows, front and apps.
    assert_eq!(count(&fake, &observe_run(&m.cfg)), 1);
}

#[test]
fn an_exe_is_preferred_over_a_cmd_shim_in_the_same_directory() {
    let mut m = Machine::new(console_state());
    let shim = CLAUDE_EXE.replace(".exe", ".cmd");
    m.file(&shim, b"@echo off".to_vec());
    let fake = m.build();
    let snap = WindowsObserver::new(&fake, m.cfg.clone()).observe(&claude());
    let claude = snap.tools.iter().find(|t| t.tool == Tool::ClaudeCli);
    assert_eq!(claude.and_then(|t| t.path.as_deref()), Some(CLAUDE_EXE));
}

#[test]
fn a_failed_powershell_run_leaves_its_facts_unknown() {
    let m = Machine::new(console_state());
    let fake = m.build();
    fake.on(&observe_run(&m.cfg), vec![Err(RunError::TimedOut)]);
    let snap = WindowsObserver::new(&fake, m.cfg.clone()).observe(&claude());
    assert_eq!(snap.boot, Fact::Unknown);
    assert_eq!(snap.processes, Fact::Unknown);
    assert_eq!(snap.apps, Fact::Unknown);
    assert_eq!(snap.front.app, Fact::Unknown);
    assert_eq!(snap.front.window, Fact::Unknown);
    assert_eq!(snap.surfaces.windows, Fact::Unknown);
    // The rest of the snapshot stands on its own.
    assert_eq!(snap.reservation, Fact::Known(None));
    for bad in [&b"not json"[..], b"{\"bootId\":1}\n\x00"] {
        fake.on(&observe_run(&m.cfg), vec![Ok(Output::ok(bad.to_vec()))]);
        let snap = WindowsObserver::new(&fake, m.cfg.clone()).observe(&claude());
        assert_eq!(snap.processes, Fact::Unknown);
    }
    fake.on(
        &observe_run(&m.cfg),
        vec![Ok(Output::exit(
            1,
            b"{}".to_vec(),
            b"Add-Type failed".to_vec(),
        ))],
    );
    let snap = WindowsObserver::new(&fake, m.cfg.clone()).observe(&claude());
    assert_eq!(snap.processes, Fact::Unknown);
}

#[test]
fn a_report_from_the_fixture_reads_through_the_observer() {
    let m = Machine::new(console_state());
    let fake = m.build();
    fake.on(
        &observe_run(&m.cfg),
        vec![Ok(Output::ok(fixture("observe-single-bom.json")))],
    );
    let snap = WindowsObserver::new(&fake, m.cfg.clone()).observe(&claude());
    assert_eq!(snap.processes.known().map(Vec::len), Some(1));
    assert_eq!(snap.front.app, Fact::Known(None));
    let apps = snap.apps.known().expect("apps");
    assert!(!apps
        .iter()
        .any(|a| a.app == App::WindowsTerminal && a.installed));
}

#[test]
fn a_wsl_snapshot_describes_the_distribution() {
    let mut m = Machine::new(console_state());
    m.cfg = wsl_cfg();
    let home = m.claude_home();
    m.transcript(&home, "/home/dev/code/project");
    m.dirs
        .push(r"\\wsl.localhost\Ubuntu\home\dev\code\project".to_owned());
    let fake = m.build();
    let table = String::from_utf8(fixture("wsl-table.txt")).unwrap_or_default();
    let tools = String::from_utf8(fixture("wsl-tools.txt")).unwrap_or_default();
    script_wsl(&fake, &table, &tools);
    let snap = WindowsObserver::new(&fake, m.cfg.clone()).observe(&claude());
    assert_eq!(
        snap.runtime,
        Fact::Known(Runtime::Wsl {
            distro: DISTRO.into()
        })
    );
    assert_eq!(snap.boot, Fact::Known(BootId(0x3f1b_9c2e_8d4a_4b6f)));
    let table = snap.processes.known().expect("table");
    assert!(table.iter().any(|p| p.tty.as_deref() == Some("/dev/pts/0")));
    let claude = snap.tools.iter().find(|t| t.tool == Tool::ClaudeCli);
    assert_eq!(
        claude.and_then(|t| t.path.as_deref()),
        Some("/home/dev/.local/bin/claude")
    );
    assert_eq!(snap.cwd, Fact::Known("/home/dev/code/project".into()));
    assert_eq!(snap.cwd_exists, Fact::Known(true));
    assert_eq!(snap.surfaces.tmux, Fact::Unknown);
    // The Windows desktop around it is still read.
    assert_eq!(snap.front.app, Fact::Known(Some(App::ConsoleHost)));
    assert!(matches!(snap.provider, ProviderEvidence::Claude(_)));
}

#[test]
fn a_distribution_that_is_not_installed_is_not_probed() {
    let mut m = Machine::new(console_state());
    m.cfg = session_navigation_native::windows::WindowsConfig::wsl(&cfg(), "Arch", WSL_HOME)
        .expect("cfg");
    let fake = m.build();
    script_wsl(&fake, "", "");
    let snap = WindowsObserver::new(&fake, m.cfg.clone()).observe(&claude());
    assert_eq!(snap.processes, Fact::Unknown);
    assert_eq!(count(&fake, &wsl::table_run(&m.cfg, "Arch")), 0);
}

#[test]
fn configs_refuse_unsafe_values() {
    use session_navigation_native::windows::WindowsConfig;
    assert!(WindowsConfig::native("Windows", PROFILE, APPDATA, LOCAL).is_none());
    assert!(WindowsConfig::wsl(&cfg(), "-d", WSL_HOME).is_none());
    assert!(WindowsConfig::wsl(&cfg(), "Ubuntu", "/home/a b").is_none());
    assert!(WindowsConfig::wsl(&cfg(), "Ubuntu", "relative").is_none());
    let w = wsl_cfg();
    assert_eq!(
        w.common.paths.claude_home,
        r"\\wsl.localhost\Ubuntu\home\dev\.claude"
    );
    assert_eq!(
        w.common.paths.reservation_dir,
        cfg().common.paths.reservation_dir
    );
    assert_eq!(
        cfg().powershell(),
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"
    );
}
