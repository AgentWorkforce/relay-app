use super::*;
use ai_hist::retention::{RetentionOptions, RetentionPolicy};

fn history(directory: &tempfile::TempDir) -> ai_hist::SessionStore {
    let store = ai_hist::SessionStore::open(
        ai_hist::StoreOptions::default().db_path(directory.path().join("history.db")),
    )
    .unwrap();
    store
        .set_retention_policy(RetentionPolicy::from_days(7))
        .unwrap();
    store.register_retention_consumer("census-test").unwrap();
    store
        .acknowledge_retention("census-test", store.head_revision().unwrap())
        .unwrap();
    store
}

fn evict(history: &ai_hist::SessionStore, max_rows: u32) -> ai_hist::retention::RetentionReport {
    history
        .prune_expired(RetentionOptions {
            max_rows,
            min_interval_ms: 0,
            allow_unuploaded: false,
        })
        .unwrap()
}

fn expired(conn: &Connection, events: usize) {
    crate::history::delivery::test_evidence(&conn).execute("INSERT INTO sessions(source,session_id,last_activity_ms,discovery_state) VALUES ('codex','expired',1,'full')", []).unwrap();
    let evidence = crate::history::delivery::test_evidence(&conn);
    let tx = evidence.unchecked_transaction().unwrap();
    for n in 0..events {
        tx.execute("INSERT INTO session_events(source,session_id,event_uid,message_id,ts_ms,role,kind,text) VALUES ('codex','expired',?,?,1,'user','text','fixture')", params![format!("e{n}"), format!("m{n}")]).unwrap();
    }
    tx.commit().unwrap();
}

#[test]
fn feed_and_retention_identities_are_unioned_before_judging() {
    let (directory, conn) = store();
    let job_id = job(&conn);
    expired(&conn, 0);
    capture(&conn, "codex:child");
    delegate(&conn, "expired", "child");
    set_session_excluded(&conn, &identity("codex:expired"), true).unwrap();
    refresh(&conn).unwrap();
    capture(&conn, "codex:added");
    crate::history::delivery::test_evidence(&conn)
        .execute(
            "UPDATE sessions SET title='changed' WHERE session_id='child'",
            [],
        )
        .unwrap();
    let history = history(&directory);
    assert_eq!(evict(&history, 1024).sessions, 1);
    assert!(matches!(
        refresh(&conn).unwrap(),
        CensusRefresh::Followed { judged: 3, .. }
    ));
    agrees(&conn, &job_id);
    assert_eq!(
        counts(&conn, &job_id).unwrap().unwrap(),
        SessionCounts {
            total: 2,
            shared: 2,
            excluded: 0
        }
    );
    assert_eq!(refresh(&conn).unwrap(), CensusRefresh::Current);
}

#[test]
fn surviving_change_before_an_evicted_tail_advances_the_cursor_to_head() {
    let (directory, conn) = store();
    let job_id = job(&conn);
    refresh(&conn).unwrap();
    let before = position(&conn).unwrap().unwrap().0;
    let fresh = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    crate::history::delivery::test_evidence(&conn).execute("INSERT INTO sessions(source,session_id,last_activity_ms,discovery_state) VALUES ('codex','kept',?,'full')", [fresh]).unwrap();
    expired(&conn, 1);
    let history = history(&directory);
    assert_eq!(evict(&history, 1).sessions, 0);
    assert_eq!(evict(&history, 1).sessions, 1);

    let head = history.head_revision().unwrap();
    let surviving = history
        .changes_since(before, ChangeQuery::default())
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(surviving.len(), 1);
    assert_eq!(surviving[0].session_id, "kept");
    assert!(
        surviving
            .iter()
            .map(|change| change.revision)
            .max()
            .unwrap()
            < head.revision,
        "evicted later writes must leave a suppressed feed tail"
    );
    assert_eq!(
        refresh(&conn).unwrap(),
        CensusRefresh::Followed {
            changes: 1,
            judged: 2
        }
    );
    agrees(&conn, &job_id);
    assert_eq!(
        position(&conn).unwrap().unwrap().0,
        head,
        "a drained feed advances beyond its final surviving change"
    );
    assert_eq!(refresh(&conn).unwrap(), CensusRefresh::Current);
}

#[test]
fn missing_old_writer_journal_batch_requires_a_full_census() {
    let (directory, conn) = store();
    let job_id = job(&conn);
    expired(&conn, 0);
    refresh(&conn).unwrap();
    let history = history(&directory);
    assert_eq!(evict(&history, 1024).sessions, 1);
    // Simulate an older writer that advanced the generation but omitted
    // its journal entry. The durable generation is left intact.
    crate::history::delivery::test_evidence(&conn)
        .execute("DELETE FROM retention_changes", [])
        .unwrap();
    assert_eq!(
        refresh(&conn).unwrap(),
        CensusRefresh::Taken { sessions: 0 }
    );
    agrees(&conn, &job_id);
    assert_eq!(
        position(&conn).unwrap().unwrap().1,
        history.retention_generation().unwrap()
    );
    assert_eq!(refresh(&conn).unwrap(), CensusRefresh::Current);
}

#[test]
fn expired_journal_cursor_requires_a_full_census() {
    let (directory, conn) = store();
    let job_id = job(&conn);
    expired(&conn, 1025);
    refresh(&conn).unwrap();
    let history = history(&directory);
    for batch in 0..1027 {
        if evict(&history, 1).sessions == 1 {
            break;
        }
        assert!(batch < 1026, "the expired session must finish draining");
    }
    assert!(history
        .retention_changes_since(0)
        .unwrap()
        .sessions
        .is_none());
    assert_eq!(
        refresh(&conn).unwrap(),
        CensusRefresh::Taken { sessions: 0 }
    );
    agrees(&conn, &job_id);
    assert_eq!(refresh(&conn).unwrap(), CensusRefresh::Current);
}

#[test]
fn oversized_retention_identity_set_requires_a_full_census() {
    let (directory, conn) = store();
    let job_id = job(&conn);
    expired(&conn, 0);
    let evidence = crate::history::delivery::test_evidence(&conn);
    let tx = evidence.unchecked_transaction().unwrap();
    for n in 0..4128 {
        delegate(&tx, "expired", &format!("child-{n}"));
    }
    tx.commit().unwrap();
    refresh(&conn).unwrap();
    let history = history(&directory);
    for batch in 0..132 {
        if evict(&history, 1024).sessions == 1 {
            break;
        }
        assert!(batch < 131, "the expired session must finish draining");
    }
    assert!(history
        .retention_changes_since(0)
        .unwrap()
        .sessions
        .is_none());
    assert_eq!(
        refresh(&conn).unwrap(),
        CensusRefresh::Taken { sessions: 0 }
    );
    agrees(&conn, &job_id);
    assert_eq!(refresh(&conn).unwrap(), CensusRefresh::Current);
}

#[test]
fn a_new_epoch_rebuilds_even_with_complete_journal_coverage() {
    let (directory, conn) = store();
    let job_id = job(&conn);
    expired(&conn, 0);
    capture(&conn, "codex:kept");
    refresh(&conn).unwrap();
    let history = history(&directory);
    evict(&history, 1024);
    conn.execute("UPDATE delivery_session_census_state SET epoch=~epoch", [])
        .unwrap();
    assert_eq!(
        refresh(&conn).unwrap(),
        CensusRefresh::Taken { sessions: 1 }
    );
    agrees(&conn, &job_id);
    assert_eq!(
        position(&conn).unwrap().unwrap().0.epoch,
        history.head_revision().unwrap().epoch
    );
}

#[test]
fn missing_legacy_state_rebuilds_membership_and_preserves_exclusions() {
    let (directory, conn) = store();
    let job_id = job(&conn);
    expired(&conn, 0);
    capture(&conn, "codex:kept");
    set_session_excluded(&conn, &identity("codex:kept"), true).unwrap();
    refresh(&conn).unwrap();
    let history = history(&directory);
    evict(&history, 1024);
    conn.execute("DELETE FROM delivery_session_census_state", [])
        .unwrap();
    assert_eq!(
        refresh(&conn).unwrap(),
        CensusRefresh::Taken { sessions: 1 }
    );
    agrees(&conn, &job_id);
    assert_eq!(counts(&conn, &job_id).unwrap().unwrap().excluded, 1);
}

#[test]
fn eviction_after_the_journal_snapshot_remains_queued() {
    let (directory, conn) = store();
    let job_id = job(&conn);
    expired(&conn, 1);
    refresh(&conn).unwrap();
    let history = history(&directory);
    let (before, generation) = position(&conn).unwrap().unwrap();
    assert_eq!(evict(&history, 1).sessions, 0);
    let snapshot = history.retention_changes_since(generation).unwrap();
    let followed = follow(&history, before).unwrap().unwrap();
    assert_eq!(evict(&history, 1).sessions, 1);
    assert!(history.retention_generation().unwrap() > snapshot.generation);
    record_followed(
        &conn,
        followed,
        snapshot.sessions.unwrap(),
        snapshot.generation,
    )
    .unwrap();
    assert_eq!(position(&conn).unwrap().unwrap().1, snapshot.generation);
    agrees(&conn, &job_id);
    assert_eq!(
        refresh(&conn).unwrap(),
        CensusRefresh::Followed {
            changes: 0,
            judged: 1
        }
    );
    assert_eq!(
        position(&conn).unwrap().unwrap().1,
        history.retention_generation().unwrap()
    );
    assert_eq!(refresh(&conn).unwrap(), CensusRefresh::Current);
}

#[test]
fn failed_cursor_write_rolls_back_membership_and_both_positions() {
    let (directory, conn) = store();
    let job_id = job(&conn);
    expired(&conn, 0);
    refresh(&conn).unwrap();
    let before = position(&conn).unwrap();
    let history = history(&directory);
    evict(&history, 1024);
    conn.execute_batch("CREATE TRIGGER reject_census_position BEFORE UPDATE OF retention_generation ON delivery_session_census_state BEGIN SELECT RAISE(ABORT,'synthetic position failure'); END;").unwrap();
    assert!(refresh(&conn).is_err());
    assert_eq!(position(&conn).unwrap(), before);
    assert_eq!(counts(&conn, &job_id).unwrap().unwrap().total, 1);
    conn.execute_batch("DROP TRIGGER reject_census_position")
        .unwrap();
    refresh(&conn).unwrap();
    agrees(&conn, &job_id);
}
