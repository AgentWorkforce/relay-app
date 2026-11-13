//! The collector's pass under the operator policy it reads from the cache:
//! a withheld policy sends nothing, and an oops window holds each session
//! until it has been quiet for the window.
use super::*;
use crate::history::operator_policy::FILE;
use std::sync::Mutex;

/// Accepts every batch and records the session, kind and revision of each
/// record it was sent.
#[derive(Default)]
struct Counting {
    sent: Mutex<Vec<(Option<String>, String, String)>>,
}

impl worker::Receiver for Counting {
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
        let records = batch.records.iter();
        let sent = records.map(|r| (r.session_id.clone(), r.kind.clone(), r.revision_id.clone()));
        self.sent.lock().unwrap().extend(sent);
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

/// Caches `entry` as the policy of the fixture's workspace.
fn cache(app_data: &Path, entry: serde_json::Value) {
    let saved = json!({ "workspaces": { "workspace": entry } });
    fs::write(app_data.join(FILE), serde_json::to_vec(&saved).unwrap()).unwrap();
}

/// The store records `session`'s latest activity at `at_ms`.
fn active(conn: &Connection, session: &str, at_ms: i64) {
    let updated = delivery::test_evidence(conn)
        .execute(
            "UPDATE sessions SET last_activity_ms=?2 WHERE source='claude' AND session_id=?1",
            rusqlite::params![session, at_ms],
        )
        .unwrap();
    if updated == 0 {
        delivery::test_evidence(conn)
            .execute(
                "INSERT INTO sessions(source,session_id,last_activity_ms) VALUES ('claude',?1,?2)",
                rusqlite::params![session, at_ms],
            )
            .unwrap();
    }
}

#[test]
fn the_collector_passes_the_policy_window_into_delivery() {
    let dir = tempfile::tempdir().unwrap();
    let app_data = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("history.db");
    let conn = delivery::open_db(&db_path).unwrap();
    for n in 0..3 {
        delivery::test_evidence(&conn).execute(
            "INSERT INTO session_events(source,session_id,event_uid,ts_ms,role,kind,text) VALUES ('claude','session',?1,42,'user','text','oops')",
            [format!("event-{n}")],
        )
        .unwrap();
    }
    active(&conn, "session", now());
    let job = delivery::create_job(&conn, &job_config(true), now()).unwrap();
    let config = crate::history::probe::live::tests::config(&job, false);
    let receiver = Counting::default();
    let sent = || receiver.sent.lock().unwrap().len();

    // Signed in, the policy not read yet: the pass sends nothing at all.
    cache(
        app_data.path(),
        json!({ "workspace_id": "workspace", "revision": null, "fetched_at": null,
                "settings": {}, "withheld": true }),
    );
    let pass = deliver_under_policy(&db_path, &config, &receiver, app_data.path()).unwrap();
    assert!(pass.is_none(), "a withheld policy makes no pass");
    assert_eq!(sent(), 0);

    // An oops window of a minute: the session was just active, so it waits.
    cache(
        app_data.path(),
        json!({ "workspace_id": "workspace", "revision": 1, "fetched_at": 1,
                "settings": { "upload_control": "operator", "oops_window_seconds": 60 } }),
    );
    deliver_under_policy(&db_path, &config, &receiver, app_data.path())
        .unwrap()
        .unwrap();
    assert_eq!(sent(), 0, "inside the window nothing is sent");
    assert!(delivery::held_for_session(&conn, "claude", "session").unwrap());

    // Quiet for longer than the window: every record leaves, once.
    active(&conn, "session", now() - 61_000);
    deliver_under_policy(&db_path, &config, &receiver, app_data.path())
        .unwrap()
        .unwrap();
    let released = receiver.sent.lock().unwrap().clone();
    let events = released
        .iter()
        .filter(|(_, kind, _)| kind == "session_event");
    assert_eq!(events.count(), 3, "{released:?}");
    assert!(released
        .iter()
        .all(|(session, _, _)| session.as_deref() == Some("session")));
    let revisions: std::collections::HashSet<_> = released.iter().map(|(_, _, r)| r).collect();
    assert_eq!(revisions.len(), released.len(), "nothing is sent twice");
    assert!(!delivery::held_for_session(&conn, "claude", "session").unwrap());
    deliver_under_policy(&db_path, &config, &receiver, app_data.path())
        .unwrap()
        .unwrap();
    assert_eq!(sent(), released.len());
}
