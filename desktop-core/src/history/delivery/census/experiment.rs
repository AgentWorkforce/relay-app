//! Run explicitly with `cargo test --lib census::experiment -- --ignored --nocapture`.
use super::*;
use ai_hist::retention::{RetentionOptions, RetentionPolicy};
use std::time::{Duration, Instant};

/// A large, unchanged catalog with one expired session drained in small batches.
/// Measures legacy, diff-only and journal-delta refreshes against the same
/// dependency, profile, machine and synthetic data. These measurements
/// complement the focused correctness and cursor regression tests.
#[test]
#[ignore = "synthetic 5,000-session census performance experiment"]
fn retention_batches_in_a_large_catalog() {
    let mut measured_writes = Vec::new();
    for path_kind in ["legacy", "diff-only", "delta"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.db");
        let conn = open_db(&path).unwrap();
        let fresh = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let tx = conn.unchecked_transaction().unwrap();
        {
            let mut insert = tx.prepare("INSERT INTO sessions(source,session_id,last_activity_ms,discovery_state) VALUES ('codex',?,?, 'full')").unwrap();
            for n in 0..5_000 {
                insert
                    .execute(params![
                        format!("session-{n:05}"),
                        if n == 0 { 1 } else { fresh }
                    ])
                    .unwrap();
            }
            let mut insert = tx.prepare("INSERT INTO session_events(source,session_id,event_uid,message_id,ts_ms,role,kind,text) VALUES ('codex','session-00000',?,?,1,'user','text','fixture')").unwrap();
            for n in 0..512 {
                insert
                    .execute(params![format!("event-{n}"), format!("message-{n}")])
                    .unwrap();
            }
        }
        tx.commit().unwrap();
        let history =
            ai_hist::SessionStore::open(ai_hist::StoreOptions::default().db_path(path)).unwrap();
        history
            .set_retention_policy(RetentionPolicy::from_days(7))
            .unwrap();
        history
            .register_retention_consumer("census-experiment")
            .unwrap();
        history
            .acknowledge_retention("census-experiment", history.head_revision().unwrap())
            .unwrap();
        refresh(&conn).unwrap();
        let mut retention_elapsed = Duration::ZERO;
        let mut census_elapsed = Duration::ZERO;
        let mut walk_elapsed = Duration::ZERO;
        let mut writes = 0;
        let mut recounts = 0;
        let mut judged = 0;
        let mut batches = 0;
        let mut remaining = 5_000;
        loop {
            let started = Instant::now();
            let report = history
                .prune_expired(RetentionOptions {
                    max_rows: 32,
                    min_interval_ms: 0,
                    allow_unuploaded: false,
                })
                .unwrap();
            retention_elapsed += started.elapsed();
            let before = conn.total_changes();
            let started = Instant::now();
            let result = match path_kind {
                "legacy" => legacy_refresh(&conn),
                "diff-only" => diff_only_refresh(&conn),
                _ => refresh(&conn),
            }
            .unwrap();
            census_elapsed += started.elapsed();
            writes += conn.total_changes() - before;
            recounts += usize::from(matches!(result, CensusRefresh::Taken { .. }));
            if let CensusRefresh::Followed { judged: count, .. } = result {
                judged += count;
            }
            batches += 1;
            assert!(batches < 100, "fixture must finish in bounded batches");
            remaining -= report.sessions as i64;
            let total: i64 = conn
                .query_row(
                    "SELECT total FROM delivery_session_census_state",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(total, remaining);
            let started = Instant::now();
            let authoritative = selectable_session_identities(&conn).unwrap();
            walk_elapsed += started.elapsed();
            assert_eq!(total, authoritative.len() as i64);
            if remaining == 4_999 {
                break;
            }
        }
        assert_eq!(remaining, 4_999);
        assert_eq!(batches, 17);
        if path_kind == "delta" {
            assert_eq!(
                recounts, 0,
                "complete journal coverage must avoid identity walks"
            );
            assert_eq!(
                judged, 17,
                "only the evicted session is rechecked per batch"
            );
        } else {
            assert_eq!(recounts, 17);
        }
        eprintln!("census-experiment path={} sessions=5000 payload_rows=512 batch_rows=32 batches={batches} recounts={recounts} judged={judged} census_sql_changes={writes} retention_wall_ms={:.3} census_wall_ms={:.3} separate_authoritative_walk_wall_ms={:.3}", path_kind, retention_elapsed.as_secs_f64()*1000., census_elapsed.as_secs_f64()*1000., walk_elapsed.as_secs_f64()*1000.);
        measured_writes.push(writes);
    }
    assert!(measured_writes[1..].iter().all(|writes| *writes < measured_writes[0]),
        "both optimized census paths must change fewer SQL rows than the legacy rebuild: {measured_writes:?}");
}

fn legacy_refresh(conn: &Connection) -> Result<CensusRefresh> {
    init(conn)?;
    let store = feed::store(conn)?;
    let head = store.head_revision()?;
    let retention = store.retention_generation()?;
    if position(conn)?.is_some_and(|(position, counted)| position == head && counted == retention) {
        return Ok(CensusRefresh::Current);
    }
    let sessions = selectable_session_identities(conn)?;
    let tx = write_transaction(conn)?;
    tx.execute_batch(
        "DELETE FROM delivery_session_census_state; DELETE FROM delivery_session_census;",
    )?;
    tx.execute("INSERT INTO delivery_session_census_state(singleton,epoch,revision,retention_generation,total,excluded) VALUES (1,?,?,?,0,0)", params![head.epoch as i64, feed::revision(head.revision)?, retention as i64])?;
    {
        let mut insert = tx.prepare(
            "INSERT OR IGNORE INTO delivery_session_census(source,session_id) VALUES (?,?)",
        )?;
        for identity in &sessions {
            insert.execute(params![identity.source, identity.session_id])?;
        }
    }
    tx.commit()?;
    Ok(CensusRefresh::Taken {
        sessions: sessions.len(),
    })
}

/// The prior optimization still walked all identities after every eviction.
fn diff_only_refresh(conn: &Connection) -> Result<CensusRefresh> {
    init(conn)?;
    let store = feed::store(conn)?;
    let head = store.head_revision()?;
    let retention = store.retention_generation()?;
    if position(conn)?.is_some_and(|(position, counted)| position == head && counted == retention) {
        return Ok(CensusRefresh::Current);
    }
    take(conn, head, retention)
}
