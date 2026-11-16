//! Real batch POSTs from distinct processes, sharing only the coordinator.
use super::fake_s3::{FakeS3, Reply};
use super::{accepted, batch, context};
use agent_relay_desktop_core::history::{
    cloud::StoredAuth,
    delivery::{worker::Receiver, DeliveryFailure},
    destination::{account_id, prepare, ReceiverState, RelayHistoryReceiver},
};
use serde_json::json;
use std::{
    io::Write,
    net::TcpListener,
    path::Path,
    process::{Child, Command, Output, Stdio},
    sync::mpsc,
    time::Duration,
};

fn command(home: &Path, base: &str, workspace: &str, expected: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "serialized_posts::post_from_process",
            "--nocapture",
        ])
        .env("RELAYHISTORY_HOME", home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("RELAY_POST_TEST_BASE", base)
        .env("RELAY_POST_TEST_WORKSPACE", workspace)
        .env("RELAY_POST_TEST_EXPECTED", expected)
        .env_remove("RELAYHISTORY_BASE_URL")
        .env_remove("AI_HIST_BASE_URL")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}
fn success(output: Output) {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
fn finish(child: Child) {
    success(child.wait_with_output().unwrap())
}
fn retry_at(output: &Output) -> i64 {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("RETRY_AT="))
        .expect("child reported its retry deadline")
        .parse()
        .unwrap()
}

#[test]
fn post_from_process() {
    let Ok(base) = std::env::var("RELAY_POST_TEST_BASE") else {
        return;
    };
    let workspace = std::env::var("RELAY_POST_TEST_WORKSPACE").unwrap();
    let expected = std::env::var("RELAY_POST_TEST_EXPECTED").unwrap();
    let auth = StoredAuth {
        base_url: base.clone(),
        access_token: "rth_at_fixture".into(),
        access_token_expires_at: Some("2099-01-01T00:00:00Z".into()),
        org_id: Some("org-fixture".into()),
        workspace_id: Some(workspace.clone()),
        ..Default::default()
    };
    let receiver = RelayHistoryReceiver {
        base_url: Some(base.clone()),
        state: ReceiverState::hosted_with_session(base, auth),
        ..Default::default()
    };
    let mut batch = batch();
    batch.job_id = format!("synthetic-job-{}", std::process::id());
    batch.batch_id = format!("synthetic-batch-{}", std::process::id());
    batch.account_id = account_id("org-fixture", Some(&workspace));
    let prepared = prepare(batch.clone()).unwrap();
    let mut context = context(&batch);
    context.timeout_ms = 1_500;
    let result = receiver.send(&prepared, &batch, &context);
    match expected.as_str() {
        "ok" => {
            result.unwrap();
        }
        "busy" => {
            let failure = result.unwrap_err();
            assert_eq!(failure.failure, DeliveryFailure::Transient);
            assert_eq!(failure.local_wait_ms, Some(1_000));
        }
        "rate_limited" => {
            let failure = result.unwrap_err();
            assert_eq!(failure.failure, DeliveryFailure::RateLimited);
            assert!(failure.retry_after_ms.unwrap() > chrono::Utc::now().timestamp_millis());
            println!("RETRY_AT={}", failure.retry_after_ms.unwrap());
        }
        "transient_hint" => {
            let failure = result.unwrap_err();
            assert_eq!(failure.failure, DeliveryFailure::Transient);
            assert!(failure.retry_after_ms.unwrap() > chrono::Utc::now().timestamp_millis());
            println!("RETRY_AT={}", failure.retry_after_ms.unwrap());
        }
        _ => panic!("unknown expected outcome"),
    }
}

#[test]
fn another_process_defers_until_the_entire_response_is_read() {
    // Send headers and part of the receipt, then pause its body. Releasing
    // the gate when POST returns headers would permit an overlapping send.
    let home = tempfile::tempdir().unwrap();
    let (arrived_tx, arrived_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let request = super::fake_s3::read(&stream);
        assert_eq!(request.path(), "/v1/delivery/batches");
        let batch = serde_json::from_value(request.json()["batch"].clone()).unwrap();
        let body = accepted(&batch).to_string();
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
        let middle = body.len() / 2;
        stream.write_all(&body.as_bytes()[..middle]).unwrap();
        stream.flush().unwrap();
        arrived_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        stream.write_all(&body.as_bytes()[middle..]).unwrap();
        listener.set_nonblocking(true).unwrap();
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    });
    let first = command(home.path(), &base, "workspace", "ok")
        .spawn()
        .unwrap();
    arrived_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let second = command(home.path(), &base, "workspace", "busy")
        .env("RELAYHISTORY_HOME", home.path().join("another-install"))
        .output()
        .unwrap();
    release_tx.send(()).unwrap();
    finish(first);
    server.join().unwrap();
    success(second);
}

#[test]
fn another_process_and_another_job_cannot_bypass_a_receiver_cooldown() {
    for (status, expected) in [(429, "rate_limited"), (503, "transient_hint")] {
        let home = tempfile::tempdir().unwrap();
        let server = FakeS3::serve(move |_, request| {
            assert_eq!(request.path(), "/v1/delivery/batches");
            Reply::json(status, json!({"error":{"code":"rate_limited"}})).with("Retry-After", "300")
        });
        let before = chrono::Utc::now().timestamp_millis();
        let first = command(home.path(), &server.base, "workspace", expected)
            .output()
            .unwrap();
        let deadline = retry_at(&first);
        assert!(deadline >= before + 300_000);
        success(first);
        // A fresh process, receiver state and batch attempt share the deadline.
        let second = command(home.path(), &server.base, "workspace", expected)
            .env("RELAYHISTORY_HOME", home.path().join("another-install"))
            .output()
            .unwrap();
        assert_eq!(retry_at(&second), deadline);
        success(second);
        assert_eq!(server.join().len(), 1);
    }
}

#[test]
fn an_explicit_sandbox_profile_keeps_its_coordinator_separate() {
    let desktop = tempfile::tempdir().unwrap();
    let sandbox = tempfile::tempdir().unwrap();
    let mut first = true;
    let server = FakeS3::serve(move |_, request| {
        if first {
            first = false;
            Reply::json(429, json!({"error":{"code":"rate_limited"}})).with("Retry-After", "300")
        } else {
            let batch = serde_json::from_value(request.json()["batch"].clone()).unwrap();
            Reply::json(200, accepted(&batch))
        }
    });
    success(
        command(desktop.path(), &server.base, "workspace", "rate_limited")
            .output()
            .unwrap(),
    );
    success(
        command(sandbox.path(), &server.base, "workspace", "ok")
            .output()
            .unwrap(),
    );
    assert_eq!(server.join().len(), 2);
    for profile in [desktop.path(), sandbox.path()] {
        assert_eq!(
            std::fs::read_dir(profile.join(".agentworkforce/delivery-posts"))
                .unwrap()
                .count(),
            1
        );
    }
}

#[test]
fn a_workspace_cooldown_does_not_block_another_workspace() {
    let home = tempfile::tempdir().unwrap();
    let server = FakeS3::serve(move |_, request| {
        let batch = serde_json::from_value(request.json()["batch"].clone()).unwrap();
        if request.json()["batch"]["account_id"] == account_id("org-fixture", Some("limited")) {
            Reply::json(429, json!({"error":{"code":"rate_limited"}})).with("Retry-After", "300")
        } else {
            Reply::json(200, accepted(&batch))
        }
    });
    success(
        command(home.path(), &server.base, "limited", "rate_limited")
            .output()
            .unwrap(),
    );
    success(
        command(home.path(), &server.base, "free", "ok")
            .output()
            .unwrap(),
    );
    assert_eq!(server.join().len(), 2);
}
