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
    /// The incarnation each ack named, in order.
    incarnations: Mutex<Vec<Option<String>>>,
    /// What each ack answers; `Done` when unset.
    answer: Mutex<Option<Ack>>,
    /// Incarnations whose v2 ack finds no match (404).
    stale: Mutex<HashSet<String>>,
    /// List under the legacy contract instead of [`CONTRACT_V3`].
    legacy: AtomicBool,
    /// Answer every listing with [`Listing::Failed`].
    failed: AtomicBool,
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
        if self.failed.load(Ordering::SeqCst) {
            return Box::pin(async { Listing::Failed });
        }
        let contract = if self.legacy.load(Ordering::SeqCst) {
            ListingContract::Legacy
        } else {
            ListingContract::V3
        };
        Box::pin(async move { Listing::Advisories(rows, contract) })
    }
    fn ack<'a>(
        &'a self,
        _: &'a ReadToken,
        _: &'a Target,
        id: &'a str,
        incarnation: Option<&'a str>,
    ) -> BoxFuture<'a, Ack> {
        self.acked.lock().unwrap().push(id.into());
        self.incarnations
            .lock()
            .unwrap()
            .push(incarnation.map(String::from));
        let stale = incarnation.is_some_and(|i| self.stale.lock().unwrap().contains(i));
        let answer = match *self.answer.lock().unwrap() {
            _ if stale => Ack::NoMatch,
            Some(answer) => answer,
            None => Ack::Done,
        };
        Box::pin(async move { answer })
    }
}

#[derive(Default)]
struct Sink {
    calls: Mutex<Vec<(Target, String)>>,
    notices: Mutex<Vec<(String, String)>>,
    outcome: Mutex<Option<InjectOutcome>>,
    unavailable: AtomicBool,
    /// What `observe` reports; `None` is the trait default (unsupported).
    observation: Mutex<Option<Observation>>,
    observes: std::sync::atomic::AtomicUsize,
    /// The keys observed, in order.
    observed: Mutex<Vec<String>>,
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
    fn observe<'a>(&'a self, _: &'a Target, id: &'a str) -> BoxFuture<'a, Observation> {
        self.observes.fetch_add(1, Ordering::SeqCst);
        self.observed.lock().unwrap().push(id.into());
        let seen = self
            .observation
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(Observation::Unsupported);
        Box::pin(async move { seen })
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
        incarnation: Some(INCARNATION.into()),
        key: String::new(),
    }
}
/// Every test row's incarnation, unless a test says otherwise.
const INCARNATION: &str = "5f0c8a1e-2b3d-4c5e-8f6a-7b8c9d0e1f2a";
/// The key `id`'s instance (incarnation [`INCARNATION`]) is held under for `t`.
fn key(t: &Target, id: &str) -> String {
    key_of(t, id, INCARNATION)
}
/// `advisory` keyed as the poll caches it for `t`.
fn keyed(t: &Target, advisory: Advisory) -> Advisory {
    advisory.keyed(&receipts::Scope::from(&Tokens.principal().unwrap()), t)
}
/// The keys of `ids`' instances for `t`, as a turn end hands them out.
fn keys(t: &Target, ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| key(t, id)).collect()
}
fn key_of(t: &Target, id: &str, incarnation: &str) -> String {
    let scope = receipts::Scope::from(&Tokens.principal().unwrap());
    instance::key(&scope, t, id, Some(incarnation))
}
/// `advisories`, with a live report that an app can show its system
/// notifications, as a connected app keeps sending.
fn displayed(advisories: Arc<Advisories>) -> Arc<Advisories> {
    advisories.report_display(NotificationPermission::Allowed);
    advisories
}
fn setup() -> (tempfile::TempDir, Arc<Api>, Arc<Advisories>) {
    let dir = tempfile::tempdir().unwrap();
    let api = Arc::new(Api::default());
    let advisories = displayed(Advisories::new(dir.path(), Arc::new(Tokens), api.clone()));
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
    let restarted = displayed(Advisories::new(dir.path(), Arc::new(Tokens), api.clone()));
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
    assert_eq!(note.ids, keys(&t, &["unproven", "legacy", "p2"]));
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
    assert_eq!(advisories.take_at(&t, now).unwrap().ids, keys(&t, &["low"]));
    *sink.outcome.lock().unwrap() = Some(InjectOutcome::Delivered);
    advisories.deliver_urgent(&sink, now).await;
    advisories.send_shown(now).await;
    assert_eq!(
        sink.calls
            .lock()
            .unwrap()
            .iter()
            .map(|(_, id)| id.clone())
            .collect::<Vec<_>>(),
        keys(&t, &["p1"])
    );
    assert!(api.acked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn uncertain_or_crashed_reservations_never_blindly_reinject() {
    for attempt in [Attempt::Reserved, Attempt::Uncertain] {
        let (dir, api, advisories) = setup();
        let t = target(A);
        let principal = Tokens.principal().unwrap();
        advisories.urgent.lock().unwrap().set_agent(
            &principal,
            &t,
            &key(&t, "p1"),
            attempt,
            unix_now(),
        );
        assert!(advisories.save_urgent().await);
        let restarted = displayed(Advisories::new(dir.path(), Arc::new(Tokens), api.clone()));
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
    assert_eq!(advisories.take_at(&t, now).unwrap().ids, keys(&t, &["p1"]));
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
    let restarted = displayed(Advisories::new(dir.path(), Arc::new(Tokens), api));
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
            displayed(Advisories::new(dir.path(), Arc::new(Tokens), api.clone()))
        } else {
            advisories
        };
        api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
        let now = Instant::now();
        advisories.poll(&[t], now).await;
        advisories.deliver_urgent(&sink, now).await;
        // A damaged file blocks at load (and stays in place); a write
        // failure blocks the save. Either way nothing external happens.
        assert!(sink.calls.lock().unwrap().is_empty());
        assert!(sink.notices.lock().unwrap().is_empty());
        assert!(advisories.urgent_status().contains("storage"));
        if corrupt {
            assert!(dir.path().join(FILE).exists());
        }
    }
}

#[test]
fn scheduler_keeps_normal_and_known_urgent_cadences_separate() {
    let mut schedule = PollSchedule::default();
    let live = [target(A), target(B)];
    let fast = HashSet::from([target(A)]);
    let now = Instant::now();
    assert_eq!(schedule.due(&live, &fast, now).len(), 2);
    schedule.checked_at(&live, now);
    let later = now + Duration::from_secs(30);
    assert_eq!(schedule.due(&live, &fast, later), vec![target(A)]);
    schedule.checked_at(&[target(A)], later);
    assert_eq!(
        schedule
            .due(&live, &fast, now + Duration::from_secs(60))
            .len(),
        2
    );
}

#[test]
fn a_target_the_poll_never_answered_is_due_again_next_tick() {
    let mut schedule = PollSchedule::default();
    let live = [target(A), target(B)];
    let now = Instant::now();
    assert_eq!(schedule.due(&live, &HashSet::new(), now).len(), 2);
    // Throttled after the first: only A was answered.
    schedule.checked_at(&[target(A)], now);
    assert_eq!(
        schedule.due(&live, &HashSet::new(), now + Duration::from_secs(1)),
        vec![target(B)]
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
        paced.ack(&token, &target, "ack", None),
        paced.list(&token, &target)
    );
    assert!(matches!(a, Listing::Advisories(..)));
    assert_eq!(b, Ack::Done);
    assert!(matches!(c, Listing::Advisories(..)));
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
        let row = json!({"advisoryId":"p1","urgency":"high","priority":priority,"notifyEligible":true,"message":"Test","peer":{"source":"codex","sessionId":"peer"},"incarnation":INCARNATION});
        assert!(!super::super::rows::advisory(&row, true).unwrap().urgent());
    }
    let row = json!({"advisoryId":"p1","urgency":"high","priority":1,"notifyEligible":"true","message":"Test","peer":{"source":"codex","sessionId":"peer"},"incarnation":INCARNATION});
    assert!(
        !super::super::rows::advisory(&row, true)
            .unwrap()
            .notify_eligible
    );
}

#[tokio::test]
async fn an_existing_notes_setting_does_not_opt_the_human_into_os_alerts() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(super::super::FILE), r#"{"enabled":true}"#).unwrap();
    let api = Arc::new(Api::default());
    let advisories = displayed(Advisories::new(dir.path(), Arc::new(Tokens), api.clone()));
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
    assert_eq!(*sink.2.lock().unwrap(), Some(keys(&target(A), &["race"])));
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
            .agent_receipt(&principal, &t, &key(&t, "p1"))
            .unwrap()
            .0;
        advisories
            .urgent
            .lock()
            .unwrap()
            .set_agent(&principal, &t, &key(&t, "p1"), attempt, old);
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
        .agent_receipt(&principal, &t, &key(&t, "p1"))
        .is_none());
    assert_eq!(advisories.take_at(&t, now).unwrap().ids, keys(&t, &["p1"]));
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
    let advisories = displayed(Advisories::new(dir.path(), tokens.clone(), api.clone()));
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

#[tokio::test]
async fn a_pass_stopped_after_its_reservation_undoes_it() {
    struct OffDuringSave(Sink);
    impl Delivery for OffDuringSave {
        fn inject<'a>(
            &'a self,
            target: &'a Target,
            id: &'a str,
            text: &'a str,
        ) -> BoxFuture<'a, InjectOutcome> {
            self.0.inject(target, id, text)
        }
        fn notifications_available(&self) -> bool {
            false
        }
        fn notify(&self, _: &str, _: &str) {}
    }
    let (_dir, api, advisories) = setup();
    let t = target(A);
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(&[t.clone()], now).await;
    let principal = Tokens.principal().unwrap();
    let before = advisories.urgent.lock().unwrap().saved.clone();
    // The switch goes off between the durable reservation and the inject.
    advisories.enabled.store(false, Ordering::SeqCst);
    let advisory = keyed(&t, p1("p1"));
    let step = advisories
        .deliver_urgent_agent(&OffDuringSave(Sink::default()), &principal, &t, &advisory, unix_now(), now)
        .await;
    assert!(step.is_break());
    assert!(advisories
        .urgent
        .lock()
        .unwrap()
        .agent_receipt(&principal, &t, &key(&t, "p1"))
        .is_none());
    assert_eq!(advisories.urgent.lock().unwrap().saved.agents.len(), before.agents.len());
    // The undo is on disk too: a restart does not find the reservation.
    assert!(Journal::load(_dir.path())
        .agent_receipt(&principal, &t, &key(&t, "p1"))
        .is_none());
}

#[tokio::test]
async fn read_access_unavailable_survives_the_urgent_pass_and_clears_when_reads_work() {
    struct NoToken(AtomicBool);
    impl TokenSource for NoToken {
        fn principal(&self) -> Option<Principal> {
            Tokens.principal()
        }
        fn token(&self) -> BoxFuture<'_, Option<ReadToken>> {
            let off = self.0.load(Ordering::SeqCst);
            Box::pin(async move { if off { None } else { Tokens.token().await } })
        }
        fn refused(&self, _: &ReadToken) {}
    }
    let dir = tempfile::tempdir().unwrap();
    let api = Arc::new(Api::default());
    let tokens = Arc::new(NoToken(AtomicBool::new(false)));
    let advisories = displayed(Advisories::new(dir.path(), tokens.clone(), api.clone()));
    advisories.set_enabled(true).unwrap();
    advisories.set_notifications_enabled(true).unwrap();
    let t = target(A);
    let now = Instant::now();
    // The cache belongs to this account (nothing urgent is listed), so the
    // urgent pass runs; before the fix it reset the status to idle.
    advisories.poll(&[t.clone()], now).await;
    tokens.0.store(true, Ordering::SeqCst);
    advisories.poll(&[t.clone()], now).await;
    advisories.deliver_urgent(&Sink::default(), now).await;
    assert!(advisories.urgent_status().starts_with("Read access unavailable"));
    tokens.0.store(false, Ordering::SeqCst);
    advisories.poll(&[t], now).await;
    assert!(!advisories.urgent_status().starts_with("Read access unavailable"));
}

#[test]
fn a_damaged_or_newer_version_journal_blocks_in_place_and_again_on_relaunch() {
    for (body, fault) in [
        (&b"broken"[..], "is damaged"),
        (&br#"{"version":1,"agents":[],"humans":[],"later":1}"#[..], "is damaged"),
        (&br#"{"version":2,"agents":[],"humans":[],"later":1}"#[..], "is damaged"),
        (&br#"{"version":0,"agents":[],"humans":[]}"#[..], "is damaged"),
        (&br#"{"version":3,"agents":[],"humans":[]}"#[..], "newer version"),
        (&br#"{"version":9,"shape":"unknown"}"#[..], "newer version"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        std::fs::write(&path, body).unwrap();
        for _launch in 0..2 {
            let journal = Journal::load(dir.path());
            assert!(journal.blocked());
            let text = journal.fault.clone().unwrap();
            assert!(text.contains(fault), "{text}");
            assert!(text.contains(&path.display().to_string()), "{text}");
            assert_eq!(std::fs::read(&path).unwrap(), body);
        }
    }
    // A missing file is a fresh start.
    let dir = tempfile::tempdir().unwrap();
    assert!(!Journal::load(dir.path()).blocked());
}

#[cfg(unix)]
#[test]
fn an_io_error_on_a_valid_journal_keeps_it_blocked_and_in_place() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(FILE);
    std::fs::write(&path, br#"{"version":1,"agents":[],"humans":[]}"#).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&path).is_ok() {
        // Running as a user that ignores file modes (root): nothing to test.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        return;
    }
    let journal = Journal::load(dir.path());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(journal.blocked());
    assert!(journal.fault.unwrap().contains("cannot be read"));
    assert!(path.exists());
}

#[tokio::test]
async fn each_urgent_pass_clears_last_passes_one_off_status_but_not_read_access() {
    let (_dir, api, advisories) = setup();
    let t = target(A);
    let now = Instant::now();
    advisories.poll(&[t.clone()], now).await;
    advisories.urgent.lock().unwrap().status = "System notification hourly limit reached.";
    advisories.deliver_urgent(&Sink::default(), now).await;
    assert_eq!(advisories.urgent_status(), "idle");
    advisories.urgent.lock().unwrap().status =
        "Read access unavailable; urgent advisories cannot be fetched.";
    advisories.deliver_urgent(&Sink::default(), now).await;
    assert!(advisories.urgent_status().starts_with("Read access unavailable"));
    drop(api);
}

#[tokio::test]
async fn a_session_in_its_own_backoff_counts_as_checked() {
    let (_dir, _api, advisories) = setup();
    let (a, b) = (target(A), target(B));
    let now = Instant::now();
    advisories.poll(&[a.clone(), b.clone()], now).await;
    advisories
        .state()
        .sessions
        .entry(a.clone())
        .or_default()
        .until = Some(now + Duration::from_secs(600));
    let live = [a.clone(), b.clone()];
    let mut answered = advisories.poll_selected(&live, Some(&live), now).await;
    answered.sort_by(|x, y| x.id.cmp(&y.id));
    assert_eq!(answered, vec![a.clone(), b.clone()]);
    let mut schedule = PollSchedule::default();
    schedule.checked_at(&answered, now);
    assert!(schedule.due(&live, &HashSet::new(), now + Duration::from_secs(1)).is_empty());
}

/// [`setup`] with no app's report that it can show a notification.
fn unreported() -> (tempfile::TempDir, Arc<Api>, Arc<Advisories>) {
    let dir = tempfile::tempdir().unwrap();
    let api = Arc::new(Api::default());
    let advisories = Advisories::new(dir.path(), Arc::new(Tokens), api.clone());
    advisories.set_enabled(true).unwrap();
    advisories.set_notifications_enabled(true).unwrap();
    (dir, api, advisories)
}

#[tokio::test]
async fn a_legacy_only_listing_says_the_workspace_has_no_urgent_alerts_and_v3_is_idle() {
    let (_dir, api, advisories) = setup();
    let t = target(A);
    let now = Instant::now();
    // Nothing listed yet: idle.
    assert_eq!(advisories.report().notification_status, IDLE_STATUS);
    api.legacy.store(true, Ordering::SeqCst);
    advisories.poll(&[t.clone()], now).await;
    advisories.deliver_urgent(&Sink::default(), now).await;
    assert_eq!(advisories.report().notification_status, LEGACY_STATUS);
    // Turning notes off forgets the listing's contract.
    advisories.set_enabled(false).unwrap();
    advisories.set_enabled(true).unwrap();
    advisories.set_notifications_enabled(true).unwrap();
    assert_eq!(advisories.report().notification_status, IDLE_STATUS);
    advisories.poll(&[t.clone()], now).await;
    assert_eq!(advisories.report().notification_status, LEGACY_STATUS);
    api.legacy.store(false, Ordering::SeqCst);
    advisories.poll(&[t], now).await;
    advisories.deliver_urgent(&Sink::default(), now).await;
    assert_eq!(advisories.report().notification_status, IDLE_STATUS);
}

#[tokio::test]
async fn a_legacy_listing_is_forgotten_when_the_account_changes() {
    let (_dir, api, tokens, advisories) = switchable();
    let t = target(A);
    let now = Instant::now();
    api.legacy.store(true, Ordering::SeqCst);
    advisories.poll(&[t.clone()], now).await;
    assert_eq!(advisories.report().notification_status, LEGACY_STATUS);
    *tokens.0.lock().unwrap() = "bob";
    assert_eq!(advisories.report().notification_status, IDLE_STATUS);
    // The cache now belongs to bob, through a poll that lists nothing: the
    // previous account's legacy listing is not his.
    api.failed.store(true, Ordering::SeqCst);
    advisories.poll(&[t], now).await;
    assert_eq!(
        advisories.state().principal,
        Some(Switch::principal_for("bob"))
    );
    assert_eq!(advisories.report().notification_status, IDLE_STATUS);
}

/// An account with no user identity.
struct NoUser;
impl TokenSource for NoUser {
    fn principal(&self) -> Option<Principal> {
        Tokens.principal().map(|p| Principal { user_id: None, ..p })
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

/// [`setup`] for an account with no user identity ([`NoUser`]).
fn no_user() -> (tempfile::TempDir, Arc<Api>, Arc<Advisories>) {
    let dir = tempfile::tempdir().unwrap();
    let api = Arc::new(Api::default());
    let advisories = displayed(Advisories::new(dir.path(), Arc::new(NoUser), api.clone()));
    advisories.set_enabled(true).unwrap();
    advisories.set_notifications_enabled(true).unwrap();
    (dir, api, advisories)
}

#[tokio::test]
async fn an_account_with_no_user_identity_says_so_in_the_pass_and_at_read_time() {
    let (_dir, api, advisories) = no_user();
    // A standing condition: shown before any pass ran.
    assert_eq!(advisories.report().notification_status, NO_USER_STATUS);
    let t = target(A);
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(&[t], now).await;
    let sink = Sink::default();
    advisories.deliver_urgent(&sink, now).await;
    assert!(sink.notices.lock().unwrap().is_empty());
    assert_eq!(advisories.urgent_status(), NO_USER_STATUS);
    assert_eq!(advisories.report().notification_status, NO_USER_STATUS);
}

#[tokio::test]
async fn an_agent_path_failure_shows_over_a_standing_condition_until_a_clean_pass() {
    let (dir, api, advisories) = no_user();
    let t = target(A);
    let now = Instant::now();
    // The agent path's receipt cannot be written.
    std::fs::create_dir(dir.path().join(FILE)).unwrap();
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(&[t], now).await;
    let sink = Sink::default();
    advisories.deliver_urgent(&sink, now).await;
    assert!(sink.calls.lock().unwrap().is_empty());
    assert_eq!(advisories.urgent_status(), SAVE_FAILED_STATUS);
    assert_eq!(advisories.report().notification_status, SAVE_FAILED_STATUS);
    // Storage works again: the next pass is clean, and the standing
    // condition is the status again.
    std::fs::remove_dir(dir.path().join(FILE)).unwrap();
    advisories.deliver_urgent(&sink, now).await;
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
    assert_eq!(advisories.urgent_status(), NO_USER_STATUS);
    assert_eq!(advisories.report().notification_status, NO_USER_STATUS);
}

#[tokio::test]
async fn no_app_report_requests_nothing_until_one_arrives_then_notifies_once() {
    let (_dir, api, advisories) = unreported();
    let t = target(A);
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(&[t.clone()], now).await;
    // The agent is mid-turn, so the row is still waiting on the next pass.
    let sink = Sink::default();
    *sink.outcome.lock().unwrap() = Some(InjectOutcome::WaitingForTurn("busy".into()));
    advisories.deliver_urgent(&sink, now).await;
    assert!(sink.notices.lock().unwrap().is_empty());
    assert!(advisories.urgent.lock().unwrap().saved.humans.is_empty());
    assert_eq!(advisories.report().notification_status, IDLE_STATUS);
    advisories.report_display(NotificationPermission::Allowed);
    advisories.deliver_urgent(&sink, now).await;
    advisories.deliver_urgent(&sink, now).await;
    let notices = sink.notices.lock().unwrap().clone();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].1.contains("Review overlap p1."), "{notices:?}");
    assert_eq!(advisories.urgent.lock().unwrap().saved.humans.len(), 1);
}

#[tokio::test]
async fn a_report_older_than_its_lifetime_counts_as_none() {
    let (_dir, api, advisories) = unreported();
    let stale = Instant::now()
        .checked_sub(DISPLAY_REPORT_LIVE + Duration::from_secs(1))
        .unwrap();
    advisories.report_display_at(NotificationPermission::Allowed, stale);
    let t = target(A);
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(&[t], now).await;
    let sink = Sink::default();
    advisories.deliver_urgent(&sink, now).await;
    assert!(sink.notices.lock().unwrap().is_empty());
    assert!(advisories.urgent.lock().unwrap().saved.humans.is_empty());
    // A stale denial is no denial either.
    advisories.report_display_at(NotificationPermission::Denied, stale);
    assert_eq!(advisories.report().notification_status, IDLE_STATUS);
}

#[tokio::test]
async fn denied_requests_nothing_says_so_and_never_spends_the_hourly_cap() {
    let (_dir, api, advisories) = unreported();
    advisories.report_display(NotificationPermission::Denied);
    let now = Instant::now();
    let sink = Sink::default();
    for (i, session) in [A, B, "33333333-3333-4333-8333-333333333333"]
        .into_iter()
        .enumerate()
    {
        let t = target(session);
        api.rows
            .lock()
            .unwrap()
            .insert(t.clone(), vec![p1(&format!("denied-{i}"))]);
        advisories.poll(&[t], now).await;
        advisories.deliver_urgent(&sink, now).await;
        assert_eq!(advisories.urgent_status(), DENIED_STATUS);
        assert_eq!(advisories.report().notification_status, DENIED_STATUS);
    }
    assert!(sink.notices.lock().unwrap().is_empty());
    assert!(advisories.urgent.lock().unwrap().saved.humans.is_empty());
    advisories.report_display(NotificationPermission::Allowed);
    let t = target("44444444-4444-4444-8444-444444444444");
    api.rows
        .lock()
        .unwrap()
        .insert(t.clone(), vec![p1("allowed")]);
    advisories.poll(&[t], now).await;
    advisories.deliver_urgent(&sink, now).await;
    assert_eq!(sink.notices.lock().unwrap().len(), 1);
    assert_ne!(advisories.report().notification_status, DENIED_STATUS);
}

#[tokio::test]
async fn allowed_and_unknown_both_notify_as_before() {
    for permission in [
        NotificationPermission::Allowed,
        NotificationPermission::Unknown,
    ] {
        let (_dir, api, advisories) = unreported();
        advisories.report_display(permission);
        let t = target(A);
        let now = Instant::now();
        api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
        advisories.poll(&[t], now).await;
        let sink = Sink::default();
        advisories.deliver_urgent(&sink, now).await;
        assert_eq!(sink.notices.lock().unwrap().len(), 1, "{permission:?}");
    }
}

mod instances;
mod observe;
mod release;
