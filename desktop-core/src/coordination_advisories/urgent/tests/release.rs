//! Held notes past their time go to the turn end once, labelled; a blocked
//! journal is loaded again on each pass; a full one evicts stale holds first.

use super::*;
use crate::coordination_advisories::delivery::MAY_REPEAT;
use crate::coordination_advisories::urgent::release::UNOBSERVED_GRACE;
use agent_relay_node::relay::daemon::TurnEndAdvisories;
use observe::{accepted, age_observation, receipt};

/// Backdates `p1`'s held receipt for `A` by `age` seconds.
fn held_for(advisories: &Advisories, t: &Target, attempt: Attempt, age: u64) {
    let principal = Tokens.principal().unwrap();
    advisories.urgent.lock().unwrap().set_agent(
        &principal,
        t,
        &key(t, "p1"),
        attempt,
        unix_now() - age,
    );
}

/// One observed pass, as if a minute had passed since the last one.
async fn observe_once(advisories: &Advisories, sink: &Sink, seen: Observation, now: Instant) {
    *sink.observation.lock().unwrap() = Some(seen);
    age_observation(advisories);
    advisories.deliver_urgent(sink, now).await;
}

#[tokio::test]
async fn pending_in_doubt_and_fenced_hold_until_the_cap_and_then_release() {
    let held = [Attempt::Accepted, Attempt::Reserved, Attempt::Uncertain];
    for (seen, attempt) in [
        Observation::Pending,
        Observation::InDoubt,
        Observation::Fenced,
    ]
    .into_iter()
    .flat_map(|seen| held.clone().map(|attempt| (seen.clone(), attempt)))
    {
        let (_dir, api, advisories, sink, t, now) = accepted().await;
        // Inside the Unsupported grace, past it, and a minute short of the cap.
        for age in [60, UNOBSERVED_GRACE + 60, ACCEPTED_OWNED_FOR - 60] {
            held_for(&advisories, &t, attempt.clone(), age);
            observe_once(&advisories, &sink, seen.clone(), now).await;
            assert_eq!(
                receipt(&advisories, &t),
                Some(attempt.clone()),
                "{seen:?} {attempt:?} {age}"
            );
            assert!(
                advisories.take_at(&t, now).is_none(),
                "{seen:?} {attempt:?} {age}"
            );
        }
        held_for(&advisories, &t, attempt.clone(), ACCEPTED_OWNED_FOR);
        observe_once(&advisories, &sink, seen.clone(), now).await;
        assert_eq!(
            receipt(&advisories, &t),
            Some(Attempt::Released),
            "{seen:?}"
        );
        assert_eq!(
            advisories.take_at(&t, now).unwrap().ids,
            keys(&t, &["p1"]),
            "{seen:?}"
        );
        assert_eq!(sink.calls.lock().unwrap().len(), 1, "{seen:?}");
        assert!(api.acked.lock().unwrap().is_empty(), "{seen:?}");
    }
}

#[tokio::test]
async fn unsupported_releases_after_the_grace_and_not_before() {
    let (_dir, _api, advisories, sink, t, now) = accepted().await;
    held_for(&advisories, &t, Attempt::Accepted, UNOBSERVED_GRACE - 60);
    observe_once(&advisories, &sink, Observation::Unsupported, now).await;
    assert_eq!(receipt(&advisories, &t), Some(Attempt::Accepted));
    assert!(advisories.take_at(&t, now).is_none());
    held_for(&advisories, &t, Attempt::Accepted, UNOBSERVED_GRACE);
    observe_once(&advisories, &sink, Observation::Unsupported, now).await;
    assert_eq!(receipt(&advisories, &t), Some(Attempt::Released));
}

#[tokio::test]
async fn reserved_and_uncertain_receipts_release_the_same_way() {
    for attempt in [Attempt::Reserved, Attempt::Uncertain] {
        let (_dir, _api, advisories, sink, t, now) = accepted().await;
        held_for(&advisories, &t, attempt.clone(), UNOBSERVED_GRACE);
        observe_once(&advisories, &sink, Observation::Unsupported, now).await;
        assert_eq!(
            receipt(&advisories, &t),
            Some(Attempt::Released),
            "{attempt:?}"
        );
        assert_eq!(sink.calls.lock().unwrap().len(), 1, "{attempt:?}");
    }
}

#[tokio::test]
async fn a_backwards_clock_never_releases() {
    let (_dir, _api, advisories, sink, t, now) = accepted().await;
    let principal = Tokens.principal().unwrap();
    // Recorded "in the future": the clock went back since.
    advisories.urgent.lock().unwrap().set_agent(
        &principal,
        &t,
        &key(&t, "p1"),
        Attempt::Accepted,
        unix_now() + ACCEPTED_OWNED_FOR,
    );
    observe_once(&advisories, &sink, Observation::Unsupported, now).await;
    observe_once(&advisories, &sink, Observation::Pending, now).await;
    assert_eq!(receipt(&advisories, &t), Some(Attempt::Accepted));
}

#[tokio::test]
async fn a_released_note_is_handed_out_once_labelled_never_injected_and_acked_once() {
    let (_dir, api, advisories, sink, t, now) = accepted().await;
    held_for(&advisories, &t, Attempt::Accepted, ACCEPTED_OWNED_FOR);
    observe_once(&advisories, &sink, Observation::Pending, now).await;
    let note = advisories.take_at(&t, now).unwrap();
    assert_eq!(note.ids, keys(&t, &["p1"]));
    assert!(
        note.text.contains(&format!("advisory p1) {MAY_REPEAT}")),
        "{}",
        note.text
    );
    assert!(advisories.take_at(&t, now).is_none());
    advisories.shown(&t, &note.ids);
    advisories.send_shown(now).await;
    // Later passes and polls: no second injection, observation or ack.
    let observed = sink.observes.load(Ordering::SeqCst);
    for _ in 0..2 {
        advisories.poll(std::slice::from_ref(&t), now).await;
        observe_once(&advisories, &sink, Observation::Delivered, now).await;
        advisories.send_shown(now).await;
        assert!(advisories.take_at(&t, now).is_none());
    }
    assert_eq!(*api.acked.lock().unwrap(), vec!["p1"]);
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
    assert_eq!(sink.observes.load(Ordering::SeqCst), observed);
    assert_eq!(receipt(&advisories, &t), Some(Attempt::Released));
}

#[tokio::test]
async fn a_release_survives_a_restart_and_is_still_never_injected() {
    let (dir, api, advisories, sink, t, now) = accepted().await;
    held_for(&advisories, &t, Attempt::Accepted, ACCEPTED_OWNED_FOR);
    observe_once(&advisories, &sink, Observation::Fenced, now).await;
    let restarted = displayed(Advisories::new(dir.path(), Arc::new(Tokens), api.clone()));
    restarted.poll(std::slice::from_ref(&t), now).await;
    let next = Sink::default();
    *next.outcome.lock().unwrap() = Some(InjectOutcome::Delivered);
    restarted.deliver_urgent(&next, now).await;
    assert!(next.calls.lock().unwrap().is_empty());
    assert_eq!(next.observes.load(Ordering::SeqCst), 0);
    let note = restarted.take_at(&t, now).unwrap();
    assert!(note.text.contains(MAY_REPEAT));
}

#[tokio::test]
async fn a_release_that_cannot_be_saved_keeps_the_note_held() {
    let (dir, _api, advisories, sink, t, now) = accepted().await;
    held_for(&advisories, &t, Attempt::Accepted, ACCEPTED_OWNED_FOR);
    let path = dir.path().join(FILE);
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    observe_once(&advisories, &sink, Observation::Pending, now).await;
    assert_eq!(receipt(&advisories, &t), Some(Attempt::Accepted));
    assert!(advisories.take_at(&t, now).is_none());
    assert!(advisories.urgent_status().contains("could not be saved"));
    // Storage works again: the next observation releases it.
    std::fs::remove_dir(&path).unwrap();
    observe_once(&advisories, &sink, Observation::Pending, now).await;
    assert_eq!(receipt(&advisories, &t), Some(Attempt::Released));
    assert_eq!(sink.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn nothing_is_released_while_the_journal_is_blocked() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FILE), b"broken").unwrap();
    let api = Arc::new(Api::default());
    let advisories = displayed(Advisories::new(dir.path(), Arc::new(Tokens), api.clone()));
    advisories.set_enabled(true).unwrap();
    let t = target(A);
    let now = Instant::now();
    api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
    advisories.poll(std::slice::from_ref(&t), now).await;
    // A held receipt in memory, as if one had slipped in: never released.
    held_for(&advisories, &t, Attempt::Accepted, ACCEPTED_OWNED_FOR);
    let sink = Sink::default();
    observe_once(&advisories, &sink, Observation::Pending, now).await;
    assert_eq!(receipt(&advisories, &t), Some(Attempt::Accepted));
    assert!(advisories.take_at(&t, now).is_none());
    assert!(sink.calls.lock().unwrap().is_empty());
    assert_eq!(std::fs::read(dir.path().join(FILE)).unwrap(), b"broken");
}

#[test]
fn at_capacity_stale_unlisted_holds_go_oldest_first_and_cached_ones_stay() {
    let principal = Tokens.principal().unwrap();
    let scope = receipts::Scope::from(&principal);
    let dir = tempfile::tempdir().unwrap();
    let mut journal = Journal::load(dir.path());
    let now = unix_now();
    let stale = now - RETENTION;
    let t = target(B);
    // Oldest of all, but its row is still cached: left for release.
    journal.set_agent(&principal, &t, "cached", Attempt::Accepted, stale - 100);
    // Past the cap but inside the server's retention, uncached: its row may
    // come back, so it stays.
    journal.set_agent(
        &principal,
        &t,
        "returning",
        Attempt::Accepted,
        now - ACCEPTED_OWNED_FOR - 60,
    );
    for i in 0..15 {
        journal.set_agent(
            &principal,
            &t,
            &format!("stale-{i}"),
            Attempt::Accepted,
            stale - i,
        );
    }
    for i in 0..MAX_RECORDS - 17 {
        journal.set_agent(
            &principal,
            &t,
            &format!("fresh-{i}"),
            Attempt::Accepted,
            now,
        );
    }
    let cached = HashSet::from([(scope, t.clone(), "cached".to_string())]);
    journal.prune(now, &cached);
    let kept = |id: &str| journal.agent_receipt(&principal, &t, id).is_some();
    assert_eq!(journal.saved.agents.len(), MAX_RECORDS - MAX_ROWS);
    assert!(kept("cached") && kept("returning") && kept("fresh-0"));
    // stale-14 is the oldest of the evictable ones; the 10 oldest go.
    assert!((5..15).all(|i| !kept(&format!("stale-{i}"))));
    assert!((0..5).all(|i| kept(&format!("stale-{i}"))));
    assert!(journal.save(dir.path()));
}

#[test]
fn below_capacity_or_with_nothing_stale_nothing_unsettled_is_evicted_and_full_fails_closed() {
    let principal = Tokens.principal().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut journal = Journal::load(dir.path());
    let now = unix_now();
    let t = target(B);
    journal.set_agent(&principal, &t, "stale", Attempt::Accepted, now - RETENTION);
    // Past the cap, uncached, but inside the server's retention: its row may
    // come back, so it is never evicted, even over capacity.
    journal.set_agent(
        &principal,
        &t,
        "returning",
        Attempt::Accepted,
        now - RETENTION + 60,
    );
    journal.prune(now, &HashSet::new());
    assert!(journal.agent_receipt(&principal, &t, "stale").is_some());
    for i in 0..MAX_RECORDS {
        journal.set_agent(
            &principal,
            &t,
            &format!("fresh-{i}"),
            Attempt::Uncertain,
            now - 60,
        );
    }
    journal.prune(now, &HashSet::new());
    // Only the one stale hold could go: still over, so the save fails closed.
    assert!(journal.agent_receipt(&principal, &t, "stale").is_none());
    assert!(journal.agent_receipt(&principal, &t, "returning").is_some());
    assert_eq!(journal.saved.agents.len(), MAX_RECORDS + 1);
    journal.set_agent(&principal, &t, "one-more", Attempt::Reserved, now);
    assert!(!journal.save(dir.path()));
    assert!(journal.status.contains("full"));
    assert!(!dir.path().join(FILE).exists());
}

#[cfg(unix)]
#[tokio::test]
async fn an_io_fault_that_clears_unblocks_on_the_next_pass_with_its_receipts() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, api, _advisories, _sink, t, now) = accepted().await;
    let path = dir.path().join(FILE);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&path).is_ok() {
        // Running as a user that ignores file modes (root): nothing to test.
        return;
    }
    let restarted = displayed(Advisories::new(dir.path(), Arc::new(Tokens), api.clone()));
    restarted.set_enabled(true).unwrap();
    restarted.poll(std::slice::from_ref(&t), now).await;
    let next = Sink::default();
    restarted.deliver_urgent(&next, now).await;
    let status = restarted.urgent_status();
    assert!(
        status.contains("cannot be read") && status.contains(&path.display().to_string()),
        "{status}"
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    restarted.deliver_urgent(&next, now).await;
    assert!(!restarted.urgent.lock().unwrap().blocked());
    assert_eq!(receipt(&restarted, &t), Some(Attempt::Accepted));
    assert!(next.calls.lock().unwrap().is_empty());
    assert!(restarted.take_at(&t, now).is_none());
}

#[tokio::test]
async fn damaged_and_newer_files_keep_blocking_untouched_until_moved_aside() {
    for (body, fault) in [
        (&b"broken"[..], "move it aside"),
        (
            &br#"{"version":3,"agents":[],"humans":[]}"#[..],
            "Update Agent Relay and leave the file in place",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        std::fs::write(&path, body).unwrap();
        let api = Arc::new(Api::default());
        let advisories = displayed(Advisories::new(dir.path(), Arc::new(Tokens), api.clone()));
        advisories.set_enabled(true).unwrap();
        advisories.set_notifications_enabled(true).unwrap();
        let t = target(A);
        let now = Instant::now();
        api.rows.lock().unwrap().insert(t.clone(), vec![p1("p1")]);
        advisories.poll(std::slice::from_ref(&t), now).await;
        let sink = Sink::default();
        for _pass in 0..2 {
            advisories.deliver_urgent(&sink, now).await;
            let status = advisories.urgent_status();
            assert!(
                status.contains(fault) && status.contains(&path.display().to_string()),
                "{status}"
            );
            assert_eq!(std::fs::read(&path).unwrap(), body);
            assert!(sink.calls.lock().unwrap().is_empty());
            assert!(sink.notices.lock().unwrap().is_empty());
        }
        // The person moves it aside: the next pass starts fresh.
        std::fs::rename(&path, dir.path().join("aside.json")).unwrap();
        advisories.deliver_urgent(&sink, now).await;
        assert_eq!(sink.calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn while_blocked_the_turn_end_keeps_back_urgent_notes_only_where_a_notice_reaches() {
    struct OnlyA(Sink);
    impl Delivery for OnlyA {
        fn inject<'a>(
            &'a self,
            t: &'a Target,
            id: &'a str,
            text: &'a str,
        ) -> BoxFuture<'a, InjectOutcome> {
            self.0.inject(t, id, text)
        }
        fn notifications_available(&self) -> bool {
            false
        }
        fn notify(&self, _: &str, _: &str) {}
        fn reaches(&self, t: &Target) -> bool {
            t.id == A
        }
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(FILE), b"broken").unwrap();
    let api = Arc::new(Api::default());
    let advisories = displayed(Advisories::new(dir.path(), Arc::new(Tokens), api.clone()));
    advisories.set_enabled(true).unwrap();
    let (a, b) = (target(A), target(B));
    let now = Instant::now();
    api.rows.lock().unwrap().insert(a.clone(), vec![p1("on-a")]);
    api.rows.lock().unwrap().insert(b.clone(), vec![p1("on-b")]);
    advisories.poll(&[a.clone(), b.clone()], now).await;
    // Before any urgent pass, reach is unknown: both are kept back.
    assert!(advisories.take_at(&b, now).is_none());
    let delivery = OnlyA(Sink::default());
    advisories.deliver_urgent(&delivery, now).await;
    assert!(delivery.0.calls.lock().unwrap().is_empty());
    assert!(advisories.take_at(&a, now).is_none());
    let note = advisories.take_at(&b, now).unwrap();
    assert_eq!(note.ids, keys(&target(B), &["on-b"]));
    // The unreadable journal may record that a provider holds it.
    assert!(note.text.contains(MAY_REPEAT));
}
