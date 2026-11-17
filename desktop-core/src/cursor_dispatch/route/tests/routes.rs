//! The intent route's accepted path, its announce budget, ambiguous intents
//! of tools without an identity, and the MCP route's envelope checks.
use super::*;

pub(super) const HOOK_PID: i32 = 4501;

/// A `cursor-agent` holding `sid()`'s store, with the hook's shell below it.
fn hook_table(f: &Fixture) -> FakeProcesses {
    holding(f, &sid())
}

/// A `cursor-agent` holding chat `chat`'s store, with the hook's shell
/// (`HOOK_PID`) below it.
pub(super) fn holding(f: &Fixture, chat: &str) -> FakeProcesses {
    let store = f
        .installer
        .cursor_config()
        .chats()
        .join("87460ce610ea3b92e1e9d04e6e66eac4")
        .join(chat);
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join("store.db"), b"").unwrap();
    let uid = agent_relay_node::relay::peer::current_uid();
    let table = FakeProcesses::default();
    table.insert(
        4400,
        FakeProcess {
            parent: 1,
            uid,
            start: 5,
            executable: Some("/opt/cursor-agent/versions/2026.10.22/node".into()),
            arguments: vec!["cursor-agent".into()],
            open_files: vec![store.join("store.db")],
            ..Default::default()
        },
    );
    table.insert(
        HOOK_PID,
        FakeProcess {
            parent: 4400,
            uid,
            start: 6,
            executable: Some("/bin/zsh".into()),
            arguments: vec!["/bin/zsh".into(), "-c".into(), "hook".into()],
            ..Default::default()
        },
    );
    table
}

fn post(route: &str, body: Vec<u8>) -> Request {
    let (path, query) = route.split_once('?').unwrap();
    let (name, value) = query.split_once('=').unwrap();
    Request {
        method: "POST".into(),
        path: path.into(),
        query: vec![(name.into(), value.into())],
        body,
        ..Request::default()
    }
}

#[tokio::test]
async fn an_accepted_announcement_signs_the_identity_call_as_its_chat() {
    let f = fixture().await;
    let account = account(&f);
    let hooks = hook_table(&f);
    assert_eq!(
        accepted(&account, &f.core, HOOK_PID, &sid(), &hooks),
        Ok(Chat::Registered)
    );
    assert_eq!(
        accepted(&account, &f.core, 4400 + 1, &sid(), &hooks),
        Err("cursor_hook_unverified")
    );
    let arguments = json!({"to":"synthetic-peer","text":"synthetic-message"});
    let input = json!({
        "conversation_id": REGISTERED, "tool_use_id": TUID,
        "tool_name": "send_relay_message", "tool_input": arguments.to_string(),
    });
    let generation = f.cloud.generation.load(std::sync::atomic::Ordering::SeqCst);
    let now = Instant::now();
    let intents = &account.copilot_dispatch.cursor_intents;
    keep(intents, announced(&input, generation).unwrap(), now, now).unwrap();
    let (reply, posted) = run(
        dispatch(&f, "send_relay_message", arguments),
        &helper_table(&f),
    );
    assert!(reply.get("error").is_none(), "{reply}");
    let proof = posted.unwrap().1["params"]["arguments"][FIELD]
        .as_str()
        .unwrap()
        .to_owned();
    let encoded = proof.split('.').nth(1).unwrap();
    let payload: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).unwrap()).unwrap();
    assert_eq!(
        (payload["sid"].as_str(), payload["tuid"].as_str()),
        (Some(sid().as_str()), Some(TUID))
    );
}

#[tokio::test]
async fn announcements_are_refused_while_cursor_s_calls_are_not_attested() {
    // Cloud verifies Cursor proofs, but Cursor is not set up with this
    // build's helper and hooks.
    let f = registered_cursor().await.unwrap();
    let account = account(&f);
    assert!(!attesting(&account, &f.core));
    assert_eq!(
        accepted(&account, &f.core, HOOK_PID, &sid(), &hook_table(&f)),
        Err("cursor_attestation_unavailable")
    );
    let body = json!({
        "conversation_id": REGISTERED, "tool_use_id": TUID,
        "tool_name": "send_relay_message", "tool_input": "{}",
    });
    let uid = agent_relay_node::relay::peer::current_uid();
    let request = post(
        crate::cursor_dispatch::INTENT_ROUTE,
        serde_json::to_vec(&body).unwrap(),
    );
    let error = f
        .outbound
        .handle(HOOK_PID, uid, &request)
        .await
        .unwrap_err();
    assert_eq!(
        (error.status, error.code.as_str()),
        (409, "cursor_attestation_unavailable")
    );
    assert_eq!(account.copilot_dispatch.cursor_intents.len(), 0);
}

#[test]
fn an_announcement_past_the_hook_s_budget_is_not_kept() {
    assert!(crate::cursor_dispatch::ANNOUNCE_BUDGET < crate::cursor_dispatch::ANNOUNCE_TIMEOUT);
    let input = json!({
        "conversation_id": REGISTERED, "tool_use_id": TUID,
        "tool_name": "send_relay_message", "tool_input": "{}",
    });
    let intents = Intents::default();
    let started = Instant::now();
    let late = started + crate::cursor_dispatch::ANNOUNCE_BUDGET;
    assert_eq!(
        keep(&intents, announced(&input, 1).unwrap(), started, late),
        Err("deadline_expired")
    );
    assert_eq!(intents.len(), 0, "the hook already gave up on it");
    let in_time = late - Duration::from_millis(1);
    assert_eq!(
        keep(&intents, announced(&input, 1).unwrap(), started, in_time),
        Ok(())
    );
    assert_eq!(intents.len(), 1);
}

#[tokio::test]
async fn an_ambiguous_intent_refuses_only_identity_calls() {
    let f = fixture().await;
    let table = helper_table(&f);
    let intents = &account(&f).copilot_dispatch.cursor_intents;
    let generation = f.cloud.generation.load(std::sync::atomic::Ordering::SeqCst);
    let two_chats = |tool: &str, arguments: &Value| {
        for (sid, tuid) in [(sid(), "toolu_a"), ("other-chat".into(), "toolu_b")] {
            let intent = Intent {
                sid,
                tool: tool.into(),
                args: proof::arguments(arguments).unwrap().1,
                tuid: tuid.into(),
                generation,
            };
            intents.register(intent, Instant::now()).unwrap();
        }
    };
    let arguments = json!({"limit": 5});
    two_chats("list_session_summaries", &arguments);
    let (reply, posted) = run(dispatch(&f, "list_session_summaries", arguments), &table);
    assert!(reply.get("error").is_none(), "{reply}");
    assert!(posted.unwrap().1["params"]["arguments"]
        .get(FIELD)
        .is_none());
    assert_eq!(intents.len(), 2, "a read never takes an intent");

    let arguments = json!({"to":"peer","text":"x"});
    two_chats("send_relay_message", &arguments);
    let (reply, posted) = run(dispatch(&f, "send_relay_message", arguments), &table);
    assert_eq!(class(&reply), "cursor_intent_ambiguous");
    assert!(posted.is_none());
}

#[tokio::test]
async fn the_mcp_route_refuses_another_binding_and_invalid_bodies() {
    let f = fixture().await;
    let uid = agent_relay_node::relay::peer::current_uid();
    let list = json!({"jsonrpc":"2.0","id":1,"method":"tools/list"});
    let mut other = serde_json::to_value(binding()).unwrap();
    other["workspace_id"] = json!("another-workspace");
    let ask = |body: Vec<u8>| {
        let request = post(crate::cursor_dispatch::ROUTE, body);
        let outbound = &f.outbound;
        async move { outbound.handle(HELPER_PID, uid, &request).await }
    };
    let error = ask(serde_json::to_vec(&json!({"binding": other, "request": list})).unwrap())
        .await
        .unwrap_err();
    assert_eq!(
        (error.status, error.code.as_str()),
        (409, "binding_mismatch")
    );
    let current = serde_json::to_value(binding()).unwrap();
    for body in [
        b"not json".to_vec(),
        serde_json::to_vec(&json!({"binding": current, "request": list, "extra": 1})).unwrap(),
        serde_json::to_vec(
            &json!({"binding": current, "request": {"jsonrpc":"1.0","id":1,"method":"tools/list"}}),
        )
        .unwrap(),
        serde_json::to_vec(&json!({"binding": current, "request": [list]})).unwrap(),
    ] {
        let error = ask(body).await.unwrap_err();
        assert_eq!(
            (error.status, error.code.as_str()),
            (409, "request_invalid")
        );
    }
}

/// The hook's view of Core: the class the intent route refused with.
struct Refusing(String);

impl crate::cursor_guide::Relay for Refusing {
    fn attesting(&self) -> bool {
        panic!("the refusal class already says Cursor's calls are attested")
    }
    fn announce(&self, _: &Value) -> Result<(), String> {
        Err(self.0.clone())
    }
}

#[tokio::test]
async fn an_unknown_chat_s_identity_call_is_left_to_the_helper_which_refuses_it() {
    let f = fixture().await;
    let account = account(&f);
    assert!(attesting(&account, &f.core));
    let unknown = "5d1c0b8e-7a2f-4c3d-9e8b-1a2b3c4d5e6f";
    let arguments = json!({"to":"synthetic-peer","text":"synthetic-message"});
    let hook = json!({
        "hook_event_name": "beforeMCPExecution", "mcp_server_name": "agent-relay",
        "conversation_id": unknown, "tool_use_id": TUID,
        "tool_name": "send_relay_message", "tool_input": arguments.to_string(),
    });
    let uid = agent_relay_node::relay::peer::current_uid();
    let request = post(
        crate::cursor_dispatch::INTENT_ROUTE,
        serde_json::to_vec(&hook).unwrap(),
    );
    let error = f
        .outbound
        .handle(HOOK_PID, uid, &request)
        .await
        .unwrap_err();
    assert_eq!(
        (error.status, error.code.as_str()),
        (409, "cursor_chat_unknown")
    );
    let relay = Refusing(crate::copilot_dispatch::refusal_class(&error.code).to_owned());
    let never = || -> crate::cursor_guide::Commands { panic!("the helper refuses, not the hook") };
    let output =
        crate::cursor_guide::answer(hook.to_string().as_bytes(), "agent-relay", &relay, never);
    assert_eq!(output, json!({}));
    assert_eq!(account.copilot_dispatch.cursor_intents.len(), 0);
    let (reply, posted) = run(
        dispatch(&f, "send_relay_message", arguments),
        &helper_table(&f),
    );
    assert_eq!(class(&reply), "cursor_intent_missing");
    assert!(posted.is_none());
}
