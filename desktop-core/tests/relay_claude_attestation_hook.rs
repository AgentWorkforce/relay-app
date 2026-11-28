//! The Claude Code PreToolUse hook binary against a synthetic session socket:
//! Claude's stdin in, `updatedInput` out. No core, broker or Claude runs.
#![cfg(unix)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Output, Stdio};

fn hook_input() -> Value {
    json!({
        "session_id": "0c6b1f0e-2f2a-4b7e-9d3c-5a1e8f7b6c4d",
        "hook_event_name": "PreToolUse",
        "tool_name": "mcp__agent-relay__send_message",
        "tool_use_id": "toolu_01ABC",
        "tool_input": {"to": "alice", "text": "hi", "_agent_relay_attestation": "forged"},
    })
}

/// Runs the hook against a socket that answers one request with `status`
/// and `reply`, returning the hook's output and the request body it sent.
fn run(status: u16, reply: Value) -> (Output, Value) {
    run_with_input(status, reply, &hook_input())
}

fn run_with_input(status: u16, reply: Value, input: &Value) -> (Output, Value) {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("relay.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line.trim_end(), "POST /claude/attestation HTTP/1.1");
        let mut length = 0;
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line.trim_end().is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse().unwrap();
                }
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let payload = reply.to_string();
        write!(
            stream,
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        )
        .unwrap();
        serde_json::from_slice::<Value>(&body).unwrap()
    });
    let output = invoke(&socket, input);
    (output, server.join().unwrap())
}

fn invoke(socket: &std::path::Path, input: &Value) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agent-relay-desktop-probe"))
        .args(["claude-attestation-hook", "--socket"])
        .arg(socket)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn a_signed_call_replaces_the_arguments_with_the_proof_added() {
    let (output, sent) = run(200, json!({"ok": true, "data": {"attestation": "h.p.s"}}));
    assert!(output.status.success());
    assert_eq!(
        sent,
        json!({
            "session_id": "0c6b1f0e-2f2a-4b7e-9d3c-5a1e8f7b6c4d",
            "tool_name": "mcp__agent-relay__send_message",
            "tool_use_id": "toolu_01ABC",
        })
    );
    let printed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        printed,
        json!({"hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "updatedInput": {"to": "alice", "text": "hi", "_agent_relay_attestation": "h.p.s"},
        }})
    );
}

#[test]
fn a_refusal_prints_nothing_and_exits_cleanly() {
    let (output, _) = run(
        403,
        json!({"ok": false, "error": {"code": "not_a_relay_session", "message": "x"}}),
    );
    assert!(output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn an_unreachable_core_prints_nothing_and_exits_cleanly() {
    let root = tempfile::tempdir().unwrap();
    let output = invoke(&root.path().join("missing.sock"), &hook_input());
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn handoff_retry_guidance_requires_both_status_and_error_code() {
    for (tool, fallback) in [
        ("join_relay", Some("retry this join")),
        ("send_message", None),
    ] {
        let mut input = hook_input();
        input["tool_name"] = json!(format!("mcp__agent-relay__{tool}"));
        for (status, code, expected_reason) in [
            (503, "handoff_failed", Some("retry this tool call")),
            (503, "registration_required", fallback),
            (403, "handoff_failed", fallback),
        ] {
            let (output, _) = run_with_input(
                status,
                json!({"ok": false, "error": {"code": code}}),
                &input,
            );
            assert!(output.status.success());
            if let Some(expected) = expected_reason {
                let printed: Value = serde_json::from_slice(&output.stdout).unwrap();
                let decision = &printed["hookSpecificOutput"];
                assert_eq!(decision["permissionDecision"], "deny");
                assert!(decision.get("updatedInput").is_none());
                let reason = decision["permissionDecisionReason"].as_str().unwrap();
                assert!(
                    reason.contains(expected),
                    "{tool} {status} {code}: {reason}"
                );
            } else {
                assert!(output.stdout.is_empty(), "{tool} {status} {code}");
            }
        }
    }
}

#[test]
fn stale_desktop_identity_denies_the_call_without_a_proof() {
    let (output, _) = run(
        409,
        json!({"ok": false, "error": {"code": "claude_identity_stale"}}),
    );
    assert!(output.status.success());
    let printed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(printed["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(printed["hookSpecificOutput"].get("updatedInput").is_none());
}

#[test]
fn a_core_that_closes_after_taking_the_request_denies_the_call() {
    // #2283: the request may have started a handoff, so the call must not
    // go out unsigned ahead of its proof.
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("relay.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line.trim_end(), "POST /claude/attestation HTTP/1.1");
        // Closed without an answer.
    });
    let output = invoke(&socket, &hook_input());
    server.join().unwrap();
    assert!(output.status.success());
    let printed: Value = serde_json::from_slice(&output.stdout).unwrap();
    let decision = &printed["hookSpecificOutput"];
    assert_eq!(decision["permissionDecision"], "deny");
    assert!(decision.get("updatedInput").is_none());
    assert!(decision["permissionDecisionReason"]
        .as_str()
        .unwrap()
        .contains("retry this tool call"));
}
