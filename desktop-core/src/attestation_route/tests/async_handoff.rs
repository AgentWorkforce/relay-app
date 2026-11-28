//! The async handoff (`claude_attestation_async_handoff`): the hook is
//! answered at once and the proof, or a marker, reaches Cloud behind it.
use super::*;

const ALL: &str = r#"{"claude_session_attestation":1,"claude_attestation_handoff":1,"claude_attestation_async_handoff":1}"#;
const SEND: &str = "mcp__agent-relay__send_relay_message";

fn posts(f: &Fixture) -> Vec<Value> {
    let calls = f.cloud.calls.lock().unwrap();
    calls
        .iter()
        .filter(|(method, path, _)| {
            method == "POST"
                && path
                    == &format!("/workspaces/{WORKSPACE}/messaging/desktop-identities/attestations")
        })
        .map(|(_, _, body)| body.clone().unwrap())
        .collect()
}

/// Waits (in real time) for a handoff POST `matches` picks out.
async fn posted(f: &Fixture, matches: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..100 {
        if let Some(body) = posts(f).into_iter().find(|body| matches(body)) {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    panic!("no such handoff POST: {:?}", posts(f));
}

async fn declared_fixture() -> Fixture {
    let f = fixture();
    f.negotiate(serde_json::from_str(ALL).unwrap()).await;
    Identities::new(f.core.clone())
        .sync_desktop_identities()
        .await;
    Identities::new(f.core.clone())
        .maintain_async_handoff()
        .await;
    f
}

#[tokio::test]
async fn the_login_declares_only_when_cloud_offers_it_and_renews_it_half_hourly() {
    let f = fixture();
    f.negotiate(json!({"claude_session_attestation": 1, "claude_attestation_handoff": 1}))
        .await;
    let identities = Identities::new(f.core.clone());
    identities.maintain_async_handoff().await;
    assert!(
        posts(&f).is_empty(),
        "nothing is declared to a Cloud without it"
    );
    assert!(!identities.claude_async_handoff());

    f.negotiate(serde_json::from_str(ALL).unwrap()).await;
    identities.maintain_async_handoff().await;
    assert_eq!(posts(&f), vec![json!({"async_handoff": true})]);
    assert!(identities.claude_async_handoff());
    identities.maintain_async_handoff().await;
    assert_eq!(posts(&f).len(), 1, "a recent declaration is not sent again");

    identities.age_async_declaration(Duration::from_secs(31 * 60));
    identities.maintain_async_handoff().await;
    assert_eq!(posts(&f).len(), 2);

    // A declaration Cloud did not take leaves the handoff synchronous.
    let f = fixture();
    f.negotiate(serde_json::from_str(ALL).unwrap()).await;
    *f.cloud.failure.lock().unwrap() = Some(CloudError::Status {
        status: 400,
        message: None,
    });
    let identities = Identities::new(f.core.clone());
    identities.maintain_async_handoff().await;
    assert!(!identities.claude_async_handoff());
    assert!(!identities.claude_markers_expected());
}

#[tokio::test]
async fn the_hook_is_answered_before_a_slow_cloud_takes_the_proof() {
    let f = declared_fixture().await;
    f.cloud
        .post_delay_ms
        .store(1_500, std::sync::atomic::Ordering::SeqCst);
    let answer = tokio::time::timeout(
        Duration::from_millis(500),
        f.ask(IN_REGISTERED, call(REGISTERED, SEND)),
    )
    .await
    .expect("the hook does not wait for Cloud")
    .unwrap();
    assert_eq!(answer["handoff"], true);
    assert_eq!(answer["pending"], true);
    assert!(answer["attestation"].is_null());

    let body = posted(&f, |body| body.get("attestation").is_some()).await;
    let proof = body["attestation"].as_str().unwrap();
    let claims = decode(proof.split('.').nth(1).unwrap());
    assert_eq!(claims["tuid"], "toolu_01ABC");
    assert_eq!(claims["sid"], REGISTERED.to_ascii_lowercase());
    assert_eq!(claims["tool"], "send_relay_message");
}

#[tokio::test]
async fn a_stale_declaration_returns_to_the_synchronous_handoff() {
    let f = declared_fixture().await;
    let identities = Identities::new(f.core.clone());
    identities.age_async_declaration(Duration::from_secs(91 * 60));
    let answer = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
    assert_eq!(answer["handoff"], true);
    assert!(answer.get("pending").is_none());
    assert!(
        answer["attestation"].is_string(),
        "the proof was handed off before the answer"
    );
}

#[tokio::test]
async fn a_call_this_chat_cannot_sign_gets_a_marker_while_cloud_may_wait() {
    let f = declared_fixture().await;
    // A Claude chat this desktop has not registered.
    let refused = f
        .ask(IN_UNREGISTERED, call(UNREGISTERED, SEND))
        .await
        .unwrap_err();
    assert_eq!(refused.code, OutboundError::not_registered().code);
    let marker = posted(&f, |body| body.get("unattested").is_some()).await;
    assert_eq!(marker, json!({"unattested": "toolu_01ABC"}));

    // Another process naming a chat it does not run in marks nothing.
    let before = posts(&f).len();
    let forged = f
        .ask(IN_UNREGISTERED, call(REGISTERED, SEND))
        .await
        .unwrap_err();
    assert_eq!(forged.code, OutboundError::not_a_session().code);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(posts(&f).len(), before);
}

#[tokio::test]
async fn without_a_declaration_no_marker_is_sent() {
    let f = fixture();
    f.negotiate(serde_json::from_str(ALL).unwrap()).await;
    let refused = f
        .ask(IN_UNREGISTERED, call(UNREGISTERED, SEND))
        .await
        .unwrap_err();
    assert_eq!(refused.code, OutboundError::not_registered().code);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(posts(&f).is_empty());
}

#[tokio::test]
async fn a_background_handoff_retries_an_unreachable_cloud() {
    let f = declared_fixture().await;
    f.cloud
        .offline
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let answer = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
    assert_eq!(answer["pending"], true);
    tokio::time::sleep(Duration::from_millis(400)).await;
    let tries = posts(&f)
        .iter()
        .filter(|body| body.get("attestation").is_some())
        .count();
    assert!(tries >= 1);
    f.cloud
        .offline
        .store(false, std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(600)).await;
    let later = posts(&f)
        .iter()
        .filter(|body| body.get("attestation").is_some())
        .count();
    assert!(later > tries, "the proof is sent again once Cloud answers");
    let settled = later;
    tokio::time::sleep(Duration::from_millis(600)).await;
    let after = posts(&f)
        .iter()
        .filter(|body| body.get("attestation").is_some())
        .count();
    assert_eq!(after, settled, "an accepted proof is not sent again");
}

#[tokio::test]
async fn a_refused_background_proof_is_followed_by_a_marker_not_retried() {
    let f = declared_fixture().await;
    *f.cloud.failure.lock().unwrap() = Some(CloudError::Status {
        status: 403,
        message: None,
    });
    let answer = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
    assert_eq!(answer["pending"], true);
    let marker = posted(&f, |body| body.get("unattested").is_some()).await;
    assert_eq!(marker, json!({"unattested": "toolu_01ABC"}));
    tokio::time::sleep(Duration::from_millis(400)).await;
    let proofs = posts(&f)
        .iter()
        .filter(|body| body.get("attestation").is_some())
        .count();
    assert_eq!(proofs, 1, "a refused proof is not sent again");
}

mod guards;
