//! `agent-relay-probe session-open plan|open|release|strings`, as the macOS
//! app runs it:
//! request JSON on stdin, one answer object on stdout, exit 0 for any
//! answer (refusals included), 2 and a stderr line for a malformed request.
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Output, Stdio};

const PROBE: &str = env!("CARGO_BIN_EXE_agent-relay-desktop-probe");

/// Runs the command with nothing from this computer's profile: a fresh home.
fn run(home: &std::path::Path, args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(PROBE)
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("APPDATA", home.join("AppData/Roaming"))
        .env("LOCALAPPDATA", home.join("AppData/Local"))
        .env(
            "SystemRoot",
            std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn a_refusal_is_an_answer_on_stdout() {
    let home = tempfile::tempdir().unwrap();
    let app = home.path().join("app");
    let request = json!({"source": "cursor", "session_id": "made-up", "app_data_dir": app});
    for verb in ["plan", "open", "release"] {
        let out = run(
            home.path(),
            &["session-open", verb],
            request.to_string().as_bytes(),
        );
        assert!(out.status.success(), "{verb}: {out:?}");
        assert!(out.stderr.is_empty(), "{verb}: {out:?}");
        let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
        let view = if verb == "open" {
            &answer["plan"]
        } else {
            &answer
        };
        assert_eq!(view["action"], "unavailable");
        assert_eq!(view["reason_code"], "unsupported_source");
    }
}

#[test]
fn a_malformed_request_is_exit_two_and_one_stderr_line() {
    let home = tempfile::tempdir().unwrap();
    for input in [&b"{not json"[..], br#"{"source":"codex"}"#] {
        let out = run(home.path(), &["session-open", "plan"], input);
        assert_eq!(out.status.code(), Some(2), "{out:?}");
        assert!(out.stdout.is_empty());
        let line = String::from_utf8(out.stderr).unwrap();
        assert!(line.starts_with("session-open:"), "{line}");
    }
    let out = run(home.path(), &["session-open", "resume"], b"{}");
    assert_eq!(out.status.code(), Some(2));
    let help = run(home.path(), &["session-open", "--help"], b"");
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("session-open plan|open"));
}

#[test]
fn a_plan_for_a_made_up_session_answers_and_writes_nothing() {
    let home = tempfile::tempdir().unwrap();
    let app = home.path().join("app");
    let request = json!({
        "source": "codex",
        "session_id": "01900000-0000-7000-8000-0000000000ff",
        "app_data_dir": app,
    });
    let out = run(
        home.path(),
        &["session-open", "plan"],
        request.to_string().as_bytes(),
    );
    assert!(out.status.success(), "{out:?}");
    let view: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_ne!(view["action"], "resume", "{view}");
    assert!(view["state"].is_string(), "{view}");
    assert!(
        !app.exists(),
        "a plan creates nothing in the app's directory"
    );
}

#[test]
fn strings_prints_the_checked_in_table() {
    let home = tempfile::tempdir().unwrap();
    let out = run(home.path(), &["session-open", "strings"], b"");
    assert!(out.status.success(), "{out:?}");
    assert!(out.stderr.is_empty());
    let file = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../shared/session-open-strings.json"
    ))
    .unwrap();
    assert_eq!(
        out.stdout, file,
        "shared/session-open-strings.json is stale"
    );
}

#[test]
fn a_release_without_a_pending_launch_answers_a_plan_and_opens_nothing() {
    let home = tempfile::tempdir().unwrap();
    let app = home.path().join("app");
    let request = json!({
        "source": "codex",
        "session_id": "01900000-0000-7000-8000-0000000000ff",
        "app_data_dir": app,
    });
    let out = run(
        home.path(),
        &["session-open", "release"],
        request.to_string().as_bytes(),
    );
    assert!(out.status.success(), "{out:?}");
    let view: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(view["state"].is_string(), "{view}");
    assert_eq!(view["pending_launch"], false, "{view}");
    assert!(view["strings"]["open_session"].is_string(), "{view}");
    assert!(view["destination"].is_string(), "{view}");
}
