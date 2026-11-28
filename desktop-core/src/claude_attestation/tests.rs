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
    let request = hook_request(&input).unwrap();
    assert_eq!(request["desktop_address"], "claude@desktop");
    assert!(request.get("registration").is_none());
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
        hook::Reply::Answer(403, br#"{"ok":false,"error":{"code":"registration_required"}}"#.to_vec())
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

#[test]
fn a_handed_off_proof_leaves_the_call_unchanged() {
    // Auto mode refuses a call whose input a hook changed, so nothing is printed.
    let input = pre_tool_use(json!({"to": "alice", "text": "hi"}));
    let output = hook::output(&input, |_| {
        Some(br#"{"ok":true,"data":{"attestation":"h.p.s","handoff":true}}"#.to_vec())
    });
    assert!(output.is_none());

    // A native join keeps its own name: Cloud accepts it, so nothing is stripped.
    let mut input = pre_tool_use(json!({"name":"claude-assistant", "description":"My chat"}));
    input["tool_name"] = json!("mcp__agent-relay__join_relay");
    let output = hook::output(&input, |_| {
        Some(
            br#"{"ok":true,"data":{"attestation":"h.p.s","native_join":true,"handoff":true}}"#
                .to_vec(),
        )
    });
    assert!(output.is_none());
}

#[test]
fn a_core_still_answering_when_the_hook_stops_waiting_denies_the_call() {
    // #2283: the core took the request and was still handing the proof to
    // Cloud when the hook's wait ran out. Sent unsigned, the call reached
    // Cloud before its proof, so Cloud ran it without the chat's identity.
    // It is denied with retry guidance instead.
    for tool in ["send_relay_message", "join_relay", "spawn_relay_agent"] {
        let mut input = pre_tool_use(json!({"to": "alice", "text": "hi"}));
        input["tool_name"] = json!(format!("mcp__agent-relay__{tool}"));
        let output = hook::output_at(&input, None, |_| hook::Reply::Late).unwrap();
        let decision = &output["hookSpecificOutput"];
        assert_eq!(decision["permissionDecision"], "deny", "{tool}");
        assert!(decision.get("updatedInput").is_none(), "{tool}");
        let retry = if tool == "join_relay" { "retry this join" } else { "retry this tool call" };
        assert!(
            decision["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .contains(retry),
            "{tool}"
        );
    }
    // With no core to ask, nothing was sent and nothing is on its way.
    let input = pre_tool_use(json!({"to": "alice", "text": "hi"}));
    assert!(hook::output_at(&input, None, |_| hook::Reply::Unavailable).is_none());
    // A tool that never acts as the chat is never held back.
    let mut input = pre_tool_use(json!({}));
    input["tool_name"] = json!("mcp__agent-relay__list_relay_agents");
    assert!(hook::output_at(&input, None, |_| panic!("the core is not asked")).is_none());
}

#[test]
fn tools_that_never_act_as_the_chat_are_not_signed() {
    for tool in [
        "mcp__agent-relay__list_relay_agents",
        "mcp__agent-relay__get_relay_agent_card",
        "mcp__agent-relay__search_shared_sessions",
        "mcp__agent-relay__list_session_summaries",
        "mcp__plugin_relay_agent-relay__get_shared_session",
        "mcp__agent-relay__list_team_activity",
        "mcp__agent-relay__show_ui",
    ] {
        let mut input = pre_tool_use(json!({"query": "x"}));
        input["tool_name"] = json!(tool);
        assert!(hook_request(&input).is_none(), "{tool}");
        assert!(hook::output(&input, |_| panic!("the core is not asked for {tool}")).is_none());
    }
    for tool in [
        "mcp__agent-relay__send_relay_message",
        "mcp__agent-relay__check_relay_inbox",
        "mcp__agent-relay__describe_capabilities",
        "mcp__agent-relay__a_tool_cloud_adds_later",
    ] {
        let mut input = pre_tool_use(json!({}));
        input["tool_name"] = json!(tool);
        assert!(hook_request(&input).is_some(), "{tool}");
    }
}

/// A core at a real socket that takes one request and then does `then`.
#[cfg(unix)]
fn core_that(
    then: impl FnOnce(std::os::unix::net::UnixStream) + Send + 'static,
) -> (tempfile::TempDir, std::path::PathBuf) {
    use std::io::Read;
    let root = tempfile::tempdir().unwrap();
    let socket = root.path().join("relay.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        // Read the whole request, body included, as the core does. Closing a
        // Unix socket with unread data resets the connection on Linux, so the
        // hook would read an error instead of the reply the fixture sent.
        let mut seen = Vec::new();
        let mut chunk = [0; 4096];
        let complete = |seen: &[u8]| {
            let end = seen.windows(4).position(|w| w == b"\r\n\r\n")?;
            let headers = std::str::from_utf8(&seen[..end]).ok()?;
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().ok())?
                })
                .unwrap_or(0);
            (seen.len() >= end + 4 + length).then_some(())
        };
        while complete(&seen).is_none() {
            let read = stream.read(&mut chunk).unwrap();
            assert!(read > 0, "the hook closed before sending its whole request");
            seen.extend_from_slice(&chunk[..read]);
        }
        then(stream);
    });
    (root, socket)
}

#[cfg(unix)]
#[test]
fn only_a_request_never_sent_counts_as_no_core() {
    let body = br#"{"session_id":"s","tool_name":"t","tool_use_id":"u"}"#;
    // Every case but the late one ends on its own (at once, at the core's
    // close, or at its answer), so its wait is generous: a loaded runner
    // cannot turn an answer into Late. Only the late case runs out its wait,
    // and that core holds the request until the test lets it go, so it can
    // never answer first.
    let wait = std::time::Duration::from_secs(30);
    let short = std::time::Duration::from_millis(200);

    // Absent: no socket file at all.
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("relay.sock");
    assert!(matches!(hook::ask_core(Some(&missing), body, wait), hook::Reply::Unavailable));
    assert!(matches!(hook::ask_core(None, body, wait), hook::Reply::Unavailable));

    // Absent: a socket file nothing listens on (a stopped core).
    let stale = root.path().join("stale.sock");
    drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
    assert!(matches!(hook::ask_core(Some(&stale), body, wait), hook::Reply::Unavailable));

    // Ambiguous: the core took the request and closed without answering.
    let (_root, socket) = core_that(drop);
    let reply = hook::ask_core(Some(&socket), body, wait);
    assert!(matches!(reply, hook::Reply::Late), "{reply:?}");

    // Late: the core took the request and is still working on it.
    let (release, held) = std::sync::mpsc::channel::<()>();
    let (_root, socket) = core_that(move |stream| {
        let _ = held.recv();
        drop(stream);
    });
    let reply = hook::ask_core(Some(&socket), body, short);
    drop(release);
    assert!(matches!(reply, hook::Reply::Late), "{reply:?}");

    // Answered.
    let (_root, socket) = core_that(|mut stream| {
        use std::io::Write;
        let payload = br#"{"ok":true,"data":{"attestation":"h.p.s","handoff":true}}"#;
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len()).unwrap();
        stream.write_all(payload).unwrap();
    });
    let reply = hook::ask_core(Some(&socket), body, wait);
    assert!(matches!(reply, hook::Reply::Answer(200, _)), "{reply:?}");
}

#[test]
fn an_answer_that_cannot_be_read_denies_the_call() {
    // #2283 review: the core's reply was cut short (or garbled), so whether
    // it started a handoff is unknown. The call is refused, not sent unsigned.
    let full = br#"{"ok":true,"data":{"attestation":"h.p.s","handoff":true}}"#;
    for (status, body) in [
        (200, full[..full.len() / 2].to_vec()),
        (200, Vec::new()),
        (503, br#"{"ok":false,"error":{"co"#.to_vec()),
        (500, b"not json".to_vec()),
    ] {
        let input = pre_tool_use(json!({"to": "alice", "text": "hi"}));
        let output = hook::output_at(&input, None, |_| hook::Reply::Answer(status, body.clone()))
            .unwrap_or_else(|| panic!("{status} {body:?} went out unsigned"));
        let decision = &output["hookSpecificOutput"];
        assert_eq!(decision["permissionDecision"], "deny");
        assert!(decision.get("updatedInput").is_none());
        assert!(decision["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("retry this tool call"));
    }
    // An explicit refusal the core sent in full still leaves the call as
    // it was: the core said it signed nothing.
    let input = pre_tool_use(json!({"to": "alice", "text": "hi"}));
    for (status, body) in [
        (409, &br#"{"ok":false,"error":{"code":"attestation_unavailable"}}"#[..]),
        (403, br#"{"ok":false,"error":{"code":"not_registered"}}"#),
        (500, br#"{"ok":false,"error":{"code":"internal"}}"#),
    ] {
        assert!(
            hook::output_at(&input, None, |_| hook::Reply::Answer(status, body.to_vec())).is_none(),
            "{status}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_reply_cut_short_on_the_socket_denies_the_call() {
    // The pinned socket client reads to the close and returns whatever body
    // arrived, so a reply truncated mid-body comes back as an answer.
    let (_root, socket) = core_that(|mut stream| {
        use std::io::Write;
        let payload = br#"{"ok":true,"data":{"attestation":"h.p.s","handoff":true}}"#;
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len()).unwrap();
        stream.write_all(&payload[..20]).unwrap();
    });
    let input = pre_tool_use(json!({"to": "alice", "text": "hi"}));
    let output = hook::output_at(&input, Some(&socket), |body| {
        // Ends at the core's close; the wait only bounds a broken fixture.
        hook::ask_core(Some(&socket), body, std::time::Duration::from_secs(30))
    })
    .expect("a truncated reply must not send the call unsigned");
    assert_eq!(output["hookSpecificOutput"]["permissionDecision"], "deny");
}
