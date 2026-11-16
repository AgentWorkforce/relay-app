use super::*;
use crate::history::delivery;

fn queued_job(events: usize) -> (tempfile::TempDir, rusqlite::Connection, String) {
    let directory = tempfile::tempdir().unwrap();
    let conn = delivery::open_db(&directory.path().join("history.db")).unwrap();
    for n in 0..events {
        delivery::test_evidence(&conn).execute("INSERT INTO session_events(source,session_id,event_uid,ts_ms,role,kind,text) VALUES ('claude','session',?1,42,'user','text','synthetic')", [format!("event-{n}")]).unwrap();
    }
    let config = delivery::DeliveryJobConfig {
        destination_id: "fixture".into(),
        instance_id: "fixture".into(),
        account_id: "fixture-account".into(),
        mapping_version: "1".into(),
        selection: delivery::ExportSelection {
            all_sources: true,
            kinds: vec!["session_event".into()],
            ..Default::default()
        },
        limits: delivery::DeliveryLimits {
            max_batch_records: 1,
            ..Default::default()
        },
    };
    let job = delivery::create_job(&conn, &config, 0).unwrap().job_id;
    let store = ai_hist::SessionStore::open(
        ai_hist::StoreOptions::default().db_path(directory.path().join("history.db")),
    )
    .unwrap();
    for _ in 0..events {
        delivery::prepare_batch_from(
            &conn,
            &store,
            &job,
            0,
            events,
            delivery::PublishedLimits::default(),
        )
        .unwrap();
    }
    conn.execute("UPDATE delivery_jobs SET attempts=9 WHERE id=?", [&job])
        .unwrap();
    (directory, conn, job)
}

fn row(conn: &rusqlite::Connection, job: &str) -> (i64, i64, i64) {
    conn.query_row(
        "SELECT attempts,next_attempt_ms,receiver_retry_after_ms FROM delivery_jobs WHERE id=?",
        [job],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .unwrap()
}

fn decision(deferred: Deferred) -> ReceiverFailure {
    super::super::tls::verdict(&anyhow::Error::new(deferred))
}

#[test]
fn deferred_verdicts_preserve_attempts_and_the_exact_shared_deadline_in_the_job() {
    let (directory, conn, job) = queued_job(1);
    for step in 0..16 {
        let now = 1_000 + step * 1_000;
        let claim = delivery::claim_batch(&conn, &job, "worker", 60_000, &|| now)
            .unwrap()
            .unwrap();
        let verdict = decision(Deferred::Busy);
        assert!(verdict.deferred);
        verdict.record(&conn, &claim.lease, &|| now).unwrap();
        assert_eq!(row(&conn, &job), (9, now + 1_000, 0));
    }
    assert!(delivery::retry_requested(&conn, &job, 16_000).unwrap());
    let claim = delivery::claim_batch(&conn, &job, "worker", 60_000, &|| 16_000)
        .unwrap()
        .unwrap();
    decision(Deferred::Until {
        at: 21_000,
        failure: DeliveryFailure::RateLimited,
    })
    .record(&conn, &claim.lease, &|| 16_000)
    .unwrap();
    assert_eq!(row(&conn, &job), (9, 21_000, 21_000));
    drop(conn);
    let conn = delivery::open_db(&directory.path().join("history.db")).unwrap();
    assert_eq!(row(&conn, &job), (9, 21_000, 21_000));
    assert!(!delivery::retry_requested(&conn, &job, 20_000).unwrap());
    assert!(!delivery::clamp_retry_wait(&conn, &job, 20_000).unwrap());
    assert!(
        delivery::claim_batch(&conn, &job, "worker", 60_000, &|| 20_000)
            .unwrap()
            .is_none()
    );
    let sent = delivery::claim_batch(&conn, &job, "worker", 60_000, &|| 21_000)
        .unwrap()
        .unwrap();
    let real_failure: ReceiverFailure = DeliveryFailure::Transient.into();
    real_failure.record(&conn, &sent.lease, &|| 21_000).unwrap();
    assert_eq!(
        row(&conn, &job).0,
        10,
        "only the actual failed attempt advances backoff"
    );
}

#[test]
fn deferred_siblings_undo_the_window_increment_once_and_real_failures_still_count() {
    for real_first in [true, false] {
        let (_directory, conn, job) = queued_job(3);
        let first = delivery::claim_batch(&conn, &job, "worker", 60_000, &|| 1_000)
            .unwrap()
            .unwrap();
        let mut claims = vec![first];
        for _ in 0..2 {
            match delivery::claim_next(&conn, &claims[0].lease, &|| 1_000).unwrap() {
                delivery::NextClaim::Claimed(claim) => claims.push(*claim),
                _ => panic!("another batch in the same window"),
            }
        }
        let real_index = if real_first { 0 } else { 2 };
        for (index, claim) in claims.iter().enumerate() {
            let verdict = if index == real_index {
                ReceiverFailure {
                    retry_after_ms: Some(6_000),
                    ..DeliveryFailure::RateLimited.into()
                }
            } else {
                decision(Deferred::Busy)
            };
            verdict.record(&conn, &claim.lease, &|| 1_000).unwrap();
            assert_eq!(row(&conn, &job).0, if index < real_index { 9 } else { 10 });
        }
    }
    let (_directory, conn, job) = queued_job(3);
    let first = delivery::claim_batch(&conn, &job, "worker", 60_000, &|| 1_000)
        .unwrap()
        .unwrap();
    let mut claims = vec![first];
    for _ in 0..2 {
        match delivery::claim_next(&conn, &claims[0].lease, &|| 1_000).unwrap() {
            delivery::NextClaim::Claimed(claim) => claims.push(*claim),
            _ => panic!("another batch in the same window"),
        }
    }
    for claim in claims {
        decision(Deferred::Until {
            at: 6_000,
            failure: DeliveryFailure::Transient,
        })
        .record(&conn, &claim.lease, &|| 1_000)
        .unwrap();
        assert_eq!(row(&conn, &job), (9, 6_000, 6_000));
    }
    assert_eq!(
        delivery::status(&conn, &job).unwrap().failure.as_deref(),
        Some("transient")
    );
}

#[test]
fn existing_jobs_gain_the_window_marker_without_resetting_attempt_history() {
    let (directory, conn, job) = queued_job(1);
    conn.execute_batch("ALTER TABLE delivery_jobs DROP COLUMN attempt_outcome")
        .unwrap();
    drop(conn);
    let conn = delivery::open_db(&directory.path().join("history.db")).unwrap();
    assert_eq!(row(&conn, &job).0, 9);
    let marker: i64 = conn
        .query_row(
            "SELECT attempt_outcome FROM delivery_jobs WHERE id=?",
            [&job],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(marker, 0);
}

#[test]
fn a_deferral_cannot_restore_attempts_after_a_sibling_ack_reset_them() {
    let (_directory, conn, job) = queued_job(3);
    let first = delivery::claim_batch(&conn, &job, "worker", 60_000, &|| 1_000)
        .unwrap()
        .unwrap();
    let mut claims = vec![first];
    for _ in 0..2 {
        match delivery::claim_next(&conn, &claims[0].lease, &|| 1_000).unwrap() {
            delivery::NextClaim::Claimed(claim) => claims.push(*claim),
            _ => panic!("another batch in the same window"),
        }
    }
    let first = &claims[0];
    delivery::store_prepared_payload(&conn, &first.lease, "1", "application/json", "{}", &|| {
        1_000
    })
    .unwrap();
    let ack = delivery::DeliveryAcknowledgment {
        batch_id: first.batch.batch_id.clone(),
        accepted_revision_ids: first
            .batch
            .records
            .iter()
            .map(|r| r.revision_id.clone())
            .collect(),
        unsupported_revision_ids: vec![],
        suppressed_revision_ids: vec![],
        rejected_revision_ids: vec![],
        isolation_frontier: vec![],
        acceptance_level: delivery::AcceptanceLevel::Durable,
    };
    delivery::acknowledge(&conn, &first.lease, &ack, &|| 1_000).unwrap();
    assert_eq!(row(&conn, &job).0, 0);
    decision(Deferred::Busy)
        .record(&conn, &claims[1].lease, &|| 1_000)
        .unwrap();
    let real: ReceiverFailure = DeliveryFailure::Transient.into();
    real.record(&conn, &claims[2].lease, &|| 1_000).unwrap();
    assert_eq!(
        row(&conn, &job).0,
        0,
        "a deferral must not undo a completed reset then restore an increment"
    );
}

fn auth() -> cloud::StoredAuth {
    cloud::StoredAuth {
        base_url: "http://localhost:32123".into(),
        org_id: Some("synthetic-org".into()),
        workspace_id: Some("synthetic-workspace".into()),
        ..Default::default()
    }
}

fn directory() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    private::set_mode(directory.path(), 0o700).unwrap();
    directory
}

#[test]
fn scopes_and_token_rotations_share_only_their_workspace_gate() {
    let dir = directory();
    let original = auth();
    let held = PostGate::open(dir.path(), &original, 1_000).unwrap();
    let mut rotated = original.clone();
    rotated.access_token = "a-different-synthetic-token".into();
    rotated.base_url.push('/');
    assert!(matches!(
        PostGate::open(dir.path(), &rotated, 1_000)
            .err()
            .unwrap()
            .downcast_ref(),
        Some(Deferred::Busy)
    ));
    for change in [0, 1, 2] {
        let mut other = original.clone();
        match change {
            0 => other.org_id = Some("another-org".into()),
            1 => other.workspace_id = Some("another-workspace".into()),
            _ => other.base_url = "http://localhost:32124".into(),
        }
        assert!(PostGate::open(dir.path(), &other, 1_000).is_ok());
    }
    drop(held);
    assert!(PostGate::open(dir.path(), &rotated, 1_000).is_ok());
}

#[test]
fn the_shared_deadline_survives_reopening_and_expires_without_a_reset() {
    let dir = directory();
    let mut gate = PostGate::open(dir.path(), &auth(), 1_000).unwrap();
    gate.defer_until(Some(301_000), DeliveryFailure::RateLimited)
        .unwrap();
    gate.defer_until(Some(6_000), DeliveryFailure::Transient)
        .unwrap(); // Cannot shorten it or change its class.
    drop(gate);
    for now in [1_000, 300_999] {
        let error = PostGate::open(dir.path(), &auth(), now).err().unwrap();
        let deferred = error.downcast_ref::<Deferred>().unwrap();
        assert!(matches!(
            deferred,
            Deferred::Until {
                at: 301_000,
                failure: DeliveryFailure::RateLimited
            }
        ));
        assert_eq!(deferred.verdict().retry_after_ms, Some(301_000));
        assert!(deferred.verdict().deferred);
    }
    assert!(PostGate::open(dir.path(), &auth(), 301_000).is_ok());
}

#[test]
fn corrupt_coordinator_state_is_repaired_under_the_lock() {
    for bytes in [
        b"bad".to_vec(),
        vec![0; 10],
        vec![255; 9],
        vec![0, 0, 0, 0, 0, 0, 0, 0, 7],
    ] {
        let dir = directory();
        let mut gate = PostGate::open(dir.path(), &auth(), 1_000).unwrap();
        gate.0.write_all(&bytes).unwrap();
        drop(gate);
        let mut repaired = PostGate::open(dir.path(), &auth(), 1_000).unwrap();
        assert!(repaired.cooldown().unwrap().is_none());
        assert_eq!(repaired.0.metadata().unwrap().len(), 0);
        repaired
            .defer_until(Some(6_000), DeliveryFailure::Transient)
            .unwrap();
        drop(repaired);
        let error = PostGate::open(dir.path(), &auth(), 1_000).err().unwrap();
        assert!(matches!(
            error.downcast_ref::<Deferred>(),
            Some(Deferred::Until {
                at: 6_000,
                failure: DeliveryFailure::Transient
            })
        ));
    }
}

#[test]
fn legacy_timestamps_keep_their_deadline_without_asserting_a_rate_limit() {
    let dir = directory();
    let mut gate = PostGate::open(dir.path(), &auth(), 1_000).unwrap();
    gate.0.write_all(&6_000i64.to_le_bytes()).unwrap();
    drop(gate);
    let error = PostGate::open(dir.path(), &auth(), 1_000).err().unwrap();
    assert!(matches!(
        error.downcast_ref::<Deferred>(),
        Some(Deferred::Until {
            at: 6_000,
            failure: DeliveryFailure::Transient
        })
    ));
}

#[cfg(unix)]
#[test]
fn coordinator_files_are_private_and_symlinks_are_refused() {
    use std::os::unix::{fs::symlink, fs::PermissionsExt};
    let dir = directory();
    drop(PostGate::open(dir.path(), &auth(), 1_000).unwrap());
    let path = fs::read_dir(dir.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(PostGate::open(dir.path(), &auth(), 1_000).is_err());
    fs::remove_file(&path).unwrap();
    let target = dir.path().join("target");
    fs::write(&target, b"untouched").unwrap();
    symlink(&target, &path).unwrap();
    assert!(PostGate::open(dir.path(), &auth(), 1_000).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"untouched");
}

#[test]
fn process_gate_child() {
    let Some(dir) = std::env::var_os("RELAY_POST_GATE_CHILD") else {
        return;
    };
    let _held = PostGate::open(Path::new(&dir), &auth(), 1_000).unwrap();
    fs::write(Path::new(&dir).join("child-ready"), []).unwrap();
    let _ = std::io::stdin().read(&mut [0]);
}

#[test]
fn process_exit_releases_the_os_lock_without_deleting_the_coordinator() {
    use std::process::{Command, Stdio};
    let dir = directory();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "history::destination::post_gate::tests::process_gate_child",
        ])
        .env("RELAY_POST_GATE_CHILD", dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !dir.path().join("child-ready").exists() {
        assert!(
            child.try_wait().unwrap().is_none(),
            "child exited before acquiring the gate"
        );
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("child did not acquire the gate");
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(matches!(
        PostGate::open(dir.path(), &auth(), 1_000)
            .err()
            .unwrap()
            .downcast_ref(),
        Some(Deferred::Busy)
    ));
    child.kill().unwrap(); // Only this isolated test subprocess, never a running core.
    child.wait().unwrap();
    assert!(PostGate::open(dir.path(), &auth(), 1_000).is_ok());
}
