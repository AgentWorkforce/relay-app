use super::*;

struct Tokens;
impl TokenSource for Tokens {
    fn principal(&self) -> Option<Principal> {
        Some(Principal {
            site: "https://test.example".into(),
            workspace_id: "test-workspace".into(),
            user_id: Some("test-human".into()),
            generation: 1,
        })
    }
    fn token(&self) -> BoxFuture<'_, Option<ReadToken>> {
        let principal = self.principal().unwrap();
        Box::pin(async move {
            Some(ReadToken {
                origin: "https://test.example".into(),
                bearer: Secret::new("synthetic-read-token"),
                workspace_id: principal.workspace_id.clone(),
                principal,
            })
        })
    }
    fn refused(&self, _: &ReadToken) {}
}

#[derive(Default)]
struct Api {
    rows: Mutex<HashMap<Target, Vec<Advisory>>>,
    acked: Mutex<Vec<String>>,
}
impl AdvisoryApi for Api {
    fn list<'a>(&'a self, _: &'a ReadToken, target: &'a Target) -> BoxFuture<'a, Listing> {
        let rows = self
            .rows
            .lock()
            .unwrap()
            .get(target)
            .cloned()
            .unwrap_or_default();
        Box::pin(async move { Listing::Advisories(rows) })
    }
    fn ack<'a>(&'a self, _: &'a ReadToken, _: &'a Target, id: &'a str) -> BoxFuture<'a, Ack> {
        self.acked.lock().unwrap().push(id.into());
        Box::pin(async { Ack::Done })
    }
}

#[derive(Default)]
struct Sink {
    calls: Mutex<Vec<(Target, String)>>,
    notices: Mutex<Vec<(String, String)>>,
    outcome: Mutex<Option<InjectOutcome>>,
    unavailable: AtomicBool,
}
impl Delivery for Sink {
    fn inject<'a>(
        &'a self,
        target: &'a Target,
        id: &'a str,
        _: &'a str,
    ) -> BoxFuture<'a, InjectOutcome> {
        self.calls.lock().unwrap().push((target.clone(), id.into()));
        let outcome = self
            .outcome
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(InjectOutcome::Delivered);
        Box::pin(async move { outcome })
    }
    fn notifications_available(&self) -> bool {
        !self.unavailable.load(Ordering::SeqCst)
    }
    fn notify(&self, title: &str, body: &str) {
        self.notices
            .lock()
            .unwrap()
            .push((title.into(), body.into()));
    }
}

fn target(id: &str) -> Target {
    Target::new(SessionKind::Codex, id)
}
fn p1(id: &str) -> Advisory {
    Advisory {
        id: id.into(),
        urgency: "high",
        priority: Some(1),
        dm_delivered: false,
        notify_eligible: true,
        message: format!("Review overlap {id}."),
        peer_source: "codex".into(),
        peer_session_id: "peer".into(),
    }
}
fn setup() -> (tempfile::TempDir, Arc<Api>, Arc<Advisories>) {
    let dir = tempfile::tempdir().unwrap();
    let api = Arc::new(Api::default());
    let advisories = Advisories::new(dir.path(), Arc::new(Tokens), api.clone());
    advisories.set_enabled(true).unwrap();
    advisories.set_notifications_enabled(true).unwrap();
    (dir, api, advisories)
}
const A: &str = "11111111-1111-4111-8111-111111111111";
const B: &str = "22222222-2222-4222-8222-222222222222";

#[tokio::test]
async fn both_owned_recipients_get_the_ping_but_one_human_notification_survives_restart() {
    let (dir, api, advisories) = setup();
    let sink = Sink::default();
    let live = [target(A), target(B)];
    for target in &live {
        api.rows
            .lock()
            .unwrap()
            .insert(target.clone(), vec![p1("same-alert")]);
    }
    let now = Instant::now();
    advisories.poll(&live, now).await;
    advisories.deliver_urgent(&sink, now).await;
    assert_eq!(sink.calls.lock().unwrap().len(), 2);
    assert_eq!(sink.notices.lock().unwrap().len(), 1);
    assert!(api.acked.lock().unwrap().is_empty());
    let restarted = Advisories::new(dir.path(), Arc::new(Tokens), api.clone());
    let next = Sink::default();
    restarted.poll(&live, now).await;
    restarted.deliver_urgent(&next, now).await;
    restarted.send_shown(now).await;
    assert!(next.calls.lock().unwrap().is_empty());
    assert!(next.notices.lock().unwrap().is_empty());
    assert_eq!(api.acked.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn unproven_and_legacy_priorities_never_trigger_os_notices() {
    let (_dir, api, advisories) = setup();
    let sink = Sink::default();
    let t = target(A);
    let mut unproven = p1("unproven");
    unproven.notify_eligible = false;
    let mut legacy = p1("legacy");
    legacy.priority = None;
    let mut p2 = p1("p2");
    p2.priority = Some(2);
    api.rows
        .lock()
        .unwrap()
        .insert(t.clone(), vec![unproven, legacy, p2]);
    let now = Instant::now();
    advisories.poll(&[t.clone()], now).await;
    advisories.deliver_urgent(&sink, now).await;
    // Without the server's validated-quality decision nothing is injected
    // mid-turn either; every row waits for the turn's end.
    assert!(sink.calls.lock().unwrap().is_empty());
    assert!(sink.notices.lock().unwrap().is_empty());
    let note = advisories.take_at(&t, now).unwrap();
    assert_eq!(note.ids, vec!["unproven", "legacy", "p2"]);
    assert!(advisories.urgent_status().contains("validated"));
}

#[tokio::test]
async fn accepted_is_handed_off_once_not_acknowledged_and_does_not_block_other_turn_end_notes() {
    let (_dir, api, advisories) = setup();
    let sink = Sink::default();
    let t = target(A);
    let now = Instant::now();
    *sink.outcome.lock().unwrap() = Some(InjectOutcome::WaitingForTurn("busy".into()));
    let mut low = p1("low");
    low.priority = Some(3);
    low.urgency = "low";
    api.rows
        .lock()
        .unwrap()
        .insert(t.clone(), vec![p1("p1"), low]);
    advisories.poll(&[t.clone()], now).await;
    advisories.deliver_urgent(&sink, now).await;
    advisories.send_shown(now).await;
    assert!(api.acked.lock().unwrap().is_empty());
    assert_eq!(advisories.take_at(&t, now).unwrap().ids, vec!["low"]);
    *sink.outcome.lock().unwrap() = Some(InjectOutcome::Delivered);
    advisories.deliver_urgent(&sink, now).await;
    advisories.send_shown(now).await;
    assert_eq!(
        sink.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, id)| id.as_str())
            .collect::<Vec<_>>(),
        vec!["p1"]
    );
    assert!(api.acked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn uncertain_or_crashed_reservations_never_blindly_reinject() {
    for attempt in [Attempt::Reserved, Attempt::Uncertain] {
        let (dir, api, advisories) = setup();
        let t = target(A);
        let principal = Tokens.principal().unwrap();
        advisories
            .urgent
            .lock()
            .unwrap()
            .set_agent(&principal, &t, "p1", attempt, unix_now());
        assert!(advisories.save_urgent().await);
        let restarted = Advisories::new(dir.path(), Arc::new(Tokens), api.clone());
        let sink = Sink::default();
        api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
        let now = Instant::now();
        restarted.poll(&[t.clone()], now).await;
        restarted.deliver_urgent(&sink, now).await;
        assert!(sink.calls.lock().unwrap().is_empty());
        assert!(restarted.take_at(&t, now).is_none());
        assert!(api.acked.lock().unwrap().is_empty());
        assert_eq!(sink.notices.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn unregistered_provider_still_notifies_the_human_and_keeps_turn_end_fallback() {
    let (_dir, api, advisories) = setup();
    let sink = Sink::default();
    let t = target(A);
    let now = Instant::now();
    *sink.outcome.lock().unwrap() = Some(InjectOutcome::Refused("not registered".into()));
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(&[t.clone()], now).await;
    advisories.deliver_urgent(&sink, now).await;
    assert_eq!(sink.notices.lock().unwrap().len(), 1);
    assert_eq!(advisories.take_at(&t, now).unwrap().ids, vec!["p1"]);
}

#[tokio::test]
async fn human_cap_is_per_event_and_persisted() {
    let (dir, api, advisories) = setup();
    let sink = Sink::default();
    let t = target(A);
    let now = Instant::now();
    for n in 0..4 {
        api.rows
            .lock()
            .unwrap()
            .insert(t.clone(), vec![p1(&format!("p{n}"))]);
        advisories.poll(&[t.clone()], now).await;
        advisories.deliver_urgent(&sink, now).await;
    }
    assert_eq!(sink.notices.lock().unwrap().len(), 3);
    assert!(advisories.urgent_status().contains("limit"));
    let restarted = Advisories::new(dir.path(), Arc::new(Tokens), api);
    let next = Sink::default();
    restarted.poll(&[t], now).await;
    restarted.deliver_urgent(&next, now).await;
    assert!(next.notices.lock().unwrap().is_empty());
}

#[tokio::test]
async fn storage_failures_precede_every_external_side_effect() {
    for corrupt in [false, true] {
        let (dir, api, advisories) = setup();
        let t = target(A);
        let sink = Sink::default();
        if corrupt {
            std::fs::write(dir.path().join(FILE), b"broken").unwrap();
        } else {
            std::fs::create_dir(dir.path().join(FILE)).unwrap();
        }
        let advisories = if corrupt {
            Advisories::new(dir.path(), Arc::new(Tokens), api.clone())
        } else {
            advisories
        };
        api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
        let now = Instant::now();
        advisories.poll(&[t], now).await;
        advisories.deliver_urgent(&sink, now).await;
        assert!(sink.calls.lock().unwrap().is_empty());
        assert!(sink.notices.lock().unwrap().is_empty());
        assert!(advisories.urgent_status().contains("storage"));
    }
}

#[test]
fn scheduler_keeps_normal_and_known_urgent_cadences_separate() {
    let mut schedule = PollSchedule::default();
    let live = [target(A), target(B)];
    let fast = HashSet::from([target(A)]);
    let now = Instant::now();
    assert_eq!(schedule.due(&live, &fast, now).len(), 2);
    assert_eq!(
        schedule.due(&live, &fast, now + Duration::from_secs(30)),
        vec![target(A)]
    );
    assert_eq!(
        schedule
            .due(&live, &fast, now + Duration::from_secs(60))
            .len(),
        2
    );
}

#[tokio::test(start_paused = true)]
async fn list_and_ack_share_one_device_request_budget() {
    let tokens: Arc<dyn TokenSource> = Arc::new(Tokens);
    let api = Arc::new(Api::default());
    let paced = PacedApi::new(api, tokens.clone());
    let token = tokens.token().await.unwrap();
    let target = target(A);
    let start = tokio::time::Instant::now();
    let (a, b, c) = tokio::join!(
        paced.list(&token, &target),
        paced.ack(&token, &target, "ack"),
        paced.list(&token, &target)
    );
    assert!(matches!(a, Listing::Advisories(_)));
    assert_eq!(b, Ack::Done);
    assert!(matches!(c, Listing::Advisories(_)));
    assert!(tokio::time::Instant::now() - start >= Duration::from_secs(1));
}

#[tokio::test]
async fn polling_a_subset_does_not_end_the_other_live_sessions() {
    let (_dir, api, advisories) = setup();
    let live = [target(A), target(B)];
    let now = Instant::now();
    for target in &live {
        api.rows
            .lock()
            .unwrap()
            .insert(target.clone(), vec![p1("waiting")]);
    }
    advisories.poll(&live, now).await;
    advisories
        .poll_selected(&live, Some(&[target(A)]), now + Duration::from_secs(30))
        .await;
    assert!(advisories.pending(&target(B)));
}

#[test]
fn server_gates_fail_closed_for_missing_and_malformed_fields() {
    for priority in [Value::Null, json!(0), json!(4), json!("1"), json!(true)] {
        let row = json!({"advisoryId":"p1","urgency":"high","priority":priority,"notifyEligible":true,"message":"Test","peer":{"source":"codex","sessionId":"peer"}});
        assert!(!super::super::rows::advisory(&row).unwrap().urgent());
    }
    let row = json!({"advisoryId":"p1","urgency":"high","priority":1,"notifyEligible":"true","message":"Test","peer":{"source":"codex","sessionId":"peer"}});
    assert!(!super::super::rows::advisory(&row).unwrap().notify_eligible);
}

#[tokio::test]
async fn an_existing_notes_setting_does_not_opt_the_human_into_os_alerts() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(super::super::FILE), r#"{"enabled":true}"#).unwrap();
    let api = Arc::new(Api::default());
    let advisories = Advisories::new(dir.path(), Arc::new(Tokens), api.clone());
    assert!(advisories.enabled());
    assert!(!advisories.notifications_enabled());
    let t = target(A);
    api.rows
        .lock()
        .unwrap()
        .insert(t.clone(), vec![p1("existing")]);
    let now = Instant::now();
    advisories.poll(&[t], now).await;
    let sink = Sink::default();
    advisories.deliver_urgent(&sink, now).await;
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
    assert!(sink.notices.lock().unwrap().is_empty());
}

#[tokio::test]
async fn opting_into_notifications_requires_a_fresh_server_eligibility_decision() {
    let (_dir, api, advisories) = setup();
    let t = target(A);
    let now = Instant::now();
    api.rows
        .lock()
        .unwrap()
        .insert(t.clone(), vec![p1("fresh")]);
    advisories.poll(&[t.clone()], now).await;
    advisories.set_notifications_enabled(false).unwrap();
    advisories.set_notifications_enabled(true).unwrap();
    let sink = Sink::default();
    advisories.deliver_urgent(&sink, now).await;
    assert!(sink.notices.lock().unwrap().is_empty());
    assert!(sink.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_turn_end_that_wins_during_human_notification_is_not_also_injected() {
    struct TurnEnd(Arc<Advisories>, Target, Mutex<Option<Vec<String>>>);
    impl Delivery for TurnEnd {
        fn inject<'a>(
            &'a self,
            _: &'a Target,
            _: &'a str,
            _: &'a str,
        ) -> BoxFuture<'a, InjectOutcome> {
            panic!("the turn-end hook already owns this row");
        }
        fn notifications_available(&self) -> bool {
            true
        }
        fn notify(&self, _: &str, _: &str) {
            *self.2.lock().unwrap() = self.0.take_at(&self.1, Instant::now()).map(|n| n.ids);
        }
    }
    let (_dir, api, advisories) = setup();
    let t = target(A);
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("race")]);
    advisories.poll(&[t.clone()], now).await;
    let sink = TurnEnd(advisories.clone(), t, Mutex::new(None));
    advisories.deliver_urgent(&sink, now).await;
    assert_eq!(*sink.2.lock().unwrap(), Some(vec!["race".to_string()]));
    assert!(api.acked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn refused_is_terminal_and_retry_waits_out_its_backoff() {
    for (outcome, again_now) in [
        (InjectOutcome::Refused("gone".into()), false),
        (InjectOutcome::Retry("later".into()), false),
    ] {
        let (_dir, api, advisories) = setup();
        let sink = Sink::default();
        let t = target(A);
        let now = Instant::now();
        *sink.outcome.lock().unwrap() = Some(outcome.clone());
        api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
        advisories.poll(&[t.clone()], now).await;
        advisories.deliver_urgent(&sink, now).await;
        advisories.deliver_urgent(&sink, now).await;
        assert_eq!(sink.calls.lock().unwrap().len(), 1, "{outcome:?} again: {again_now}");
        // Long after the attempt, only a retry is tried again.
        let principal = Tokens.principal().unwrap();
        let old = unix_now() - URGENT_RETRY_AFTER - 1;
        let attempt = advisories
            .urgent
            .lock()
            .unwrap()
            .agent_receipt(&principal, &t, "p1")
            .unwrap()
            .0;
        advisories
            .urgent
            .lock()
            .unwrap()
            .set_agent(&principal, &t, "p1", attempt, old);
        advisories.deliver_urgent(&sink, now).await;
        let expected = if matches!(outcome, InjectOutcome::Retry(_)) { 2 } else { 1 };
        assert_eq!(sink.calls.lock().unwrap().len(), expected, "{outcome:?}");
    }
}

#[tokio::test]
async fn a_session_no_local_notice_reaches_is_never_reserved_or_injected() {
    struct Unreached(Sink);
    impl Delivery for Unreached {
        fn inject<'a>(
            &'a self,
            target: &'a Target,
            id: &'a str,
            text: &'a str,
        ) -> BoxFuture<'a, InjectOutcome> {
            self.0.inject(target, id, text)
        }
        fn notifications_available(&self) -> bool {
            true
        }
        fn notify(&self, title: &str, body: &str) {
            self.0.notify(title, body);
        }
        fn reaches(&self, _: &Target) -> bool {
            false
        }
    }
    let (_dir, api, advisories) = setup();
    let t = target(A);
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(&[t.clone()], now).await;
    let sink = Unreached(Sink::default());
    advisories.deliver_urgent(&sink, now).await;
    assert!(sink.0.calls.lock().unwrap().is_empty());
    let principal = Tokens.principal().unwrap();
    assert!(advisories
        .urgent
        .lock()
        .unwrap()
        .agent_receipt(&principal, &t, "p1")
        .is_none());
    assert_eq!(advisories.take_at(&t, now).unwrap().ids, vec!["p1"]);
}

#[tokio::test]
async fn with_advisories_off_an_urgent_pass_injects_and_notifies_nothing() {
    let (_dir, api, advisories) = setup();
    let t = target(A);
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(&[t.clone()], now).await;
    advisories.enabled.store(false, Ordering::SeqCst);
    let sink = Sink::default();
    advisories.deliver_urgent(&sink, now).await;
    assert!(sink.calls.lock().unwrap().is_empty());
    assert!(sink.notices.lock().unwrap().is_empty());
}

/// A token source whose signed-in user can change mid-test.
struct Switch(Mutex<&'static str>);
impl Switch {
    fn principal_for(user: &str) -> Principal {
        Principal {
            site: "https://test.example".into(),
            workspace_id: "test-workspace".into(),
            user_id: Some(user.into()),
            generation: 1,
        }
    }
}
impl TokenSource for Switch {
    fn principal(&self) -> Option<Principal> {
        Some(Self::principal_for(&self.0.lock().unwrap()))
    }
    fn token(&self) -> BoxFuture<'_, Option<ReadToken>> {
        let principal = self.principal().unwrap();
        Box::pin(async move {
            Some(ReadToken {
                origin: "https://test.example".into(),
                bearer: Secret::new("synthetic-read-token"),
                workspace_id: principal.workspace_id.clone(),
                principal,
            })
        })
    }
    fn refused(&self, _: &ReadToken) {}
}

fn switchable() -> (tempfile::TempDir, Arc<Api>, Arc<Switch>, Arc<Advisories>) {
    let dir = tempfile::tempdir().unwrap();
    let api = Arc::new(Api::default());
    let tokens = Arc::new(Switch(Mutex::new("alice")));
    let advisories = Advisories::new(dir.path(), tokens.clone(), api.clone());
    advisories.set_enabled(true).unwrap();
    advisories.set_notifications_enabled(true).unwrap();
    (dir, api, tokens, advisories)
}

#[tokio::test]
async fn alices_urgent_notes_are_never_injected_once_bob_is_signed_in() {
    let (_dir, api, tokens, advisories) = switchable();
    let t = target(A);
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(&[t.clone()], now).await;
    *tokens.0.lock().unwrap() = "bob";
    let sink = Sink::default();
    advisories.deliver_urgent(&sink, now).await;
    assert!(sink.calls.lock().unwrap().is_empty());
    assert!(sink.notices.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_account_switch_during_a_pass_stops_it_before_the_next_note() {
    struct Switching(Sink, Arc<Switch>);
    impl Delivery for Switching {
        fn inject<'a>(
            &'a self,
            target: &'a Target,
            id: &'a str,
            text: &'a str,
        ) -> BoxFuture<'a, InjectOutcome> {
            *self.1 .0.lock().unwrap() = "bob";
            self.0.inject(target, id, text)
        }
        fn notifications_available(&self) -> bool {
            false
        }
        fn notify(&self, _: &str, _: &str) {}
    }
    let (_dir, api, tokens, advisories) = switchable();
    let (a, b) = (target(A), target(B));
    let now = Instant::now();
    api.rows.lock().unwrap().insert(a.clone(), vec![p1("first")]);
    api.rows.lock().unwrap().insert(b.clone(), vec![p1("second")]);
    advisories.poll(&[a, b], now).await;
    let sink = Switching(Sink::default(), tokens);
    advisories.deliver_urgent(&sink, now).await;
    assert_eq!(sink.0.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_changed_setting_clears_a_stale_urgent_status() {
    let (_dir, api, advisories) = setup();
    let t = target(A);
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(&[t], now).await;
    advisories.set_notifications_enabled(false).unwrap();
    advisories.deliver_urgent(&Sink::default(), now).await;
    assert_eq!(advisories.urgent_status(), "Urgent system notifications are off.");
    advisories.set_notifications_enabled(true).unwrap();
    assert_eq!(advisories.urgent_status(), "idle");
}
