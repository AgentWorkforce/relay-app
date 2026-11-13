//! Synthetic private-core replies verify the hosted client's credential helper.
#![cfg(unix)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::process::{Command, Output};

fn invoke(reply: Value) -> Output {
    invoke_at(reply, false)
}

fn invoke_at(reply: Value, explicit_socket: bool) -> Output {
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("core.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                Err(error) => panic!("Synthetic core was not reached: {error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        for method in ["hello", "account.mcp_headers"] {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], method);
            let mut response = json!({"jsonrpc":"2.0", "id":request["id"]});
            if method == "hello" {
                response["result"] = json!({});
            } else {
                assert_eq!(
                    request["params"],
                    json!({
                        "site_url":"https://agentrelay.com", "workspace_id":"workspace-1", "account_id":"account-1"
                    })
                );
                response
                    .as_object_mut()
                    .unwrap()
                    .extend(reply.as_object().unwrap().clone());
            }
            writeln!(stream, "{response}").unwrap();
        }
    });
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-relay-desktop-probe"));
    command.args([
        "mcp-headers",
        "--site",
        "https://agentrelay.com",
        "--workspace",
        "workspace-1",
        "--account",
        "account-1",
    ]);
    if explicit_socket {
        command.arg("--socket").arg(&socket).env(
            "AGENT_RELAY_CORE_SOCKET",
            root.path().join("unreachable-production.sock"),
        );
    } else {
        command.env("AGENT_RELAY_CORE_SOCKET", &socket);
    }
    let output = command.output().unwrap();
    server.join().unwrap();
    output
}

#[test]
fn explicit_dev_socket_overrides_an_unrelated_default_without_fallback() {
    let output = invoke_at(
        json!({"result":{"headers":{"Authorization":"Bearer synthetic-dev-only"}}}),
        true,
    );
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["Authorization"],
        "Bearer synthetic-dev-only"
    );
}

#[test]
fn helper_prints_only_the_synthetic_header_object() {
    let output =
        invoke(json!({"result":{"headers":{"Authorization":"Bearer synthetic-fixture-only"}}}));
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"Authorization":"Bearer synthetic-fixture-only", "X-Agent-Relay-Desktop":"1"})
    );
}

#[test]
fn helper_hides_raw_core_errors_and_emits_no_headers_on_failure() {
    let output = invoke(json!({"error":{"code":-32000,
        "message":"Authorization: Bearer synthetic-private-fixture", "data":{"kind":"unauthorized","retryable":false}}}));
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("Open the app and check Account health"));
    assert!(!error.contains("synthetic-private-fixture"));
    assert!(!error.contains("Bearer"));
}

#[test]
fn all_account_pins_are_required_before_contacting_the_core() {
    let output = Command::new(env!("CARGO_BIN_EXE_agent-relay-desktop-probe"))
        .args(["mcp-headers", "--site", "https://agentrelay.com"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("--workspace"));
    assert!(error.contains("--account"));
}
