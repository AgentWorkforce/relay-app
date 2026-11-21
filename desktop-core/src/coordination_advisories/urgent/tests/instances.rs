//! Advisory instances: only `advisories/v3` lists them and only they are
//! urgent; each is keyed, injected and acked as itself; receipts saved before
//! instances hold back or label a row of their id; a v2 ack's 404 settles it.

use super::*;
use crate::coordination_advisories::delivery::MAY_REPEAT;
use crate::coordination_advisories::poller::NO_MATCH_STATUS;
use observe::age_observation;

/// The incarnation a promotion gives the same advisory id.
const PROMOTED: &str = "9a8b7c6d-5e4f-4a3b-8c2d-1e0f9a8b7c6d";
const RECEIPTS: &str = "coordination-advisory-receipts.json";

fn listing(contract: Option<Value>, rows: Vec<Value>) -> Value {
    let mut body = json!({"advisories": rows, "nextCursor": null});
    if let Some(contract) = contract {
        body["contract"] = contract;
    }
    body
}

/// A priority-one, eligible row as the read API lists it.
fn wire_row(id: &str, incarnation: Value) -> Value {
    json!({
        "advisoryId": id, "urgency": "high", "priority": 1, "notifyEligible": true,
        "message": "Review overlap.", "peer": {"source": "codex", "sessionId": "peer"},
        "incarnation": incarnation,
    })
}

/// `id`'s row as a legacy listing gives it: no incarnation.
fn legacy(id: &str) -> Advisory {
    Advisory {
        incarnation: None,
        ..p1(id)
    }
}

/// `id`'s instance row, low and priority three: a turn-end row only.
fn low(id: &str) -> Advisory {
    Advisory {
        urgency: "low",
        priority: Some(3),
        ..p1(id)
    }
}

fn acks(api: &Api) -> Vec<(String, Option<String>)> {
    let ids = api.acked.lock().unwrap().clone();
    ids.into_iter()
        .zip(api.incarnations.lock().unwrap().clone())
        .collect()
}

fn scope_json() -> Value {
    json!({"site": "https://test.example", "workspace_id": "test-workspace", "user_id": "test-human"})
}

/// Advisories over `dir`, with both switches on.
fn opened(dir: &Path, api: &Arc<Api>) -> Arc<Advisories> {
    let advisories = Advisories::new(dir, Arc::new(Tokens), api.clone());
    advisories.set_enabled(true).unwrap();
    advisories.set_notifications_enabled(true).unwrap();
    advisories
}

#[test]
fn exactly_advisories_v3_lists_instance_rows() {
    let parsed = parse(&listing(
        Some(json!(CONTRACT_V3)),
        vec![wire_row("p1", json!(INCARNATION))],
    ))
    .unwrap();
    assert_eq!(parsed[0].incarnation.as_deref(), Some(INCARNATION));
    assert!(parsed[0].urgent());
}

#[test]
fn any_other_contract_value_or_none_is_legacy_and_never_urgent() {
    for contract in [
        None,
        Some(Value::Null),
        Some(json!(CONTRACT)),
        Some(json!("coordination-advisories/2")),
        Some(json!("advisories/v2")),
        Some(json!("advisories/v4")),
        Some(json!("Advisories/v3")),
        Some(json!("advisories/v3 ")),
        Some(json!("")),
    ] {
        let rows = vec![wire_row("p1", json!(INCARNATION))];
        let parsed = parse(&listing(contract.clone(), rows)).unwrap();
        // Still a turn-end row, keyed and acked by its id.
        assert_eq!(parsed.len(), 1, "{contract:?}");
        assert!(parsed[0].incarnation.is_none(), "{contract:?}");
        assert!(!parsed[0].urgent(), "{contract:?}");
    }
    // A contract that is not a string at all is no listing.
    assert!(parse(&listing(Some(json!(3)), vec![])).is_none());
}

#[test]
fn under_v3_a_null_or_absent_incarnation_is_legacy_and_a_malformed_one_drops_the_row() {
    let mut absent = wire_row("absent", Value::Null);
    absent.as_object_mut().unwrap().remove("incarnation");
    let rows = vec![
        wire_row("null", Value::Null),
        absent,
        wire_row("bad", json!("not-a-uuid")),
        wire_row("number", json!(7)),
        wire_row("ok", json!(INCARNATION)),
    ];
    let parsed = parse(&listing(Some(json!(CONTRACT_V3)), rows)).unwrap();
    let ids: Vec<(&str, bool)> = parsed.iter().map(|a| (a.id.as_str(), a.urgent())).collect();
    assert_eq!(ids, vec![("null", false), ("absent", false), ("ok", true)]);
}

#[tokio::test]
async fn against_a_legacy_listing_nothing_is_urgent_and_its_rows_reach_the_turn_end_acked_v1() {
    let (_dir, api, advisories) = setup();
    let sink = Sink::default();
    let t = target(A);
    let now = Instant::now();
    api.rows
        .lock()
        .unwrap()
        .insert(t.clone(), vec![legacy("p1")]);
    advisories.poll(&[t.clone()], now).await;
    advisories.deliver_urgent(&sink, now).await;
    assert!(sink.calls.lock().unwrap().is_empty());
    assert!(sink.notices.lock().unwrap().is_empty());
    let note = advisories.take_at(&t, now).unwrap();
    assert_eq!(note.ids, vec!["p1"]);
    assert!(!note.text.contains(MAY_REPEAT));
    advisories.mark_shown(&t, &note.ids);
    advisories.send_shown(now).await;
    assert_eq!(acks(&api), vec![("p1".into(), None)]);
}

#[tokio::test]
async fn an_instance_is_injected_under_its_key_and_only_ever_acked_v2_with_its_incarnation() {
    let (_dir, api, advisories) = setup();
    let sink = Sink::default();
    let t = target(A);
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    *api.answer.lock().unwrap() = Some(Ack::Failed);
    advisories.poll(&[t.clone()], now).await;
    advisories.deliver_urgent(&sink, now).await;
    assert_eq!(
        *sink.calls.lock().unwrap(),
        vec![(t.clone(), key(&t, "p1"))]
    );
    assert_eq!(sink.notices.lock().unwrap().len(), 1);
    // Failed acks are retried, each as the same v2 ack: never a v1 one.
    let mut at = now;
    for _ in 0..3 {
        advisories.send_shown(at).await;
        at += Duration::from_secs(60 * 60);
    }
    *api.answer.lock().unwrap() = None;
    advisories.send_shown(at).await;
    let sent = acks(&api);
    assert_eq!(sent.len(), 4);
    assert!(sent
        .iter()
        .all(|ack| *ack == ("p1".to_string(), Some(INCARNATION.to_string()))));
    // Listed again: never injected or handed out again.
    advisories.poll(&[t.clone()], at).await;
    advisories.deliver_urgent(&sink, at).await;
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
    assert!(advisories.take_at(&t, at).is_none());
}

#[tokio::test]
async fn a_v2_ack_404_is_a_settled_no_match_never_retried_shown_again_or_delivered() {
    let (dir, api, advisories) = setup();
    let t = target(A);
    let now = Instant::now();
    api.stale.lock().unwrap().insert(INCARNATION.into());
    api.rows.lock().unwrap().insert(t.clone(), vec![low("p1")]);
    advisories.poll(&[t.clone()], now).await;
    let note = advisories.take_at(&t, now).unwrap();
    assert_eq!(note.ids, keys(&t, &["p1"]));
    advisories.mark_shown(&t, &note.ids);
    advisories.send_shown(now).await;
    assert_eq!(acks(&api), vec![("p1".into(), Some(INCARNATION.into()))]);
    {
        let state = advisories.state();
        let taken = &state.taken[&t][&key(&t, "p1")];
        assert!(taken.acked && taken.no_match);
    }
    assert_eq!(advisories.report().notification_status, NO_MATCH_STATUS);
    // Not retried, and not handed out again while it is still listed.
    advisories.send_shown(now + ACK_FORBIDDEN_RETRY).await;
    advisories.poll(&[t.clone()], now).await;
    assert!(advisories.take_at(&t, now).is_none());
    // Nor after a restart: the settled receipt is saved as such, and no
    // urgent receipt ever says delivered.
    let restarted = Advisories::new(dir.path(), Arc::new(Tokens), api.clone());
    restarted.poll(&[t.clone()], now).await;
    assert!(restarted.take_at(&t, now).is_none());
    restarted.send_shown(now).await;
    assert_eq!(acks(&api).len(), 1);
    let journal = restarted.urgent.lock().unwrap();
    let principal = Tokens.principal().unwrap();
    assert!(journal
        .agent_receipt(&principal, &t, &key(&t, "p1"))
        .is_none());
}

#[tokio::test]
async fn a_held_legacy_receipt_keeps_an_instance_of_its_id_back_and_is_observed_under_its_old_key()
{
    let (_dir, api, advisories) = setup();
    let sink = Sink::default();
    let t = target(A);
    let now = Instant::now();
    let principal = Tokens.principal().unwrap();
    // Saved before instances: the provider holds the note under its id.
    advisories.urgent.lock().unwrap().set_agent(
        &principal,
        &t,
        "p1",
        Attempt::Accepted,
        unix_now(),
    );
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(&[t.clone()], now).await;
    advisories.deliver_urgent(&sink, now).await;
    assert!(sink.calls.lock().unwrap().is_empty());
    assert_eq!(*sink.observed.lock().unwrap(), vec!["p1"]);
    // Neither the human nor the turn end gets it while it is held.
    assert!(sink.notices.lock().unwrap().is_empty());
    assert!(advisories.take_at(&t, now).is_none());
    // Delivered under the old key: that may not have been this instance, so
    // it is not acked v2 from there, and the urgent path never injects it.
    *sink.observation.lock().unwrap() = Some(Observation::Delivered);
    age_observation(&advisories);
    advisories.deliver_urgent(&sink, now).await;
    advisories.send_shown(now).await;
    assert!(api.acked.lock().unwrap().is_empty());
    assert!(sink.calls.lock().unwrap().is_empty());
    {
        let journal = advisories.urgent.lock().unwrap();
        let legacy = journal.agent_receipt(&principal, &t, "p1").map(|r| r.0);
        assert_eq!(legacy, Some(Attempt::Delivered));
        assert!(journal
            .agent_receipt(&principal, &t, &key(&t, "p1"))
            .is_none());
    }
    // The turn end hands the instance out once, labelled, and acks it v2.
    let note = advisories.take_at(&t, now).unwrap();
    assert_eq!(note.ids, keys(&t, &["p1"]));
    assert!(note.text.contains(MAY_REPEAT));
    advisories.mark_shown(&t, &note.ids);
    advisories.send_shown(now).await;
    assert_eq!(acks(&api), vec![("p1".into(), Some(INCARNATION.into()))]);
    advisories.poll(&[t.clone()], now).await;
    advisories.deliver_urgent(&sink, now).await;
    assert!(sink.calls.lock().unwrap().is_empty());
    assert!(advisories.take_at(&t, now).is_none());
}

#[tokio::test]
async fn a_settled_legacy_receipt_hands_an_instance_of_its_id_out_once_labelled_and_acks_it_v2() {
    for attempt in [
        Attempt::Delivered,
        Attempt::Released,
        Attempt::Refused,
        Attempt::Retry,
    ] {
        let (_dir, api, advisories) = setup();
        let sink = Sink::default();
        let t = target(A);
        let now = Instant::now();
        let principal = Tokens.principal().unwrap();
        advisories.urgent.lock().unwrap().set_agent(
            &principal,
            &t,
            "p1",
            attempt.clone(),
            unix_now() - URGENT_RETRY_AFTER - 60,
        );
        api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
        advisories.poll(&[t.clone()], now).await;
        advisories.deliver_urgent(&sink, now).await;
        assert!(sink.calls.lock().unwrap().is_empty(), "{attempt:?}");
        advisories.send_shown(now).await;
        assert!(api.acked.lock().unwrap().is_empty(), "{attempt:?}");
        let note = advisories.take_at(&t, now).unwrap();
        assert_eq!(note.ids, keys(&t, &["p1"]), "{attempt:?}");
        assert!(note.text.contains(MAY_REPEAT), "{attempt:?}");
        advisories.mark_shown(&t, &note.ids);
        advisories.send_shown(now).await;
        assert_eq!(acks(&api), vec![("p1".into(), Some(INCARNATION.into()))]);
        advisories.poll(&[t.clone()], now).await;
        advisories.deliver_urgent(&sink, now).await;
        assert!(sink.calls.lock().unwrap().is_empty(), "{attempt:?}");
        assert!(advisories.take_at(&t, now).is_none(), "{attempt:?}");
    }
}

#[tokio::test]
async fn a_legacy_turn_end_receipt_labels_an_instance_of_its_id_and_each_is_acked_as_itself() {
    let dir = tempfile::tempdir().unwrap();
    let t = target(A);
    // A version 1 receipt: shown before instances, its v1 ack pending.
    let saved = json!({"version": 1, "receipts": [{
        "scope": scope_json(), "target": t, "advisory_id": "p1", "shown_at": unix_now() - 60,
    }]});
    std::fs::write(dir.path().join(RECEIPTS), saved.to_string()).unwrap();
    let api = Arc::new(Api::default());
    let advisories = opened(dir.path(), &api);
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![low("p1")]);
    advisories.poll(&[t.clone()], now).await;
    // The legacy receipt's own ack is a v1 ack: it names no incarnation.
    assert_eq!(acks(&api), vec![("p1".into(), None)]);
    // A later poll keeps the legacy hand-out while the instance is listed.
    advisories.poll(&[t.clone()], now).await;
    let note = advisories.take_at(&t, now).unwrap();
    assert_eq!(note.ids, keys(&t, &["p1"]));
    assert!(note.text.contains(MAY_REPEAT));
    advisories.mark_shown(&t, &note.ids);
    advisories.send_shown(now).await;
    assert_eq!(
        acks(&api),
        vec![("p1".into(), None), ("p1".into(), Some(INCARNATION.into()))]
    );
    // Saved as version 2.
    let on_disk: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join(RECEIPTS)).unwrap()).unwrap();
    assert_eq!(on_disk["version"], 2);
}

#[tokio::test]
async fn a_promoted_row_is_a_new_instance_eligible_afresh_and_a_stale_ack_settles_only_the_old_one()
{
    let (_dir, api, advisories) = setup();
    let sink = Sink::default();
    let t = target(A);
    let now = Instant::now();
    let principal = Tokens.principal().unwrap();
    // The low instance is shown at a turn end; its first ack fails.
    api.rows.lock().unwrap().insert(t.clone(), vec![low("p1")]);
    *api.answer.lock().unwrap() = Some(Ack::Failed);
    advisories.poll(&[t.clone()], now).await;
    advisories.deliver_urgent(&sink, now).await;
    assert!(sink.calls.lock().unwrap().is_empty());
    let note = advisories.take_at(&t, now).unwrap();
    advisories.mark_shown(&t, &note.ids);
    advisories.send_shown(now).await;
    // A journal entry held for the low instance suppresses nothing else.
    advisories.urgent.lock().unwrap().set_agent(
        &principal,
        &t,
        &key(&t, "p1"),
        Attempt::Accepted,
        unix_now(),
    );
    // Promoted: the same id, a new incarnation, now high.
    let high = Advisory {
        incarnation: Some(PROMOTED.into()),
        ..p1("p1")
    };
    api.rows.lock().unwrap().insert(t.clone(), vec![high]);
    *api.answer.lock().unwrap() = None;
    api.stale.lock().unwrap().insert(INCARNATION.into());
    advisories.poll(&[t.clone()], now).await;
    advisories.deliver_urgent(&sink, now).await;
    let promoted = key_of(&t, "p1", PROMOTED);
    assert_eq!(
        *sink.calls.lock().unwrap(),
        vec![(t.clone(), promoted.clone())]
    );
    assert_eq!(sink.notices.lock().unwrap().len(), 1);
    // Each instance is acked with its own incarnation: the stale one finds
    // no match and settles only the low instance.
    advisories.send_shown(now + ACK_RETRY).await;
    let mut sent = acks(&api);
    assert_eq!(sent.remove(0), ("p1".into(), Some(INCARNATION.into())));
    sent.sort();
    assert_eq!(
        sent,
        vec![
            ("p1".into(), Some(INCARNATION.into())),
            ("p1".into(), Some(PROMOTED.into()))
        ]
    );
    {
        let state = advisories.state();
        assert!(state.taken[&t][&key(&t, "p1")].no_match);
        let high = &state.taken[&t][&promoted];
        assert!(high.acked && !high.no_match);
    }
    advisories.poll(&[t.clone()], now + ACK_RETRY).await;
    advisories.deliver_urgent(&sink, now).await;
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
    assert!(advisories.take_at(&t, now).is_none());
}

#[tokio::test]
async fn a_version_1_journal_is_read_as_legacy_keyed_and_saved_back_as_version_2() {
    let dir = tempfile::tempdir().unwrap();
    let t = target(A);
    let saved = json!({"version": 1,
        "agents": [{"scope": scope_json(), "target": t, "id": "p1", "at": unix_now(), "attempt": "Accepted"}],
        "humans": [{"scope": scope_json(), "ids": ["p1"], "at": unix_now()}],
    });
    std::fs::write(dir.path().join(FILE), saved.to_string()).unwrap();
    let api = Arc::new(Api::default());
    let advisories = opened(dir.path(), &api);
    let sink = Sink::default();
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(&[t.clone()], now).await;
    advisories.deliver_urgent(&sink, now).await;
    assert!(!advisories.urgent.lock().unwrap().blocked());
    // Held under its old key: observed, not injected; already notified.
    assert!(sink.calls.lock().unwrap().is_empty());
    assert_eq!(*sink.observed.lock().unwrap(), vec!["p1"]);
    assert!(sink.notices.lock().unwrap().is_empty());
    assert!(advisories.save_urgent().await);
    let on_disk: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join(FILE)).unwrap()).unwrap();
    assert_eq!(on_disk["version"], 2);
    assert_eq!(on_disk["agents"][0]["id"], "p1");
    assert_eq!(on_disk["agents"][0]["attempt"], "Accepted");
}

#[tokio::test]
async fn a_newer_receipt_file_is_left_alone_and_pauses_acknowledgements_with_a_status() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(RECEIPTS);
    let newer = br#"{"version":3,"receipts":[],"later":true}"#;
    std::fs::write(&path, newer).unwrap();
    let api = Arc::new(Api::default());
    let advisories = opened(dir.path(), &api);
    let t = target(A);
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![low("p1")]);
    advisories.poll(&[t.clone()], now).await;
    let note = advisories.take_at(&t, now).unwrap();
    advisories.mark_shown(&t, &note.ids);
    advisories.send_shown(now).await;
    assert!(api.acked.lock().unwrap().is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), newer);
    let status = advisories.report().notification_status;
    assert!(status.contains("newer version"), "{status}");
    assert!(status.contains(&path.display().to_string()), "{status}");
}

#[tokio::test]
async fn while_the_journal_is_blocked_a_legacy_priority_one_row_is_kept_back_too() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FILE), b"broken").unwrap();
    let api = Arc::new(Api::default());
    let advisories = opened(dir.path(), &api);
    let sink = Sink::default();
    let t = target(A);
    let now = Instant::now();
    // An earlier build may have handed the legacy priority-one row over, and
    // the unreadable journal may say so; a legacy low row is not at stake.
    let low_legacy = Advisory {
        incarnation: None,
        ..low("plain")
    };
    api.rows
        .lock()
        .unwrap()
        .insert(t.clone(), vec![legacy("p1"), low_legacy]);
    advisories.poll(&[t.clone()], now).await;
    advisories.deliver_urgent(&sink, now).await;
    assert!(sink.calls.lock().unwrap().is_empty());
    let note = advisories.take_at(&t, now).unwrap();
    assert_eq!(note.ids, vec!["plain"]);
    // Where no local notice reaches, it is handed out, labelled.
    struct Unreached;
    impl Delivery for Unreached {
        fn inject<'a>(
            &'a self,
            _: &'a Target,
            _: &'a str,
            _: &'a str,
        ) -> BoxFuture<'a, InjectOutcome> {
            panic!("never injected");
        }
        fn notifications_available(&self) -> bool {
            false
        }
        fn notify(&self, _: &str, _: &str) {}
        fn reaches(&self, _: &Target) -> bool {
            false
        }
    }
    advisories.deliver_urgent(&Unreached, now).await;
    let note = advisories.take_at(&t, now).unwrap();
    assert_eq!(note.ids, vec!["p1"]);
    assert!(note.text.contains(MAY_REPEAT));
}
