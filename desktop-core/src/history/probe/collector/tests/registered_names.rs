//! The collector's delivery pass applies the published registered agent
//! names to the store before the drain fills and scrubs a batch.
use super::*;
use crate::history::registered_names::{self, Names};
use std::cell::Cell;

/// Accepts every batch, and checks at each `prepare` (which runs after the
/// batch was filled and scrubbed) that the names were already applied.
struct NamesFirst<'a> {
    directory: &'a Path,
    expected: u64,
    prepared: Cell<usize>,
}

impl worker::Receiver for NamesFirst<'_> {
    fn mapping_version(&self) -> &str {
        destination::MAPPING_VERSION
    }
    fn supported_kinds(&self) -> &[&str] {
        delivery::SUPPORTED_KINDS
    }
    fn supports_tombstones(&self) -> bool {
        true
    }
    fn prepare(
        &self,
        _: &delivery::HistoryExportBatch,
        _: &worker::ReceiverContext<'_>,
    ) -> std::result::Result<worker::PreparedBody, worker::ReceiverFailure> {
        assert_eq!(
            registered_names::applied(self.directory),
            Some(self.expected),
            "the names reach the store before a batch is filled"
        );
        self.prepared.set(self.prepared.get() + 1);
        Ok(worker::PreparedBody {
            content_type: "application/json".into(),
            body: "{}".into(),
        })
    }
    fn send(
        &self,
        _: &delivery::PreparedPayload,
        batch: &delivery::HistoryExportBatch,
        _: &worker::ReceiverContext<'_>,
    ) -> std::result::Result<delivery::DeliveryAcknowledgment, worker::ReceiverFailure> {
        Ok(delivery::DeliveryAcknowledgment {
            batch_id: batch.batch_id.clone(),
            accepted_revision_ids: batch
                .records
                .iter()
                .map(|record| record.revision_id.clone())
                .collect(),
            unsupported_revision_ids: vec![],
            suppressed_revision_ids: vec![],
            rejected_revision_ids: vec![],
            isolation_frontier: vec![],
            acceptance_level: delivery::AcceptanceLevel::Durable,
        })
    }
}

#[test]
fn a_delivery_pass_applies_the_published_registered_names_before_it_fills_a_batch() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("history.db");
    let conn = delivery::open_db(&db_path).unwrap();
    for n in 0..3 {
        delivery::test_evidence(&conn).execute(
            "INSERT INTO session_events(source,session_id,event_uid,ts_ms,role,kind,text) VALUES ('claude','session',?1,42,'user','text','hello')",
            [format!("event-{n}")],
        )
        .unwrap();
    }
    // The store holds the named session, so the name is recorded and the
    // pass does not offer it again.
    delivery::test_evidence(&conn)
        .execute(
            "INSERT INTO sessions(source,session_id) VALUES ('claude','session')",
            [],
        )
        .unwrap();
    let job = delivery::create_job(&conn, &job_config(true), now()).unwrap();
    let config = crate::history::probe::live::tests::config(&job, false);
    let names: Names = [("claude:session".to_string(), "reviewer".to_string())]
        .into_iter()
        .collect();
    registered_names::write_published(dir.path(), &names).unwrap();
    assert_eq!(registered_names::applied(dir.path()), None);

    let receiver = NamesFirst {
        directory: dir.path(),
        expected: registered_names::digest(&registered_names::resolve(&names)),
        prepared: Cell::new(0),
    };
    deliver_with_receiver(&db_path, &config, &receiver, &|| false).unwrap();
    assert!(receiver.prepared.get() > 0, "the pass filled a batch");
}
