//! The oops window, with an injected clock: every preparation names the time
//! it runs at.
use super::*;

/// The window these tests hold for.
const WINDOW: i64 = 500;

/// A job over `sessions` of `claude`, each with `events` events: an
/// all-sources job, or a session job with each session a member.
fn job(
    session_job: bool,
    sessions: &[&str],
    events: usize,
) -> (tempfile::TempDir, Connection, String) {
    let directory = tempfile::tempdir().unwrap();
    let conn = open_db(&directory.path().join("history.db")).unwrap();
    for session in sessions {
        add_events(&conn, session, 0..events);
    }
    let config = DeliveryJobConfig {
        destination_id: "fixture".into(),
        instance_id: "oops".into(),
        account_id: "fixture-account".into(),
        mapping_version: "1".into(),
        selection: ExportSelection {
            all_sources: true,
            kinds: vec!["session_event".into()],
            ..ExportSelection::default()
        },
        limits: DeliveryLimits::default(),
    };
    let job_id = if session_job {
        let job_id = create_session_job(&conn, &config, 0).unwrap().job_id;
        for session in sessions {
            set_job_session(&conn, &job_id, &identity(session), true).unwrap();
        }
        job_id
    } else {
        create_job(&conn, &config, 0).unwrap().job_id
    };
    (directory, conn, job_id)
}

fn add_events(conn: &Connection, session: &str, events: std::ops::Range<usize>) {
    for n in events {
        test_evidence(conn).execute(
            "INSERT INTO session_events(source,session_id,event_uid,ts_ms,role,kind,text) VALUES ('claude',?1,?2,42,'user','text','oops')",
            params![session, format!("{session}-{n}")],
        )
        .unwrap();
    }
}

fn identity(session: &str) -> SessionIdentity {
    SessionIdentity {
        source: "claude".into(),
        session_id: session.into(),
    }
}

/// The store records `session`'s latest activity at `at_ms`.
fn active(conn: &Connection, session: &str, at_ms: i64) {
    let updated = test_evidence(conn)
        .execute(
            "UPDATE sessions SET last_activity_ms=?3 WHERE source=?1 AND session_id=?2",
            params!["claude", session, at_ms],
        )
        .unwrap();
    if updated == 0 {
        test_evidence(conn)
            .execute(
                "INSERT INTO sessions(source,session_id,last_activity_ms) VALUES (?1,?2,?3)",
                params!["claude", session, at_ms],
            )
            .unwrap();
    }
}

/// One preparation at `now_ms` under a window of `window_ms`.
fn prepare(conn: &Connection, job_id: &str, now_ms: i64, window_ms: Option<i64>) -> PrepareResult {
    prepare_scrubbed(
        conn,
        job_id,
        now_ms,
        window_ms,
        &ScrubEngine::default_engine(),
    )
}

/// [`prepare`] with every record through `scrub`.
fn prepare_scrubbed(
    conn: &Connection,
    job_id: &str,
    now_ms: i64,
    window_ms: Option<i64>,
    scrub: &ScrubEngine,
) -> PrepareResult {
    let store = feed::store(conn).unwrap();
    prepare_batch_scrubbed(
        conn,
        &store,
        job_id,
        now_ms,
        100,
        PublishedLimits::default(),
        Intake {
            oops_window_ms: window_ms,
            scrub,
        },
    )
    .unwrap()
}

/// Prepare at `now_ms` until a pass prepares nothing more.
fn prepare_all(conn: &Connection, job_id: &str, now_ms: i64, window_ms: Option<i64>) {
    for _ in 0..50 {
        if prepare(conn, job_id, now_ms, window_ms).batch_id.is_none() {
            return;
        }
    }
    panic!("preparation did not settle");
}

/// The `(session, event)` of every record in every batch the job prepared,
/// asserting no revision is in two of them.
fn delivered(conn: &Connection, job_id: &str) -> Vec<String> {
    let records: Vec<HistoryExportRecord> = conn
        .prepare("SELECT payload FROM delivery_batches WHERE job_id=? ORDER BY seq")
        .unwrap()
        .query_map([job_id], |r| r.get::<_, String>(0))
        .unwrap()
        .flat_map(|payload| {
            serde_json::from_str::<HistoryExportBatch>(&payload.unwrap())
                .unwrap()
                .records
        })
        .collect();
    let mut revisions = HashSet::new();
    for record in &records {
        assert!(
            revisions.insert(record.revision_id.clone()),
            "{} was delivered twice",
            record.revision_id
        );
    }
    records
        .into_iter()
        .map(|record| record.session_id.unwrap_or_default())
        .collect()
}

fn count(sessions: &[String], session: &str) -> usize {
    sessions.iter().filter(|s| *s == session).count()
}

fn held(conn: &Connection, job_id: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM delivery_held_records WHERE job_id=?",
        [job_id],
        |r| r.get(0),
    )
    .unwrap()
}

/// (a) A session is held until it has been quiet for the whole window, and
/// activity that resumes restarts the window.
#[test]
fn a_session_is_held_until_it_has_been_quiet_for_the_window() {
    for session_job in [false, true] {
        let (_directory, conn, job_id) = job(session_job, &["session"], 3);
        active(&conn, "session", 1_000);
        prepare_all(&conn, &job_id, 1_200, Some(WINDOW));
        assert!(delivered(&conn, &job_id).is_empty(), "{session_job}");
        assert_eq!(held(&conn, &job_id), 3);
        assert_eq!(
            waiting_until(&conn, &identity("session"), WINDOW).unwrap(),
            Some(1_500)
        );
        prepare_all(&conn, &job_id, 1_499, Some(WINDOW));
        assert!(delivered(&conn, &job_id).is_empty());
        // Activity resumes inside the window: it starts again.
        add_events(&conn, "session", 3..5);
        active(&conn, "session", 1_450);
        prepare_all(&conn, &job_id, 1_500, Some(WINDOW));
        assert!(delivered(&conn, &job_id).is_empty());
        assert_eq!(held(&conn, &job_id), 5);
        prepare_all(&conn, &job_id, 1_949, Some(WINDOW));
        assert!(delivered(&conn, &job_id).is_empty());
        // Quiet for the window: everything leaves, at once and in order.
        prepare_all(&conn, &job_id, 1_950, Some(WINDOW));
        assert_eq!(count(&delivered(&conn, &job_id), "session"), 5);
        assert_eq!(held(&conn, &job_id), 0);
    }
}

/// (b) Kept back inside the window, a session never uploads and stays kept
/// back; once the window has ended it can no longer be kept back.
#[test]
fn a_session_kept_back_in_its_window_never_uploads() {
    for session_job in [false, true] {
        let (_directory, conn, job_id) = job(session_job, &["kept", "other"], 2);
        active(&conn, "kept", 1_000);
        active(&conn, "other", 1_000);
        prepare_all(&conn, &job_id, 1_100, Some(WINDOW));
        assert_eq!(held(&conn, &job_id), 4);
        assert!(!window_ended(&conn, &[identity("kept")], WINDOW, 1_100).unwrap());
        set_session_excluded(&conn, &identity("kept"), true).unwrap();
        assert_eq!(held(&conn, &job_id), 2, "its waiting records are dropped");
        // More of it, after the window: none of it leaves.
        add_events(&conn, "kept", 2..4);
        prepare_all(&conn, &job_id, 10_000, Some(WINDOW));
        let sessions = delivered(&conn, &job_id);
        assert_eq!(count(&sessions, "kept"), 0, "{session_job}");
        assert_eq!(count(&sessions, "other"), 2);
        let excluded: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM delivery_exclusions WHERE source='claude' AND session_id='kept')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(excluded);
        // `other`'s window ended at 1 500: too late to keep it back.
        assert!(window_ended(&conn, &[identity("other")], WINDOW, 1_500).unwrap());
        assert!(window_ended(
            &conn,
            &[identity("kept"), identity("other")],
            WINDOW,
            10_000
        )
        .unwrap());
    }
}

/// A session the store has no activity time for is measured from when its
/// first record was held.
#[test]
fn a_session_without_activity_is_measured_from_when_it_was_held() {
    let (_directory, conn, job_id) = job(false, &["session"], 2);
    test_evidence(&conn)
        .execute("DELETE FROM sessions WHERE session_id='session'", [])
        .unwrap();
    assert_eq!(
        waiting_until(&conn, &identity("session"), WINDOW).unwrap(),
        None,
        "nothing waits yet"
    );
    assert!(!window_ended(&conn, &[identity("session")], WINDOW, 5_000).unwrap());
    prepare_all(&conn, &job_id, 2_000, Some(WINDOW));
    assert_eq!(held(&conn, &job_id), 2);
    assert_eq!(
        waiting_until(&conn, &identity("session"), WINDOW).unwrap(),
        Some(2_500)
    );
    prepare_all(&conn, &job_id, 2_499, Some(WINDOW));
    assert!(delivered(&conn, &job_id).is_empty());
    prepare_all(&conn, &job_id, 2_500, Some(WINDOW));
    assert_eq!(count(&delivered(&conn, &job_id), "session"), 2);
}

/// (c) Without a window, or with a window of zero, nothing waits.
#[test]
fn without_a_window_uploads_are_immediate() {
    for window in [None, Some(0)] {
        for session_job in [false, true] {
            let (_directory, conn, job_id) = job(session_job, &["session"], 3);
            active(&conn, "session", 1_000);
            prepare_all(&conn, &job_id, 1_000, window);
            assert_eq!(count(&delivered(&conn, &job_id), "session"), 3);
            assert_eq!(held(&conn, &job_id), 0);
        }
    }
}

/// (c) After the window a session uploads with nothing asked of anyone, and
/// a window turned off releases what waited at the next pass.
#[test]
fn after_the_window_a_session_uploads_regardless() {
    let (_directory, conn, job_id) = job(false, &["session"], 3);
    active(&conn, "session", 1_000);
    prepare_all(&conn, &job_id, 1_000, Some(WINDOW));
    assert_eq!(held(&conn, &job_id), 3);
    prepare_all(&conn, &job_id, 1_500, Some(WINDOW));
    assert_eq!(count(&delivered(&conn, &job_id), "session"), 3);

    let (_directory, conn, job_id) = job(true, &["session"], 3);
    active(&conn, "session", 1_000);
    prepare_all(&conn, &job_id, 1_000, Some(WINDOW));
    assert_eq!(held(&conn, &job_id), 3);
    // The operator removes the window: the hold is released at once.
    prepare_all(&conn, &job_id, 1_001, None);
    assert_eq!(count(&delivered(&conn, &job_id), "session"), 3);
}

/// (d) The hold is the database's: a restart keeps a session waiting and a
/// session kept back, and a window that ended while the app was closed
/// uploads at the first pass.
#[test]
fn the_hold_survives_a_restart() {
    for session_job in [false, true] {
        let (directory, conn, job_id) = job(session_job, &["waits", "kept"], 2);
        active(&conn, "waits", 1_000);
        active(&conn, "kept", 1_000);
        prepare_all(&conn, &job_id, 1_100, Some(WINDOW));
        set_session_excluded(&conn, &identity("kept"), true).unwrap();
        drop(conn);

        let conn = open_db(&directory.path().join("history.db")).unwrap();
        assert_eq!(held(&conn, &job_id), 2);
        prepare_all(&conn, &job_id, 1_200, Some(WINDOW));
        assert!(delivered(&conn, &job_id).is_empty(), "{session_job}");
        drop(conn);

        // Closed for a day.
        let conn = open_db(&directory.path().join("history.db")).unwrap();
        assert!(prepare(&conn, &job_id, 86_400_000, Some(WINDOW))
            .batch_id
            .is_some());
        let sessions = delivered(&conn, &job_id);
        assert_eq!(count(&sessions, "waits"), 2);
        assert_eq!(count(&sessions, "kept"), 0);
        assert_eq!(held(&conn, &job_id), 0);
    }
}

/// (e) Records held, released and captured while others wait each leave
/// exactly once, through any number of passes.
#[test]
fn an_existing_hold_moves_to_the_sidecar_without_early_upload() {
    let directory = tempfile::tempdir().unwrap();
    let history = directory.path().join("history.db");
    drop(ai_hist::SessionStore::open(ai_hist::StoreOptions::default().db_path(&history)).unwrap());
    let old = open_connection(&history).unwrap();
    schema::init(&old).unwrap();
    policy::init(&old).unwrap();
    schema::rebuild(&old, || feed::store(&old)).unwrap();
    counters::init(&old).unwrap();
    pending::init(&old).unwrap();
    add_events(&old, "session", 0..2);
    active(&old, "session", 1_000);
    let config = DeliveryJobConfig {
        destination_id: "fixture".into(),
        instance_id: "migration-hold".into(),
        account_id: "fixture".into(),
        mapping_version: "1".into(),
        selection: ExportSelection {
            all_sources: true,
            kinds: vec!["session_event".into()],
            ..Default::default()
        },
        limits: Default::default(),
    };
    let job_id = create_job(&old, &config, 0).unwrap().job_id;
    prepare_all(&old, &job_id, 1_100, Some(WINDOW));
    assert_eq!(held(&old, &job_id), 2);
    let copies = |conn: &Connection| {
        conn.prepare("SELECT revision_id,record_json,key_json,held_ms FROM delivery_held_records ORDER BY revision_id")
            .unwrap().query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?)))
            .unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap()
    };
    let before = copies(&old);
    drop(old);
    let queue = open_db(&history).unwrap();
    assert_eq!(
        copies(&queue),
        before,
        "migration preserves held records and their original clock"
    );
    assert!(
        queue
            .execute("UPDATE sessions SET last_activity_ms=0", [])
            .is_err(),
        "delivery still cannot write evidence"
    );
    assert!(prepare(&queue, &job_id, 1_499, Some(WINDOW))
        .batch_id
        .is_none());
    assert_eq!(held(&queue, &job_id), 2);
    prepare_all(&queue, &job_id, 1_500, Some(WINDOW));
    assert_eq!(count(&delivered(&queue, &job_id), "session"), 2);
    assert_eq!(held(&queue, &job_id), 0);
}

/// Every record leaves its hold once even as other sessions keep waiting.
#[test]
fn held_records_are_delivered_once() {
    for session_job in [false, true] {
        let (_directory, conn, job_id) = job(session_job, &["early", "late"], 3);
        active(&conn, "early", 1_000);
        active(&conn, "late", 1_300);
        let mut now = 1_000;
        while now <= 2_000 {
            prepare_all(&conn, &job_id, now, Some(WINDOW));
            if now == 1_200 {
                // `late` goes on while `early` waits.
                add_events(&conn, "late", 3..6);
                active(&conn, "late", 1_250);
            }
            now += 50;
        }
        let sessions = delivered(&conn, &job_id);
        assert_eq!(count(&sessions, "early"), 3, "{session_job}");
        assert_eq!(count(&sessions, "late"), 6);
        assert_eq!(held(&conn, &job_id), 0);
        // Passes after the release find nothing more.
        prepare_all(&conn, &job_id, 5_000, Some(WINDOW));
        assert_eq!(delivered(&conn, &job_id).len(), 9);
    }
}

/// A session's held copies leave in revision order before anything newer.
#[test]
fn released_records_keep_their_order() {
    let (_directory, conn, job_id) = job(false, &["session"], 4);
    active(&conn, "session", 1_000);
    prepare_all(&conn, &job_id, 1_000, Some(WINDOW));
    prepare_all(&conn, &job_id, 1_500, Some(WINDOW));
    let revisions: Vec<i64> = conn
        .prepare("SELECT payload FROM delivery_batches WHERE job_id=? ORDER BY seq")
        .unwrap()
        .query_map([&job_id], |r| r.get::<_, String>(0))
        .unwrap()
        .flat_map(|payload| {
            serde_json::from_str::<HistoryExportBatch>(&payload.unwrap())
                .unwrap()
                .records
        })
        .map(|record| record.revision)
        .collect();
    let mut sorted = revisions.clone();
    sorted.sort();
    assert_eq!(revisions, sorted);
    assert_eq!(revisions.len(), 4);
}

/// A feed renumbering drops the copies held under the old numbering: the
/// replay holds the records again, and each leaves once.
#[test]
fn a_renumbering_drops_the_stale_held_copies() {
    for session_job in [false, true] {
        let (_directory, conn, job_id) = job(session_job, &["session"], 3);
        active(&conn, "session", 1_000);
        prepare_all(&conn, &job_id, 1_100, Some(WINDOW));
        assert_eq!(held(&conn, &job_id), 3, "{session_job}");
        let stale: i64 = conn
            .query_row(
                "SELECT MAX(revision) FROM delivery_held_records WHERE job_id=?",
                [&job_id],
                |r| r.get(0),
            )
            .unwrap();
        // The origin no longer recognises the feed's epoch: it renumbers.
        conn.execute(
            "UPDATE delivery_origin SET feed_epoch=feed_epoch+7919,max_revision=MAX(max_revision,?)",
            [stale],
        )
        .unwrap();
        prepare_all(&conn, &job_id, 1_200, Some(WINDOW));
        assert_eq!(held(&conn, &job_id), 3, "the replay holds each record once");
        let lowest: i64 = conn
            .query_row(
                "SELECT MIN(revision) FROM delivery_held_records WHERE job_id=?",
                [&job_id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(lowest > stale, "only copies of the new numbering are held");
        prepare_all(&conn, &job_id, 1_500, Some(WINDOW));
        assert_eq!(count(&delivered(&conn, &job_id), "session"), 3);
        assert_eq!(held(&conn, &job_id), 0);
    }
}

/// Every text the queue holds for `job_id`: batch rows and oops-window copies.
fn queued_text(conn: &Connection, job_id: &str) -> String {
    let mut text = String::new();
    for sql in [
        "SELECT COALESCE(payload,'') || COALESCE(prepared,'') FROM delivery_batches WHERE job_id=?",
        "SELECT record_json FROM delivery_held_records WHERE job_id=?",
    ] {
        let rows = conn
            .prepare(sql)
            .unwrap()
            .query_map([job_id], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        text.extend(rows);
    }
    text
}

#[test]
fn a_held_copy_and_a_batch_row_store_the_scrubbed_payload_never_the_original() {
    for session_job in [false, true] {
        let (_dir, conn, job_id) = job(session_job, &["s1"], 2);
        let scrub = crate::history::scrub::testing::rewrite("oops", "clean");
        // Inside the window: the records are kept as oops-window copies.
        active(&conn, "s1", 1_000);
        prepare_scrubbed(&conn, &job_id, 1_000, Some(WINDOW), &scrub);
        let held = queued_text(&conn, &job_id);
        assert!(
            held.contains(r#""text":"clean""#),
            "session_job={session_job}: {held}"
        );
        assert!(
            !held.contains(r#""text":"oops""#),
            "session_job={session_job}: {held}"
        );
        // Past it: they leave the hold for a batch row.
        for _ in 0..50 {
            if prepare_scrubbed(&conn, &job_id, 1_000 + WINDOW + 1, Some(WINDOW), &scrub)
                .batch_id
                .is_none()
            {
                break;
            }
        }
        assert_eq!(
            delivered(&conn, &job_id).len(),
            2,
            "session_job={session_job}"
        );
        let queued = queued_text(&conn, &job_id);
        assert!(
            queued.contains(r#""text":"clean""#),
            "session_job={session_job}"
        );
        assert!(
            !queued.contains(r#""text":"oops""#),
            "session_job={session_job}: {queued}"
        );
    }
}

/// Where `job_id` stands in the feed: its cursor and every member's.
fn cursors(conn: &Connection, job_id: &str) -> String {
    conn.query_row(
        "SELECT j.journal_cursor || '/' || j.bootstrap_done || '/' || COALESCE((SELECT group_concat(m.session_id || ':' || m.cursor || ':' || m.phase || ':' || m.bootstrap_done) FROM delivery_session_members m WHERE m.job_id=j.id),'') FROM delivery_jobs j WHERE j.id=?",
        [job_id],
        |r| r.get(0),
    )
    .unwrap()
}

/// The job's state, failure and suppressed count.
fn verdict(conn: &Connection, job_id: &str) -> (String, Option<String>, i64) {
    conn.query_row(
        "SELECT state,failure,suppressed_records FROM delivery_jobs WHERE id=?",
        [job_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .unwrap()
}

#[test]
fn a_record_the_engine_refuses_blocks_the_job_without_moving_its_cursor() {
    for (session_job, window) in [(false, None), (true, None), (false, Some(WINDOW))] {
        let label = format!("session_job={session_job} window={window:?}");
        let (_dir, conn, job_id) = job(session_job, &["s1"], 2);
        let before = cursors(&conn, &job_id);
        let (scrub, calls) = crate::history::scrub::testing::refuse();
        active(&conn, "s1", 1_000);
        let result = prepare_scrubbed(&conn, &job_id, 1_000, window, &scrub);
        assert!(result.batch_id.is_none(), "{label}");
        assert_eq!(result.scanned_records, 0, "{label}");
        assert!(calls.load(std::sync::atomic::Ordering::SeqCst) >= 1);
        let queued = queued_text(&conn, &job_id);
        assert!(queued.is_empty(), "{label}: {queued}");
        assert_eq!(cursors(&conn, &job_id), before, "{label}: the cursor moved");
        assert_eq!(
            verdict(&conn, &job_id),
            ("blocked".into(), Some("invalid_payload".into()), 0),
            "{label}: blocked, and nothing counted as suppressed"
        );

        // A retry offers the same records again; an engine that accepts
        // them now queues both, so none was lost.
        retry_job(&conn, &job_id).unwrap();
        for _ in 0..50 {
            if prepare(&conn, &job_id, 1_000 + WINDOW + 1, window)
                .batch_id
                .is_none()
            {
                break;
            }
        }
        assert_eq!(delivered(&conn, &job_id).len(), 2, "{label}");
        assert_eq!(verdict(&conn, &job_id).2, 0, "{label}");
    }
}

/// An engine rewriting `oops` to `clean`, and its call count.
fn counted_rewrite() -> (ScrubEngine, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let engine = ScrubEngine::new(crate::history::scrub::testing::Rewrite {
        from: "oops",
        to: "clean",
        calls: calls.clone(),
    });
    (engine, calls)
}

/// The engine names `job_id`'s held copies record, in revision order.
fn held_engines(conn: &Connection, job_id: &str) -> Vec<Option<String>> {
    conn.prepare("SELECT scrub_engine FROM delivery_held_records WHERE job_id=? ORDER BY revision")
        .unwrap()
        .query_map([job_id], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

/// Releases `job_id`'s copies past the window under `scrub`, until a pass
/// prepares nothing more.
fn release_all(conn: &Connection, job_id: &str, scrub: &ScrubEngine) {
    for _ in 0..50 {
        let now_ms = 1_000 + WINDOW + 1;
        if prepare_scrubbed(conn, job_id, now_ms, Some(WINDOW), scrub)
            .batch_id
            .is_none()
        {
            return;
        }
    }
    panic!("release did not settle");
}

#[test]
fn a_released_copy_is_queued_as_it_was_kept_without_a_second_scrub() {
    use std::sync::atomic::Ordering;
    for session_job in [false, true] {
        let label = format!("session_job={session_job}");
        let (_dir, conn, job_id) = job(session_job, &["s1"], 2);
        let (scrub, calls) = counted_rewrite();
        active(&conn, "s1", 1_000);
        prepare_scrubbed(&conn, &job_id, 1_000, Some(WINDOW), &scrub);
        assert_eq!(held(&conn, &job_id), 2, "{label}");
        let engine = Some(scrub.name().to_owned());
        assert_eq!(
            held_engines(&conn, &job_id),
            [engine.clone(), engine],
            "{label}"
        );
        // Past the window, under the engine that scrubbed them when they
        // were kept: they are queued as stored and it is not asked again.
        let kept_calls = calls.load(Ordering::SeqCst);
        release_all(&conn, &job_id, &scrub);
        assert_eq!(calls.load(Ordering::SeqCst), kept_calls, "{label}");
        assert_eq!(held(&conn, &job_id), 0, "{label}");
        assert_eq!(delivered(&conn, &job_id).len(), 2, "{label}");
        assert_eq!(verdict(&conn, &job_id).0, "active", "{label}");
        let queued = queued_text(&conn, &job_id);
        assert!(queued.contains(r#""text":"clean""#), "{label}: {queued}");
        assert!(!queued.contains(r#""text":"oops""#), "{label}: {queued}");
    }
}

/// Holds `job_id`'s records under the placeholder passthrough, which keeps
/// them as they were read.
fn hold_unscrubbed(conn: &Connection, job_id: &str) {
    active(conn, "s1", 1_000);
    prepare(conn, job_id, 1_000, Some(WINDOW));
    assert_eq!(held(conn, job_id), 2);
    assert!(queued_text(conn, job_id).contains(r#""text":"oops""#));
}

#[test]
fn a_copy_kept_without_an_engine_name_is_scrubbed_when_released() {
    use std::sync::atomic::Ordering;
    for session_job in [false, true] {
        let label = format!("session_job={session_job}");
        let (_dir, conn, job_id) = job(session_job, &["s1"], 2);
        // As a build from before the seam kept them: raw, and no engine.
        hold_unscrubbed(&conn, &job_id);
        conn.execute("UPDATE delivery_held_records SET scrub_engine=NULL", [])
            .unwrap();
        assert_eq!(held_engines(&conn, &job_id), [None, None], "{label}");
        let (scrub, calls) = counted_rewrite();
        release_all(&conn, &job_id, &scrub);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "{label}");
        assert_eq!(held(&conn, &job_id), 0, "{label}");
        assert_eq!(delivered(&conn, &job_id).len(), 2, "{label}");
        let queued = queued_text(&conn, &job_id);
        assert!(queued.contains(r#""text":"clean""#), "{label}: {queued}");
        assert!(!queued.contains(r#""text":"oops""#), "{label}: {queued}");
    }
}

#[test]
fn a_copy_kept_under_another_engine_is_scrubbed_when_released() {
    use std::sync::atomic::Ordering;
    for session_job in [false, true] {
        let label = format!("session_job={session_job}");
        let (_dir, conn, job_id) = job(session_job, &["s1"], 2);
        hold_unscrubbed(&conn, &job_id);
        let placeholder = Some(ScrubEngine::default_engine().name().to_owned());
        assert_eq!(
            held_engines(&conn, &job_id),
            [placeholder.clone(), placeholder],
            "{label}"
        );
        // A build that pins a real engine releases them through it.
        let (scrub, calls) = counted_rewrite();
        release_all(&conn, &job_id, &scrub);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "{label}");
        assert_eq!(held(&conn, &job_id), 0, "{label}");
        assert_eq!(delivered(&conn, &job_id).len(), 2, "{label}");
        let queued = queued_text(&conn, &job_id);
        assert!(queued.contains(r#""text":"clean""#), "{label}: {queued}");
        assert!(!queued.contains(r#""text":"oops""#), "{label}: {queued}");
    }
}

#[test]
fn a_copy_kept_under_the_previous_relayscrubber_is_scrubbed_again_by_the_pinned_one() {
    const FAKE_TOKEN: &str = "ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij";
    const PREVIOUS: &str = "relayscrubber-0.5.0-alpha.2";
    let pinned = ScrubEngine::default_engine();
    assert_ne!(pinned.name(), PREVIOUS);
    assert_eq!(pinned.name(), crate::history::scrub::DEFAULT_ENGINE);
    for session_job in [false, true] {
        let label = format!("session_job={session_job}");
        let (_dir, conn, job_id) = job(session_job, &["s1"], 2);
        test_evidence(&conn)
            .execute(
                "UPDATE session_events SET text=?1",
                params![format!("token {FAKE_TOKEN}")],
            )
            .unwrap();
        // Held raw: the stand-in engine leaves this text alone.
        active(&conn, "s1", 1_000);
        let (stand_in, _) = counted_rewrite();
        prepare_scrubbed(&conn, &job_id, 1_000, Some(WINDOW), &stand_in);
        assert_eq!(held(&conn, &job_id), 2, "{label}");
        conn.execute(
            "UPDATE delivery_held_records SET scrub_engine='relayscrubber-0.5.0-alpha.2'",
            [],
        )
        .unwrap();
        assert_eq!(
            held_engines(&conn, &job_id),
            [Some(PREVIOUS.to_owned()), Some(PREVIOUS.to_owned())],
            "{label}"
        );
        assert!(queued_text(&conn, &job_id).contains(FAKE_TOKEN), "{label}");
        release_all(&conn, &job_id, &pinned);
        assert_eq!(held(&conn, &job_id), 0, "{label}");
        assert_eq!(delivered(&conn, &job_id).len(), 2, "{label}");
        let queued = queued_text(&conn, &job_id);
        assert!(queued.contains("[REDACTED]"), "{label}: {queued}");
        assert!(!queued.contains(FAKE_TOKEN), "{label}: {queued}");
    }
}

#[test]
fn a_held_records_table_from_an_older_build_gains_the_engine_column() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE delivery_held_records (
            job_id TEXT NOT NULL, revision_id TEXT NOT NULL, revision INTEGER NOT NULL,
            source TEXT NOT NULL, session_id TEXT NOT NULL, child_source TEXT, child_session_id TEXT,
            record_json TEXT NOT NULL, key_json TEXT NOT NULL, held_ms INTEGER NOT NULL,
            PRIMARY KEY(job_id,revision_id)
        );
        INSERT INTO delivery_held_records(job_id,revision_id,revision,source,session_id,record_json,key_json,held_ms)
            VALUES ('job','held-1',1,'claude','s1','{}','[]',10);",
    )
    .unwrap();
    assert!(!schema::column_exists(&conn, "delivery_held_records", "scrub_engine").unwrap());
    schema::init(&conn).unwrap();
    assert!(schema::column_exists(&conn, "delivery_held_records", "scrub_engine").unwrap());
    assert_eq!(held_engines(&conn, "job"), [None]);
    // A second start finds the column and leaves it.
    schema::init(&conn).unwrap();
    assert_eq!(held_engines(&conn, "job"), [None]);
}

#[test]
fn a_record_an_unresolved_batch_holds_never_reaches_the_engine() {
    use std::sync::atomic::Ordering;
    let (_dir, conn, job_id) = job(true, &["s1"], 2);
    let first = prepare(&conn, &job_id, 1_000, None);
    assert!(first.batch_id.is_some());
    assert_eq!(delivered(&conn, &job_id).len(), 2);
    // A membership change while that batch is unresolved restarts the
    // member's backfill, which reads both revisions again. Both are
    // accounted for, so a refusing engine is never asked and the job is
    // not blocked.
    set_job_session(&conn, &job_id, &identity("s1"), false).unwrap();
    set_job_session(&conn, &job_id, &identity("s1"), true).unwrap();
    let (refuse, calls) = crate::history::scrub::testing::refuse();
    let again = prepare_scrubbed(&conn, &job_id, 1_000, None, &refuse);
    assert!(again.batch_id.is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(verdict(&conn, &job_id), ("active".into(), None, 0));
    assert_eq!(delivered(&conn, &job_id).len(), 2);
}
