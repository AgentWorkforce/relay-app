//! An engine that panics on one record, as relayscrubber 0.5.0-alpha.10 did
//! on a quoted value that ended where it started: the record is quarantined
//! with a count, the rest of the history is stored and delivered, nothing of
//! the record is stored or printed, the panic never unwinds out of the scrub
//! seam, and a later engine offers the record again.
use super::*;
use crate::history::scrub::testing::panic_on;
use anyhow::Context;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

/// The synthetic text the stand-in engine panics on. Fixture content only.
const POISON: &str = "poison-synthetic-marker";

/// The stand-in for the engine that panics.
fn panicking() -> ScrubEngine {
    panic_on("panicking-engine-1", POISON, "oops", "clean")
}

/// The stand-in for a later engine with the panic fixed.
fn fixed() -> ScrubEngine {
    panic_on("fixed-engine-2", "\u{0}never", "oops", "clean")
}

/// The stand-in for a later engine that still panics on the record.
fn still_panicking() -> ScrubEngine {
    panic_on("still-panicking-engine-2", POISON, "oops", "clean")
}

/// What the receiver was sent, one entry per record: an event's text, or a
/// relationship's id.
fn sent(receiver: &Echo) -> Result<Vec<String>> {
    let mut sent = Vec::new();
    for body in receiver.bodies() {
        let batch: HistoryExportBatch = serde_json::from_str(&body)?;
        for record in batch.records {
            let payload = &record.payload;
            let text = payload.get("text").or(payload.get("relationship_uid"));
            let text = text.and_then(serde_json::Value::as_str);
            sent.push(text.context("a record with no text")?.to_owned());
        }
    }
    Ok(sent)
}

/// Every quarantine row's stored text for `job_id`.
fn quarantine_rows(db: &std::path::Path, job_id: &str) -> Result<Vec<String>> {
    let conn = open_db(db)?;
    let mut statement = conn.prepare("SELECT record_json || submitted_digest || current_digest FROM delivery_quarantine WHERE job_id=?")?;
    let rows = statement.query_map([job_id], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// How many of `job_id`'s quarantine rows were last tried by `engine`.
fn quarantined_under(db: &std::path::Path, job_id: &str, engine: &str) -> Result<i64> {
    Ok(open_db(db)?.query_row(
        "SELECT COUNT(*) FROM delivery_quarantine WHERE job_id=? AND submitted_digest=?",
        params![job_id, engine],
        |r| r.get(0),
    )?)
}

fn texts() -> [&'static str; 3] {
    ["oops", POISON, "oops"]
}

fn member(session_id: &str) -> SessionIdentity {
    SessionIdentity {
        source: "claude".into(),
        session_id: session_id.into(),
    }
}

#[test]
fn a_panic_on_one_record_quarantines_it_and_delivers_the_rest() -> Result<()> {
    for in_flight in IN_FLIGHT {
        let (_dir, db, job_id) = fixture_with(&texts(), DeliveryLimits::default());
        let receiver = Echo::default();
        drain_with(&db, &receiver, panicking(), in_flight);

        // The rest went, scrubbed; the bad record did not.
        assert_eq!(
            sent(&receiver)?,
            ["clean", "clean"],
            "in_flight={in_flight}"
        );
        assert!(!stored(&db, &job_id).contains(POISON));
        let job = status(&open_db(&db)?, &job_id)?;
        assert_eq!(job.state, "active", "in_flight={in_flight}");
        assert_eq!(job.failure, None, "in_flight={in_flight}");
        assert_eq!(job.acknowledged_records, 2, "in_flight={in_flight}");
        assert_eq!(job.quarantined_records, 1, "in_flight={in_flight}");
        assert_eq!(job.suppressed_records, 0, "in_flight={in_flight}");
        // The quarantine holds the record's identity, not its content.
        let rows = quarantine_rows(&db, &job_id)?;
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].contains(POISON), "{}", rows[0]);
        assert!(rows[0].contains("panicking-engine-1"), "{}", rows[0]);

        // The same engine does not offer it again, and the count holds.
        drain_with(&db, &receiver, panicking(), in_flight);
        assert_eq!(sent(&receiver)?.len(), 2, "in_flight={in_flight}");
        let job = status(&open_db(&db)?, &job_id)?;
        assert_eq!(job.quarantined_records, 1, "in_flight={in_flight}");
    }
    Ok(())
}

#[test]
fn a_later_engine_retries_a_quarantined_record_and_clears_it() -> Result<()> {
    for in_flight in IN_FLIGHT {
        let (_dir, db, job_id) = fixture_with(&texts(), DeliveryLimits::default());
        let receiver = Echo::default();
        drain_with(&db, &receiver, panicking(), in_flight);
        assert_eq!(quarantine_rows(&db, &job_id)?.len(), 1);

        drain_with(&db, &receiver, fixed(), in_flight);
        assert_eq!(
            sent(&receiver)?,
            ["clean", "clean", POISON],
            "in_flight={in_flight}"
        );
        let job = status(&open_db(&db)?, &job_id)?;
        assert_eq!(job.acknowledged_records, 3, "in_flight={in_flight}");
        assert_eq!(job.quarantined_records, 0, "in_flight={in_flight}");
        assert!(quarantine_rows(&db, &job_id)?.is_empty());
    }
    Ok(())
}

#[test]
fn a_record_that_panics_a_later_engine_too_stays_quarantined_under_it() -> Result<()> {
    let (_dir, db, job_id) = fixture_with(&texts(), DeliveryLimits::default());
    let receiver = Echo::default();
    drain_with(&db, &receiver, panicking(), 1);
    drain_with(&db, &receiver, still_panicking(), 1);
    assert_eq!(sent(&receiver)?, ["clean", "clean"]);
    let rows = quarantine_rows(&db, &job_id)?;
    assert_eq!(rows.len(), 1);
    assert!(rows[0].contains("still-panicking-engine-2"), "{}", rows[0]);
    let job = status(&open_db(&db)?, &job_id)?;
    assert_eq!(job.quarantined_records, 1);
    assert_eq!(job.state, "active");
    Ok(())
}

/// `engine`, counting every body it is asked to scrub, under its own name.
struct Counting {
    engine: ScrubEngine,
    calls: Arc<AtomicUsize>,
}

impl Scrub for Counting {
    fn name(&self) -> &str {
        self.engine.name()
    }
    fn scrub(&self, body: &[u8], context: &ScrubContext<'_>) -> Result<Vec<u8>, ScrubError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.engine.scrub(body, context)
    }
}

fn counting(engine: ScrubEngine) -> (ScrubEngine, Arc<AtomicUsize>) {
    let calls = Arc::default();
    let counted = Counting {
        engine,
        calls: Arc::clone(&calls),
    };
    (ScrubEngine::new(counted), calls)
}

/// The engine a record was quarantined under is never asked about it again:
/// a drain with nothing new calls it not once. An engine of another name is
/// asked again.
#[test]
fn the_engine_a_record_was_quarantined_under_is_not_called_on_it_again() -> Result<()> {
    let (_dir, db, job_id) = fixture_with(&texts(), DeliveryLimits::default());
    let receiver = Echo::default();
    let (engine, calls) = counting(panicking());
    drain_with(&db, &receiver, engine.clone(), 1);
    assert!(calls.load(Ordering::SeqCst) > 0);
    assert_eq!(status(&open_db(&db)?, &job_id)?.quarantined_records, 1);

    calls.store(0, Ordering::SeqCst);
    drain_with(&db, &receiver, engine, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 0, "the same engine retried");
    assert_eq!(quarantined_under(&db, &job_id, "panicking-engine-1")?, 1);

    let (later, later_calls) = counting(still_panicking());
    drain_with(&db, &receiver, later, 1);
    assert_eq!(
        later_calls.load(Ordering::SeqCst),
        1,
        "a later engine is asked once"
    );
    assert_eq!(
        quarantined_under(&db, &job_id, "still-panicking-engine-2")?,
        1
    );
    Ok(())
}

/// Sets the feed position job `job_id`'s one quarantine row names.
fn point_marker_at(db: &std::path::Path, job_id: &str, feed_revision: i64) -> Result<()> {
    let changed = open_db(db)?.execute(
        "UPDATE delivery_quarantine SET record_json=json_set(record_json,'$.feed_revision',?) WHERE job_id=?",
        params![feed_revision, job_id],
    )?;
    anyhow::ensure!(changed == 1, "expected one quarantine row, found {changed}");
    Ok(())
}

/// A quarantined record the store replaced before the retry: its feed
/// position no longer holds it, so the retry drops the marker without
/// sending anything, and the replacement reaches the receiver through the
/// feed like any other change. The old content is never sent.
#[test]
fn a_quarantined_record_superseded_before_its_retry_is_dropped_for_its_replacement() -> Result<()> {
    let (_dir, db, job_id) = fixture_with(&texts(), DeliveryLimits::default());
    let receiver = Echo::default();
    drain_with(&db, &receiver, panicking(), 1);
    assert_eq!(status(&open_db(&db)?, &job_id)?.quarantined_records, 1);
    let conn = open_db(&db)?;
    let replaced = test_evidence(&conn).execute(
        "UPDATE session_events SET text='oops replaced' WHERE event_uid='e-1'",
        [],
    )?;
    assert_eq!(replaced, 1);
    drop(conn);

    drain_with(&db, &receiver, fixed(), 1);
    assert_eq!(sent(&receiver)?, ["clean", "clean", "clean replaced"]);
    let job = status(&open_db(&db)?, &job_id)?;
    assert_eq!(job.quarantined_records, 0);
    assert_eq!(job.acknowledged_records, 3);
    Ok(())
}

/// A marker whose feed position now holds another record, as after the feed
/// was renumbered, one that names no position, or one past the store's head,
/// as after the store was replaced by a shorter one: the retry finds no
/// record of the quarantined revision there, so it drops the marker and
/// sends nothing, never the record now at that position a second time, and
/// the preparation still succeeds.
#[test]
fn a_quarantine_marker_pointing_at_another_record_or_nowhere_is_dropped_unsent() -> Result<()> {
    for feed_revision in [1, 0, 1_000_000] {
        let (_dir, db, job_id) = fixture_with(&texts(), DeliveryLimits::default());
        let receiver = Echo::default();
        drain_with(&db, &receiver, panicking(), 1);
        point_marker_at(&db, &job_id, feed_revision)?;

        drain_with(&db, &receiver, fixed(), 1);
        assert_eq!(
            sent(&receiver)?,
            ["clean", "clean"],
            "feed_revision={feed_revision}"
        );
        let job = status(&open_db(&db)?, &job_id)?;
        assert_eq!(job.quarantined_records, 0, "feed_revision={feed_revision}");
        assert_eq!(job.acknowledged_records, 2, "feed_revision={feed_revision}");
    }
    Ok(())
}

/// A preparation reads and offers at most `RETRY_BUDGET` quarantined records,
/// and the next one moves on to the next slice, so a long quarantine that
/// panics a later engine again is worked through without holding up a
/// preparation, and one a fixed engine accepts is delivered in full.
#[test]
fn a_long_quarantine_is_retried_a_bounded_slice_per_preparation() -> Result<()> {
    let total = 2 * filling::RETRY_BUDGET + 3;
    let (_dir, db, job_id) = fixture_with(&vec![POISON; total], DeliveryLimits::default());
    let receiver = Echo::default();
    drain_with(&db, &receiver, panicking(), 1);
    assert_eq!(quarantine_rows(&db, &job_id)?.len(), total);

    let conn = open_db(&db)?;
    let store = feed::store(&conn)?;
    let engine = still_panicking();
    let intake = || hold::Intake {
        oops_window_ms: None,
        scrub: &engine,
    };
    let published = PublishedLimits::default;
    let mut retried = Vec::new();
    for _ in 0..3 {
        hold::prepare_batch_scrubbed(&conn, &store, &job_id, 1_000, 1, published(), intake())?;
        retried.push(quarantined_under(&db, &job_id, engine.name())?);
    }
    let budget = filling::RETRY_BUDGET as i64;
    assert_eq!(retried, [budget, 2 * budget, total as i64]);
    assert_eq!(status(&conn, &job_id)?.quarantined_records, total as i64);

    drain_with(&db, &receiver, fixed(), 1);
    assert_eq!(sent(&receiver)?.len(), total);
    assert_eq!(status(&conn, &job_id)?.quarantined_records, 0);
    Ok(())
}

#[test]
fn a_session_job_quarantines_a_panicking_record_and_delivers_the_rest() -> Result<()> {
    let (_dir, db, job_id) = fixture_with(&texts(), DeliveryLimits::default());
    adopt_session_job(&open_db(&db)?, &job_id, &[member("s1")])?;
    let receiver = Echo::default();
    drain_with(&db, &receiver, panicking(), 1);
    assert_eq!(sent(&receiver)?, ["clean", "clean"]);
    assert!(!stored(&db, &job_id).contains(POISON));
    let job = status(&open_db(&db)?, &job_id)?;
    assert_eq!(job.quarantined_records, 1);
    assert_eq!(job.failure, None);

    drain_with(&db, &receiver, fixed(), 1);
    assert_eq!(sent(&receiver)?, ["clean", "clean", POISON]);
    let job = status(&open_db(&db)?, &job_id)?;
    assert_eq!(job.quarantined_records, 0);
    Ok(())
}

/// A session job sending events and relationships whose members are
/// `members`, in that order, over an event of `child` and an edge to it from
/// `parent`. The edge's id carries the text the engine panics on. With both
/// members, the child was added after the parent, so the child's
/// incoming-edges phase sends the edge and quarantines it.
fn edge_fixture(members: &[&str]) -> Result<(tempfile::TempDir, std::path::PathBuf, String)> {
    let dir = tempfile::tempdir()?;
    let db = dir.path().join("history.db");
    let conn = open_db(&db)?;
    let evidence = test_evidence(&conn);
    evidence.execute(
        "INSERT INTO session_events(source,session_id,event_uid,ts_ms,role,kind,text) VALUES ('claude','child','e-child',42,'user','text','oops')",
        [],
    )?;
    evidence.execute(
        "INSERT INTO session_relationships(source,parent_session_id,relationship_uid,child_session_id,relationship,identity_status,evidence_kind,created_ms,updated_ms) VALUES('claude','parent',?1,'child','handoff','observed','fixture',1,1)",
        [format!("edge-{POISON}")],
    )?;
    drop(evidence);
    let config = DeliveryJobConfig {
        destination_id: "fixture".into(),
        instance_id: "scrub".into(),
        account_id: "fixture-account".into(),
        mapping_version: "1".into(),
        selection: ExportSelection {
            all_sources: true,
            kinds: vec!["session_event".into(), "relationship".into()],
            ..ExportSelection::default()
        },
        limits: DeliveryLimits::default(),
    };
    let job_id = create_job(&conn, &config, 0)?.job_id;
    let identities: Vec<_> = members.iter().map(|id| member(id)).collect();
    adopt_session_job(&conn, &job_id, &identities)?;
    Ok((dir, db, job_id))
}

/// An edge the child's incoming-edges phase quarantined is sent once a
/// later engine accepts it, while both ends are members.
#[test]
fn a_quarantined_edge_is_resent_while_both_ends_are_members() -> Result<()> {
    let (_dir, db, job_id) = edge_fixture(&["parent", "child"])?;
    let receiver = Echo::default();
    drain_with(&db, &receiver, panicking(), 1);
    assert_eq!(sent(&receiver)?, ["clean"]);
    assert_eq!(status(&open_db(&db)?, &job_id)?.quarantined_records, 1);

    drain_with(&db, &receiver, fixed(), 1);
    assert_eq!(
        sent(&receiver)?,
        ["clean".to_owned(), format!("edge-{POISON}")]
    );
    assert_eq!(status(&open_db(&db)?, &job_id)?.quarantined_records, 0);
    Ok(())
}

/// The edge was quarantined from its member child's incoming-edges phase;
/// its parent then left the job. The child still owns the edge, so the retry
/// offers it to the job's consent instead of dropping it unseen. Consent
/// sends a session job's relationship only while both ends are members, so
/// the edge is withheld and counted as suppressed, never dropped uncounted.
#[test]
fn a_quarantined_edge_whose_parent_left_is_offered_to_consent_for_its_member_child() -> Result<()> {
    let (_dir, db, job_id) = edge_fixture(&["parent", "child"])?;
    let receiver = Echo::default();
    drain_with(&db, &receiver, panicking(), 1);
    assert_eq!(status(&open_db(&db)?, &job_id)?.quarantined_records, 1);
    set_job_session(&open_db(&db)?, &job_id, &member("parent"), false)?;
    assert!(job_session_included(
        &open_db(&db)?,
        &job_id,
        &member("child")
    )?);
    let before = status(&open_db(&db)?, &job_id)?.suppressed_records;

    drain_with(&db, &receiver, fixed(), 1);
    assert_eq!(sent(&receiver)?, ["clean"]);
    let job = status(&open_db(&db)?, &job_id)?;
    assert_eq!(job.suppressed_records, before + 1);
    assert_eq!(job.quarantined_records, 0);
    Ok(())
}

/// Once neither end of a quarantined edge is a member, the retry drops it
/// from the quarantine without sending it or offering it to consent.
#[test]
fn a_quarantined_edge_is_not_resent_once_no_member_owns_it() -> Result<()> {
    let (_dir, db, job_id) = edge_fixture(&["parent", "child", "other"])?;
    let receiver = Echo::default();
    drain_with(&db, &receiver, panicking(), 1);
    assert_eq!(status(&open_db(&db)?, &job_id)?.quarantined_records, 1);
    for removed in ["parent", "child"] {
        set_job_session(&open_db(&db)?, &job_id, &member(removed), false)?;
    }
    let before = status(&open_db(&db)?, &job_id)?.suppressed_records;

    drain_with(&db, &receiver, fixed(), 1);
    assert_eq!(sent(&receiver)?, ["clean"]);
    let job = status(&open_db(&db)?, &job_id)?;
    assert_eq!(job.quarantined_records, 0);
    assert_eq!(job.suppressed_records, before);
    Ok(())
}

/// Run by [`an_engine_panic_never_reaches_an_aborting_panic_hook`] in a
/// child process. The build still unwinds; the child installs a panic hook
/// that aborts the process, so any panic that escapes the scrub seam's own
/// hook ends the child, as an escaping panic ended the collector. A drain
/// over a record the engine panics on must finish and deliver the rest.
#[test]
fn engine_panic_child() -> Result<()> {
    if std::env::var_os("RELAY_SCRUB_PANIC_CHILD").is_none() {
        return Ok(());
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        std::process::abort();
    }));
    let (_dir, db, job_id) = fixture_with(&texts(), DeliveryLimits::default());
    let receiver = Echo::default();
    drain_with(&db, &receiver, panicking(), 1);
    assert_eq!(sent(&receiver)?, ["clean", "clean"]);
    assert_eq!(status(&open_db(&db)?, &job_id)?.quarantined_records, 1);
    println!("child-drain-finished");
    Ok(())
}

#[test]
fn an_engine_panic_never_reaches_an_aborting_panic_hook() -> Result<()> {
    let output = std::process::Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "history::delivery::scrub_tests::panics::engine_panic_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("RELAY_SCRUB_PANIC_CHILD", "1")
        .output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(
        stdout.contains("child-drain-finished"),
        "{stdout}\n{stderr}"
    );
    // The engine's panic is reported as a redacted class; the slicing
    // panic's own message, which quotes the record, is never printed.
    assert!(stderr.contains("reason=engine_panicked"), "{stderr}");
    assert!(stderr.contains("message withheld"), "{stderr}");
    assert!(!stderr.contains(POISON), "{stderr}");
    assert!(!stdout.contains(POISON), "{stdout}");
    Ok(())
}
