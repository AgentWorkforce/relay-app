//! The scrub seam on the upload path: a drain stores and sends only what the
//! engine returned, and a body the engine refuses is neither stored nor sent.
use super::worker::{drain, DrainOptions, PreparedBody, Receiver, ReceiverContext};
use super::worker::{ReceiverFailure, SingleReceiver};
use super::*;
use crate::history::scrub::testing::{refuse, rewrite};
use crate::history::scrub::{Scrub, ScrubContext, ScrubError, ScrubSurface};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Maps a batch to its JSON, after `marker`, and keeps every body it is
/// asked to send with the digest it was sent under and the body its own
/// mapping made of the batch.
#[derive(Default)]
struct Echo {
    marker: &'static str,
    sent: Mutex<Vec<(String, String, String)>>,
    lose_next_ack: AtomicBool,
    partial_next_ack: AtomicBool,
}

impl Echo {
    fn mapped(&self, batch: &HistoryExportBatch) -> String {
        format!("{}{}", self.marker, serde_json::to_string(batch).unwrap())
    }
    fn bodies(&self) -> Vec<String> {
        let sent = self.sent.lock().unwrap();
        sent.iter().map(|(body, _, _)| body.clone()).collect()
    }
}

impl Receiver for Echo {
    fn mapping_version(&self) -> &str {
        "1"
    }
    fn supported_kinds(&self) -> &[&str] {
        SUPPORTED_KINDS
    }
    fn supports_tombstones(&self) -> bool {
        true
    }
    fn concurrent(&self) -> Option<&(dyn Receiver + Sync)> {
        Some(self)
    }
    fn prepare(
        &self,
        batch: &HistoryExportBatch,
        context: &ReceiverContext<'_>,
    ) -> std::result::Result<PreparedBody, ReceiverFailure> {
        assert_eq!(context.idempotency_key, batch.batch_id);
        Ok(PreparedBody {
            content_type: "application/json".into(),
            body: self.mapped(batch),
        })
    }
    fn send(
        &self,
        payload: &PreparedPayload,
        batch: &HistoryExportBatch,
        context: &ReceiverContext<'_>,
    ) -> std::result::Result<DeliveryAcknowledgment, ReceiverFailure> {
        self.sent.lock().unwrap().push((
            payload.body.clone(),
            payload.sha256.clone(),
            self.mapped(batch),
        ));
        if self.lose_next_ack.swap(false, Ordering::SeqCst) {
            return Err(DeliveryFailure::Transient.into());
        }
        let partial = self.partial_next_ack.swap(false, Ordering::SeqCst);
        Ok(DeliveryAcknowledgment {
            batch_id: batch.batch_id.clone(),
            accepted_revision_ids: batch
                .records
                .iter()
                .filter(|record| {
                    !context
                        .suppressed_revision_ids
                        .contains(&record.revision_id)
                })
                .take(if partial { 1 } else { usize::MAX })
                .map(|record| record.revision_id.clone())
                .collect(),
            unsupported_revision_ids: vec![],
            suppressed_revision_ids: if partial {
                vec![batch.records[1].revision_id.clone()]
            } else {
                context.suppressed_revision_ids.to_vec()
            },
            rejected_revision_ids: vec![],
            isolation_frontier: vec![],
            acceptance_level: AcceptanceLevel::Durable,
        })
    }
}

/// The drains every test runs: one batch at a time, and pipelined.
const IN_FLIGHT: [usize; 2] = [1, 2];

/// A job over two events of one session, each with the text `oops`.
fn fixture() -> (tempfile::TempDir, std::path::PathBuf, String) {
    fixture_with(&["oops", "oops"], DeliveryLimits::default())
}

fn fixture_with(
    texts: &[&str],
    limits: DeliveryLimits,
) -> (tempfile::TempDir, std::path::PathBuf, String) {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("history.db");
    let conn = open_db(&db).unwrap();
    for (n, text) in texts.iter().enumerate() {
        test_evidence(&conn)
            .execute(
                "INSERT INTO session_events(source,session_id,event_uid,ts_ms,role,kind,text) VALUES ('claude','s1',?1,42,'user','text',?2)",
                params![format!("e-{n}"), text],
            )
            .unwrap();
    }
    let config = DeliveryJobConfig {
        destination_id: "fixture".into(),
        instance_id: "scrub".into(),
        account_id: "fixture-account".into(),
        mapping_version: "1".into(),
        selection: ExportSelection {
            all_sources: true,
            kinds: vec!["session_event".into()],
            ..ExportSelection::default()
        },
        limits,
    };
    let job_id = create_job(&conn, &config, 0).unwrap().job_id;
    (directory, db, job_id)
}

/// One drain of the fixture through `receiver` under `scrub`, with up to
/// `in_flight` batches in flight.
fn drain_with(db: &std::path::Path, receiver: &Echo, scrub: ScrubEngine, in_flight: usize) {
    let mut options = DrainOptions::new("scrub-test");
    options.scrub = scrub;
    options.max_in_flight = in_flight;
    let receivers = SingleReceiver::new("fixture", "scrub", receiver);
    drain(db, &receivers, &options, &|| 1_000, &|| false).unwrap();
}

/// Every batch row's stored text for `job_id`.
fn stored(db: &std::path::Path, job_id: &str) -> String {
    let conn = open_db(db).unwrap();
    let rows = conn
        .prepare("SELECT COALESCE(payload,'') || COALESCE(prepared,'') FROM delivery_batches WHERE job_id=?")
        .unwrap()
        .query_map([job_id], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    rows.concat()
}

/// The job's state, failure, cursor and suppressed count.
fn verdict(db: &std::path::Path, job_id: &str) -> (String, Option<String>, i64, i64) {
    open_db(db)
        .unwrap()
        .query_row(
            "SELECT state,failure,journal_cursor,suppressed_records FROM delivery_jobs WHERE id=?",
            [job_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap()
}

/// The one batch row's state and stored prepared body.
fn batch_row(db: &std::path::Path, job_id: &str) -> (String, Option<String>) {
    open_db(db)
        .unwrap()
        .query_row(
            "SELECT state,prepared FROM delivery_batches WHERE job_id=?",
            [job_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
}

#[test]
fn a_drain_sends_and_stores_the_rewritten_body_never_the_original() {
    for in_flight in IN_FLIGHT {
        let (_dir, db, job_id) = fixture();
        let receiver = Echo::default();
        drain_with(&db, &receiver, rewrite("oops", "clean"), in_flight);
        let sent = receiver.bodies();
        assert!(!sent.is_empty(), "in_flight={in_flight}");
        for body in &sent {
            assert!(body.contains("clean"), "in_flight={in_flight}: {body}");
            assert!(!body.contains("oops"), "in_flight={in_flight}: {body}");
        }
        assert!(
            !stored(&db, &job_id).contains("oops"),
            "in_flight={in_flight}"
        );
    }
}

/// Passes local stores through and refuses every upload.
struct RefuseUpload;
impl Scrub for RefuseUpload {
    fn name(&self) -> &str {
        "refuse-upload"
    }
    fn scrub(&self, body: &[u8], context: &ScrubContext<'_>) -> Result<Vec<u8>, ScrubError> {
        match context.surface {
            ScrubSurface::LocalStore => Ok(body.to_vec()),
            ScrubSurface::Upload => Err(ScrubError::new("refused at upload")),
        }
    }
}

#[test]
fn a_body_refused_before_sending_is_never_sent_and_its_batch_is_blocked() {
    for in_flight in IN_FLIGHT {
        let (_dir, db, job_id) = fixture();
        let receiver = Echo::default();
        drain_with(&db, &receiver, ScrubEngine::new(RefuseUpload), in_flight);
        drain_with(&db, &receiver, ScrubEngine::new(RefuseUpload), in_flight);
        assert!(receiver.bodies().is_empty(), "in_flight={in_flight}: sent");
        assert_eq!(
            batch_row(&db, &job_id).0,
            "blocked",
            "in_flight={in_flight}"
        );
        let status = status(&open_db(&db).unwrap(), &job_id).unwrap();
        assert_eq!(
            status.failure.as_deref(),
            Some("invalid_payload"),
            "in_flight={in_flight}"
        );
    }
}

/// Stores every body as it is and rewrites `oops` only in what is sent.
struct RewriteUpload;
impl Scrub for RewriteUpload {
    fn name(&self) -> &str {
        "rewrite-upload"
    }
    fn scrub(&self, body: &[u8], context: &ScrubContext<'_>) -> Result<Vec<u8>, ScrubError> {
        let text = std::str::from_utf8(body).unwrap();
        Ok(match context.surface {
            ScrubSurface::LocalStore => text.to_owned(),
            ScrubSurface::Upload => text.replace("oops", "clean"),
        }
        .into_bytes())
    }
}

#[test]
fn a_body_rewritten_only_for_upload_is_sent_rewritten_under_its_own_digest() {
    for in_flight in IN_FLIGHT {
        let (_dir, db, _) = fixture();
        let receiver = Echo::default();
        drain_with(&db, &receiver, ScrubEngine::new(RewriteUpload), in_flight);
        let sent = receiver.sent.lock().unwrap().clone();
        assert!(!sent.is_empty(), "in_flight={in_flight}");
        for (body, sha256, mapped) in &sent {
            assert!(body.contains("clean") && !body.contains("oops"), "{body}");
            assert_eq!(*sha256, hash(body), "in_flight={in_flight}: digest");
            // The source record retains its LocalStore representation.
            // The receiver request was scrubbed before its digest was frozen.
            assert!(mapped.contains("oops"), "{mapped}");
            assert_ne!(body, mapped, "in_flight={in_flight}");
            assert_ne!(*sha256, hash(mapped), "in_flight={in_flight}");
        }
    }
}

#[test]
fn a_frozen_batch_with_a_lost_ack_retries_the_scrubbed_successor_after_restart() {
    successor_retries_after_restart("");
}

#[test]
fn a_frozen_opaque_request_is_remapped_under_its_successor_identity() {
    successor_retries_after_restart("opaque:");
}

fn successor_retries_after_restart(marker: &'static str) {
    for in_flight in IN_FLIGHT {
        let (_dir, db, job_id) = fixture();
        let receiver = Echo {
            marker,
            ..Echo::default()
        };
        receiver.lose_next_ack.store(true, Ordering::SeqCst);
        drain_with(&db, &receiver, rewrite("unused", "unused"), in_flight);
        let first = receiver.bodies();
        assert_eq!(first.len(), 1);
        let old: HistoryExportBatch =
            serde_json::from_str(first[0].strip_prefix(marker).unwrap()).unwrap();
        assert!(first[0].contains("oops"));

        retry_job(&open_db(&db).unwrap(), &job_id).unwrap();
        receiver.lose_next_ack.store(true, Ordering::SeqCst);
        drain_with(&db, &receiver, ScrubEngine::new(RewriteUpload), in_flight);
        let second = receiver.bodies();
        assert_eq!(second.len(), 2);
        let new: HistoryExportBatch =
            serde_json::from_str(second[1].strip_prefix(marker).unwrap()).unwrap();
        assert_ne!(new.batch_id, old.batch_id);
        assert!(!second[1].contains("oops"));
        assert!(second[1].contains("clean"));
        assert_eq!(
            new.records
                .iter()
                .map(|r| &r.revision_id)
                .collect::<Vec<_>>(),
            old.records
                .iter()
                .map(|r| &r.revision_id)
                .collect::<Vec<_>>()
        );
        assert!(!stored(&db, &job_id).contains("oops"));

        retry_job(&open_db(&db).unwrap(), &job_id).unwrap();
        drain_with(&db, &receiver, ScrubEngine::new(RewriteUpload), in_flight);
        let third = receiver.bodies();
        assert_eq!(third.len(), 3);
        assert_eq!(third[2], third[1]);
        let status = status(&open_db(&db).unwrap(), &job_id).unwrap();
        assert_eq!(status.pending_records, 0);
        assert_eq!(status.acknowledged_records, 2);
        assert_eq!(status.suppressed_records, 0);
    }
}

#[test]
fn a_bounded_pipeline_drain_defers_later_claims_until_the_successor_is_sent() {
    let (_dir, db, job_id) = fixture_with(
        &["oops", "safe"],
        DeliveryLimits {
            max_batch_records: 1,
            ..DeliveryLimits::default()
        },
    );
    let conn = open_db(&db).unwrap();
    let store = feed::store(&conn).unwrap();
    for _ in 0..2 {
        prepare_batch_from(&conn, &store, &job_id, 0, 2, PublishedLimits::default()).unwrap();
    }
    let first = claim_batch(&conn, &job_id, "freeze", 30_000, &|| 1_000)
        .unwrap()
        .unwrap();
    let NextClaim::Claimed(second) = claim_next(&conn, &first.lease, &|| 1_000).unwrap() else {
        panic!("second batch not prepared");
    };
    for claim in [&first, &*second] {
        store_prepared_payload(
            &conn,
            &claim.lease,
            "1",
            "application/json",
            &serde_json::to_string(&claim.batch).unwrap(),
            &|| 1_000,
        )
        .unwrap();
    }
    release_stopped_claim(&conn, &first.lease, &|| 1_000).unwrap();
    release_stopped_claim(&conn, &second.lease, &|| 1_000).unwrap();
    drop(store);
    drop(conn);
    let receiver = Echo::default();
    let receivers = SingleReceiver::new("fixture", "scrub", &receiver);
    let mut options = DrainOptions::new("bounded-upgrade");
    options.max_in_flight = 2;
    options.max_batches = 2;
    options.scrub = ScrubEngine::new(RewriteUpload);
    drain(&db, &receivers, &options, &|| 1_000, &|| false).unwrap();
    assert!(
        receiver.bodies().is_empty(),
        "later batch overtook the persisted successor"
    );
    options.max_batches = 1;
    drain(&db, &receivers, &options, &|| 1_000, &|| false).unwrap();
    let sent = receiver.bodies();
    assert_eq!(sent.len(), 1);
    let successor: HistoryExportBatch = serde_json::from_str(&sent[0]).unwrap();
    assert_ne!(successor.batch_id, first.batch.batch_id);
    assert_eq!(
        successor.records[0].revision_id,
        first.batch.records[0].revision_id
    );
    drain(&db, &receivers, &options, &|| 1_000, &|| false).unwrap();
    let sent = receiver.bodies();
    assert_eq!(sent.len(), 2);
    let later: HistoryExportBatch = serde_json::from_str(&sent[1]).unwrap();
    assert_eq!(later.batch_id, second.batch.batch_id);
    assert_eq!(sent[1], serde_json::to_string(&second.batch).unwrap());
    let complete = status(&open_db(&db).unwrap(), &job_id).unwrap();
    assert_eq!(complete.acknowledged_records, 2);
    assert_eq!(complete.pending_records, 0);
}

#[test]
fn a_partial_receipt_survives_successor_replacement_and_restart() {
    for in_flight in IN_FLIGHT {
        let (_dir, db, job_id) = fixture_with(&["oops", "oops", "oops"], DeliveryLimits::default());
        let receiver = Echo::default();
        receiver.partial_next_ack.store(true, Ordering::SeqCst);
        drain_with(&db, &receiver, rewrite("unused", "unused"), in_flight);
        let first: HistoryExportBatch = serde_json::from_str(&receiver.bodies()[0]).unwrap();
        let accepted = vec![first.records[0].revision_id.clone()];
        let suppressed = vec![first.records[1].revision_id.clone()];
        retry_job(&open_db(&db).unwrap(), &job_id).unwrap();
        let mut options = DrainOptions::new("receipt-upgrade");
        options.max_in_flight = in_flight;
        options.max_batches = 1;
        options.scrub = ScrubEngine::new(RewriteUpload);
        let receivers = SingleReceiver::new("fixture", "scrub", &receiver);
        drain(&db, &receivers, &options, &|| 1_000, &|| false).unwrap();
        assert_eq!(
            receiver.bodies().len(),
            1,
            "replacement sent before restart"
        );
        let conn = open_db(&db).unwrap();
        let next = claim_batch(&conn, &job_id, "restart", 30_000, &|| 1_000)
            .unwrap()
            .unwrap();
        assert_ne!(next.batch.batch_id, first.batch_id);
        assert_eq!(next.accepted_revision_ids, accepted);
        assert_eq!(next.suppressed_revision_ids, suppressed);
        let count: i64 = conn
            .query_row(
                "SELECT accepted_records FROM delivery_batches WHERE id=?",
                [&next.batch.batch_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        release_stopped_claim(&conn, &next.lease, &|| 1_000).unwrap();
        drop(conn);
        drain_with(&db, &receiver, ScrubEngine::new(RewriteUpload), in_flight);
        let complete = status(&open_db(&db).unwrap(), &job_id).unwrap();
        assert_eq!(complete.pending_records, 0);
        assert_eq!(complete.acknowledged_records, 2);
        assert_eq!(complete.suppressed_records, 1);
        drain_with(&db, &receiver, ScrubEngine::new(RewriteUpload), in_flight);
        assert_eq!(
            receiver.bodies().len(),
            2,
            "settled receipt counted or sent twice"
        );
    }
}

#[test]
fn a_failed_successor_write_keeps_the_frozen_request_for_a_safe_retry() {
    for in_flight in IN_FLIGHT {
        let (_dir, db, job_id) = fixture_with(&["oops", "oops", "oops"], DeliveryLimits::default());
        let receiver = Echo::default();
        receiver.lose_next_ack.store(true, Ordering::SeqCst);
        drain_with(&db, &receiver, rewrite("unused", "unused"), in_flight);
        let original = batch_row(&db, &job_id).1.unwrap();
        let conn = open_db(&db).unwrap();
        let old: HistoryExportBatch = serde_json::from_str(&receiver.bodies()[0]).unwrap();
        let accepted = serde_json::to_string(&[&old.records[0].revision_id]).unwrap();
        let suppressed = serde_json::to_string(&[&old.records[1].revision_id]).unwrap();
        let original_queue = frozen_position(&conn, &job_id);
        conn.execute(
            "UPDATE delivery_batches SET accepted_revision_ids=?,suppressed_revision_ids=?,accepted_records=1 WHERE job_id=?",
            params![accepted, suppressed, job_id],
        )
        .unwrap();
        conn.execute_batch(
            "CREATE TRIGGER refuse_scrub_successor BEFORE UPDATE ON delivery_batches
             WHEN NEW.state='suppressed' AND OLD.state='leased'
               AND (SELECT COUNT(*) FROM delivery_batches)=2
             BEGIN SELECT RAISE(ABORT,'synthetic successor failure'); END;",
        )
        .unwrap();
        retry_job(&conn, &job_id).unwrap();
        drop(conn);

        drain_with(&db, &receiver, ScrubEngine::new(RewriteUpload), in_flight);
        assert_eq!(receiver.bodies().len(), 1);
        assert_eq!(batch_row(&db, &job_id).1.unwrap(), original);
        let conn = open_db(&db).unwrap();
        assert_eq!(
            frozen_position(&conn, &job_id),
            original_queue,
            "original payload and claim position changed after rollback"
        );
        let row: (i64, String, String, i64) = conn.query_row(
            "SELECT COUNT(*),accepted_revision_ids,suppressed_revision_ids,accepted_records FROM delivery_batches WHERE job_id=?",
            [&job_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).unwrap();
        assert_eq!(
            row,
            (1, accepted, suppressed, 1),
            "successor and receipt transfer rolled back"
        );
        let pending = status(&conn, &job_id).unwrap();
        assert_eq!(pending.pending_records, 3);
        assert_eq!(pending.acknowledged_records, 0);
        conn.execute_batch("DROP TRIGGER refuse_scrub_successor")
            .unwrap();
        retry_job(&conn, &job_id).unwrap();
        drop(conn);

        drain_with(&db, &receiver, ScrubEngine::new(RewriteUpload), in_flight);
        let sent = receiver.bodies();
        assert_eq!(sent.len(), 2);
        let old: HistoryExportBatch = serde_json::from_str(&sent[0]).unwrap();
        let new: HistoryExportBatch = serde_json::from_str(&sent[1]).unwrap();
        assert_ne!(old.batch_id, new.batch_id);
        assert!(!sent[1].contains("oops"));
        let complete = status(&open_db(&db).unwrap(), &job_id).unwrap();
        assert_eq!(complete.pending_records, 0);
        assert_eq!(complete.acknowledged_records, 2);
        assert_eq!(complete.suppressed_records, 1);
    }
}

/// The frozen source content and every part of its durable dispatch position.
fn frozen_position(conn: &Connection, job: &str) -> (String, i64, i64, Option<i64>) {
    conn.query_row(
        "SELECT payload,seq,journal_end,claim_seq FROM delivery_batches WHERE job_id=?",
        [job],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )
    .unwrap()
}

/// The marker only a receiver's mapping adds, never a record.
const PREPARED_MARKER: &str = "prepared-only-marker";

/// Refuses, at the local store, a body carrying [`PREPARED_MARKER`].
struct RefusePreparedMarker;
impl Scrub for RefusePreparedMarker {
    fn name(&self) -> &str {
        "refuse-prepared-marker"
    }
    fn scrub(&self, body: &[u8], context: &ScrubContext<'_>) -> Result<Vec<u8>, ScrubError> {
        let marked = std::str::from_utf8(body).unwrap().contains(PREPARED_MARKER);
        if marked && context.surface == ScrubSurface::LocalStore {
            return Err(ScrubError::new("marker"));
        }
        Ok(body.to_vec())
    }
}

#[test]
fn a_mapped_body_refused_before_it_is_stored_is_never_stored_or_sent() {
    for in_flight in IN_FLIGHT {
        let (_dir, db, job_id) = fixture();
        let receiver = Echo {
            marker: PREPARED_MARKER,
            ..Echo::default()
        };
        drain_with(
            &db,
            &receiver,
            ScrubEngine::new(RefusePreparedMarker),
            in_flight,
        );
        assert!(receiver.bodies().is_empty(), "in_flight={in_flight}: sent");
        let (state, prepared) = batch_row(&db, &job_id);
        assert_eq!(state, "blocked", "in_flight={in_flight}");
        assert!(
            !prepared.unwrap_or_default().contains(PREPARED_MARKER),
            "in_flight={in_flight}: the mapped body was stored"
        );
        assert_eq!(
            verdict(&db, &job_id).1.as_deref(),
            Some("invalid_payload"),
            "in_flight={in_flight}"
        );
    }
}

#[test]
fn a_refusing_engine_queues_nothing_sends_nothing_and_holds_its_place() {
    for in_flight in IN_FLIGHT {
        let (_dir, db, job_id) = fixture();
        let receiver = Echo::default();
        let (scrub, calls) = refuse();
        drain_with(&db, &receiver, scrub, in_flight);
        assert!(calls.load(std::sync::atomic::Ordering::SeqCst) > 0);
        assert!(receiver.bodies().is_empty(), "in_flight={in_flight}: sent");
        assert!(stored(&db, &job_id).is_empty(), "no body is stored");
        assert_eq!(
            verdict(&db, &job_id),
            ("blocked".into(), Some("invalid_payload".into()), 0, 0),
            "in_flight={in_flight}: blocked before the refused records, none suppressed"
        );
        // Retried with an engine that accepts them, both records go.
        retry_job(&open_db(&db).unwrap(), &job_id).unwrap();
        drain_with(&db, &receiver, ScrubEngine::default_engine(), in_flight);
        let sent = receiver.bodies().concat();
        assert_eq!(
            sent.matches("\"oops\"").count(),
            2,
            "in_flight={in_flight}: {sent}"
        );
    }
}

#[test]
fn a_drain_scrubs_with_the_default_engine_unless_told_otherwise() {
    assert_eq!(
        DrainOptions::new("scrub-test").scrub.name(),
        crate::history::scrub::DEFAULT_ENGINE
    );
}
