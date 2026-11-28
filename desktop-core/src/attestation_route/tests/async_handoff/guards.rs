//! What the async handoff must never do: let a join run without its proof,
//! send an old account's proof or marker, or outlive its windows.
use super::*;
use std::sync::atomic::Ordering;

const JOIN: &str = "mcp__agent-relay__join_relay";

/// Handoff POSTs to any workspace, `{"attestation"}` and `{"unattested"}` alike.
fn handoffs(f: &Fixture) -> usize {
    let calls = f.cloud.calls.lock().unwrap();
    calls
        .iter()
        .filter(|(method, path, body)| {
            method == "POST"
                && path.ends_with("/messaging/desktop-identities/attestations")
                && body
                    .as_ref()
                    .is_some_and(|body| body.get("async_handoff").is_none())
        })
        .count()
}

fn declarations(f: &Fixture) -> usize {
    posts(f)
        .iter()
        .filter(|body| body.get("async_handoff").is_some())
        .count()
}

fn markers(f: &Fixture) -> usize {
    posts(f)
        .iter()
        .filter(|body| body.get("unattested").is_some())
        .count()
}

/// Waits for a first background attempt, then lets the one in flight end.
async fn retrying(f: &Fixture) {
    for _ in 0..100 {
        if handoffs(f) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(handoffs(f) > 0, "the background handoff started");
}

/// Asserts no further handoff POST goes out, past the retry pause.
async fn settled(f: &Fixture, why: &str) {
    tokio::time::sleep(Duration::from_millis(300)).await;
    let before = handoffs(f);
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(handoffs(f), before, "{why}");
}

#[tokio::test]
async fn a_join_keeps_the_synchronous_handoff_while_calls_go_async() {
    let f = declared_fixture().await;
    let mut named = call(REGISTERED, JOIN);
    named["registration"] = json!({"name": "claude-chat"});
    for join in [named, call(REGISTERED, JOIN)] {
        let answer = f.ask(IN_REGISTERED, join).await.unwrap();
        assert!(answer.get("pending").is_none(), "a join is never pending");
        assert_eq!(answer["handoff"], true);
        assert!(
            answer["attestation"].is_string(),
            "Cloud took the join's proof before the answer"
        );
    }
    let proofs = posts(&f)
        .iter()
        .filter(|body| body.get("attestation").is_some())
        .count();
    assert_eq!(proofs, 2);
    // Other calls still go async.
    let answer = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
    assert_eq!(answer["pending"], true);
}

#[tokio::test]
async fn a_join_whose_proof_fails_is_refused_rather_than_run_unattested() {
    let f = declared_fixture().await;
    *f.cloud.failure.lock().unwrap() = Some(CloudError::Status {
        status: 503,
        message: None,
    });
    let mut named = call(REGISTERED, JOIN);
    named["registration"] = json!({"name": "claude-chat"});
    for join in [named, call(REGISTERED, JOIN)] {
        let refused = f.ask(IN_REGISTERED, join).await.unwrap_err();
        assert_eq!(refused.code, "handoff_failed");
    }
}

#[tokio::test]
async fn a_background_proof_is_dropped_after_an_account_change() {
    let f = declared_fixture().await;
    f.cloud.offline.store(true, Ordering::SeqCst);
    let answer = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
    assert_eq!(answer["pending"], true);
    retrying(&f).await;
    *f.cloud.account_override.lock().unwrap() = Some(CloudAccount {
        site_url: "https://agentrelay.com".into(),
        workspace_id: "6c1f3a52-0d4e-4b8a-9f27-3e5d8c7b1a90".into(),
        user_id: Some("user-2".into()),
    });
    f.cloud.offline.store(false, Ordering::SeqCst);
    settled(&f, "the old account's proof is not sent to the new one").await;
}

#[tokio::test]
async fn a_background_proof_is_dropped_after_a_generation_change() {
    let f = declared_fixture().await;
    f.cloud.offline.store(true, Ordering::SeqCst);
    let answer = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
    assert_eq!(answer["pending"], true);
    retrying(&f).await;
    f.cloud.generation.fetch_add(1, Ordering::SeqCst);
    f.cloud.offline.store(false, Ordering::SeqCst);
    settled(&f, "an old generation's proof is not retried").await;
}

#[tokio::test]
async fn a_marker_is_dropped_after_a_generation_change() {
    let f = declared_fixture().await;
    f.cloud.offline.store(true, Ordering::SeqCst);
    let refused = f
        .ask(IN_UNREGISTERED, call(UNREGISTERED, SEND))
        .await
        .unwrap_err();
    assert_eq!(refused.code, OutboundError::not_registered().code);
    retrying(&f).await;
    f.cloud.generation.fetch_add(1, Ordering::SeqCst);
    f.cloud.offline.store(false, Ordering::SeqCst);
    settled(&f, "an old generation's marker is not retried").await;
}

#[tokio::test]
async fn a_signing_failure_after_pending_sends_a_marker() {
    let f = fixture();
    f.negotiate(serde_json::from_str(ALL).unwrap()).await;
    // Cloud skips this chat's identity, so the background sync cannot make
    // it current and no proof is signed.
    *f.cloud.put_response.lock().unwrap() = Some(json!({"synced": 1, "skipped": [
        {"agent_id": "agent-claude-chat", "reason": "unverified"}
    ]}));
    Identities::new(f.core.clone())
        .maintain_async_handoff()
        .await;
    let answer = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
    assert_eq!(answer["pending"], true, "the hook was answered first");
    let marker = posted(&f, |body| body.get("unattested").is_some()).await;
    assert_eq!(marker, json!({"unattested": "toolu_01ABC"}));
    assert!(posts(&f)
        .iter()
        .all(|body| body.get("attestation").is_none()));
}

#[tokio::test]
async fn background_retries_stop_inside_clouds_ten_second_wait() {
    assert!(crate::attestation_route::BACKGROUND_BUDGET < Duration::from_secs(10));
    let f = declared_fixture().await;
    f.cloud.offline.store(true, Ordering::SeqCst);
    let start = std::time::Instant::now();
    let answer = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
    assert_eq!(answer["pending"], true);
    let at = |secs: f64| tokio::time::sleep_until((start + Duration::from_secs_f64(secs)).into());
    at(7.5).await;
    let early = handoffs(&f);
    at(8.7).await;
    assert!(handoffs(&f) > early, "retries continue until 9 seconds");
    at(9.4).await;
    let last = handoffs(&f);
    at(10.2).await;
    assert_eq!(handoffs(&f), last, "no retry after 9 seconds");
}

#[tokio::test]
async fn a_withdrawn_capability_returns_to_the_synchronous_handoff() {
    let f = declared_fixture().await;
    f.negotiate(json!({"claude_session_attestation": 1, "claude_attestation_handoff": 1}))
        .await;
    let identities = Identities::new(f.core.clone());
    assert!(!identities.claude_async_handoff());
    let answer = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
    assert!(answer.get("pending").is_none());
    assert!(answer["attestation"].is_string());

    identities.age_async_declaration(Duration::from_secs(31 * 60));
    identities.maintain_async_handoff().await;
    assert_eq!(declarations(&f), 1, "a withdrawn capability is not renewed");
    // Cloud may still hold the last declaration, so a call that gets no
    // proof is still marked.
    assert!(identities.claude_markers_expected());
    f.ask(IN_UNREGISTERED, call(UNREGISTERED, SEND))
        .await
        .unwrap_err();
    posted(&f, |body| body.get("unattested").is_some()).await;
}

#[tokio::test]
async fn markers_outlast_the_background_handoff_until_135_minutes() {
    let f = declared_fixture().await;
    let identities = Identities::new(f.core.clone());
    identities.age_async_declaration(Duration::from_secs(89 * 60));
    assert!(identities.claude_async_handoff());
    identities.age_async_declaration(Duration::from_secs(11 * 60));
    assert!(
        !identities.claude_async_handoff(),
        "100 minutes: synchronous"
    );
    assert!(
        identities.claude_markers_expected(),
        "100 minutes: still marked"
    );
    f.ask(IN_UNREGISTERED, call(UNREGISTERED, SEND))
        .await
        .unwrap_err();
    posted(&f, |body| body.get("unattested").is_some()).await;
    assert_eq!(markers(&f), 1);

    identities.age_async_declaration(Duration::from_secs(36 * 60));
    assert!(
        !identities.claude_markers_expected(),
        "136 minutes: expired"
    );
    f.ask(IN_UNREGISTERED, call(UNREGISTERED, SEND))
        .await
        .unwrap_err();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(markers(&f), 1, "no marker after 135 minutes");
}

#[tokio::test]
async fn aging_a_declaration_past_the_clocks_origin_does_not_panic() {
    // On Windows an `Instant` starts at boot: moving one back by hours would
    // underflow on a fresh host. Aging adds to the declaration's age instead.
    let f = declared_fixture().await;
    let identities = Identities::new(f.core.clone());
    identities.age_async_declaration(Duration::MAX);
    identities.age_async_declaration(Duration::MAX);
    assert!(!identities.claude_async_handoff());
    assert!(!identities.claude_markers_expected());
}
