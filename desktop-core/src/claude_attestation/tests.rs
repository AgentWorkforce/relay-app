use super::*;
use ring::signature::{UnparsedPublicKey, ED25519};

const SID: &str = "0c6b1f0e-2f2a-4b7e-9d3c-5a1e8f7b6c4d";
const SOURCE: &str = "6f1d2c3b-4a59-4e8f-9b7a-1c2d3e4f5a6b";

fn key() -> (tempfile::TempDir, Key) {
    let dir = tempfile::tempdir().unwrap();
    let key = Key::load_or_create(&dir.path().join(KEY_FILE)).unwrap();
    (dir, key)
}

fn decode(part: &str) -> Value {
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).unwrap()).unwrap()
}

#[test]
fn the_proof_is_a_compact_eddsa_jws_the_registered_key_verifies() {
    let (_dir, key) = key();
    let registration = key.registration();
    assert_eq!(registration["alg"], "Ed25519");
    assert_eq!(registration["kid"], key.kid());
    let public = URL_SAFE_NO_PAD
        .decode(registration["public_key"].as_str().unwrap())
        .unwrap();
    assert_eq!(public.len(), 32);
    assert_eq!(
        key.kid(),
        URL_SAFE_NO_PAD.encode(&Sha256::digest(&public)[..16])
    );
    assert_eq!(key.kid().len(), 22, "16 bytes, base64url without padding");

    let claims = Claims::new(SID, SOURCE, "toolu_01ABC", "send_message", 1_800_000_000);
    let proof = key.sign(&claims).unwrap();
    let parts: Vec<&str> = proof.split('.').collect();
    assert_eq!(parts.len(), 3);
    assert!(parts.iter().all(|part| !part.contains('=')));
    assert_eq!(
        decode(parts[0]),
        json!({"alg":"EdDSA","kid":key.kid(),"typ":"agent-relay-attest+jws"})
    );
    assert_eq!(
        decode(parts[1]),
        json!({
            "v":1, "kind":"claude", "sid":SID, "src":SOURCE, "tuid":"toolu_01ABC",
            "tool":"send_message", "iat":1_800_000_000u64, "exp":1_800_000_060u64,
        })
    );
    let signature = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
    let signed = format!("{}.{}", parts[0], parts[1]);
    UnparsedPublicKey::new(&ED25519, &public)
        .verify(signed.as_bytes(), &signature)
        .expect("the registered public key verifies the proof");
    // Any change to the payload breaks the signature.
    let forged = format!(
        "{}.{}",
        parts[0],
        URL_SAFE_NO_PAD.encode(br#"{"v":1,"kind":"claude","sid":"other"}"#)
    );
    assert!(UnparsedPublicKey::new(&ED25519, &public)
        .verify(forged.as_bytes(), &signature)
        .is_err());
}

#[test]
fn one_private_key_per_installation_is_kept_and_reloaded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app-data").join(KEY_FILE);
    let first = Key::load_or_create(&path).unwrap();
    let again = Key::load_or_create(&path).unwrap();
    assert_eq!(first.kid(), again.kid());
    assert_eq!(first.public_key(), again.public_key());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    let other = tempfile::tempdir().unwrap();
    let elsewhere = Key::load_or_create(&other.path().join(KEY_FILE)).unwrap();
    assert_ne!(first.kid(), elsewhere.kid(), "another installation");
}

#[test]
fn an_unreadable_key_is_an_error_and_is_left_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(KEY_FILE);
    std::fs::write(&path, b"{\"version\":1,\"key\":\"not base64!\"}").unwrap();
    let error = Key::load_or_create(&path).err().unwrap();
    assert_eq!(error, "The Claude attestation key is unreadable.");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"{\"version\":1,\"key\":\"not base64!\"}"
    );
    let slot = KeySlot::default();
    assert!(slot.get(dir.path()).is_none());
    std::fs::remove_file(&path).unwrap();
    let loaded = slot.get(dir.path()).expect("retried after a failure");
    assert_eq!(slot.get(dir.path()).unwrap().kid(), loaded.kid());
}

#[test]
fn only_hosted_agent_relay_tools_are_named() {
    assert_eq!(tool("mcp__agent-relay__send_message"), Some("send_message"));
    assert_eq!(
        tool("mcp__plugin_relay_agent-relay__join_relay"),
        Some("join_relay")
    );
    assert_eq!(
        tool("mcp__plugin_my_plugin_agent-relay__list-agents"),
        Some("list-agents")
    );
    for name in [
        "mcp__github__create_issue",
        "mcp__agent-relay-sessions__send_relay_message",
        "mcp__agent-relay__",
        "mcp__plugin__agent-relay__send",
        "mcp__agent-relay__send message",
        "agent-relay__send_message",
        "Bash",
    ] {
        assert_eq!(tool(name), None, "{name}");
    }
}

#[test]
fn tool_use_ids_are_bounded_printable_ascii() {
    assert!(valid_tool_use_id("toolu_01ABCdef"));
    assert!(!valid_tool_use_id(""));
    assert!(!valid_tool_use_id("toolu with space"));
    assert!(!valid_tool_use_id("toolu\n"));
    assert!(!valid_tool_use_id(&"a".repeat(TOOL_USE_ID_MAX + 1)));
}

fn pre_tool_use(input: Value) -> Value {
    json!({
        "session_id": SID,
        "transcript_path": "/tmp/t.jsonl",
        "cwd": "/tmp",
        "permission_mode": "default",
        "hook_event_name": "PreToolUse",
        "tool_name": "mcp__agent-relay__send_message",
        "tool_use_id": "toolu_01ABC",
        "tool_input": input,
    })
}

#[test]
fn the_hook_sends_only_the_call_identity_and_overwrites_a_forged_proof() {
    let input = pre_tool_use(json!({
        "to": "alice", "text": "hi", "nested": {"keep": [1, 2]},
        FIELD: "forged.by.model",
    }));
    let mut sent = None;
    let output = hook::output(&input, |body| {
        sent = Some(serde_json::from_slice::<Value>(body).unwrap());
        Some(br#"{"ok":true,"data":{"attestation":"h.p.s"}}"#.to_vec())
    })
    .unwrap();
    assert_eq!(
        sent.unwrap(),
        json!({"session_id":SID, "tool_name":"mcp__agent-relay__send_message", "tool_use_id":"toolu_01ABC"})
    );
    assert_eq!(
        output,
        json!({"hookSpecificOutput":{
            "hookEventName":"PreToolUse",
            "updatedInput":{"to":"alice","text":"hi","nested":{"keep":[1,2]},FIELD:"h.p.s"},
        }})
    );
    assert!(output["hookSpecificOutput"]
        .get("permissionDecision")
        .is_none());
}

#[test]
fn a_named_join_is_applied_locally_before_hosted_arguments_are_rewritten() {
    let mut input = pre_tool_use(json!({"name":"claude-assistant", "description":"My chat"}));
    input["tool_name"] = json!("mcp__agent-relay__join_relay");
    let output = hook::output(&input, |body| {
        let request: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(request["registration"], json!({"name":"claude-assistant", "description":"My chat"}));
        Some(br#"{"ok":true,"data":{"attestation":"h.p.s","native_join":true}}"#.to_vec())
    }).unwrap();
    let arguments = &output["hookSpecificOutput"]["updatedInput"];
    assert_eq!(arguments, &json!({FIELD:"h.p.s"}));

    // A proof alone does not confirm that this name is the local identity.
    let output = hook::output(&input, |_| Some(br#"{"ok":true,"data":{"attestation":"h.p.s"}}"#.to_vec())).unwrap();
    assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(output["hookSpecificOutput"].get("updatedInput").is_none());
    input["tool_input"] = json!({"desktop_address":"claude@desktop", "name":"must-not-register"});
    assert!(hook_request(&input).unwrap().get("registration").is_none());
    input["tool_input"] = json!({});
    assert!(hook_request(&input).unwrap().get("registration").is_none());
}

#[test]
fn an_unnamed_unregistered_join_explains_the_local_registration_step() {
    let mut input = pre_tool_use(json!({}));
    input["tool_name"] = json!("mcp__agent-relay__join_relay");
    let output = hook::output(&input, |_| Some(br#"{"ok":false,"error":{"code":"not_a_relay_session"}}"#.to_vec())).unwrap();
    assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
    assert!(output["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap().contains("call join_relay with that name"));
    assert!(output["hookSpecificOutput"].get("updatedInput").is_none());
}

#[test]
fn the_hook_is_silent_without_a_proof() {
    let input = pre_tool_use(json!({"text":"hi"}));
    // The core is unreachable or refuses (an unregistered chat, say).
    assert!(hook::output(&input, |_| None).is_none());
    for body in [
        &br#"{"ok":false,"error":{"code":"not_registered"}}"#[..],
        br#"{"ok":true,"data":{}}"#,
        br#"{"ok":true,"data":{"attestation":7}}"#,
        b"not json",
    ] {
        assert!(hook::output(&input, |_| Some(body.to_vec())).is_none());
    }
    // Not a PreToolUse call with object arguments: the core is not asked.
    for input in [
        json!({"hook_event_name":"PostToolUse","session_id":SID,"tool_name":"mcp__agent-relay__x","tool_use_id":"t","tool_input":{}}),
        pre_tool_use(json!("text")),
        json!({"hook_event_name":"PreToolUse","tool_name":"mcp__agent-relay__x","tool_use_id":"t","tool_input":{}}),
    ] {
        let output = hook::output(&input, |_| panic!("the core is not asked"));
        assert!(output.is_none());
    }
}

#[test]
fn a_named_join_gives_a_quoted_local_command_without_running_it() {
    let mut input = pre_tool_use(json!({"name":"claude-assistant"}));
    input["tool_name"] = json!("mcp__agent-relay__join_relay");
    let output = hook::output_at(&input, Some(Path::new("/tmp/Agent Relay/relay.sock")), |_| {
        Some(br#"{"ok":false,"error":{"code":"registration_required"}}"#.to_vec())
    }).unwrap();
    assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
    let reason = output["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
    for text in ["socket-request", "--path /register", "claude-assistant", "normal command permission check", "shares its conversation", "without name or description"] {
        assert!(reason.contains(text), "{reason}");
    }
    let command = hook::registration_command_for(Path::new("/Apps/Agent Relay/helper"), Path::new("/tmp/a'b.sock"), &json!({"name":"claude-assistant", "description":"$(not-executed) ' text"})).unwrap();
    assert!(command.starts_with("'/Apps/Agent Relay/helper' socket-request"));
    assert!(command.contains("'\\''"));
    assert!(command.contains("--body '{"));
}

#[test]
fn desktop_joins_never_continue_without_a_well_formed_proof() {
    for arguments in [json!({"name":"claude-assistant"}), json!({})] {
        let mut input = pre_tool_use(arguments);
        input["tool_name"] = json!("mcp__agent-relay__join_relay");
        assert_eq!(hook::output(&input, |_| None).unwrap()["hookSpecificOutput"]["permissionDecision"], "deny");
        for response in [
            b"not json".to_vec(),
            br#"{"ok":false,"error":{"code":"attestation_unavailable"}}"#.to_vec(),
            br#"{"ok":false,"error":{"code":"registration_failed"}}"#.to_vec(),
            br#"{"ok":true,"data":{}}"#.to_vec(),
            br#"{"ok":true,"data":{"attestation":7}}"#.to_vec(),
            br#"{"ok":true,"data":{"attestation":""}}"#.to_vec(),
            br#"{"ok":true,"data":{"attestation":"bad"}}"#.to_vec(),
            br#"{"ok":true,"data":{"attestation":".p.s"}}"#.to_vec(),
        ] {
            let output = hook::output(&input, |_| Some(response.clone())).unwrap();
            assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
            let reason = output["hookSpecificOutput"]["permissionDecisionReason"].as_str().unwrap();
            assert!(reason.contains("retry this join"));
            assert!(reason.contains("Do not substitute a hosted account identity"));
            assert!(output["hookSpecificOutput"].get("updatedInput").is_none());
        }
    }
    assert!(hook::output(&pre_tool_use(json!({"text":"hi"})), |_| None).is_none());
}
fn core_on(socket: &str, app_data: &str) -> agent_relay_node::relay::daemon::Config {
    agent_relay_node::relay::daemon::Config {
        home: "/home/me".into(),
        app_data_dir: app_data.into(),
        paths: agent_relay_node::relay::paths::CorePaths::for_socket(socket.into()).unwrap(),
        env: Default::default(),
        version: "test".into(),
        host_name: "test".into(),
        rediscover_every: std::time::Duration::from_secs(3600),
        push_timing: Default::default(),
        delivery_timing: Default::default(),
    }
}

#[cfg(unix)]
#[test]
fn a_profile_build_names_its_own_server_and_only_its_tools() {
    let production = core_on("/home/me/.agentworkforce/desktop/core.sock", "/a/desktop");
    assert_eq!(server_for(&production), SERVER);
    assert_eq!(server_for(&core_on("/s/core.dev.sock", "/a/x.dev")), SERVER);
    let profile = core_on("/s/core.com.agentrelay.desktop.devaws.sock", "/a/p");
    assert_eq!(server_for(&profile), "agent-relay-devaws");
    assert_eq!(
        server_for(&core_on("/s/core.devaws.sock", "/a/p")),
        "agent-relay-devaws"
    );
    let long = server_for(&core_on(
        "/s/core.a-very-long-profile-identifier.sock",
        "/a/p",
    ));
    assert_eq!(long.len(), SERVER.len() + 1 + 12);
    assert!(long.len() + "mcp____join_relay".len() <= 64);
    assert_eq!(matcher_for(SERVER), MATCHER);
    assert_eq!(
        matcher_for("agent-relay-devaws"),
        "^mcp__agent-relay-devaws__.+$"
    );
    let call = "mcp__agent-relay-devaws__join_relay";
    assert_eq!(tool_on(call, "agent-relay-devaws"), Some("join_relay"));
    assert_eq!(tool(call), None);
    assert_eq!(
        tool_on("mcp__agent-relay__join_relay", "agent-relay-devaws"),
        None
    );
    assert_eq!(
        tool_on(
            "mcp__plugin_x_agent-relay__join_relay",
            "agent-relay-devaws"
        ),
        None
    );
    assert_eq!(hook_tool(call), Some("join_relay"));
    assert_eq!(
        hook_tool("mcp__agent-relay__join_relay"),
        Some("join_relay")
    );
}
