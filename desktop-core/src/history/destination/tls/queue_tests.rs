use super::*;
use crate::history::delivery;
use crate::history::destination::{http_error, verdict};
use std::net::TcpListener;
use std::path::Path;
use std::time::Duration;

const EXPECTED_RETRY_MS: i64 = 60_000;

/// A job with one claimed batch, leased at `now`.
fn claimed(conn: &rusqlite::Connection, now: i64) -> delivery::ClaimedBatch {
    delivery::test_evidence(conn).execute(
        "INSERT INTO session_events(source,session_id,event_uid,ts_ms,role,kind,text) VALUES ('claude','a','a',42,'user','text','synthetic')", [],
    ).unwrap();
    let config = delivery::DeliveryJobConfig {
        destination_id: "relayhistory".into(),
        instance_id: "probe".into(),
        account_id: "relayhistory:account".into(),
        mapping_version: super::super::MAPPING_VERSION.into(),
        selection: delivery::ExportSelection {
            all_sources: true,
            kinds: vec!["session_event".into()],
            ..Default::default()
        },
        limits: delivery::DeliveryLimits::default(),
    };
    let job = delivery::create_job(conn, &config, now).unwrap();
    delivery::prepare_batch(conn, &job.job_id, now).unwrap();
    delivery::claim_batch(conn, &job.job_id, "first", 60_000, &|| now)
        .unwrap()
        .unwrap()
}

fn record(path: &Path, error: anyhow::Error, local: bool) {
    let conn = delivery::open_db(path).unwrap();
    let now = chrono::Utc::now().timestamp_millis();
    let claim = claimed(&conn, now);
    let outcome = verdict(&error);
    let status = outcome.record(&conn, &claim.lease, &|| now).unwrap();
    assert_eq!(status.state, if local { "active" } else { "blocked" });
    if local {
        assert_eq!(status.failure.as_deref(), Some("transient"));
        assert_eq!(outcome.retry_after_ms, None);
        assert_eq!(status.next_attempt_ms, now + EXPECTED_RETRY_MS);
    } else {
        assert_eq!(status.failure.as_deref(), Some("permission_denied"));
        assert_eq!(outcome.retry_after_ms, None);
    }
}

#[test]
fn queue_recovery_child() {
    let Ok(mode) = std::env::var("RELAY_HISTORY_OVERRIDE_QUEUE_CHILD") else {
        return;
    };
    let path =
        std::path::PathBuf::from(std::env::var_os("RELAY_HISTORY_OVERRIDE_QUEUE_PATH").unwrap());
    if mode == "record-local-guard" {
        // The real upload calls this guard before its HTTP transport. This
        // used to flatten the typed TLS refusal to PermissionDenied.
        let error =
            super::super::guard::require_destination("https://history.agentrelay.com").unwrap_err();
        assert!(cloud::HistoryTlsConfigurationRefused::caused_by(&error));
        record(&path, error, true);
        return;
    }
    if mode == "record-unsafe-guard" || mode == "record-private-guard" {
        let destination = if mode == "record-private-guard" {
            "https://localhost:8443"
        } else {
            "http://192.168.0.1"
        };
        let error = super::super::guard::require_destination(destination).unwrap_err();
        assert!(!cloud::HistoryTlsConfigurationRefused::caused_by(&error));
        record(&path, error, false);
        return;
    }
    if mode == "record-local" {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let agent = cloud::history_agent()
            .timeout(Duration::from_millis(100))
            .build();
        let error = agent
            .get(&format!("http://{}/", listener.local_addr().unwrap()))
            .call()
            .unwrap_err();
        record(&path, http_error(error, ""), true);
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        return;
    }
    if mode == "record-server" {
        let response = "HTTP/1.1 403 Forbidden\r\n\r\n".parse().unwrap();
        record(
            &path,
            http_error(ureq::Error::Status(403, response), ""),
            false,
        );
        return;
    }
    let conn = delivery::open_db(&path).unwrap();
    let job: String = conn
        .query_row("SELECT id FROM delivery_jobs", [], |r| r.get(0))
        .unwrap();
    let batch: String = conn
        .query_row("SELECT id FROM delivery_batches", [], |r| r.get(0))
        .unwrap();
    let status = delivery::status(&conn, &job).unwrap();
    if mode == "check-server" {
        assert_eq!(status.state, "blocked");
        assert!(
            delivery::claim_batch(&conn, &job, "restarted", 60_000, &|| status
                .next_attempt_ms
                .saturating_add(86_400_000))
            .unwrap()
            .is_none()
        );
        return;
    }
    assert_eq!(status.state, "active");
    if mode == "retry-local" {
        let now = chrono::Utc::now().timestamp_millis();
        assert!(
            now < status.next_attempt_ms,
            "the original local wait is still live"
        );
        assert!(
            delivery::claim_batch(&conn, &job, "waiting", 60_000, &|| now)
                .unwrap()
                .is_none()
        );
        assert!(delivery::retry_requested(&conn, &job, now).unwrap());
        let retried = delivery::claim_batch(&conn, &job, "retried", 60_000, &|| now)
            .unwrap()
            .unwrap();
        assert_eq!(retried.batch.batch_id, batch);
        return;
    }
    let claim = delivery::claim_batch(&conn, &job, "restarted", 60_000, &|| {
        status.next_attempt_ms.saturating_add(1)
    })
    .unwrap()
    .unwrap();
    assert_eq!(
        claim.batch.batch_id, batch,
        "retry must retain the same batch identity"
    );
    // A fresh process after clearing the override reaches the normal transport;
    // it needs neither an explicit retry nor a queue reset.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let agent = cloud::history_agent()
        .timeout(Duration::from_millis(100))
        .build();
    let _ = agent
        .get(&format!("http://{}/", listener.local_addr().unwrap()))
        .call();
    assert!(listener.accept().is_ok());
}

fn child(path: &Path, mode: &str, override_key: Option<&str>) {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "history::destination::tls::queue_tests::queue_recovery_child",
        ])
        .env_remove("SSL_CERT_FILE")
        .env_remove("SSL_CERT_DIR")
        .env_remove("RELAYHISTORY_ALLOW_UNTRUSTED_CLOUD_BASE_URL")
        .env("RELAY_HISTORY_OVERRIDE_QUEUE_CHILD", mode)
        .env("RELAY_HISTORY_OVERRIDE_QUEUE_PATH", path);
    if let Some(key) = override_key {
        command.env(key, "never-read-test-ca");
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{mode}: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("never-read-test-ca"));
}

#[test]
fn clearing_an_override_and_restarting_reclaims_the_same_persisted_batch() {
    for key in ["SSL_CERT_FILE", "SSL_CERT_DIR"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.db");
        child(&path, "record-local", Some(key));
        child(&path, "resume", None);
    }
}

#[test]
fn a_server_permission_refusal_stays_blocked_after_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.db");
    child(&path, "record-server", None);
    child(&path, "check-server", None);
}

#[test]
fn retry_wakes_a_persisted_configuration_wait_from_the_real_destination_guard() {
    for key in ["SSL_CERT_FILE", "SSL_CERT_DIR"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.db");
        child(&path, "record-local-guard", Some(key));
        child(&path, "retry-local", None);
    }
}

#[test]
fn an_unsafe_destination_stays_blocked_after_restart() {
    for mode in ["record-unsafe-guard", "record-private-guard"] {
        for override_key in [None, Some("SSL_CERT_FILE"), Some("SSL_CERT_DIR")] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("history.db");
            child(&path, mode, override_key);
            child(&path, "check-server", None);
        }
    }
}

/// The local wait is this computer's, not the receiver's: the job is retried
/// a minute later by itself, and Retry, pressed once the person has cleared
/// the override, wakes it at once instead of leaving the minute to run.
#[test]
fn retry_wakes_a_local_configuration_wait_at_once() {
    let directory = tempfile::tempdir().unwrap();
    let conn = delivery::open_db(&directory.path().join("history.db")).unwrap();
    let now = 1_000_000;
    let claim = claimed(&conn, now);
    let outcome = verdict(&cloud::HistoryTlsConfigurationRefused.into());
    let status = outcome.record(&conn, &claim.lease, &|| now).unwrap();
    assert_eq!(
        (status.state.as_str(), status.next_attempt_ms),
        ("active", now + EXPECTED_RETRY_MS)
    );
    let receiver_wait: i64 = conn
        .query_row(
            "SELECT receiver_retry_after_ms FROM delivery_jobs WHERE id=?",
            [&claim.lease.job_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(receiver_wait, 0);
    assert!(delivery::retry_requested(&conn, &claim.lease.job_id, now + 1).unwrap());
    let status = delivery::status(&conn, &claim.lease.job_id).unwrap();
    assert!(status.next_attempt_ms <= now + 1);
    assert!(
        delivery::claim_batch(&conn, &claim.lease.job_id, "retried", 60_000, &|| now + 1)
            .unwrap()
            .is_some()
    );
}
