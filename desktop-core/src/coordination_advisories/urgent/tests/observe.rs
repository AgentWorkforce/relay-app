//! Held notes are observed, never injected again; only an observed delivery
//! settles one, with exactly one ack.

use super::*;

/// One P1 note for `A`, handed to the provider and held as accepted.
async fn accepted() -> (tempfile::TempDir, Arc<Api>, Arc<Advisories>, Sink, Target, Instant) {
    let (dir, api, advisories) = setup();
    let sink = Sink::default();
    let t = target(A);
    let now = Instant::now();
    *sink.outcome.lock().unwrap() = Some(InjectOutcome::Accepted("queued".into()));
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(&[t.clone()], now).await;
    advisories.deliver_urgent(&sink, now).await;
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
    (dir, api, advisories, sink, t, now)
}

fn receipt(advisories: &Advisories, t: &Target) -> Option<Attempt> {
    let principal = Tokens.principal().unwrap();
    advisories
        .urgent
        .lock()
        .unwrap()
        .agent_receipt(&principal, t, "p1")
        .map(|r| r.0)
}

/// Lets the next pass observe again, as if a minute had passed.
fn age_observation(advisories: &Advisories) {
    for at in advisories.urgent.lock().unwrap().observed.values_mut() {
        *at = at.saturating_sub(61);
    }
}

#[tokio::test]
async fn an_accepted_note_observed_delivered_is_acked_exactly_once() {
    let (_dir, api, advisories, sink, t, now) = accepted().await;
    *sink.observation.lock().unwrap() = Some(Observation::Delivered);
    advisories.deliver_urgent(&sink, now).await;
    assert_eq!(receipt(&advisories, &t), Some(Attempt::Delivered));
    advisories.send_shown(now).await;
    assert_eq!(*api.acked.lock().unwrap(), vec!["p1"]);
    // Later passes neither inject nor ack it again.
    age_observation(&advisories);
    advisories.deliver_urgent(&sink, now).await;
    advisories.send_shown(now).await;
    assert_eq!(*api.acked.lock().unwrap(), vec!["p1"]);
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
    assert_eq!(sink.observes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn after_a_restart_the_held_note_is_observed_not_injected() {
    let (dir, api, _advisories, _sink, t, now) = accepted().await;
    let restarted = Advisories::new(dir.path(), Arc::new(Tokens), api.clone());
    restarted.poll(&[t.clone()], now).await;
    let next = Sink::default();
    *next.observation.lock().unwrap() = Some(Observation::Delivered);
    restarted.deliver_urgent(&next, now).await;
    restarted.send_shown(now).await;
    assert!(next.calls.lock().unwrap().is_empty());
    assert_eq!(next.observes.load(Ordering::SeqCst), 1);
    assert_eq!(*api.acked.lock().unwrap(), vec!["p1"]);
}

#[tokio::test]
async fn pending_in_doubt_unsupported_or_fenced_change_nothing() {
    for seen in [
        Observation::Pending,
        Observation::InDoubt,
        Observation::Unsupported,
        Observation::Fenced,
    ] {
        let (_dir, api, advisories, sink, t, now) = accepted().await;
        *sink.observation.lock().unwrap() = Some(seen.clone());
        advisories.deliver_urgent(&sink, now).await;
        advisories.send_shown(now).await;
        assert_eq!(receipt(&advisories, &t), Some(Attempt::Accepted), "{seen:?}");
        assert!(api.acked.lock().unwrap().is_empty(), "{seen:?}");
        assert!(advisories.take_at(&t, now).is_none(), "{seen:?}");
        assert_eq!(sink.calls.lock().unwrap().len(), 1, "{seen:?}");
        assert_eq!(sink.observes.load(Ordering::SeqCst), 1, "{seen:?}");
    }
}

#[tokio::test]
async fn observation_is_at_most_once_a_minute() {
    let (_dir, _api, advisories, sink, _t, now) = accepted().await;
    *sink.observation.lock().unwrap() = Some(Observation::Pending);
    advisories.deliver_urgent(&sink, now).await;
    advisories.deliver_urgent(&sink, now).await;
    assert_eq!(sink.observes.load(Ordering::SeqCst), 1);
    age_observation(&advisories);
    advisories.deliver_urgent(&sink, now).await;
    assert_eq!(sink.observes.load(Ordering::SeqCst), 2);
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_held_note_is_never_handed_out_at_a_turn_end_and_a_late_delivery_still_acks_once() {
    let (_dir, api, advisories, sink, t, now) = accepted().await;
    let principal = Tokens.principal().unwrap();
    advisories.urgent.lock().unwrap().set_agent(
        &principal,
        &t,
        "p1",
        Attempt::Accepted,
        unix_now() - 31 * 60,
    );
    assert!(advisories.take_at(&t, now).is_none());
    *sink.observation.lock().unwrap() = Some(Observation::Delivered);
    advisories.deliver_urgent(&sink, now).await;
    advisories.send_shown(now).await;
    assert_eq!(*api.acked.lock().unwrap(), vec!["p1"]);
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn reserved_and_uncertain_receipts_are_observed_too() {
    for attempt in [Attempt::Reserved, Attempt::Uncertain] {
        let (dir, api, advisories) = setup();
        let t = target(A);
        let principal = Tokens.principal().unwrap();
        advisories
            .urgent
            .lock()
            .unwrap()
            .set_agent(&principal, &t, "p1", attempt.clone(), unix_now());
        assert!(advisories.save_urgent().await);
        let restarted = Advisories::new(dir.path(), Arc::new(Tokens), api.clone());
        api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
        let now = Instant::now();
        restarted.poll(&[t.clone()], now).await;
        let sink = Sink::default();
        *sink.observation.lock().unwrap() = Some(Observation::Delivered);
        restarted.deliver_urgent(&sink, now).await;
        restarted.send_shown(now).await;
        assert!(sink.calls.lock().unwrap().is_empty(), "{attempt:?}");
        assert_eq!(*api.acked.lock().unwrap(), vec!["p1"], "{attempt:?}");
    }
}

#[tokio::test]
async fn an_account_change_during_observation_records_nothing() {
    struct SwitchOnObserve(Sink, Arc<Switch>);
    impl Delivery for SwitchOnObserve {
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
        fn observe<'a>(&'a self, _: &'a Target, _: &'a str) -> BoxFuture<'a, Observation> {
            *self.1 .0.lock().unwrap() = "bob";
            Box::pin(async { Observation::Delivered })
        }
    }
    let (_dir, api, tokens, advisories) = switchable();
    let t = target(A);
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(&[t.clone()], now).await;
    let alice = Switch::principal_for("alice");
    advisories
        .urgent
        .lock()
        .unwrap()
        .set_agent(&alice, &t, "p1", Attempt::Accepted, unix_now());
    let sink = SwitchOnObserve(Sink::default(), tokens);
    advisories.deliver_urgent(&sink, now).await;
    let held = advisories
        .urgent
        .lock()
        .unwrap()
        .agent_receipt(&alice, &t, "p1")
        .map(|r| r.0);
    assert_eq!(held, Some(Attempt::Accepted));
    assert!(api.acked.lock().unwrap().is_empty());
    assert!(sink.0.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_unsaved_observed_delivery_is_not_shown_or_acked() {
    let (dir, api, advisories, sink, t, now) = accepted().await;
    let path = dir.path().join(FILE);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    *sink.observation.lock().unwrap() = Some(Observation::Delivered);
    advisories.deliver_urgent(&sink, now).await;
    advisories.send_shown(now).await;
    assert_eq!(receipt(&advisories, &t), Some(Attempt::Accepted));
    assert!(api.acked.lock().unwrap().is_empty());
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn an_old_accepted_receipt_survives_the_prune_and_still_blocks_inject_and_take() {
    let (_dir, _api, advisories, sink, t, now) = accepted().await;
    let principal = Tokens.principal().unwrap();
    let old = unix_now() - RETENTION - 60;
    {
        let mut journal = advisories.urgent.lock().unwrap();
        journal.set_agent(&principal, &t, "p1", Attempt::Accepted, old);
        journal.set_agent(&principal, &t, "settled", Attempt::Delivered, old);
        journal.prune(unix_now(), &HashSet::new());
        assert!(journal.agent_receipt(&principal, &t, "settled").is_none());
    }
    assert_eq!(receipt(&advisories, &t), Some(Attempt::Accepted));
    age_observation(&advisories);
    advisories.deliver_urgent(&sink, now).await;
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
    assert!(advisories.take_at(&t, now).is_none());
}

#[tokio::test]
async fn a_full_journal_fails_closed() {
    let (_dir, api, advisories) = setup();
    let principal = Tokens.principal().unwrap();
    {
        let mut journal = advisories.urgent.lock().unwrap();
        for i in 0..=MAX_RECORDS {
            journal.set_agent(
                &principal,
                &target(B),
                &format!("held-{i}"),
                Attempt::Accepted,
                unix_now(),
            );
        }
    }
    assert!(!advisories.save_urgent().await);
    assert!(advisories.urgent_status().contains("receipt storage"));
    let t = target(A);
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(&[t.clone()], now).await;
    let sink = Sink::default();
    advisories.deliver_urgent(&sink, now).await;
    // Its reservation cannot be saved, so it is never injected.
    assert!(sink.calls.lock().unwrap().is_empty());
}

/// The receipt of `id` for `A`, as a fresh process would load it.
fn on_disk(dir: &Path, id: &str) -> Option<Attempt> {
    let principal = Tokens.principal().unwrap();
    Journal::load(dir).agent_receipt(&principal, &target(A), id).map(|r| r.0)
}

#[tokio::test]
async fn a_held_id_outside_a_full_page_is_kept_past_8_days_and_never_resubmitted() {
    let (dir, api, advisories, _sink, t, now) = accepted().await;
    let principal = Tokens.principal().unwrap();
    advisories.urgent.lock().unwrap().set_agent(
        &principal,
        &t,
        "p1",
        Attempt::Accepted,
        unix_now() - RETENTION - 60,
    );
    assert!(advisories.save_urgent().await);
    // A full page of other pending rows: the held id is not on it.
    let page: Vec<Advisory> = (0..10).map(|i| p1(&format!("other-{i}"))).collect();
    api.rows.lock().unwrap().insert(t.clone(), page);
    for _launch in 0..2 {
        let relaunched = Advisories::new(dir.path(), Arc::new(Tokens), api.clone());
        relaunched.poll(&[t.clone()], now).await;
        let next = Sink::default();
        *next.outcome.lock().unwrap() = Some(InjectOutcome::Accepted("queued".into()));
        relaunched.deliver_urgent(&next, now).await;
        assert!(!next.calls.lock().unwrap().iter().any(|(_, id)| id == "p1"));
        let taken = relaunched.take_at(&t, now).map(|n| n.ids).unwrap_or_default();
        assert!(!taken.contains(&"p1".to_string()));
        assert_eq!(on_disk(dir.path(), "p1"), Some(Attempt::Accepted));
    }
}

#[test]
fn past_retention_unsettled_receipts_stay_and_settled_ones_go_unless_still_cached() {
    let principal = Tokens.principal().unwrap();
    let scope = receipts::Scope::from(&principal);
    let dir = tempfile::tempdir().unwrap();
    let mut journal = Journal::load(dir.path());
    let old = unix_now() - RETENTION - 60;
    let unlisted = target(B);
    for (id, attempt) in [
        ("accepted", Attempt::Accepted),
        ("reserved", Attempt::Reserved),
        ("uncertain", Attempt::Uncertain),
        ("delivered", Attempt::Delivered),
        ("refused", Attempt::Refused),
        ("retry", Attempt::Retry),
        ("cached", Attempt::Delivered),
    ] {
        journal.set_agent(&principal, &unlisted, id, attempt, old);
    }
    let cached = HashSet::from([(scope, unlisted.clone(), "cached".to_string())]);
    journal.prune(unix_now(), &cached);
    let kept = |id: &str| journal.agent_receipt(&principal, &unlisted, id).is_some();
    assert!(kept("accepted") && kept("reserved") && kept("uncertain"));
    assert!(!kept("delivered") && !kept("refused") && !kept("retry"));
    assert!(kept("cached"));
}

#[tokio::test]
async fn a_settled_receipt_whose_row_is_still_cached_survives_failing_lists() {
    let (_dir, api, advisories) = setup();
    let t = target(A);
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(&[t.clone()], now).await;
    let principal = Tokens.principal().unwrap();
    advisories.urgent.lock().unwrap().set_agent(
        &principal,
        &t,
        "p1",
        Attempt::Refused,
        unix_now() - RETENTION - 60,
    );
    // The row stays cached (lists would fail); a pass prunes with the cache.
    let sink = Sink::default();
    advisories.deliver_urgent(&sink, now).await;
    assert_eq!(receipt(&advisories, &t), Some(Attempt::Refused));
    assert!(sink.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_delivered_that_could_not_be_saved_is_acked_once_after_a_later_save() {
    let (dir, api, advisories) = setup();
    let t = target(A);
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(&[t.clone()], now).await;
    let sink = Sink::default();
    let principal = Tokens.principal().unwrap();
    // The reservation saves; the Delivered record does not.
    struct FailAfterReserve(Sink, std::path::PathBuf);
    impl Delivery for FailAfterReserve {
        fn inject<'a>(&'a self, target: &'a Target, id: &'a str, text: &'a str) -> BoxFuture<'a, InjectOutcome> {
            std::fs::remove_file(self.1.join(FILE)).unwrap();
            std::fs::create_dir(self.1.join(FILE)).unwrap();
            self.0.inject(target, id, text)
        }
        fn notifications_available(&self) -> bool {
            false
        }
        fn notify(&self, _: &str, _: &str) {}
    }
    let failing = FailAfterReserve(sink, dir.path().to_path_buf());
    advisories.deliver_urgent(&failing, now).await;
    advisories.send_shown(now).await;
    assert!(api.acked.lock().unwrap().is_empty());
    assert_eq!(
        advisories.urgent.lock().unwrap().agent_receipt(&principal, &t, "p1").map(|r| r.0),
        Some(Attempt::Delivered)
    );
    // Storage works again: the next pass saves, then shows; exactly one ack.
    std::fs::remove_dir(dir.path().join(FILE)).unwrap();
    let next = Sink::default();
    advisories.deliver_urgent(&next, now).await;
    advisories.send_shown(now).await;
    advisories.deliver_urgent(&next, now).await;
    advisories.send_shown(now).await;
    assert_eq!(*api.acked.lock().unwrap(), vec!["p1"]);
    assert!(next.calls.lock().unwrap().is_empty());
    assert_eq!(failing.0.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn while_storage_is_blocked_the_turn_end_keeps_back_urgent_agent_notes_only() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FILE), b"broken").unwrap();
    let api = Arc::new(Api::default());
    let advisories = Advisories::new(dir.path(), Arc::new(Tokens), api.clone());
    advisories.set_enabled(true).unwrap();
    let t = target(A);
    let now = Instant::now();
    let mut low = p1("low");
    low.priority = Some(3);
    low.urgency = "low";
    let mut ineligible = p1("ineligible");
    ineligible.notify_eligible = false;
    let mut by_dm = p1("by-dm");
    by_dm.dm_delivered = true;
    api.rows
        .lock()
        .unwrap()
        .insert(t.clone(), vec![p1("p1"), low, ineligible, by_dm]);
    advisories.poll(&[t.clone()], now).await;
    let sink = Sink::default();
    advisories.deliver_urgent(&sink, now).await;
    assert!(sink.calls.lock().unwrap().is_empty());
    // A row already delivered by DM is not one the urgent path would attempt.
    assert_eq!(advisories.take_at(&t, now).unwrap().ids, vec!["low", "ineligible", "by-dm"]);
    // Relaunch: still blocked; the P1 note is still kept back.
    let relaunched = Advisories::new(dir.path(), Arc::new(Tokens), api.clone());
    relaunched.poll(&[t.clone()], now).await;
    let ids = relaunched.take_at(&t, now).map(|n| n.ids).unwrap_or_default();
    assert!(!ids.contains(&"p1".to_string()), "{ids:?}");
}

#[test]
fn only_the_cores_delivered_settles_a_note() {
    use agent_relay_node::relay::local_notices::NoticeObservation as Core;
    assert_eq!(observation_of(Core::Delivered), Observation::Delivered);
    assert_eq!(observation_of(Core::Pending("queued".into())), Observation::Pending);
    assert_eq!(observation_of(Core::InDoubt("grace".into())), Observation::InDoubt);
    assert_eq!(observation_of(Core::Unsupported), Observation::Unsupported);
    assert_eq!(observation_of(Core::Fenced("stopping".into())), Observation::Fenced);
}

#[tokio::test]
async fn local_observe_without_a_live_registration_is_fenced() {
    use agent_relay_node::relay::daemon::{Config, Core, Plugins};
    let home = tempfile::tempdir().unwrap();
    let config = Config {
        paths: agent_relay_node::relay::paths::CorePaths::for_socket(home.path().join("core.sock")).unwrap(),
        home: home.path().into(),
        app_data_dir: home.path().join("app-data"),
        env: Default::default(),
        version: "test".into(),
        host_name: "Test Desk.local".into(),
        rediscover_every: Duration::from_secs(3600),
        push_timing: Default::default(),
        delivery_timing: Default::default(),
    };
    let (core, _signals) = Core::with_plugins(config, Plugins::default());
    let local = Local(core);
    assert_eq!(local.observe(&target(A), "p1").await, Observation::Fenced);
    assert!(!local.reaches(&target(A)));
}

#[tokio::test]
async fn a_held_note_is_observed_even_when_now_ineligible_or_unreachable() {
    struct Unreached(Sink);
    impl Delivery for Unreached {
        fn inject<'a>(&'a self, t: &'a Target, id: &'a str, text: &'a str) -> BoxFuture<'a, InjectOutcome> {
            self.0.inject(t, id, text)
        }
        fn notifications_available(&self) -> bool {
            false
        }
        fn notify(&self, _: &str, _: &str) {}
        fn reaches(&self, _: &Target) -> bool {
            false
        }
        fn observe<'a>(&'a self, t: &'a Target, id: &'a str) -> BoxFuture<'a, Observation> {
            self.0.observe(t, id)
        }
    }
    // Now notify-ineligible.
    let (_dir, api, advisories, sink, t, now) = accepted().await;
    let mut row = p1("p1");
    row.notify_eligible = false;
    api.rows.lock().unwrap().insert(t.clone(), vec![row]);
    advisories.poll(&[t.clone()], now).await;
    *sink.observation.lock().unwrap() = Some(Observation::Delivered);
    advisories.deliver_urgent(&sink, now).await;
    advisories.send_shown(now).await;
    assert_eq!(sink.observes.load(Ordering::SeqCst), 1);
    assert_eq!(*api.acked.lock().unwrap(), vec!["p1"]);
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
    // Now unreachable.
    let (_dir, api, advisories, sink, _t, now) = accepted().await;
    *sink.observation.lock().unwrap() = Some(Observation::Delivered);
    let unreached = Unreached(sink);
    advisories.deliver_urgent(&unreached, now).await;
    advisories.send_shown(now).await;
    assert_eq!(unreached.0.observes.load(Ordering::SeqCst), 1);
    assert_eq!(*api.acked.lock().unwrap(), vec!["p1"]);
}

#[tokio::test]
async fn the_minute_throttle_holds_at_59_seconds_and_lets_one_through_past_60() {
    let (_dir, _api, advisories, sink, _t, now) = accepted().await;
    *sink.observation.lock().unwrap() = Some(Observation::Pending);
    advisories.deliver_urgent(&sink, now).await;
    assert_eq!(sink.observes.load(Ordering::SeqCst), 1);
    let age = |by: u64| {
        for at in advisories.urgent.lock().unwrap().observed.values_mut() {
            *at = at.saturating_sub(by);
        }
    };
    age(59);
    advisories.deliver_urgent(&sink, now).await;
    assert_eq!(sink.observes.load(Ordering::SeqCst), 1);
    age(2);
    advisories.deliver_urgent(&sink, now).await;
    advisories.deliver_urgent(&sink, now).await;
    assert_eq!(sink.observes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_blocked_journal_keeps_its_fault_status_with_notifications_off() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FILE), b"broken").unwrap();
    let api = Arc::new(Api::default());
    let advisories = Advisories::new(dir.path(), Arc::new(Tokens), api.clone());
    advisories.set_enabled(true).unwrap();
    assert!(!advisories.notifications_enabled());
    let t = target(A);
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(&[t], now).await;
    advisories.deliver_urgent(&Sink::default(), now).await;
    assert!(advisories.urgent_status().contains("damaged or from another version"));
}
