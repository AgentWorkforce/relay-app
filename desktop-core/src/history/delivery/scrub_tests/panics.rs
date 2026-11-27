//! An engine that panics on one record, as relayscrubber 0.5.0-alpha.10 did
//! on a quoted value that ended where it started: the record is quarantined
//! with a count, the rest of the history is stored and delivered, nothing of
//! the record is stored or printed, the collector keeps running, and a later
//! engine offers the record again.
use super::*;
use crate::history::scrub::testing::panic_on;

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

/// Every text the receiver was sent, one entry per record.
fn sent_texts(receiver: &Echo) -> Vec<String> {
    receiver
        .bodies()
        .iter()
        .flat_map(|body| {
            let batch: HistoryExportBatch = serde_json::from_str(body).unwrap();
            batch
                .records
                .into_iter()
                .map(|record| record.payload["text"].as_str().unwrap().to_owned())
        })
        .collect()
}

/// Every quarantine row's stored text for `job_id`.
fn quarantine_rows(db: &std::path::Path, job_id: &str) -> Vec<String> {
    open_db(db)
        .unwrap()
        .prepare("SELECT record_json || submitted_digest || current_digest FROM delivery_quarantine WHERE job_id=?")
        .unwrap()
        .query_map([job_id], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn texts() -> [&'static str; 3] {
    ["oops", POISON, "oops"]
}

#[test]
fn a_panic_on_one_record_quarantines_it_and_delivers_the_rest() {
    for in_flight in IN_FLIGHT {
        let (_dir, db, job_id) = fixture_with(&texts(), DeliveryLimits::default());
        let receiver = Echo::default();
        drain_with(&db, &receiver, panicking(), in_flight);

        // The rest went, scrubbed; the bad record did not.
        assert_eq!(
            sent_texts(&receiver),
            ["clean", "clean"],
            "in_flight={in_flight}"
        );
        assert!(!stored(&db, &job_id).contains(POISON));
        let job = status(&open_db(&db).unwrap(), &job_id).unwrap();
        assert_eq!(job.state, "active", "in_flight={in_flight}");
        assert_eq!(job.failure, None, "in_flight={in_flight}");
        assert_eq!(job.acknowledged_records, 2, "in_flight={in_flight}");
        assert_eq!(job.quarantined_records, 1, "in_flight={in_flight}");
        assert_eq!(job.suppressed_records, 0, "in_flight={in_flight}");
        // The quarantine holds the record's identity, not its content.
        let rows = quarantine_rows(&db, &job_id);
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].contains(POISON), "{}", rows[0]);
        assert!(rows[0].contains("panicking-engine-1"), "{}", rows[0]);

        // The same engine does not offer it again, and the count holds.
        drain_with(&db, &receiver, panicking(), in_flight);
        assert_eq!(sent_texts(&receiver).len(), 2, "in_flight={in_flight}");
        let job = status(&open_db(&db).unwrap(), &job_id).unwrap();
        assert_eq!(job.quarantined_records, 1, "in_flight={in_flight}");
    }
}

#[test]
fn a_later_engine_retries_a_quarantined_record_and_clears_it() {
    for in_flight in IN_FLIGHT {
        let (_dir, db, job_id) = fixture_with(&texts(), DeliveryLimits::default());
        let receiver = Echo::default();
        drain_with(&db, &receiver, panicking(), in_flight);
        assert_eq!(quarantine_rows(&db, &job_id).len(), 1);

        drain_with(&db, &receiver, fixed(), in_flight);
        assert_eq!(
            sent_texts(&receiver),
            ["clean", "clean", POISON],
            "in_flight={in_flight}"
        );
        let job = status(&open_db(&db).unwrap(), &job_id).unwrap();
        assert_eq!(job.acknowledged_records, 3, "in_flight={in_flight}");
        assert_eq!(job.quarantined_records, 0, "in_flight={in_flight}");
        assert!(quarantine_rows(&db, &job_id).is_empty());
    }
}

#[test]
fn a_record_that_panics_a_later_engine_too_stays_quarantined_under_it() {
    let (_dir, db, job_id) = fixture_with(&texts(), DeliveryLimits::default());
    let receiver = Echo::default();
    drain_with(&db, &receiver, panicking(), 1);
    let still = panic_on("still-panicking-engine-2", POISON, "oops", "clean");
    drain_with(&db, &receiver, still, 1);
    assert_eq!(sent_texts(&receiver), ["clean", "clean"]);
    let rows = quarantine_rows(&db, &job_id);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].contains("still-panicking-engine-2"), "{}", rows[0]);
    let job = status(&open_db(&db).unwrap(), &job_id).unwrap();
    assert_eq!(job.quarantined_records, 1);
    assert_eq!(job.state, "active");
}

#[test]
fn a_session_job_quarantines_a_panicking_record_and_delivers_the_rest() {
    let (_dir, db, job_id) = fixture_with(&texts(), DeliveryLimits::default());
    adopt_session_job(
        &open_db(&db).unwrap(),
        &job_id,
        &[SessionIdentity {
            source: "claude".into(),
            session_id: "s1".into(),
        }],
    )
    .unwrap();
    let receiver = Echo::default();
    drain_with(&db, &receiver, panicking(), 1);
    assert_eq!(sent_texts(&receiver), ["clean", "clean"]);
    assert!(!stored(&db, &job_id).contains(POISON));
    let job = status(&open_db(&db).unwrap(), &job_id).unwrap();
    assert_eq!(job.quarantined_records, 1);
    assert_eq!(job.failure, None);

    drain_with(&db, &receiver, fixed(), 1);
    assert_eq!(sent_texts(&receiver), ["clean", "clean", POISON]);
    let job = status(&open_db(&db).unwrap(), &job_id).unwrap();
    assert_eq!(job.quarantined_records, 0);
}

/// Run by [`an_engine_panic_does_not_end_the_process`] in a child process:
/// with any panic that escapes the scrub seam aborting the process, as one
/// on the collector's main thread ends it, a drain over a record the engine
/// panics on finishes and delivers the rest.
#[test]
fn engine_panic_child() {
    if std::env::var_os("RELAY_SCRUB_PANIC_CHILD").is_none() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        previous(info);
        std::process::abort();
    }));
    let (_dir, db, job_id) = fixture_with(&texts(), DeliveryLimits::default());
    let receiver = Echo::default();
    drain_with(&db, &receiver, panicking(), 1);
    assert_eq!(sent_texts(&receiver), ["clean", "clean"]);
    let job = status(&open_db(&db).unwrap(), &job_id).unwrap();
    assert_eq!(job.quarantined_records, 1);
    println!("child-drain-finished");
}

#[test]
fn an_engine_panic_does_not_end_the_process() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "history::delivery::scrub_tests::panics::engine_panic_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("RELAY_SCRUB_PANIC_CHILD", "1")
        .output()
        .unwrap();
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
}
