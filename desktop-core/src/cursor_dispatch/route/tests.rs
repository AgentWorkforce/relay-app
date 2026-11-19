//! Synthetic Cursor chats, a fake process table and an injected HTTP
//! boundary. No installed application, account, network or live chat is used.
use super::*;
use crate::attestation_route::tests::{
    copilot::registered_cursor, Fixture, IN_REGISTERED, REGISTERED, WORKSPACE,
};
use agent_relay_node::relay::process::{FakeProcess, FakeProcesses};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::cell::RefCell;
use std::collections::BTreeMap;

const HELPER_PID: i32 = 4300;
const TUID: &str = "toolu_01.synthetic:call-1";

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

fn sid() -> String {
    REGISTERED.to_ascii_lowercase()
}

/// A registered Cursor chat, Cursor selected with this build's helper and
/// hooks, and Cloud verifying Cursor proofs.
async fn fixture() -> Fixture {
    cursor_selected(registered_cursor().await.unwrap())
}

/// `f` with Cursor selected with this build's helper and hooks.
fn cursor_selected(f: Fixture) -> Fixture {
    let binding = binding();
    f.installer
        .select(vec!["cursor".into()], Some(&binding), None)
        .unwrap();
    f.installer.set_provider_credentials(
        &binding,
        BTreeMap::from([("cursor".into(), "synthetic-cursor-grant".into())]),
    );
    assert!(f.installer.refresh(Some(&binding)).is_empty());
    f
}

/// A process table holding this build's helper, run as `mcp.json` says.
fn helper_table(f: &Fixture) -> FakeProcesses {
    let table = FakeProcesses::default();
    let mut arguments = vec!["agent-relay-probe".to_string()];
    arguments.extend(super::super::arguments(
        &binding(),
        f.installer.cursor_development(),
        f.installer.cursor_session_socket().as_deref(),
    ));
    table.insert(
        HELPER_PID,
        FakeProcess {
            parent: 1,
            uid: agent_relay_node::relay::peer::current_uid(),
            start: 7,
            executable: Some(f.installer.cursor_helper()),
            arguments,
            ..Default::default()
        },
    );
    table
}

fn dispatch(f: &Fixture, tool: &str, arguments: Value) -> Dispatch {
    Dispatch {
        account: account(f),
        core: f.core.clone(),
        installer: f.installer.clone(),
        caller: (HELPER_PID, agent_relay_node::relay::peer::current_uid()),
        generation: f.cloud.generation.load(std::sync::atomic::Ordering::SeqCst),
        envelope: Envelope {
            binding: binding(),
            request: json!({"jsonrpc":"2.0","id":"synthetic-rpc","method":"tools/call",
                "params":{"name":tool,"arguments":arguments}}),
        },
        deadline: Instant::now() + Duration::from_secs(40),
    }
}

/// Runs `dispatch` against `table`, returning the reply and what was posted.
fn run(dispatch: Dispatch, table: &FakeProcesses) -> (Value, Option<(String, Value)>) {
    let posted = RefCell::new(None);
    let reply = dispatch.run(table, |_, bearer, body, _| {
        *posted.borrow_mut() = Some((bearer.to_owned(), body.clone()));
        Ok(json!({"jsonrpc":"2.0","id":"hosted","result":{"content":[]}}))
    });
    (reply, posted.into_inner())
}

fn class(reply: &Value) -> &str {
    reply["error"]["data"]["class"].as_str().unwrap_or_default()
}

fn announce(f: &Fixture, tool: &str, arguments: &Value) {
    let generation = f.cloud.generation.load(std::sync::atomic::Ordering::SeqCst);
    let input = json!({
        "conversation_id": REGISTERED, "generation_id": "synthetic-generation",
        "tool_use_id": TUID, "tool_name": tool, "tool_input": arguments.to_string(),
    });
    account(f)
        .copilot_dispatch
        .cursor_intents
        .register(announced(&input, generation).unwrap(), Instant::now())
        .unwrap();
}

#[test]
fn announcements_read_cursor_s_payload_and_refuse_malformed_calls() {
    let input = json!({
        "conversation_id": REGISTERED, "tool_use_id": TUID, "tool_name": "send_relay_message",
        "tool_input": r#"{"to":"peer","text":"hi","_agent_relay_attestation":"forged"}"#,
    });
    let intent = announced(&input, 9).unwrap();
    assert_eq!(intent.sid, sid());
    assert_eq!(intent.tuid, TUID);
    assert_eq!(intent.args, r#"{"text":"hi","to":"peer"}"#);
    assert_eq!(intent.generation, 9);
    let mut object = input.clone();
    object["tool_input"] = json!({"b": 1, "a": 2});
    assert_eq!(announced(&object, 9).unwrap().args, r#"{"a":2,"b":1}"#);
    for (field, value) in [
        ("tool_use_id", json!("has space")),
        ("tool_use_id", json!("")),
        ("tool_name", json!("")),
        ("tool_name", json!("list_relay_agents")),
        ("conversation_id", json!(7)),
        ("tool_input", json!("not json")),
        ("tool_input", json!([1])),
    ] {
        let mut bad = input.clone();
        bad[field] = value;
        assert_eq!(
            announced(&bad, 9).err(),
            Some("cursor_call_invalid"),
            "{field}"
        );
    }
    assert_eq!(hook_arguments(&Value::Null), Ok(Value::Null));
}

#[tokio::test]
async fn an_announced_identity_call_is_signed_for_its_chat_and_forwarded_with_the_grant() {
    let f = fixture().await;
    assert!(attesting(&account(&f), &f.core));
    assert!(f.installer.cursor_calls_attested());
    let table = helper_table(&f);
    let arguments = json!({"to":"synthetic-peer","text":"synthetic-message"});
    announce(&f, "send_relay_message", &arguments);
    let mut supplied = arguments.clone();
    supplied[FIELD] = json!("forged-by-the-caller");
    let (reply, posted) = run(dispatch(&f, "send_relay_message", supplied), &table);
    assert_eq!(reply["id"], "synthetic-rpc");
    assert!(reply.get("error").is_none(), "{reply}");
    let (bearer, body) = posted.unwrap();
    assert_eq!(bearer, "synthetic-cursor-grant");
    let proof = body["params"]["arguments"][FIELD].as_str().unwrap();
    assert_ne!(proof, "forged-by-the-caller");
    let encoded = proof.split('.').nth(1).unwrap();
    let payload: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).unwrap()).unwrap();
    let text = String::from_utf8(
        URL_SAFE_NO_PAD
            .decode(proof.split('.').nth(1).unwrap())
            .unwrap(),
    )
    .unwrap();
    let at = |member: &str| text.find(&format!("\"{member}\":")).unwrap();
    let order = [
        "v", "kind", "sid", "src", "tuid", "tool", "args", "iat", "exp",
    ];
    assert!(
        order.windows(2).all(|pair| at(pair[0]) < at(pair[1])),
        "{text}"
    );
    assert_eq!(payload.as_object().unwrap().len(), order.len());
    assert_eq!(payload["kind"], "cursor");
    assert_eq!(payload["sid"], sid());
    assert_eq!(payload["tuid"], TUID);
    assert_eq!(payload["tool"], "send_relay_message");
    assert_eq!(payload["args"], arguments);
    assert_eq!(
        payload["exp"].as_u64().unwrap() - payload["iat"].as_u64().unwrap(),
        60
    );
    let mut forwarded = body["params"]["arguments"].clone();
    forwarded.as_object_mut().unwrap().remove(FIELD);
    assert_eq!(forwarded, arguments);

    // The intent is single use: the same call again was not announced.
    let (again, posted) = run(dispatch(&f, "send_relay_message", arguments), &table);
    assert_eq!(class(&again), "cursor_intent_missing");
    assert!(posted.is_none());
}

#[tokio::test]
async fn unannounced_calls_go_unsigned_only_for_tools_without_an_identity() {
    let f = fixture().await;
    let table = helper_table(&f);
    let (reply, posted) = run(
        dispatch(&f, "send_relay_message", json!({"to":"peer","text":"x"})),
        &table,
    );
    assert_eq!(class(&reply), "cursor_intent_missing");
    assert!(posted.is_none());

    let (reply, posted) = run(
        dispatch(&f, "list_session_summaries", json!({FIELD: "forged"})),
        &table,
    );
    assert!(reply.get("error").is_none(), "{reply}");
    let (_, body) = posted.unwrap();
    assert_eq!(
        body["params"]["arguments"],
        json!({}),
        "a supplied proof is stripped"
    );

    // An announcement for other arguments does not sign this call.
    announce(
        &f,
        "send_relay_message",
        &json!({"to":"peer","text":"other"}),
    );
    let (reply, _) = run(
        dispatch(&f, "send_relay_message", json!({"to":"peer","text":"x"})),
        &table,
    );
    assert_eq!(class(&reply), "cursor_intent_missing");
}

/// Cloud refuses a proof on any tool but an identity tool, so a read goes
/// out unsigned even when a kept intent matches it.
#[tokio::test]
async fn a_read_call_is_never_signed_even_with_a_matching_intent() {
    let f = fixture().await;
    let table = helper_table(&f);
    let generation = f.cloud.generation.load(std::sync::atomic::Ordering::SeqCst);
    let intent = Intent {
        sid: sid(),
        tool: "list_relay_agents".into(),
        args: "{}".into(),
        tuid: TUID.into(),
        generation,
    };
    let intents = &account(&f).copilot_dispatch.cursor_intents;
    intents.register(intent, Instant::now()).unwrap();
    let (reply, posted) = run(dispatch(&f, "list_relay_agents", json!({})), &table);
    assert!(reply.get("error").is_none(), "{reply}");
    assert_eq!(posted.unwrap().1["params"]["arguments"], json!({}));
}

#[tokio::test]
async fn only_this_build_s_helper_with_its_exact_command_line_is_served() {
    let f = fixture().await;
    let table = helper_table(&f);
    let mut other = table.processes.lock().unwrap()[&HELPER_PID].clone();
    other.arguments.push("--extra".into());
    table.insert(HELPER_PID + 1, other.clone());
    other.arguments.pop();
    other.executable = Some(std::path::PathBuf::from("/bin/sh"));
    table.insert(HELPER_PID + 2, other);
    for pid in [HELPER_PID + 1, HELPER_PID + 2, HELPER_PID + 3] {
        let mut d = dispatch(&f, "list_session_summaries", json!({}));
        d.caller.0 = pid;
        let (reply, posted) = run(d, &table);
        assert_eq!(class(&reply), "native_peer_unavailable", "{pid}");
        assert!(posted.is_none());
    }
    let mut d = dispatch(&f, "list_session_summaries", json!({}));
    d.caller.1 += 1;
    assert_eq!(class(&run(d, &table).0), "native_peer_unavailable");
}

#[tokio::test]
async fn a_changed_account_generation_refuses_the_call() {
    let f = fixture().await;
    let table = helper_table(&f);
    let d = dispatch(&f, "list_session_summaries", json!({}));
    f.cloud
        .generation
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let (reply, posted) = run(d, &table);
    assert_eq!(class(&reply), "account_changed");
    assert!(posted.is_none());
}

#[tokio::test]
async fn without_cloud_support_identity_calls_are_forwarded_unsigned() {
    let f = registered_cursor().await.unwrap();
    f.cloud
        .generation
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let binding = binding();
    f.installer
        .select(vec!["cursor".into()], Some(&binding), None)
        .unwrap();
    f.installer.set_provider_credentials(
        &binding,
        BTreeMap::from([("cursor".into(), "synthetic-cursor-grant".into())]),
    );
    f.installer.refresh(Some(&binding));
    assert!(!attesting(&account(&f), &f.core));
    let table = helper_table(&f);
    let (reply, posted) = run(
        dispatch(&f, "send_relay_message", json!({"to":"peer","text":"x"})),
        &table,
    );
    assert!(reply.get("error").is_none(), "{reply}");
    assert!(posted.unwrap().1["params"]["arguments"]
        .get(FIELD)
        .is_none());
}

#[tokio::test]
async fn the_intent_route_reports_status_and_verifies_the_announcing_hook() {
    let f = fixture().await;
    let uid = agent_relay_node::relay::peer::current_uid();
    let request = |route: &str, body: Value| {
        let (path, query) = route.split_once('?').unwrap();
        let (name, value) = query.split_once('=').unwrap();
        Request {
            method: "POST".into(),
            path: path.into(),
            query: vec![(name.into(), value.into())],
            body: serde_json::to_vec(&body).unwrap(),
            ..Request::default()
        }
    };
    let get = request(crate::cursor_dispatch::STATUS_ROUTE, json!({}));
    assert_eq!(
        f.outbound.handle(IN_REGISTERED, uid, &get).await.unwrap(),
        json!({"attesting": true})
    );
    let post = |sid: &str| {
        request(
            crate::cursor_dispatch::INTENT_ROUTE,
            json!({
                "conversation_id": sid, "generation_id": "g", "tool_use_id": TUID,
                "tool_name": "send_relay_message", "tool_input": "{}",
            }),
        )
    };
    // No Cursor process holds the chat on this host.
    let error = f
        .outbound
        .handle(IN_REGISTERED, uid, &post(&sid()))
        .await
        .unwrap_err();
    assert_eq!(
        (error.status, error.code.as_str()),
        (409, "cursor_hook_unverified")
    );
    let error = f
        .outbound
        .handle(
            IN_REGISTERED,
            uid,
            &post("11111111-2222-4333-8444-555555555555"),
        )
        .await
        .unwrap_err();
    assert_eq!(
        (error.status, error.code.as_str()),
        (409, "cursor_chat_unknown")
    );
    let error = f
        .outbound
        .handle(IN_REGISTERED, uid + 1, &post(&sid()))
        .await
        .unwrap_err();
    assert_eq!(error.code, "caller_unverified");
    assert_eq!(account(&f).copilot_dispatch.cursor_intents.len(), 0);
}

#[test]
fn cursor_routes_ride_on_the_hook_paths_the_pinned_core_forwards() {
    let request = |method: &str, route: &str| {
        let (path, query) = route.split_once('?').unwrap_or((route, ""));
        Request {
            method: method.into(),
            path: path.into(),
            query: url::form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect(),
            ..Request::default()
        }
    };
    use crate::cursor_dispatch::{which, Route, INTENT_ROUTE, ROUTE, STATUS_ROUTE};
    assert_eq!(which(&request("POST", INTENT_ROUTE)), Some(Route::Intent));
    assert_eq!(which(&request("POST", STATUS_ROUTE)), Some(Route::Status));
    assert_eq!(which(&request("POST", ROUTE)), Some(Route::Mcp));
    for (method, route) in [
        ("GET", INTENT_ROUTE),
        ("POST", "/copilot/hook"),
        ("POST", "/copilot/mcp"),
        ("POST", "/copilot/mcp?cursor=intent"),
        ("POST", "/claude/attestation?cursor=mcp"),
    ] {
        assert_eq!(which(&request(method, route)), None, "{method} {route}");
    }
}

mod control;
mod core_log;
mod fresh_chat;
mod routes;
