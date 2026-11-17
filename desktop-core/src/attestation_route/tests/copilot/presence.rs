//! Exercise convergence and revocation through the verified native hook route.
use super::*;

fn target() -> Target {
    Target::new(SessionKind::Copilot, REGISTERED.to_ascii_lowercase())
}
fn inactive(f: &Fixture) {
    f.core
        .store
        .update(|store| {
            store
                .registrations
                .iter_mut()
                .find(|r| r.target == target())
                .unwrap()
                .missing_since = Some("2026-11-16T00:00:00Z".into());
        })
        .unwrap();
}
fn request() -> Request {
    hook_request(
        "agentrelay-send_relay_message",
        json!({"text":"presence fixture"}),
    )
    .unwrap()
}

#[tokio::test]
async fn native_route_waits_for_reactivation_of_the_same_registration() {
    let f = registered_copilot().await.unwrap();
    inactive(&f);
    let original = f
        .core
        .store
        .read(|store| store.registration_for(&target()).unwrap().clone());
    let input = request();
    let pending = before_with_budget(&f, &input, Duration::from_secs(5));
    tokio::pin!(pending);
    assert!(
        tokio::time::timeout(Duration::from_millis(25), pending.as_mut())
            .await
            .is_err()
    );
    assert!(f.core.store.read(|store| store
        .registration_for(&target())
        .unwrap()
        .missing_since
        .is_some()));
    f.core
        .store
        .update(|store| {
            store
                .registrations
                .iter_mut()
                .find(|r| r.target == target())
                .unwrap()
                .missing_since = None;
        })
        .unwrap();
    let answer = tokio::time::timeout(Duration::from_secs(1), pending)
        .await
        .unwrap()
        .unwrap();
    let ticket = answer["modifiedArgs"][attestation::FIELD].as_str().unwrap();
    let call = f.dispatch.call(ticket, std::time::Instant::now()).unwrap();
    assert_eq!(
        call.owner.registration,
        format!("{}:{}", original.agent_id, original.registered_at)
    );
    assert_eq!(call.sid, target().id);
}

#[tokio::test]
async fn an_account_change_while_the_native_route_waits_refuses_the_call() {
    let f = registered_copilot().await.unwrap();
    inactive(&f);
    let input = request();
    let pending = before_with_budget(&f, &input, Duration::from_secs(5));
    tokio::pin!(pending);
    assert!(
        tokio::time::timeout(Duration::from_millis(25), pending.as_mut())
            .await
            .is_err()
    );
    let mut other = f.cloud.account().unwrap();
    other.user_id = Some("another-fixture-user".into());
    *f.cloud.account_override.lock().unwrap() = Some(other);
    let error = tokio::time::timeout(Duration::from_secs(1), pending)
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, "attestation_unavailable");
    assert!(f.core.store.read(|store| store
        .registration_for(&target())
        .unwrap()
        .missing_since
        .is_some()));
}

#[tokio::test]
async fn removing_the_registration_while_the_native_route_waits_refuses_the_call() {
    let f = registered_copilot().await.unwrap();
    inactive(&f);
    let input = request();
    let pending = before_with_budget(&f, &input, Duration::from_secs(5));
    tokio::pin!(pending);
    assert!(
        tokio::time::timeout(Duration::from_millis(25), pending.as_mut())
            .await
            .is_err()
    );
    f.core
        .store
        .update(|store| store.remove("copilot-chat"))
        .unwrap();
    let error = tokio::time::timeout(Duration::from_secs(1), pending)
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, OutboundError::not_registered().code);
    assert!(f
        .core
        .store
        .read(|store| store.registration_for(&target()).is_none()));
}
