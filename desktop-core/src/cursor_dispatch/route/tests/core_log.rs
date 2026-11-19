//! Refusals and a new chat's registration reach `core.log`, not only
//! stderr. Core's log is set once per process, when the core starts, so a
//! child test process starts it in a temporary home and the parent reads
//! the file: no other test's log changes.
//!
//! Unix only: there the start stops at the socket, after the log is set,
//! because a regular file is in the socket's place. A Windows named pipe has
//! no file to block it, so the core would serve instead.
#![cfg(unix)]
use super::fresh_chat::{admit, fresh, FRESH};
use super::routes::{holding, HOOK_PID};
use super::*;
use agent_relay_node::relay::daemon::{serve, ServeOptions};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;

const CHILD: &str = "RELAY_CURSOR_CORE_LOG_CHILD";

#[test]
fn refusals_and_a_new_chat_s_registration_reach_core_log() {
    let temp = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "cursor_dispatch::route::tests::core_log::write_core_log_child",
        ])
        .env(CHILD, temp.path())
        .env("HOME", temp.path().join("home"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("1 passed"), "{stdout}");

    let logs = if cfg!(target_os = "macos") {
        temp.path().join("home/Library/Logs/Agent Relay")
    } else {
        temp.path().join("data/logs")
    };
    let saved = std::fs::read_to_string(logs.join("core.log")).unwrap();
    assert!(
        saved.contains("[cursor-dispatch] refused a call: caller_unverified"),
        "{saved}"
    );
    assert!(
        saved.contains("[cursor-dispatch] put a chat on the relay in ")
            && saved.contains(" ms (identity synced: true)"),
        "{saved}"
    );
    // The chat itself is never written.
    assert!(!saved.contains(FRESH), "{saved}");
}

/// Run by the test above in a child process only.
#[tokio::test(flavor = "multi_thread")]
async fn write_core_log_child() {
    let Some(temp) = std::env::var_os(CHILD).map(PathBuf::from) else {
        return;
    };
    let run = temp.join("run");
    std::fs::DirBuilder::new().mode(0o700).create(&run).unwrap();
    let socket = run.join("core.sock");
    std::fs::write(&socket, "not a socket").unwrap();
    let options = ServeOptions {
        socket: Some(socket),
        app_data_dir: Some(temp.join("data")),
        exit_on_stdin_eof: false,
        headless: false,
        connect_ephemeral: false,
        cli_managed: false,
    };
    // Off the test's runtime: the core builds its own.
    let started = std::thread::spawn(move || serve(options)).join().unwrap();
    let error = started.unwrap_err().to_string();
    assert!(error.contains("is not a socket"), "{error}");

    let (f, _relaycast) = fresh(true).await;
    // A refusal on the intent route.
    let (path, query) = crate::cursor_dispatch::INTENT_ROUTE
        .split_once('?')
        .unwrap();
    let request = Request {
        method: "POST".into(),
        path: path.into(),
        query: url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect(),
        body: b"{}".to_vec(),
        ..Request::default()
    };
    let uid = agent_relay_node::relay::peer::current_uid();
    let error = f
        .outbound
        .handle(HOOK_PID, uid + 1, &request)
        .await
        .unwrap_err();
    assert_eq!(error.code, "caller_unverified");
    // A new chat put on the relay, its identity synced.
    assert_eq!(admit(&f, HOOK_PID, holding(&f, FRESH)).await, Ok(()));
}
