//! Each `Snapshot` field from the fake Mac: the happy path, and every
//! failure becoming `Fact::Unknown`.
#![cfg(feature = "macos")]
mod common;

use common::*;
use session_navigation::evidence::{Exe, Fact, ProviderEvidence, Snapshot};
use session_navigation::model::{App, BootId, Host, SessionRef, TerminalKind, Tool};
use session_navigation::ports::Observe;
use session_navigation_native::macos::machine;
use session_navigation_native::macos::scripts;
use session_navigation_native::system::{FakeSystem, Output, Run, RunError};
use session_navigation_native::{HostHistory, MacObserver};

fn observe(fake: &FakeSystem, session: &SessionRef) -> Snapshot {
    MacObserver::new(fake, cfg()).observe(session)
}

#[test]
fn boot_and_process_table() {
    let mut m = claude_machine(false, true);
    let fake = m.build();
    let snap = observe(&fake, &claude());
    assert_eq!(snap.boot, Fact::Known(BootId(BOOT)));
    assert_eq!(snap.observed_at, NOW);
    let table = snap.processes.known().cloned().unwrap_or_default();
    assert_eq!(table.len(), m.procs.len());
    let cli = table.iter().find(|p| p.identity.pid == 25354);
    assert_eq!(cli.map(|p| p.exe), Some(Exe::ClaudeCli));
    assert_eq!(
        cli.map(|p| p.cwd.clone()),
        Some(Fact::Known(PROJECT.to_owned()))
    );
    // Non-agent processes never get a cwd probe.
    assert!(table
        .iter()
        .filter(|p| p.exe != Exe::ClaudeCli)
        .all(|p| p.cwd == Fact::Unknown));

    // No boot: no identities, so no table.
    let fake = m.build();
    fake.set_run(
        machine::SYSCTL,
        vec!["-n".into(), "kern.boottime".into()],
        Err(RunError::TimedOut),
    );
    let snap = observe(&fake, &claude());
    assert_eq!((snap.boot, snap.processes), (Fact::Unknown, Fact::Unknown));

    m.ps_broken = true;
    assert_eq!(observe(&m.build(), &claude()).processes, Fact::Unknown);
    m.ps_broken = false;

    // lsof failing leaves the table known and the cwds unknown.
    let fake = m.build();
    if let Some(run) = m.lsof_cwd_run() {
        fake.on(&run, vec![Ok(Output::exit(2, "", "lsof: boom"))]);
    }
    let snap = observe(&fake, &claude());
    let table = snap.processes.known().cloned().unwrap_or_default();
    assert!(!table.is_empty() && table.iter().all(|p| p.cwd == Fact::Unknown));
}

#[test]
fn apps_schemes_and_front() {
    let m = claude_machine(false, false);
    let snap = observe(&m.build(), &claude());
    let codex = snap
        .apps
        .known()
        .into_iter()
        .flatten()
        .find(|a| a.app == App::CodexDesktop)
        .cloned();
    assert_eq!(
        codex
            .as_ref()
            .map(|a| (a.installed, a.version.clone(), a.running)),
        Some((true, Some("26.1103.60933".to_owned()), false))
    );
    assert!(snap
        .schemes
        .known()
        .into_iter()
        .flatten()
        .any(|s| s.scheme == "claude"
            && s.bundle.as_deref() == Some("com.anthropic.claudefordesktop")));
    assert_eq!(snap.front.app, Fact::Known(Some(App::ClaudeDesktop)));
    assert_eq!(snap.front.view, Fact::Unknown);
    assert_eq!(snap.front.pane_tty, Fact::Known(None));

    // A failed registry query cannot say "nothing installed": apps, schemes
    // and the front app are `Unknown`, and the process table still stands.
    let fake = m.build();
    fake.on(
        &machine::apps_run(&cfg()),
        vec![Ok(Output::exit(1, "", "boom"))],
    );
    let snap = observe(&fake, &claude());
    assert_eq!((snap.apps, snap.schemes), (Fact::Unknown, Fact::Unknown));
    assert!(snap.processes.is_known());
    assert_eq!(
        (snap.front.app, snap.front.pane_tty, snap.front.view),
        (Fact::Unknown, Fact::Unknown, Fact::Unknown)
    );
    let fake = m.build();
    fake.on(
        &machine::apps_run(&cfg()),
        vec![Ok(Output::ok("{\"not\":\"it\"}"))],
    );
    let snap = observe(&fake, &claude());
    assert_eq!(snap.apps, Fact::Unknown);
    assert!(snap.processes.is_known());
}

#[test]
fn front_pane_of_a_terminal() {
    let mut m = Machine::new();
    m.apps_json = apps_json(Some(App::Ghostty), &[App::Ghostty]);
    let snap = observe(&m.build(), &codex());
    assert_eq!(
        snap.front.pane_tty,
        Fact::Known(Some("/dev/ttys000".to_owned()))
    );
    assert_eq!(snap.front.view, Fact::Known(None));
    let refused = "execution error: Not authorized to send Apple events to Ghostty. (-1743)\n";
    for failure in [
        Ok(Output::exit(1, "", refused)),
        Err(RunError::TimedOut),
        Ok(Output::ok("not a tty")),
    ] {
        m.front_tty = Some(failure);
        assert_eq!(observe(&m.build(), &codex()).front.pane_tty, Fact::Unknown);
    }
    m.apps_json = apps_json(Some(App::AppleTerminal), &[App::AppleTerminal]);
    m.front_tty = None;
    let fake = m.build();
    let snap = observe(&fake, &codex());
    assert_eq!(
        snap.front.pane_tty,
        Fact::Known(Some("/dev/ttys000".to_owned()))
    );
    let script = machine::script_run(
        scripts::TERMINAL_FRONT_TTY,
        &[],
        cfg().common.timeouts.script_read,
    );
    assert!(fake.runs().contains(&(script.program, script.args)));
}

#[test]
fn tools_resolution_versions_and_cache() {
    let m = Machine::new();
    let fake = m.build();
    let mut observer = MacObserver::new(&fake, cfg());
    let snap = observer.observe(&codex());
    let tool = |t: Tool| snap.tools.iter().find(|i| i.tool == t).cloned();
    let codex_tool = tool(Tool::CodexCli);
    assert_eq!(
        codex_tool.as_ref().map(|t| t.version.clone()),
        Some(Fact::Known(Some("0.167.0".into())))
    );
    assert_eq!(
        codex_tool.and_then(|t| t.path).as_deref(),
        Some("/opt/homebrew/bin/codex")
    );
    let claude_tool = tool(Tool::ClaudeCli);
    assert_eq!(
        claude_tool.map(|t| (t.version, t.path)),
        Some((Fact::Known(None), None))
    );
    let _ = observer.observe(&codex());
    let version_runs = fake
        .runs()
        .iter()
        .filter(|(p, a)| p == "/opt/homebrew/bin/codex" && a == &["--version"])
        .count();
    assert_eq!(version_runs, 1, "cached per (path, mtime, size)");

    // Found but the version call fails: unknown, with the path.
    let fake = m.build();
    fake.on(
        &Run::new("/opt/homebrew/bin/codex", &["--version"]),
        vec![Err(RunError::TimedOut)],
    );
    let snap = observe(&fake, &codex());
    let info = snap
        .tools
        .iter()
        .find(|t| t.tool == Tool::CodexCli)
        .cloned();
    assert_eq!(
        info.map(|t| (t.version, t.path)),
        Some((Fact::Unknown, Some("/opt/homebrew/bin/codex".into())))
    );
    // A non-executable file is not a tool.
    let fake = FakeSystem::new();
    fake.add_file("/opt/homebrew/bin/opencode", b"x".to_vec(), 1);
    let snap = observe(&fake, &opencode());
    let info = snap
        .tools
        .iter()
        .find(|t| t.tool == Tool::OpenCodeCli)
        .cloned();
    assert_eq!(info.map(|t| t.path), Some(None));
}

#[test]
fn reservation_and_history() {
    let m = Machine::new();
    let snap = observe(&m.build(), &codex());
    assert_eq!(snap.reservation, Fact::Known(None));
    assert_eq!(snap.last_seen_host, Fact::Unknown);
    assert!(snap.capabilities.is_empty());
    let fake = m.build();
    let name =
        session_navigation_native::common::reservations::file_name(&codex()).unwrap_or_default();
    fake.add_file(
        &format!("{}/{name}", cfg().common.paths.reservation_dir),
        b"{corrupt".to_vec(),
        1,
    );
    assert_eq!(observe(&fake, &codex()).reservation, Fact::Unknown);

    struct Seen;
    impl HostHistory for Seen {
        fn last_seen(&self, _: &SessionRef) -> Fact<Host> {
            Fact::Known(Host::Terminal(TerminalKind::AppleTerminal))
        }
    }
    let fake = m.build();
    let snap = MacObserver::with_history(&fake, cfg(), Seen).observe(&codex());
    assert_eq!(
        snap.last_seen_host,
        Fact::Known(Host::Terminal(TerminalKind::AppleTerminal))
    );
}

#[test]
fn hostile_ids_touch_no_provider_store() {
    for session in [
        SessionRef::new(
            session_navigation::model::Provider::Claude,
            "../../../etc/passwd",
        ),
        SessionRef::new(session_navigation::model::Provider::Codex, "x' OR 1=1; --"),
        SessionRef::new(session_navigation::model::Provider::OpenCode, "ses_../../x"),
    ] {
        let fake = Machine::new().build();
        let snap = observe(&fake, &session);
        assert_eq!(snap.environment, Fact::Unknown);
        let probes = all_args(&fake);
        assert!(
            !probes.contains("sqlite3") && !probes.contains("curl"),
            "{probes}"
        );
        assert!(!fake
            .calls()
            .iter()
            .any(|c| format!("{c:?}").contains("passwd")));
    }
}

#[test]
fn every_failure_leaves_the_snapshot_decidable_or_unknown_never_panics() {
    // Everything broken at once: no scripted runs, no files.
    let fake = FakeSystem::new();
    for session in [claude(), codex(), opencode()] {
        let snap = observe(&fake, &session);
        assert_eq!(
            (snap.boot, snap.processes.clone()),
            (Fact::Unknown, Fact::Unknown)
        );
        let decision = session_navigation::decide::decide(&snap, &Default::default());
        assert_eq!(
            decision.plan,
            session_navigation::plan::Plan::Unavailable(
                session_navigation::model::Reason::EvidenceIncomplete
            )
        );
        assert!(matches!(
            snap.provider,
            ProviderEvidence::Claude(_)
                | ProviderEvidence::Codex(_)
                | ProviderEvidence::OpenCode(_)
        ));
    }
}
