use super::*;

#[test]
fn each_copied_key_payload_exclusion_and_count_mismatch_retains_source() {
    for change in [
        "UPDATE delivery_jobs SET rowid=42", // TEXT PK: rowid isn't one of SELECT *'s columns.
        "UPDATE delivery_batches SET seq=42", // INTEGER PK.
        "UPDATE delivery_batches SET payload=x'00fe'",
        "DELETE FROM delivery_exclusions",
        "UPDATE delivery_counts SET n=99",
        "DELETE FROM delivery_batches", // Cardinality, including the empty-table case.
    ] {
        let (_dir, history, old) = fixture();
        assert!(migration::open_with(&history, |at| {
            ensure!(at != "copied", "interrupted");
            Ok(())
        })
        .is_err());
        let queue = Connection::open(path(&history)).unwrap();
        queue.execute_batch(change).unwrap();
        assert!(migrate(&history).is_err(), "{change}");
        assert_eq!(
            old.query_row("SELECT payload FROM delivery_batches", [], |r| r
                .get::<_, Vec<u8>>(0))
                .unwrap(),
            [0, 255],
            "{change}"
        );
        assert_eq!(
            old.query_row("SELECT count(*) FROM delivery_exclusions", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            old.query_row("SELECT n FROM delivery_counts", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}

#[test]
fn another_sources_queue_is_refused_and_both_witnesses_survive() {
    let (_a, a, _) = fixture();
    let (_b, b, source) = fixture();
    drop(migrate(&a).unwrap());
    assert!(migration::open_with(&b, |at| {
        ensure!(at != "frozen", "interrupted");
        Ok(())
    })
    .is_err());
    let a_db = Connection::open(path(&a)).unwrap();
    a_db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    drop(a_db);
    std::fs::copy(path(&a), path(&b)).unwrap();
    let witness = state(&source).unwrap();
    assert!(migrate(&b)
        .unwrap_err()
        .to_string()
        .contains("another source"));
    assert_eq!(state(&source).unwrap(), witness);
    assert_eq!(
        source
            .query_row("SELECT cursor FROM delivery_jobs", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        123
    );
}

#[test]
fn a_stray_destination_is_refused_without_fencing_the_source() {
    let (_dir, history, source) = fixture();
    let stray = Connection::open(path(&history)).unwrap();
    stray
        .execute_batch(
            "CREATE TABLE unrelated(receipt TEXT); INSERT INTO unrelated VALUES('retain')",
        )
        .unwrap();
    assert!(migrate(&history).is_err());
    assert!(!started(&source).unwrap());
    source
        .execute("UPDATE delivery_jobs SET cursor=124", [])
        .unwrap();
    assert_eq!(
        stray
            .query_row("SELECT receipt FROM unrelated", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "retain"
    );
}

#[test]
fn freeze_reuses_the_witness_committed_since_an_earlier_observation() {
    let (_dir, history, source) = fixture();
    let other = crate::history::delivery::open_connection(&history).unwrap();
    assert!(state(&other).unwrap().is_none());
    let first = migration::freeze(&source, &path(&history)).unwrap();
    let second = migration::freeze(&other, &path(&history)).unwrap();
    assert_eq!(first, second);
    let ready = migrate(&history).unwrap();
    assert_eq!(state(&ready).unwrap().unwrap().1, first.1);
}

#[test]
fn a_control_opener_racing_cutover_cannot_replace_its_manifest() {
    let (_dir, history, source) = fixture();
    let (frozen_tx, frozen_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    let path_for_worker = history.clone();
    let worker = std::thread::spawn(move || {
        migration::open_with(&path_for_worker, |at| {
            if at == "frozen" {
                frozen_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
            }
            Ok(())
        })
        .map(|_| ())
    });
    frozen_rx.recv().unwrap();
    let witness = state(&source).unwrap();
    assert!(migrate(&history).unwrap_err().is::<UpgradePending>());
    assert_eq!(state(&source).unwrap(), witness);
    resume_tx.send(()).unwrap();
    worker.join().unwrap().unwrap();
    let queue = migrate(&history).unwrap();
    assert_eq!(state(&queue).unwrap().unwrap().1, witness.unwrap().1);
}

#[test]
fn an_expired_deadline_or_missing_holder_does_not_require_a_worker_to_stop() {
    for (worker, deadline) in [
        (Some("old"), Some(100)),
        (None, Some(101)),
        (Some("old"), None),
    ] {
        let (_dir, history, source) = fixture();
        source
            .execute("UPDATE delivery_batches SET state='leased'", [])
            .unwrap();
        source
            .execute(
                "UPDATE delivery_jobs SET worker_id=?,lease_until_ms=?",
                params![worker, deadline],
            )
            .unwrap();
        // Equality is already expired, not a live lease.
        migration::refuse_live_worker(&source, 100).unwrap();
        assert!(migrate(&history).is_ok());
    }
}

#[test]
fn sqlite_readonly_uris_keep_unc_in_the_path_and_escape_special_characters() {
    let url = url::Url::parse("file://server/share/space%20and%25%23%3F/history.db").unwrap();
    assert_eq!(
        readonly_uri(&url),
        "file:////server/share/space%20and%25%23%3F/history.db?mode=ro"
    );
    let dir = tempfile::tempdir().unwrap();
    let history = dir.path().join("space % #.db");
    let source = Connection::open(&history).unwrap();
    source
        .execute_batch("CREATE TABLE evidence(id INTEGER); INSERT INTO evidence VALUES(1)")
        .unwrap();
    let queue = crate::history::delivery::open_connection(&dir.path().join("queue.db")).unwrap();
    attach(&queue, &history).unwrap();
    assert_eq!(
        queue
            .query_row("SELECT id FROM delivery_evidence.evidence", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert!(queue
        .execute("DELETE FROM delivery_evidence.evidence", [])
        .is_err());
}

#[cfg(windows)]
#[test]
fn windows_unc_paths_need_no_sqlite_authority_build_option() {
    for path in [
        r"\\server\share\space #\history.db",
        r"\\?\UNC\server\share\space #\history.db",
    ] {
        let url = url::Url::from_file_path(Path::new(path)).unwrap();
        assert_eq!(
            readonly_uri(&url),
            "file:////server/share/space%20%23/history.db?mode=ro"
        );
    }
}
