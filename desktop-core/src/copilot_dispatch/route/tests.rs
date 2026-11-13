//! Synthetic native peers, fixture-owned configuration and an injected HTTP
//! boundary. No installed application, account, network or live chat is used.
use super::*;
use crate::attestation_route::tests::{
    copilot::{hook_request, registered_copilot},
    Fixture, IN_REGISTERED, REGISTERED, WORKSPACE,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};

async fn fixture() -> Fixture {
    let f = registered_copilot().await.unwrap();
    let binding = binding();
    f.installer
        .select(vec!["copilot".into()], Some(&binding), None)
        .unwrap();
    f.installer.set_provider_credentials(
        &binding,
        BTreeMap::from([("copilot".into(), "synthetic-child-grant".into())]),
    );
    assert!(f.installer.refresh(Some(&binding)).is_empty());
    assert!(f.installer.copilot_dispatch_hosted(&binding).is_some());
    f
}
fn binding() -> Binding {
    Binding {
        site_url: "https://agentrelay.com".into(),
        workspace_id: WORKSPACE.into(),
        account_id: "user-1".into(),
    }
}
fn account(f: &Fixture) -> Arc<Account> {
    f.core
        .plugins
        .account
        .as_ref()
        .unwrap()
        .clone()
        .as_any()
        .downcast::<Account>()
        .unwrap()
}
async fn pending(f: &Fixture) -> (String, Call) {
    let uid = agent_relay_node::relay::peer::current_uid();
    let request = hook_request(
        "agentrelay-send_relay_message",
        json!({"to":"synthetic-peer","text":"synthetic-message"}),
    )
    .unwrap();
    let answer = f
        .outbound
        .handle(IN_REGISTERED, uid, &request)
        .await
        .unwrap();
    let original = answer["modifiedArgs"][FIELD].as_str().unwrap();
    let call = f.dispatch.call(original, Instant::now()).unwrap();
    // Exercise a real issued capability older than the old Cloud proof TTL.
    f.dispatch
        .issue(call, Instant::now() - Duration::from_secs(185))
        .unwrap()
}
fn control(f: &Fixture, ticket: &str, call: &Call, approved: bool) {
    let mut args = call.args.clone();
    args[FIELD] = json!(ticket);
    let path = peer::journal(&f.installer, &call.sid);
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    let records = [
        json!({"type":"hook.start","data":{"hookType":"preMcpToolCall","input":{"sessionId":call.sid,"serverName":"agentrelay","toolName":call.tool,"toolCallId":"synthetic-call","arguments":args}}}),
        json!({"type":"permission.requested","data":{"requestId":"synthetic-permission","permissionRequest":{"kind":"mcp","toolCallId":"synthetic-call","serverName":"agentrelay","toolName":format!("agentrelay-{}",call.tool),"args":args}}}),
        json!({"type":"permission.completed","timestamp":chrono::Utc::now().to_rfc3339(),"data":{"requestId":"synthetic-permission","toolCallId":"synthetic-call","result":{"kind":if approved{"approved"}else{"denied"}}}}),
    ];
    for record in records {
        writeln!(file, "{record}").unwrap();
    }
}
fn dispatch(f: &Fixture, ticket: String, call: Call) -> Dispatch {
    let binding = binding();
    let uid = agent_relay_node::relay::peer::current_uid();
    let runtime = f
        .dispatch
        .verifier
        .runtime(&f.installer, &binding, IN_REGISTERED + 10, uid)
        .unwrap();
    let mut args = call.args.clone();
    args[FIELD] = json!(ticket);
    Dispatch {
        account: account(f),
        core: f.core.clone(),
        installer: f.installer.clone(),
        caller: (IN_REGISTERED + 10, uid),
        runtime,
        envelope: Envelope {
            binding,
            request: json!({"jsonrpc":"2.0","id":"synthetic-rpc","method":"tools/call","params":{"name":call.tool,"arguments":args}}),
        },
        pending: Some((ticket, call)),
        deadline: Instant::now() + Duration::from_secs(40),
        generation: f.cloud.generation.load(Ordering::SeqCst),
        authority: f.installer.copilot_epoch(),
        _permit: f.dispatch.http_permit().unwrap(),
    }
}

#[tokio::test]
async fn delayed_manual_approval_dispatches_fresh_proof_once_and_caches_the_result() {
    let f = fixture().await;
    let (ticket, call) = pending(&f).await;
    control(&f, &ticket, &call, true);
    let count = AtomicUsize::new(0);
    let reply = dispatch(&f, ticket.clone(), call.clone()).run_with(|_, grant, request, _| {
        count.fetch_add(1, Ordering::SeqCst);
        assert_eq!(grant, "synthetic-child-grant");
        let proof = request["params"]["arguments"][FIELD].as_str().unwrap();
        assert!(!proof.starts_with("copilot-intent-v1."));
        let claims: Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(proof.split('.').nth(1).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(claims["sid"], REGISTERED.to_ascii_lowercase());
        let iat = claims["iat"].as_i64().unwrap();
        assert!((chrono::Utc::now().timestamp() - iat).abs() <= 2);
        assert_eq!(claims["exp"].as_i64().unwrap() - iat, 60);
        assert_eq!(
            request["params"]["arguments"]["idempotency_key"],
            call.args["idempotency_key"]
        );
        Ok(json!({"jsonrpc":"2.0","id":"synthetic-rpc","result":{"accepted":true}}))
    });
    assert_eq!(reply["result"]["accepted"], true);
    let again = dispatch(&f, ticket, call).run_with(|_, _, _, _| {
        count.fetch_add(1, Ordering::SeqCst);
        panic!("a consumed capability must never POST twice")
    });
    assert_eq!(again, reply);
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

/// The core's rediscover tick refreshes provider setup every 30 s. A refresh
/// that changes nothing must not cancel a call waiting for "Allow Once".
#[tokio::test]
async fn unchanged_background_refreshes_keep_a_call_waiting_for_approval() {
    let f = fixture().await;
    let (ticket, call) = pending(&f).await;
    let epoch = f.installer.copilot_epoch();
    for _ in 0..2 {
        assert!(f.installer.refresh(Some(&binding())).is_empty());
    }
    assert_eq!(f.installer.copilot_epoch(), epoch);
    control(&f, &ticket, &call, true);
    let count = AtomicUsize::new(0);
    let reply = dispatch(&f, ticket, call).run_with(|_, _, _, _| {
        count.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"jsonrpc":"2.0","id":"synthetic-rpc","result":{"accepted":true}}))
    });
    assert_eq!(reply["result"]["accepted"], true, "{reply}");
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

/// A refresh that does repair or replace authority still cancels the call.
#[tokio::test]
async fn an_authoritative_refresh_still_cancels_a_call_waiting_for_approval() {
    for change in ["binding", "credential", "repair"] {
        let f = fixture().await;
        let (ticket, call) = pending(&f).await;
        let epoch = f.installer.copilot_epoch();
        match change {
            "binding" => {
                let other = Binding {
                    account_id: "user-2".into(),
                    ..binding()
                };
                f.installer.refresh(Some(&other));
                f.installer.set_provider_credentials(
                    &binding(),
                    BTreeMap::from([("copilot".into(), "synthetic-child-grant".into())]),
                );
            }
            "credential" => f.installer.set_provider_credentials(
                &binding(),
                BTreeMap::from([("copilot".into(), "rotated-synthetic-grant".into())]),
            ),
            _ => {
                let path = f
                    .installer
                    .copilot_native_config()
                    .data_dir
                    .join("mcp-config.json");
                std::fs::write(path, "{\"mcpServers\":{}}").unwrap();
            }
        }
        f.installer.refresh(Some(&binding()));
        assert_ne!(f.installer.copilot_epoch(), epoch, "{change}");
        control(&f, &ticket, &call, true);
        let reply = dispatch(&f, ticket, call)
            .run_with(|_, _, _, _| panic!("{change}: a revoked intent must not POST"));
        assert_eq!(
            reply["error"]["data"],
            json!({"class":"authority_changed"}),
            "{change}"
        );
    }
}

/// A refused call names its reason by a static class, in the route's error
/// code (which stdio forwards as `error.data.class`) and in the dispatch reply.
#[tokio::test]
async fn a_refused_call_carries_a_secret_free_class() {
    let f = fixture().await;
    let uid = agent_relay_node::relay::peer::current_uid();
    let (ticket, call) = pending(&f).await;
    control(&f, &ticket, &call, true);
    let mut args = call.args.clone();
    args[FIELD] = json!(ticket);
    let envelope = json!({"binding":binding(),"request":{"jsonrpc":"2.0","id":"synthetic-rpc","method":"tools/call","params":{"name":call.tool,"arguments":args}}});
    let request = |envelope: &Value| Request {
        method: "POST".into(),
        body: serde_json::to_vec(envelope).unwrap(),
        ..Default::default()
    };
    f.installer.set_provider_credentials(
        &binding(),
        BTreeMap::from([("copilot".into(), "rotated-synthetic-grant".into())]),
    );
    let refused = route(
        account(&f),
        &f.outbound,
        IN_REGISTERED + 10,
        uid,
        &request(&envelope),
    )
    .await
    .unwrap_err();
    let mut missing = envelope.clone();
    missing["request"]["params"]["arguments"]
        .as_object_mut()
        .unwrap()
        .remove(FIELD);
    let no_ticket = route(
        account(&f),
        &f.outbound,
        IN_REGISTERED + 10,
        uid,
        &request(&missing),
    )
    .await
    .unwrap_err();
    let foreign = route(
        account(&f),
        &f.outbound,
        IN_REGISTERED + 10,
        uid + 1,
        &request(&envelope),
    )
    .await
    .unwrap_err();
    for (error, class) in [
        (&refused, "authority_changed"),
        (&no_ticket, "ticket_missing"),
        (&foreign, "caller_unverified"),
    ] {
        assert_eq!((error.status, error.code.as_str()), (409, class));
        assert!(error.message.ends_with("Do not repeat an ambiguous send."));
        assert!(!error.message.contains(&ticket) && !error.code.contains(&ticket));
    }
}

#[tokio::test]
async fn normal_provider_revocation_after_grant_resolution_prevents_network_dispatch() {
    for change in ["deselect", "remove", "replace", "rotate"] {
        let f = fixture().await;
        let (ticket, call) = pending(&f).await;
        control(&f, &ticket, &call, true);
        let result = dispatch(&f, ticket, call).run_after_resolve(
            |_, _, _, _| panic!("revoked provider must not POST"),
            |d| match change {
                "deselect" => {
                    f.installer
                        .select(Vec::new(), Some(&d.envelope.binding), None)
                        .unwrap();
                }
                "remove" => {
                    let path = f
                        .installer
                        .copilot_native_config()
                        .data_dir
                        .join("mcp-config.json");
                    std::fs::remove_file(path).unwrap();
                }
                "replace" => {
                    let path = f
                        .installer
                        .copilot_native_config()
                        .data_dir
                        .join("mcp-config.json");
                    std::fs::write(
                        path,
                        "{\"mcpServers\":{\"agentrelay\":{\"command\":\"foreign\"}}}",
                    )
                    .unwrap();
                }
                _ => f.installer.set_provider_credentials(
                    &d.envelope.binding,
                    BTreeMap::from([("copilot".into(), "rotated-synthetic-grant".into())]),
                ),
            },
        );
        assert!(result.get("error").is_some(), "{change}");
    }
}

#[tokio::test]
async fn changed_account_later_abort_and_expired_queue_never_post_or_make_unknown_calls_retryable()
{
    for change in [
        "generation",
        "abort",
        "queue",
        "invalid_control",
        "invalid_permission",
    ] {
        let f = fixture().await;
        let (ticket, call) = pending(&f).await;
        control(&f, &ticket, &call, true);
        let mut d = dispatch(&f, ticket.clone(), call.clone());
        if change == "queue" {
            d.deadline = Instant::now() - Duration::from_secs(1);
        }
        let reply = d.run_after_resolve(
            |_, _, _, _| panic!("invalid queued call must not POST"),
            |_| {
                if change == "generation" {
                    f.cloud.generation.fetch_add(1, Ordering::SeqCst);
                }
                if change == "abort" {
                    let mut file = std::fs::OpenOptions::new()
                        .append(true)
                        .open(peer::journal(&f.installer, &call.sid))
                        .unwrap();
                    writeln!(file, "{}", json!({"type":"abort"})).unwrap();
                }
                if change == "invalid_control" {
                    let mut file = std::fs::OpenOptions::new()
                        .append(true)
                        .open(peer::journal(&f.installer, &call.sid))
                        .unwrap();
                    writeln!(file, "null").unwrap();
                }
                if change == "invalid_permission" {
                    let mut file = std::fs::OpenOptions::new()
                        .append(true)
                        .open(peer::journal(&f.installer, &call.sid))
                        .unwrap();
                    writeln!(
                        file,
                        "{}",
                        json!({"type":"permission.requested","data":null})
                    )
                    .unwrap();
                }
            },
        );
        assert!(reply.get("error").is_some());
    }
    let f = fixture().await;
    let (ticket, call) = pending(&f).await;
    control(&f, &ticket, &call, true);
    let first = dispatch(&f, ticket.clone(), call.clone())
        .run_with(|_, _, _, _| Err("synthetic_connection_lost"));
    assert!(first.get("error").is_some());
    let again = dispatch(&f, ticket, call)
        .run_with(|_, _, _, _| panic!("unknown send must not POST again"));
    assert!(again.get("error").is_some());
}

#[test]
fn malformed_requests_never_consume_an_intent_as_an_mcp_notification_or_task() {
    for request in [
        json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"send_relay_message"}}),
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"task":{}}}),
        json!([{ "jsonrpc":"2.0","id":1,"method":"ping"}]),
    ] {
        assert!(!request_valid(&request));
    }
    assert!(request_valid(
        &json!({"jsonrpc":"2.0","method":"notifications/initialized"})
    ));
}

#[test]
fn model_only_native_catalog_preserves_other_ui_metadata_and_tools() {
    let mut reply = json!({"result":{"tools":[{"name":"send_relay_message","_meta":{"other":true,"ui":{"resourceUri":"ui://fixture","visibility":["app"]}}},{"name":"get_shared_session","_meta":{"ui":{"visibility":["app"]}}}]}});
    catalog(&mut reply);
    assert_eq!(
        reply["result"]["tools"][0]["_meta"]["ui"]["visibility"],
        json!(["model"])
    );
    assert_eq!(
        reply["result"]["tools"][0]["_meta"]["ui"]["resourceUri"],
        "ui://fixture"
    );
    assert_eq!(
        reply["result"]["tools"][1]["_meta"]["ui"]["visibility"],
        json!(["app"])
    );
}
