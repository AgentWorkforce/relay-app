//! A native SID sends as its current alias, without requiring a model join.
use super::*;
use agent_relay_node::relay::store::AgentAlias;

pub(crate) fn rename(f: &Fixture) -> (String, String) {
    f.core
        .store
        .update(|store| {
            let registration = store
                .registrations
                .iter_mut()
                .find(|r| r.target.kind == SessionKind::Copilot)
                .unwrap();
            let registered = registration.link_address(&f.core.machine_prefix);
            registration.alias = Some(AgentAlias {
                name: "renamed-chat".into(),
                agent_id: "agent-renamed".into(),
                token: Secret::new("synthetic-renamed-token"),
                registered_at: iso8601_from_seconds(1.0),
                gate: None,
            });
            (
                registered,
                registration.current_address(&f.core.machine_prefix),
            )
        })
        .unwrap()
}

async fn ask(f: &Fixture, tool: &str, args: Value) -> Result<Value, OutboundError> {
    f.outbound
        .handle(
            IN_REGISTERED,
            agent_relay_node::relay::peer::current_uid(),
            &hook_request(tool, args).unwrap(),
        )
        .await
}

#[tokio::test]
async fn copilot_send_without_join_syncs_only_the_current_alias_for_the_native_sid() {
    let f = registered_copilot().await.unwrap();
    crate::desktop_identity::Identities::new(f.core.clone())
        .sync_desktop_identities()
        .await;
    ask(&f, "agentrelay-list_relay_agents", json!({}))
        .await
        .unwrap();
    rename(&f);
    let answer = ask(
        &f,
        "agentrelay-send_relay_message",
        json!({"to":"synthetic-person","text":"synthetic reply","reply_to":"123"}),
    )
    .await
    .unwrap();
    let call = f
        .dispatch
        .call(
            answer["modifiedArgs"][attestation::FIELD].as_str().unwrap(),
            std::time::Instant::now(),
        )
        .unwrap();
    assert_eq!(call.sid, REGISTERED.to_ascii_lowercase());
    assert_eq!(call.args["reply_to"], "123");
    let body = f.last_put();
    let native: Vec<_> = body["identities"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|identity| identity["native_session"]["kind"] == "copilot")
        .collect();
    assert_eq!(
        native.len(),
        1,
        "native SID must not have two candidate identities"
    );
    assert_eq!(native[0]["agent_id"], "agent-renamed");
    assert_eq!(native[0]["name"], "renamed-chat");
    assert_eq!(native[0]["token"], "synthetic-renamed-token");
    assert_eq!(
        native[0]["native_session"]["id"],
        REGISTERED.to_ascii_lowercase()
    );
    assert!(
        body["identities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|identity| identity["agent_id"] == "agent-copilot-chat"
                && identity.get("native_session").is_none()),
        "explicit legacy bindings retain the original identity without native authority"
    );
}

#[tokio::test]
async fn copilot_join_normalizes_only_its_registered_or_current_address() {
    let f = registered_copilot().await.unwrap();
    let (registered, current) = rename(&f);
    for address in [&registered, &current] {
        let answer = ask(
            &f,
            "agentrelay-join_relay",
            json!({"desktop_address":address}),
        )
        .await
        .unwrap();
        let call = f
            .dispatch
            .call(
                answer["modifiedArgs"][attestation::FIELD].as_str().unwrap(),
                std::time::Instant::now(),
            )
            .unwrap();
        assert_eq!(call.args["desktop_address"], current);
    }
    assert_eq!(
        ask(
            &f,
            "agentrelay-join_relay",
            json!({"desktop_address":"another-chat@another-machine"})
        )
        .await
        .unwrap_err()
        .code,
        "attestation_unavailable"
    );
}

#[tokio::test]
async fn copilot_rename_cannot_reuse_an_old_accepted_snapshot_when_cloud_is_offline() {
    let f = registered_copilot().await.unwrap();
    crate::desktop_identity::Identities::new(f.core.clone())
        .sync_desktop_identities()
        .await;
    ask(&f, "agentrelay-list_relay_agents", json!({}))
        .await
        .unwrap();
    rename(&f);
    f.cloud
        .offline
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        ask(
            &f,
            "agentrelay-send_relay_message",
            json!({"to":"synthetic-person","text":"no stale sender"})
        )
        .await
        .unwrap_err()
        .code,
        "attestation_unavailable"
    );
}
