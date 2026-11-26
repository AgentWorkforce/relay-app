//! Opt-in checks against the real machine. Skipped unless
//! `SESSION_NAVIGATION_NATIVE=1`:
//!
//! ```text
//! SESSION_NAVIGATION_NATIVE=1 cargo test -p session-navigation-native --test native_acceptance -- --nocapture
//! ```
//!
//! It opens two disposable Ghostty windows running fake `codex`
//! programs (a script and a copy of this test executable), focuses it, and stops it through the real executor.
//! It needs Automation permission for Ghostty already granted and touches no
//! real session. A skipped run is not evidence of anything.
#![cfg(all(feature = "macos", unix))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use session_navigation::evidence::{Exe, Fact, ProcessInfo};
use session_navigation::hosts::PaneRef;
use session_navigation::model::{BootId, ProcessIdentity, Reason, TerminalKind};
use session_navigation::plan::{Command, Dispatch, LaunchContext, Step};
use session_navigation::ports::Execute;
use session_navigation_native::macos::machine;
use session_navigation_native::{MacConfig, MacExecutor, RealSystem};

fn table(cfg: &MacConfig) -> Vec<ProcessInfo> {
    let boot = BootId(machine::boot(&RealSystem, cfg).expect("boot time"));
    let mut table = machine::table(&RealSystem, cfg, boot).expect("process table");
    machine::fill_cwds(&RealSystem, cfg, &mut table);
    table
}

fn on_tty(cfg: &MacConfig, tty: &str) -> Option<ProcessInfo> {
    table(cfg)
        .into_iter()
        .find(|p| p.tty.as_deref() == Some(tty) && p.exe == Exe::CodexCli)
}

fn wait_for<T>(mut probe: impl FnMut() -> Option<T>) -> Option<T> {
    for _ in 0..40 {
        if let Some(found) = probe() {
            return Some(found);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    None
}

const CLOSE_BY_TTY: &str = r#"on run argv
  tell application id "com.mitchellh.ghostty"
    set c to every terminal whose tty is (item 1 of argv)
    if (count of c) is 1 then close (item 1 of c)
  end tell
end run"#;

/// The `CODEX_HOME` value that turns a copy of this executable into the
/// disposable fake agent.
const FAKE_AGENT: &str = "/tmp/session-navigation-fake-agent";

#[test]
fn fake_agent_sleeps() {
    if std::env::var("CODEX_HOME").as_deref() == Ok(FAKE_AGENT) {
        std::thread::sleep(Duration::from_secs(90));
    }
}

#[test]
fn ghostty_launch_focus_and_stop_a_disposable_process() {
    if std::env::var("SESSION_NAVIGATION_NATIVE").as_deref() != Ok("1") {
        eprintln!("skipped: set SESSION_NAVIGATION_NATIVE=1 to run against this machine");
        return;
    }
    let cfg = MacConfig::from_env().expect("HOME");
    let dir = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("session-navigation-native-{}", std::process::id()));
    let work = dir.join("work dir with 'quotes' $and spaces");
    std::fs::create_dir_all(&work).unwrap();
    // 1. Quoting: a script named `codex` records the argv and environment
    // it was started with, then exits.
    let script_dir = dir.join("script");
    std::fs::create_dir_all(&script_dir).unwrap();
    let script = script_dir.join("codex");
    let marker = dir.join("argv.txt");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n{{ printf '%s\\n' \"$@\"; printf '%s\\n' \"$CODEX_HOME\"; pwd; }} > '{}'\n",
            marker.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut executor = MacExecutor::new(RealSystem, cfg.clone());
    let awkward = "it's \"quoted\" $HOME `id`; *";
    let command = Command {
        program: script.to_string_lossy().into_owned(),
        args: vec!["resume".into(), awkward.into()],
        cwd: work.to_string_lossy().into_owned(),
        env: BTreeMap::from([("CODEX_HOME".to_owned(), "/tmp/native codex home".to_owned())]),
    };

    // An environment key outside the allowlist never reaches the terminal.
    let mut leaky = command.clone();
    leaky.env.insert("AWS_SECRET".into(), "x".into());
    let refused = executor.execute(&Step::Launch {
        terminal: TerminalKind::Ghostty,
        command: leaky,
        context: LaunchContext::native(),
    });
    assert_eq!(refused, Dispatch::Failed(Reason::DispatchFailed));

    let first = executor.execute(&Step::Launch {
        terminal: TerminalKind::Ghostty,
        command,
        context: LaunchContext::native(),
    });
    eprintln!("script launch -> {first:?}");
    let Dispatch::Opened { tty: script_tty } = first else {
        panic!("expected a tty from the script launch, got {first:?}");
    };
    let seen = wait_for(|| {
        std::fs::read_to_string(&marker)
            .ok()
            .filter(|t| t.lines().count() >= 4)
    })
    .expect("marker file");
    assert_eq!(
        seen.lines().collect::<Vec<_>>(),
        vec![
            "resume",
            awkward,
            "/tmp/native codex home",
            &*work.to_string_lossy()
        ]
    );

    // 2. Identity: a copy of this test executable named `codex` keeps that
    // kernel name, so the table classifies it as a Codex CLI. It runs the
    // `fake_agent_sleeps` test below. (A copy of a system binary such as
    // `/bin/sleep` is killed at launch by macOS.)
    let fake = dir.join("codex");
    std::fs::copy(std::env::current_exe().unwrap(), &fake).unwrap();
    let opened = executor.execute(&Step::Launch {
        terminal: TerminalKind::Ghostty,
        command: Command {
            program: fake.to_string_lossy().into_owned(),
            args: vec!["fake_agent_sleeps".into(), "--exact".into()],
            cwd: work.to_string_lossy().into_owned(),
            env: BTreeMap::from([("CODEX_HOME".to_owned(), FAKE_AGENT.to_owned())]),
        },
        context: LaunchContext::native(),
    });
    eprintln!("launch -> {opened:?}");
    let Dispatch::Opened { tty } = opened else {
        panic!("expected a tty from the launch, got {opened:?}");
    };
    let process = wait_for(|| on_tty(&cfg, &tty)).expect("fake codex on the new tty");
    eprintln!(
        "process pid={} cwd_known={}",
        process.identity.pid,
        process.cwd.is_known()
    );
    assert_eq!(
        process.cwd,
        Fact::Known(work.to_string_lossy().into_owned())
    );
    assert_eq!(process.ruid, cfg.uid);

    // A recycled identity is refused before the terminal is touched.
    let stale = ProcessIdentity {
        start: process.identity.start + 1,
        ..process.identity
    };
    let focus = |executor: &mut MacExecutor<RealSystem>, identity| {
        executor.execute(&Step::FocusPane {
            terminal: TerminalKind::Ghostty,
            pane: PaneRef::Tty { tty: tty.clone() },
            process: identity,
        })
    };
    assert_eq!(
        focus(&mut executor, stale),
        Dispatch::Failed(Reason::TargetGone)
    );
    let focused = focus(&mut executor, process.identity);
    eprintln!("focus -> {focused:?}");
    assert_eq!(focused, Dispatch::Dispatched);
    assert_eq!(
        machine::front_pane(&RealSystem, &cfg, TerminalKind::Ghostty),
        Fact::Known(Some(tty.clone()))
    );

    // Stop: never a stale identity, never a non-agent process.
    assert_eq!(
        executor.execute(&Step::Stop { process: stale }),
        Dispatch::Failed(Reason::TargetGone)
    );
    let shell = table(&cfg)
        .into_iter()
        .find(|p| p.identity.pid == std::process::id())
        .expect("this test process");
    assert_eq!(
        executor.execute(&Step::Stop {
            process: shell.identity
        }),
        Dispatch::Failed(Reason::TargetGone)
    );
    assert_eq!(
        executor.execute(&Step::Stop {
            process: process.identity
        }),
        Dispatch::Dispatched
    );
    let gone = wait_for(|| on_tty(&cfg, &tty).is_none().then_some(()));
    assert!(gone.is_some(), "the fake codex should exit after TERM");
    // Ghostty keeps a window open after its command exits (seen natively):
    // close the two this test opened, and only once nothing runs on them.
    for tty in [&script_tty, &tty] {
        let idle = table(&cfg).iter().all(|p| p.tty.as_deref() != Some(tty));
        if idle {
            let _ = std::process::Command::new("/usr/bin/osascript")
                .args(["-e", CLOSE_BY_TTY, tty])
                .status();
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
