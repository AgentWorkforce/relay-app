use super::*;

fn fixture() -> (tempfile::TempDir, PathBuf, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let history = dir.path().join("history.db");
    let old = Connection::open(&history).unwrap();
    old.execute_batch("PRAGMA journal_mode=WAL;
        CREATE TABLE evidence(id INTEGER PRIMARY KEY, body TEXT);
        INSERT INTO evidence VALUES(1,'local evidence stays local');
        CREATE TABLE delivery_jobs(id TEXT PRIMARY KEY, cursor INTEGER, excluded INTEGER, worker_id TEXT, lease_until_ms INTEGER);
        INSERT INTO delivery_jobs(id,cursor,excluded) VALUES('job',123,1);
        CREATE TABLE delivery_batches(seq INTEGER PRIMARY KEY AUTOINCREMENT,id TEXT UNIQUE,
            state TEXT,payload BLOB,prepared TEXT,digest TEXT,job_id TEXT DEFAULT 'job');
        INSERT INTO delivery_batches(seq,id,state,payload,prepared,digest) VALUES(7,'old','blocked',x'00ff','frozen wire bytes','existing digest');
        INSERT INTO delivery_batches(seq,id,state) VALUES(99,'resolved','acknowledged');
        DELETE FROM delivery_batches WHERE seq=99;
        CREATE TABLE delivery_exclusions(source TEXT,session_id TEXT,PRIMARY KEY(source,session_id)) WITHOUT ROWID;
        INSERT INTO delivery_exclusions VALUES('claude','excluded');
        CREATE TABLE delivery_counts(singleton INTEGER PRIMARY KEY,n INTEGER);
        INSERT INTO delivery_counts VALUES(1,1);
        CREATE TRIGGER delivery_count_added AFTER INSERT ON delivery_batches BEGIN
            UPDATE delivery_counts SET n=n+1 WHERE singleton=1; END;").unwrap();
    (dir, history, old)
}

#[test]
fn queue_moves_exact_rows_bytes_cursors_exclusions_and_sequences_once() {
    let (_dir, history, old) = fixture();
    let queue = migrate(&history).unwrap();
    assert_eq!(
        Path::new(queue.path().unwrap()),
        path(&history).canonicalize().unwrap()
    );
    assert_eq!(
        queue
            .query_row("SELECT cursor FROM delivery_jobs", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        123
    );
    assert_eq!(
        queue
            .query_row("SELECT payload FROM delivery_batches", [], |r| r
                .get::<_, Vec<u8>>(0))
            .unwrap(),
        [0, 255]
    );
    assert_eq!(
        queue
            .query_row("SELECT prepared FROM delivery_batches", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "frozen wire bytes"
    );
    assert_eq!(
        queue
            .query_row("SELECT digest FROM delivery_batches", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "existing digest"
    );
    assert_eq!(
        queue
            .query_row("SELECT count(*) FROM delivery_exclusions", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        queue
            .query_row("SELECT n FROM delivery_counts", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    queue
        .execute(
            "INSERT INTO delivery_batches(id,state) VALUES('next','pending')",
            [],
        )
        .unwrap();
    assert_eq!(queue.last_insert_rowid(), 100);
    assert_eq!(
        queue
            .query_row("SELECT n FROM delivery_counts", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        old.query_row("SELECT body FROM evidence", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "local evidence stays local"
    );
    assert!(!exists(&queue, "main", "evidence").unwrap());
    assert!(queue
        .execute("UPDATE delivery_evidence.evidence SET body='changed'", [])
        .is_err());
    drop(queue);
    let reopened = migrate(&history).unwrap();
    assert_eq!(
        reopened
            .query_row("SELECT count(*) FROM delivery_batches", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn old_cold_connection_cannot_claim_recreate_or_read_moved_jobs() {
    let (_dir, history, old) = fixture();
    let _queue = migrate(&history).unwrap();
    assert!(old
        .execute(
            "INSERT INTO delivery_jobs(id,cursor,excluded) VALUES('late',0,0)",
            []
        )
        .is_err());
    assert!(old
        .execute("UPDATE delivery_jobs SET cursor=999", [])
        .is_err());
    assert!(old.execute_batch("CREATE TABLE IF NOT EXISTS delivery_jobs(id TEXT); INSERT INTO delivery_jobs VALUES('late');").is_err());
    assert!(old
        .query_row("SELECT * FROM delivery_jobs", [], |_| Ok(()))
        .is_err());
}

#[test]
fn only_a_live_worker_lease_refuses_before_fencing_or_copying() {
    let (_dir, history, old) = fixture();
    old.execute("UPDATE delivery_batches SET state='leased'", [])
        .unwrap();
    old.execute(
        "UPDATE delivery_jobs SET worker_id='live',lease_until_ms=?",
        [i64::MAX],
    )
    .unwrap();
    assert!(migrate(&history)
        .unwrap_err()
        .to_string()
        .contains("old upload worker"));
    assert!(!path(&history).exists());
    assert!(!started(&old).unwrap());
    old.execute("UPDATE delivery_jobs SET lease_until_ms=0", [])
        .unwrap();
    let queue = migrate(&history).unwrap();
    assert_eq!(
        queue
            .query_row("SELECT state FROM delivery_batches", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "leased"
    );
}

#[test]
fn restart_recovers_every_committed_phase_without_stale_recopy() {
    for phase in ["frozen", "copied", "source_removed", "upgrading", "ready"] {
        let (_dir, history, old) = fixture();
        let failed = migration::open_with(&history, |at| {
            ensure!(at != phase, "simulated process interruption");
            Ok(())
        });
        assert!(failed.is_err(), "{phase}");
        // Even an old connection opened before cutover cannot add a late row.
        assert!(old
            .execute(
                "INSERT INTO delivery_jobs(id,cursor,excluded) VALUES('late',0,0)",
                []
            )
            .is_err());
        let reopened = migrate(&history).unwrap();
        assert_eq!(
            reopened
                .query_row("SELECT count(*) FROM delivery_jobs", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(state(&reopened).unwrap().unwrap().0, "ready");
    }
}

#[test]
fn copied_destination_mismatch_never_deletes_source_or_overwrites_receipts() {
    let (_dir, history, old) = fixture();
    assert!(migration::open_with(&history, |at| {
        ensure!(at != "copied", "interrupted");
        Ok(())
    })
    .is_err());
    let changed = Connection::open(path(&history)).unwrap();
    changed
        .execute("UPDATE delivery_jobs SET cursor=999", [])
        .unwrap();
    assert!(migrate(&history).is_err());
    assert!(exists(&old, "main", "delivery_jobs").unwrap());
    assert_eq!(
        old.query_row("SELECT cursor FROM delivery_jobs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        123
    );
    assert_eq!(
        changed
            .query_row("SELECT cursor FROM delivery_jobs", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        999
    );
}

#[test]
fn source_schema_change_after_freeze_refuses_cleanup() {
    let (_dir, history, old) = fixture();
    assert!(migration::open_with(&history, |at| {
        ensure!(at != "copied", "interrupted");
        Ok(())
    })
    .is_err());
    old.execute_batch("ALTER TABLE delivery_jobs ADD COLUMN future_state TEXT")
        .unwrap();
    assert!(migrate(&history).is_err());
    assert!(exists(&old, "main", "delivery_jobs").unwrap());
}

#[test]
fn a_new_legacy_upload_table_after_freeze_refuses_cleanup() {
    let (_dir, history, old) = fixture();
    assert!(migration::open_with(&history, |at| {
        ensure!(at != "copied", "interrupted");
        Ok(())
    })
    .is_err());
    old.execute_batch(
        "CREATE TABLE delivery_future(id INTEGER); INSERT INTO delivery_future VALUES(1)",
    )
    .unwrap();
    assert!(migrate(&history).is_err());
    assert_eq!(
        old.query_row("SELECT cursor FROM delivery_jobs", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        123
    );
}

#[test]
fn cold_helpers_do_not_migrate_or_create_an_unready_queue() {
    let (_dir, history, old) = fixture();
    assert!(open(&history, false).unwrap_err().is::<UpgradePending>());
    assert!(!path(&history).exists());
    assert!(!started(&old).unwrap());
    drop(migrate(&history).unwrap());
    assert!(open(&history, true).is_ok());
}

#[test]
fn missing_or_foreign_destination_never_recreates_removed_source() {
    let (_dir, history, old) = fixture();
    drop(migrate(&history).unwrap());
    std::fs::remove_file(path(&history)).unwrap();
    assert!(migrate(&history).is_err());
    assert!(old
        .query_row("SELECT * FROM delivery_jobs", [], |_| Ok(()))
        .is_err());
}

#[test]
fn altered_sequence_or_trigger_after_copy_refuses_source_removal() {
    for change in [
        "UPDATE sqlite_sequence SET seq=1 WHERE name='delivery_batches'",
        "DROP TRIGGER delivery_count_added",
    ] {
        let (_dir, history, old) = fixture();
        assert!(migration::open_with(&history, |at| {
            ensure!(at != "copied", "interrupted");
            Ok(())
        })
        .is_err());
        let queue = Connection::open(path(&history)).unwrap();
        queue.execute_batch(change).unwrap();
        assert!(migrate(&history).is_err());
        assert_eq!(
            old.query_row("SELECT cursor FROM delivery_jobs", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            123
        );
    }
}

#[test]
fn sidecar_crash_child() {
    let Some(history) = std::env::var_os("RELAY_TEST_SIDECAR_CRASH_DB") else {
        return;
    };
    let phase = std::env::var("RELAY_TEST_SIDECAR_CRASH_PHASE").unwrap();
    migration::open_with(Path::new(&history), |at| {
        if at == phase {
            std::process::exit(93);
        }
        Ok(())
    })
    .unwrap();
    panic!("requested committed phase was not reached");
}

#[test]
fn a_new_process_recovers_after_exit_at_each_durable_phase() {
    for phase in ["frozen", "copied", "source_removed", "upgrading", "ready"] {
        let (_dir, history, old) = fixture();
        drop(old);
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "history::delivery::sidecar::tests::sidecar_crash_child",
                "--nocapture",
            ])
            .env("RELAY_TEST_SIDECAR_CRASH_DB", &history)
            .env("RELAY_TEST_SIDECAR_CRASH_PHASE", phase)
            .output()
            .unwrap();
        assert_eq!(
            child.status.code(),
            Some(93),
            "{phase}: {}",
            String::from_utf8_lossy(&child.stderr)
        );
        let queue = migrate(&history).unwrap();
        assert_eq!(state(&queue).unwrap().unwrap().0, "ready");
        assert_eq!(
            queue
                .query_row(
                    "SELECT payload FROM delivery_batches WHERE id='old'",
                    [],
                    |r| r.get::<_, Vec<u8>>(0)
                )
                .unwrap(),
            [0, 255]
        );
    }
}

#[test]
fn a_full_queue_keeps_the_cursor_and_evidence_for_retry() {
    let dir = tempfile::tempdir().unwrap();
    let history = dir.path().join("history.db");
    let queue = open_db(&history).unwrap();
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
    let job = create_job(&queue, &config, 0).unwrap().job_id;
    let before = job_cursor(&queue, &job);
    let evidence = test_evidence(&queue);
    evidence.execute("INSERT INTO session_events(source,session_id,event_uid,ts_ms,role,kind,text) VALUES('claude','full','first',1,'user','text',?)", ["x".repeat(256 * 1024)]).unwrap();
    let pages: i64 = queue
        .pragma_query_value(Some(rusqlite::DatabaseName::Main), "page_count", |r| {
            r.get(0)
        })
        .unwrap();
    queue
        .pragma_update(Some(rusqlite::DatabaseName::Main), "max_page_count", pages)
        .unwrap();
    let error = prepare_batch(&queue, &job, 1).unwrap_err();
    assert!(error.chain().any(|cause| matches!(cause.downcast_ref::<rusqlite::Error>(),Some(rusqlite::Error::SqliteFailure(error, _)) if error.code == rusqlite::ErrorCode::DiskFull)), "{error:#}");
    assert_eq!(job_cursor(&queue, &job), before);
    assert_eq!(
        queue
            .query_row("SELECT count(*) FROM delivery_batches", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    evidence.execute("INSERT INTO session_events(source,session_id,event_uid,ts_ms,role,kind,text) VALUES('claude','full','second',2,'user','text','capture continues')", []).unwrap();
    assert_eq!(
        evidence
            .query_row("SELECT count(*) FROM session_events", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    queue
        .pragma_update(
            Some(rusqlite::DatabaseName::Main),
            "max_page_count",
            pages + 1024,
        )
        .unwrap();
    assert!(prepare_batch(&queue, &job, 2).unwrap().batch_id.is_some());
}

fn job_cursor(conn: &Connection, job: &str) -> i64 {
    conn.query_row(
        "SELECT journal_cursor FROM delivery_jobs WHERE id=?",
        [job],
        |r| r.get(0),
    )
    .unwrap()
}

mod integrity;
mod upgrade;
