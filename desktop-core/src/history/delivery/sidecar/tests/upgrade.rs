use super::*;

fn blocked_upgrade(legacy: bool) {
    let dir = tempfile::tempdir().unwrap();
    let history = dir.path().join("history.db");
    let store =
        ai_hist::SessionStore::open(ai_hist::StoreOptions::default().db_path(&history)).unwrap();
    drop(store);
    let old = crate::history::delivery::open_connection(&history).unwrap();
    schema::init(&old).unwrap();
    policy::init(&old).unwrap();
    schema::rebuild(&old, || feed::store(&old)).unwrap();
    counters::init(&old).unwrap();
    pending::init(&old).unwrap();
    old.execute("INSERT INTO session_events(source,session_id,event_uid,ts_ms,role,kind,text) VALUES('claude','session','first',1,'user','text','first evidence')",[]).unwrap();
    let config = DeliveryJobConfig {
        destination_id: "fixture".into(),
        instance_id: "fixture".into(),
        account_id: "fixture".into(),
        mapping_version: "1".into(),
        selection: ExportSelection {
            all_sources: true,
            kinds: vec!["session_event".into()],
            ..Default::default()
        },
        limits: Default::default(),
    };
    let job = create_job(&old, &config, 0).unwrap().job_id;
    let batch_id = prepare_batch(&old, &job, 1).unwrap().batch_id.unwrap();
    let claimed = claim_batch(&old, &job, "before-upgrade", 1000, &|| 10)
        .unwrap()
        .unwrap();
    let payload = store_prepared_payload(
        &old,
        &claimed.lease,
        "1",
        "application/json",
        "{\"scrubbed\":true}",
        &|| 11,
    )
    .unwrap();
    record_failure(
        &old,
        &claimed.lease,
        DeliveryFailure::InvalidPayload,
        None,
        &|| 12,
    )
    .unwrap();
    if legacy {
        let origin: String = old
            .query_row("SELECT origin_id FROM delivery_origin", [], |r| r.get(0))
            .unwrap();
        old.execute_batch(
            "DELETE FROM delivery_origin;
            CREATE TABLE delivery_state(singleton INTEGER PRIMARY KEY,origin_id TEXT);
            CREATE TABLE delivery_journal(revision INTEGER PRIMARY KEY AUTOINCREMENT,body TEXT);
            INSERT INTO delivery_journal VALUES(10000,'retained'); DELETE FROM delivery_journal;
            CREATE TABLE history_exports(id TEXT PRIMARY KEY,body TEXT);
            INSERT INTO history_exports VALUES('export','old export');
            CREATE TRIGGER delivery_session_events_insert AFTER INSERT ON session_events BEGIN
                INSERT INTO delivery_journal(body) VALUES(new.text); END;
            CREATE INDEX delivery_identity_session_events ON session_events(event_uid);
            UPDATE delivery_batches SET journal_end=900;",
        )
        .unwrap();
        old.execute("INSERT INTO delivery_state VALUES(1,?)", [origin])
            .unwrap();
    }
    let queue = open_db(&history).unwrap();
    if legacy {
        assert_eq!(
            queue
                .query_row("SELECT revision_base FROM delivery_origin", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            10000
        );
        assert!(!schema::has_journal(&queue).unwrap());
        assert!(schema::capture_triggers(&old).unwrap().is_empty());
        assert_eq!(
            queue
                .query_row("SELECT journal_end FROM delivery_batches", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    assert_eq!(
        compact_receipts(&queue, 100).unwrap(),
        0,
        "age must not discard an unresolved blocked upload"
    );
    let far_future = 90 * 86_400_000i64;
    assert!(
        claim_batch(&queue, &job, "still-blocked", 1000, &|| far_future)
            .unwrap()
            .is_none()
    );
    drop(queue);
    // A fresh process-equivalent connection resumes the journal. Capture
    // continues in history, never through delivery's read-only attachment.
    old.execute("INSERT INTO session_events(source,session_id,event_uid,ts_ms,role,kind,text) VALUES('claude','session','second',2,'user','text','new evidence')",[]).unwrap();
    let queue = open_control_db(&history).unwrap();
    retry_job(&queue, &job).unwrap();
    let retried = claim_batch(&queue, &job, "after-upgrade", 1000, &|| far_future)
        .unwrap()
        .unwrap();
    assert_eq!(retried.batch.batch_id, batch_id);
    assert_eq!(retried.prepared.as_ref(), Some(&payload));
    assert!(store_prepared_payload(
        &queue,
        &retried.lease,
        "1",
        "application/json",
        "changed bytes",
        &|| far_future
    )
    .is_err());
    let ack = DeliveryAcknowledgment {
        batch_id,
        accepted_revision_ids: retried
            .batch
            .records
            .iter()
            .map(|record| record.revision_id.clone())
            .collect(),
        unsupported_revision_ids: vec![],
        suppressed_revision_ids: vec![],
        rejected_revision_ids: vec![],
        isolation_frontier: vec![],
        acceptance_level: AcceptanceLevel::Durable,
    };
    acknowledge(&queue, &retried.lease, &ack, &|| far_future + 1).unwrap();
    assert_eq!(compact_receipts(&queue, 100).unwrap(), 1);
    assert!(
        prepare_batch(&queue, &job, far_future + 2)
            .unwrap()
            .batch_id
            .is_some(),
        "capture after cutover remains uploadable"
    );
    assert_eq!(
        old.query_row("SELECT count(*) FROM session_events", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn blocked_current_queue_keeps_its_identity_and_bytes() {
    blocked_upgrade(false);
}

#[test]
fn retired_journal_upgrades_and_retries_immutable_work_then_resumes_capture() {
    blocked_upgrade(true);
}

#[test]
fn an_abandoned_leased_batch_migrates_and_retries_without_rewriting() {
    let dir = tempfile::tempdir().unwrap();
    let history = dir.path().join("history.db");
    drop(ai_hist::SessionStore::open(ai_hist::StoreOptions::default().db_path(&history)).unwrap());
    let old = crate::history::delivery::open_connection(&history).unwrap();
    schema::init(&old).unwrap();
    policy::init(&old).unwrap();
    schema::rebuild(&old, || feed::store(&old)).unwrap();
    counters::init(&old).unwrap();
    pending::init(&old).unwrap();
    old.execute("INSERT INTO session_events(source,session_id,event_uid,ts_ms,role,kind,text) VALUES('claude','s','first',1,'user','text','evidence')",[]).unwrap();
    let config = DeliveryJobConfig {
        destination_id: "fixture".into(),
        instance_id: "fixture".into(),
        account_id: "fixture".into(),
        mapping_version: "1".into(),
        selection: ExportSelection {
            all_sources: true,
            kinds: vec!["session_event".into()],
            ..Default::default()
        },
        limits: Default::default(),
    };
    let job = create_job(&old, &config, 0).unwrap().job_id;
    prepare_batch(&old, &job, 1).unwrap();
    let before = claim_batch(&old, &job, "crashed", 1, &|| 10)
        .unwrap()
        .unwrap();
    let prepared = store_prepared_payload(
        &old,
        &before.lease,
        "1",
        "application/json",
        "immutable",
        &|| 10,
    )
    .unwrap();
    let queue = open_db(&history).unwrap();
    let after = claim_batch(&queue, &job, "replacement", 1000, &|| 20)
        .unwrap()
        .unwrap();
    assert_eq!(after.batch, before.batch);
    assert_eq!(after.prepared, Some(prepared));
    assert!(after.lease.fence > before.lease.fence);
}

#[test]
fn a_crash_during_legacy_initialization_keeps_helpers_out_until_the_owner_recovers() {
    let (_dir, history, _) = fixture();
    assert!(migration::open_initialized(
        &history,
        |_| Ok(()),
        |queue| {
            queue.execute("UPDATE delivery_jobs SET cursor=124", [])?;
            anyhow::bail!("initialization interrupted")
        }
    )
    .is_err());
    assert!(open(&history, false).unwrap_err().is::<UpgradePending>());
    let ready = migration::open_initialized(
        &history,
        |_| Ok(()),
        |queue| {
            assert_eq!(
                queue.query_row("SELECT cursor FROM delivery_jobs", [], |r| r
                    .get::<_, i64>(0))?,
                124
            );
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(state(&ready).unwrap().unwrap().0, "ready");
    assert!(open(&history, false).is_ok());
}
