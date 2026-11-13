//! The delivery side of acquisition: which grants a batch presents, what is
//! held, and how a writer's refusal holds sessions while the rest of the
//! batch goes again under a new batch id.
use super::*;
use crate::history::acquisition::{self, hold_reason, Account, Grant, Secret, Store};
use crate::history::delivery::{self, worker, HistoryExportRecord};
use rusqlite::params;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

const ORG: &str = "org_test";
const WS: &str = "ws_test";

fn account() -> Account {
    Account {
        site: "https://agentrelay.test".into(),
        user: "user_test".into(),
        org: ORG.into(),
        workspace: WS.into(),
    }
}
fn g(n: u32) -> String {
    format!("rhg_{}", format!("{n:08x}").repeat(8))
}
fn grant(session: &str, n: u32, origin: &str) -> Grant {
    Grant {
        account: account(),
        source: "claude".into(),
        session_id: session.into(),
        grant_id: format!("00000000-0000-4000-8000-{n:012x}"),
        owner_epoch: n as u64,
        grant: Secret::new(g(n)),
        expires_at: None,
        origin_id: origin.into(),
    }
}
fn record(source: &str, session: Option<&str>, id: &str) -> HistoryExportRecord {
    HistoryExportRecord {
        schema_version: 1,
        origin_id: "origin".into(),
        record_id: id.into(),
        revision_id: format!("{id}-r1"),
        revision: 1,
        kind: "session_event".into(),
        source: source.into(),
        session_id: session.map(Into::into),
        operation: "upsert".into(),
        payload: json!({"text": id}),
    }
}
fn batch(records: Vec<HistoryExportRecord>) -> HistoryExportBatch {
    HistoryExportBatch {
        schema_version: 1,
        origin_id: "origin".into(),
        batch_id: "batch".into(),
        job_id: "job".into(),
        generation: 1,
        destination_id: "relayhistory".into(),
        instance_id: "probe".into(),
        account_id: account_id(ORG, Some(WS)),
        mapping_version: MAPPING_VERSION.into(),
        records,
    }
}
fn receiver(directory: &std::path::Path, enrolled: &[&str]) -> RelayHistoryReceiver {
    RelayHistoryReceiver {
        state: ReceiverState::acquiring(
            directory.join("acquisition"),
            Some(account()),
            Some(enrolled),
        ),
        ..Default::default()
    }
}
fn save(directory: &std::path::Path, grants: Vec<Grant>) {
    Store::at(directory.join("acquisition"))
        .update(|state| {
            grants.into_iter().for_each(|grant| state.put_grant(grant));
            Ok(())
        })
        .unwrap();
}
fn refusal(refused: Value) -> Value {
    json!({"error": {"code": "acquisition_refused", "message": "refused"}, "refused": refused})
}
fn rejected(status: u16, body: &Value, submitted: &str) -> worker::ReceiverFailure {
    let body = body.to_string();
    let response = ureq::Response::new(status, "Conflict", &body).unwrap();
    verdict(&http_error(
        ureq::Error::Status(status, response),
        submitted,
    ))
}

#[test]
fn a_source_that_is_not_enrolled_sends_exactly_the_body_it_always_did() {
    let directory = tempfile::tempdir().unwrap();
    let batch = batch(vec![
        record("claude", Some("a"), "1"),
        record("claude", None, "2"),
    ]);
    let legacy = prepare_within(batch.clone(), 100).unwrap().body;
    for receiver in [
        receiver(directory.path(), &["droid"]),
        receiver(directory.path(), &[]),
        // No signed-in account: nothing to look up, nothing presented.
        RelayHistoryReceiver {
            state: ReceiverState::acquiring(directory.path().join("acquisition"), None, None),
            ..Default::default()
        },
        RelayHistoryReceiver::default(),
    ] {
        let entries = receiver.acquisition(&batch).unwrap();
        assert!(entries.is_empty());
        assert_eq!(wire_body(batch.clone(), &entries, 100).unwrap(), legacy);
    }
    assert!(!legacy.contains("acquisition"));
    // Nothing was read or written for it.
    assert!(!directory.path().join("acquisition").exists());
}

#[test]
fn an_enrolled_batch_presents_its_sessions_grants_or_holds_the_sessions_without_one() {
    let directory = tempfile::tempdir().unwrap();
    let receiver = receiver(directory.path(), &["claude"]);
    save(
        directory.path(),
        vec![grant("b", 2, "origin"), grant("a", 1, "origin")],
    );
    let both = batch(vec![
        record("claude", Some("b"), "1"),
        record("claude", Some("a"), "2"),
        record("claude", Some("b"), "3"),
        record("codex", Some("z"), "4"),
    ]);
    let entries = receiver.acquisition(&both).unwrap();
    let body: Value =
        serde_json::from_str(&wire_body(both.clone(), &entries, 100).unwrap()).unwrap();
    assert_eq!(
        body["acquisition"],
        json!([
            {"source": "claude", "sessionId": "a", "grantId": "00000000-0000-4000-8000-000000000001", "ownerEpoch": "1", "grant": g(1)},
            {"source": "claude", "sessionId": "b", "grantId": "00000000-0000-4000-8000-000000000002", "ownerEpoch": "2", "grant": g(2)},
        ])
    );
    // A child batch carries only its own sessions' entries.
    let child = batch(vec![record("claude", Some("b"), "1")]);
    assert_eq!(entries_for(&child, &entries), vec![entries[1].clone()]);
    let child_body: Value =
        serde_json::from_str(&wire_body(child, &entries, 100).unwrap()).unwrap();
    assert_eq!(child_body["acquisition"].as_array().unwrap().len(), 1);
    // An entry for no session of the batch is left out; a malformed one is
    // refused locally rather than sent.
    let mut stray = entries.clone();
    stray[0].session_id = "zz".into();
    let stray_body: Value =
        serde_json::from_str(&wire_body(both.clone(), &stray, 100).unwrap()).unwrap();
    assert_eq!(stray_body["acquisition"].as_array().unwrap().len(), 1);
    let mut broken = entries.clone();
    broken[0].grant = "rhg_short".into();
    assert!(wire_body(both.clone(), &broken, 100).is_err());

    // Records without a session, or of a session without a grant, or a
    // grant from before a reset: held, never sent.
    let held = batch(vec![
        record("claude", Some("a"), "1"),
        record("claude", Some("c"), "2"),
        record("claude", None, "3"),
    ]);
    let failure = receiver.acquisition(&held).unwrap_err();
    assert_eq!(failure.failure, DeliveryFailure::ReceiverRejected);
    assert_eq!(
        failure.hold.unwrap(),
        vec![
            HeldSession {
                source: "claude".into(),
                session_id: "".into(),
                reason: acquisition::contract::SESSION_REQUIRED.into()
            },
            HeldSession {
                source: "claude".into(),
                session_id: "c".into(),
                reason: hold_reason::NO_GRANT.into()
            },
        ]
    );
    let mut moved = batch(vec![record("claude", Some("a"), "1")]);
    moved.origin_id = "after-reset".into();
    assert_eq!(
        receiver.acquisition(&moved).unwrap_err().hold.unwrap()[0].reason,
        hold_reason::ORIGIN_CHANGED
    );
    // Another account's grant is not this account's.
    let mut elsewhere = batch(vec![record("claude", Some("a"), "1")]);
    elsewhere.account_id = account_id(ORG, Some("ws_other"));
    assert_eq!(
        receiver.acquisition(&elsewhere).unwrap_err().hold.unwrap()[0].reason,
        hold_reason::NO_GRANT
    );
}

#[test]
fn a_writer_acquisition_refusal_holds_its_sessions_and_is_never_a_conflict() {
    let submitted = serde_json::to_string(&Request {
        protocol_version: 1,
        batch: batch(vec![record("claude", Some("a"), "1")]),
        acquisition: vec![],
    })
    .unwrap();
    let failure = rejected(
        409,
        &refusal(json!([
            {"source": "claude", "sessionId": "a", "code": "not_owner"},
            {"source": "claude", "sessionId": "", "code": "session_required"},
        ])),
        &submitted,
    );
    assert_eq!(failure.failure, DeliveryFailure::ReceiverRejected);
    assert!(failure.recovery.is_none());
    assert_eq!(
        failure.hold.unwrap(),
        vec![
            HeldSession {
                source: "claude".into(),
                session_id: "a".into(),
                reason: "not_owner".into()
            },
            HeldSession {
                source: "claude".into(),
                session_id: "".into(),
                reason: "session_required".into()
            },
        ]
    );
    // A malformed refusal holds nothing; it is the ordinary 409.
    let failure = rejected(409, &refusal(json!([])), &submitted);
    assert_eq!(failure.failure, DeliveryFailure::Transient);
    assert!(failure.hold.is_none());
    // Not a 409: no hold either.
    let failure = rejected(
        400,
        &refusal(json!([{"source": "claude", "sessionId": "a", "code": "not_owner"}])),
        &submitted,
    );
    assert!(failure.hold.is_none());
}

/// Prepares through the real receiver, which presents grants from its
/// store; sends answer from a script.
struct Writer<'a> {
    inner: &'a RelayHistoryReceiver,
    /// Sessions the writer refuses, each with its code, until emptied.
    refuse: Mutex<Vec<(&'static str, &'static str)>>,
    /// Sessions whose records the next send alone accepts, leaving the rest
    /// unknown (a partial acknowledgment), until emptied.
    accept_only: Mutex<Vec<&'static str>>,
    sent: Mutex<Vec<(String, Value)>>,
}
impl<'a> Writer<'a> {
    fn new(inner: &'a RelayHistoryReceiver) -> Self {
        Self {
            inner,
            refuse: Mutex::new(vec![]),
            accept_only: Mutex::new(vec![]),
            sent: Mutex::new(vec![]),
        }
    }
}
fn acknowledgment(batch: &HistoryExportBatch, accepted: Vec<String>) -> DeliveryAcknowledgment {
    DeliveryAcknowledgment {
        batch_id: batch.batch_id.clone(),
        accepted_revision_ids: accepted,
        unsupported_revision_ids: vec![],
        suppressed_revision_ids: vec![],
        rejected_revision_ids: vec![],
        isolation_frontier: vec![],
        acceptance_level: AcceptanceLevel::Durable,
    }
}
impl worker::Receiver for Writer<'_> {
    fn mapping_version(&self) -> &str {
        MAPPING_VERSION
    }
    fn supported_kinds(&self) -> &[&str] {
        &["session_event"]
    }
    fn supports_tombstones(&self) -> bool {
        true
    }
    fn prepare(
        &self,
        batch: &HistoryExportBatch,
        context: &worker::ReceiverContext<'_>,
    ) -> std::result::Result<worker::PreparedBody, worker::ReceiverFailure> {
        worker::Receiver::prepare(self.inner, batch, context)
    }
    fn send(
        &self,
        payload: &PreparedPayload,
        batch: &HistoryExportBatch,
        context: &worker::ReceiverContext<'_>,
    ) -> std::result::Result<DeliveryAcknowledgment, worker::ReceiverFailure> {
        // As the real receiver does: a batch whose every record already
        // has accepted progress is acknowledged without a request.
        if batch
            .records
            .iter()
            .all(|record| context.accepted_revision_ids.contains(&record.revision_id))
        {
            return Ok(acknowledgment(
                batch,
                context.accepted_revision_ids.to_vec(),
            ));
        }
        let request: Value = serde_json::from_str(&payload.body).unwrap();
        assert_eq!(request["batch"]["batch_id"], json!(batch.batch_id));
        self.sent
            .lock()
            .unwrap()
            .push((batch.batch_id.clone(), request));
        let refused: Vec<Value> = std::mem::take(&mut *self.refuse.lock().unwrap())
            .into_iter()
            .filter(|(session, _)| {
                batch
                    .records
                    .iter()
                    .any(|record| record.session_id.as_deref() == Some(session))
            })
            .map(|(session, code)| json!({"source": "claude", "sessionId": session, "code": code}))
            .collect();
        if !refused.is_empty() {
            return Err(rejected(409, &refusal(json!(refused)), &payload.body));
        }
        let only = std::mem::take(&mut *self.accept_only.lock().unwrap());
        let accepted = batch
            .records
            .iter()
            .filter(|record| {
                only.is_empty()
                    || only
                        .iter()
                        .any(|session| record.session_id.as_deref() == Some(session))
            })
            .map(|record| record.revision_id.clone())
            .collect();
        Ok(acknowledgment(batch, accepted))
    }
}

#[test]
fn held_sessions_wait_behind_the_cursor_while_the_rest_goes_again_under_a_new_batch_id() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.db");
    let conn = delivery::open_db(&path).unwrap();
    // b's record comes first in the feed, so a held b keeps the cursor back.
    for (session, ts) in [("b", 41), ("a", 42)] {
        delivery::test_evidence(&conn).execute(
            "INSERT INTO session_events(source,session_id,event_uid,ts_ms,role,kind,text) VALUES ('claude',?1,?1,?2,'user','text',?1)",
            params![session, ts],
        )
        .unwrap();
    }
    let config = delivery::DeliveryJobConfig {
        destination_id: "relayhistory".into(),
        instance_id: "probe".into(),
        account_id: account_id(ORG, Some(WS)),
        mapping_version: MAPPING_VERSION.into(),
        selection: delivery::ExportSelection {
            all_sources: true,
            kinds: vec!["session_event".into()],
            ..Default::default()
        },
        limits: delivery::DeliveryLimits {
            max_batch_records: 10,
            max_scan_records: 10,
            ..Default::default()
        },
    };
    let job = delivery::create_job(&conn, &config, 0).unwrap();
    let inner = receiver(directory.path(), &["claude"]);
    let writer = Writer::new(&inner);
    let receivers = worker::SingleReceiver::new("relayhistory", "probe", &writer);
    let mut options = worker::DrainOptions::new("worker");
    options.max_batches = 10;
    let drain = || {
        worker::drain(&path, &receivers, &options, &worker::system_clock, &|| {
            false
        })
        .unwrap()
    };

    // No grant yet: both sessions wait for ownership, nothing is sent and
    // nothing has failed.
    drain();
    assert!(writer.sent.lock().unwrap().is_empty());
    let waiting = delivery::status(&conn, &job.job_id).unwrap();
    assert_eq!(
        (waiting.held_records, waiting.pending_records),
        (2, 2),
        "{waiting:?}"
    );
    assert_eq!(waiting.failure, None);
    assert_eq!(waiting.state, "active");
    let start = waiting.acknowledged_cursor;
    assert_eq!(
        delivery::held_sessions(&conn).unwrap(),
        ["a", "b"]
            .map(|session| HeldSession {
                source: "claude".into(),
                session_id: session.into(),
                reason: hold_reason::NO_GRANT.into(),
            })
            .to_vec()
    );

    // Grants arrive for both; the writer refuses b.
    let origin = delivery::origin_id(&conn).unwrap().unwrap();
    save(
        directory.path(),
        vec![grant("a", 1, &origin), grant("b", 2, &origin)],
    );
    assert_eq!(delivery::release_held(&conn, "claude", "a").unwrap(), 1);
    assert_eq!(delivery::release_held(&conn, "claude", "b").unwrap(), 1);
    *writer.refuse.lock().unwrap() = vec![("b", "not_owner")];
    drain();
    {
        let sent = writer.sent.lock().unwrap();
        assert!(sent.len() >= 2, "{}", sent.len());
        let (first_id, first) = &sent[0];
        let (last_id, last) = sent.last().unwrap();
        assert_eq!(
            first["acquisition"].as_array().unwrap().len(),
            first["batch"]["records"].as_array().unwrap().len()
        );
        // The rest goes again under a new id, with only its own grant.
        assert_ne!(first_id, last_id);
        assert!(sent[1..].iter().all(|(id, _)| id != first_id));
        assert!(last["batch"]["records"]
            .as_array()
            .unwrap()
            .iter()
            .all(|record| record["session_id"] == "a"));
        assert_eq!(last["acquisition"].as_array().unwrap().len(), 1);
        assert_eq!(last["acquisition"][0]["sessionId"], "a");
        assert_eq!(last["acquisition"][0]["grant"], json!(g(1)));
    }
    let refused = delivery::status(&conn, &job.job_id).unwrap();
    assert_eq!(refused.failure, None, "never a conflict or transient");
    assert_eq!(refused.held_records, 1);
    assert_eq!(refused.pending_records, 1);
    assert_eq!(refused.acknowledged_records, 1);
    assert_eq!(refused.acknowledged_cursor, start, "cursor stays behind b");
    assert_eq!(
        delivery::held_sessions(&conn).unwrap(),
        vec![HeldSession {
            source: "claude".into(),
            session_id: "b".into(),
            reason: "not_owner".into(),
        }]
    );

    // Nothing moves while b is held.
    let before = writer.sent.lock().unwrap().len();
    drain();
    assert_eq!(writer.sent.lock().unwrap().len(), before);

    // b's authority changed (a new grant): released, it goes with it.
    save(directory.path(), vec![grant("b", 3, &origin)]);
    assert_eq!(delivery::release_held(&conn, "claude", "b").unwrap(), 1);
    drain();
    let (_, last) = writer.sent.lock().unwrap().last().cloned().unwrap();
    assert_eq!(last["acquisition"].as_array().unwrap().len(), 1);
    assert_eq!(last["acquisition"][0]["grant"], json!(g(3)));
    let done = delivery::status(&conn, &job.job_id).unwrap();
    assert_eq!(
        (
            done.held_records,
            done.pending_records,
            done.acknowledged_records
        ),
        (0, 0, 2)
    );
    assert!(done.acknowledged_cursor > start);
    assert!(delivery::held_sessions(&conn).unwrap().is_empty());
    // Excluding and including a session leaves the installation and grants
    // exactly as they were.
    let saved = std::fs::read(directory.path().join("acquisition/state.json")).unwrap();
    let identity = delivery::SessionIdentity {
        source: "claude".into(),
        session_id: "a".into(),
    };
    delivery::set_session_excluded(&conn, &identity, true).unwrap();
    // Lifting an exclusion takes a new generation: cancel, re-include.
    delivery::cancel_job(&conn, &job.job_id).unwrap();
    delivery::set_session_excluded(&conn, &identity, false).unwrap();
    assert_eq!(
        std::fs::read(directory.path().join("acquisition/state.json")).unwrap(),
        saved
    );
}

#[test]
fn a_grant_saved_for_another_user_or_site_is_never_presented() {
    let directory = tempfile::tempdir().unwrap();
    let receiver = receiver(directory.path(), &["claude"]);
    let mut other_user = grant("a", 1, "origin");
    other_user.account.user = "user_other".into();
    let mut other_site = grant("b", 2, "origin");
    other_site.account.site = "https://elsewhere.test".into();
    save(directory.path(), vec![other_user, other_site]);
    let failure = receiver
        .acquisition(&batch(vec![
            record("claude", Some("a"), "1"),
            record("claude", Some("b"), "2"),
        ]))
        .unwrap_err();
    assert_eq!(
        failure.hold.unwrap(),
        ["a", "b"]
            .map(|session| HeldSession {
                source: "claude".into(),
                session_id: session.into(),
                reason: hold_reason::NO_GRANT.into(),
            })
            .to_vec()
    );
}

#[test]
fn without_a_published_list_the_last_enrolled_sources_of_the_exact_account_stand() {
    let directory = tempfile::tempdir().unwrap();
    save(directory.path(), vec![grant("a", 1, "origin")]);
    Store::at(directory.path().join("acquisition"))
        .update(|state| {
            state.put_enrolled(&account(), &["claude".to_owned()].into());
            Ok(())
        })
        .unwrap();
    let one = batch(vec![record("claude", Some("a"), "1")]);
    let unpublished = RelayHistoryReceiver {
        state: ReceiverState::acquiring(
            directory.path().join("acquisition"),
            Some(account()),
            None,
        ),
        ..Default::default()
    };
    let entries = unpublished.acquisition(&one).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].grant, g(1));
    // Another user has no list kept: not enrolled for it, nothing presented.
    let mut other = account();
    other.user = "user_other".into();
    let elsewhere = RelayHistoryReceiver {
        state: ReceiverState::acquiring(directory.path().join("acquisition"), Some(other), None),
        ..Default::default()
    };
    assert_eq!(elsewhere.acquisition(&one).unwrap(), vec![]);
}

#[test]
fn a_session_accepted_before_a_refusal_is_never_held_or_resent() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.db");
    let conn = delivery::open_db(&path).unwrap();
    for (session, ts) in [("a", 41), ("b", 42)] {
        delivery::test_evidence(&conn).execute(
            "INSERT INTO session_events(source,session_id,event_uid,ts_ms,role,kind,text) VALUES ('claude',?1,?1,?2,'user','text',?1)",
            params![session, ts],
        )
        .unwrap();
    }
    let config = delivery::DeliveryJobConfig {
        destination_id: "relayhistory".into(),
        instance_id: "probe".into(),
        account_id: account_id(ORG, Some(WS)),
        mapping_version: MAPPING_VERSION.into(),
        selection: delivery::ExportSelection {
            all_sources: true,
            kinds: vec!["session_event".into()],
            ..Default::default()
        },
        limits: delivery::DeliveryLimits {
            max_batch_records: 10,
            max_scan_records: 10,
            ..Default::default()
        },
    };
    let job = delivery::create_job(&conn, &config, 0).unwrap();
    let origin = delivery::origin_id(&conn).unwrap().unwrap();
    save(
        directory.path(),
        vec![grant("a", 1, &origin), grant("b", 2, &origin)],
    );
    let inner = receiver(directory.path(), &["claude"]);
    let writer = Writer::new(&inner);
    // The first send accepts a alone; the retry of the same batch is
    // refused for both, a because its owner is now draining.
    *writer.accept_only.lock().unwrap() = vec!["a"];
    let receivers = worker::SingleReceiver::new("relayhistory", "probe", &writer);
    let mut options = worker::DrainOptions::new("worker");
    options.max_batches = 10;
    // Each drain is an hour later, past any backoff.
    let now = AtomicI64::new(worker::system_clock());
    let clock = || now.load(Ordering::SeqCst);
    let drain = || {
        now.fetch_add(3_600_000, Ordering::SeqCst);
        worker::drain(&path, &receivers, &options, &clock, &|| false).unwrap()
    };

    drain();
    assert_eq!(writer.sent.lock().unwrap().len(), 1);
    let partial = delivery::status(&conn, &job.job_id).unwrap();
    assert_eq!(partial.held_records, 0, "{partial:?}");

    *writer.refuse.lock().unwrap() = vec![("a", "owner_draining"), ("b", "not_owner")];
    drain();
    drain();
    assert_eq!(
        writer.sent.lock().unwrap().len(),
        2,
        "a, already accepted, is never sent again"
    );
    assert_eq!(
        delivery::held_sessions(&conn).unwrap(),
        vec![HeldSession {
            source: "claude".into(),
            session_id: "b".into(),
            reason: "not_owner".into(),
        }]
    );
    let after = delivery::status(&conn, &job.job_id).unwrap();
    assert_eq!(after.failure, None, "{after:?}");
    assert_eq!(after.held_records, 1);
    assert_eq!(after.acknowledged_records, 1);
}
