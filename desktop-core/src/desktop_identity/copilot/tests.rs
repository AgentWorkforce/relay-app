//! Exercise native hooks against failed refreshes and contended sync work.
use super::*;
use agent_relay_node::relay::workspace::Secret;
use serde_json::{json, Value};
use crate::attestation_route::tests::{
    copilot::{hook_request, registered_copilot, rename::rename},
    Fixture, IN_REGISTERED, REGISTERED,
};
use agent_relay_node::relay::{cloud_session::CloudError, outbound::OutboundError};
use std::time::{Duration, Instant};

async fn ask(f: &Fixture, tool: &str, budget: Duration) -> Result<Value, OutboundError> {
    let account = f
        .core
        .plugins
        .account
        .as_ref()
        .unwrap()
        .clone()
        .as_any()
        .downcast::<crate::account::Account>()
        .unwrap();
    let args = if tool == "agentrelay-send_relay_message" {
        json!({"to":"synthetic-person","text":"synthetic test"})
    } else {
        json!({})
    };
    let request = hook_request(tool, args).unwrap();
    crate::copilot_attestation::route_with_presence_budget(
        &f.outbound,
        account,
        IN_REGISTERED,
        agent_relay_node::relay::peer::current_uid(),
        serde_json::from_slice(&request.body).unwrap(),
        budget,
    )
    .await
}

#[tokio::test]
async fn unchanged_copilot_senders_survive_failed_and_backed_off_refreshes() {
    for aliased in [false, true] {
        for failure in [
            CloudError::Unreachable,
            CloudError::Status {
                status: 503,
                message: None,
            },
            CloudError::Status {
                status: 403,
                message: None,
            },
        ] {
            let f = registered_copilot().await.unwrap();
            if aliased {
                rename(&f);
            }
            let identities = Identities::new(f.core.clone());
            identities.sync_desktop_identities().await;
            // Another identity changes the complete snapshot, forcing a PUT
            // without changing this caller or its accepted credential.
            f.core
                .store
                .update(|store| {
                    store
                        .tokens
                        .insert("codex-chat".into(), Secret::new("changed-synthetic-token"));
                })
                .unwrap();
            *f.cloud.failure.lock().unwrap() = Some(failure.clone());
            identities.sync_desktop_identities().await;
            identities.sync_desktop_identities().await; // Also exercise refusal backoff.
            let _global = identities.state.lock.lock().await;
            for tool in [
                "agentrelay-send_relay_message",
                "agentrelay-check_relay_inbox",
                "agentrelay-list_relay_agents",
                "agentrelay-join_relay",
            ] {
                tokio::time::timeout(
                    Duration::from_millis(200),
                    ask(&f, tool, Duration::from_millis(25)),
                )
                .await
                .expect("an unchanged sender must not wait for global sync")
                .expect("refresh failure must not revoke this accepted sender");
            }
        }
    }
}

#[tokio::test]
async fn accepted_copilot_sender_skips_a_busy_global_sync_lock() {
    let f = registered_copilot().await.unwrap();
    rename(&f);
    let identities = Identities::new(f.core.clone());
    identities.sync_desktop_identities().await;
    let _global = identities.state.lock.lock().await;
    tokio::time::timeout(
        Duration::from_millis(200),
        ask(&f, "agentrelay-join_relay", Duration::from_millis(25)),
    )
    .await
    .expect("cached join must not wait for the global sync lock")
    .unwrap();
}

#[tokio::test]
async fn an_unsynced_copilot_sender_times_out_behind_the_global_lock() {
    let f = registered_copilot().await.unwrap();
    rename(&f);
    let identities = Identities::new(f.core.clone());
    let _global = identities.state.lock.lock().await;
    let error = tokio::time::timeout(
        Duration::from_millis(200),
        ask(
            &f,
            "agentrelay-list_relay_agents",
            Duration::from_millis(15),
        ),
    )
    .await
    .expect("native hook must respect its remaining budget")
    .unwrap_err();
    assert_eq!(error.code, "attestation_unavailable");
}

#[tokio::test]
async fn a_timed_out_copilot_put_stays_serialized_before_a_newer_sender() {
    let f = registered_copilot().await.unwrap();
    rename(&f);
    let identities = Identities::new(f.core.clone());
    let (entered, put_started) = tokio::sync::oneshot::channel();
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
    *f.cloud.next_put_gate.lock().unwrap() = Some((entered, wait_for_release));
    // The route computes its remaining budget using std::time::Instant.
    // Allow wall-clock setup, then advance Tokio's timeout with the PUT in flight.
    let budget = Duration::from_secs(30);
    let hook = ask(&f, "agentrelay-list_relay_agents", budget);
    tokio::pin!(hook);
    tokio::select! {
        entered = tokio::time::timeout(Duration::from_secs(5), put_started) => {
            entered
                .expect("the PUT must start")
                .expect("PUT entry signal");
        }
        result = &mut hook => panic!("hook returned before PUT entry: {result:?}"),
    }
    tokio::time::pause();
    tokio::time::advance(budget).await;
    let error = tokio::time::timeout(Duration::from_millis(100), hook)
        .await
        .expect("slow PUT must not overrun the hook budget")
        .unwrap_err();
    tokio::time::resume();
    assert_eq!(error.code, "attestation_unavailable");
    assert!(
        identities.state.lock.try_lock().is_err(),
        "the timed-out PUT must keep its serialization lock"
    );
    f.core
        .store
        .update(|store| {
            let alias = store
                .registrations
                .iter_mut()
                .find(|r| r.target.kind == SessionKind::Copilot)
                .unwrap()
                .alias
                .as_mut()
                .unwrap();
            alias.agent_id = "newer-alias-agent".into();
            alias.token = Secret::new("newer-synthetic-token");
        })
        .unwrap();
    release
        .send(())
        .expect("the old PUT must still be in flight");
    let reply = tokio::time::timeout(
        Duration::from_secs(2),
        ask(&f, "agentrelay-list_relay_agents", Duration::from_secs(1)),
    )
    .await
    .unwrap()
    .unwrap();
    let call = f
        .dispatch
        .call(
            reply["modifiedArgs"][crate::claude_attestation::FIELD]
                .as_str()
                .unwrap(),
            Instant::now(),
        )
        .unwrap();
    assert!(call.owner.registration.contains("newer-alias-agent"));
    assert_eq!(call.sid, REGISTERED.to_ascii_lowercase());
}
