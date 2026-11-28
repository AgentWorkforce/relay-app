//! Two invented Claude chats through the real route and a fake Cloud.
use super::*;
use agent_relay_node::relay::store::{AgentAlias, RetiredAlias};
use sha2::{Digest, Sha256};
use std::sync::atomic::Ordering;

const SEND: &str = "mcp__agent-relay__send_relay_message";
const JOIN: &str = "mcp__agent-relay__join_relay";
const ALIAS_TOKEN: &str = "synthetic-claude-alias-token";

fn capabilities(handoff: bool) -> Value {
    let mut value = json!({"claude_session_attestation":1, "claude_active_alias":1});
    if handoff {
        value["claude_attestation_handoff"] = json!(1);
    }
    value
}

fn first_chat() -> Target {
    Target::new(SessionKind::ClaudeTerminal, REGISTERED.to_ascii_lowercase())
}

fn with_first_chat(f: &Fixture, change: impl FnOnce(&mut Registration)) {
    f.core
        .store
        .update(|store| {
            let target = first_chat();
            change(
                store
                    .registrations
                    .iter_mut()
                    .find(|r| r.target == target)
                    .unwrap(),
            );
        })
        .unwrap();
}

fn rename(f: &Fixture) {
    with_first_chat(f, |r| {
        let alias = AgentAlias {
            name: "renamed-chat".into(),
            agent_id: "agent-renamed-chat".into(),
            token: Secret::new(ALIAS_TOKEN),
            registered_at: iso8601_from_seconds(2.0),
            gate: None,
        };
        r.retired_aliases = Some(vec![RetiredAlias {
            alias: AgentAlias {
                name: "previous-chat".into(),
                agent_id: "agent-previous-chat".into(),
                token: Secret::new("synthetic-claude-previous-token"),
                ..alias.clone()
            },
            retired_at: iso8601_from_seconds(1.0),
        }]);
        r.alias = Some(alias);
    });
}

fn add_second_chat(f: &Fixture) {
    f.core
        .store
        .update(|store| {
            let target = Target::new(SessionKind::ClaudeTerminal, UNREGISTERED);
            assert!(store.add(
                registration("second-chat", target),
                Secret::new("synthetic-second-token")
            ));
        })
        .unwrap();
}

fn claims(result: &Value) -> Value {
    decode(
        result["attestation"]
            .as_str()
            .unwrap()
            .split('.')
            .nth(1)
            .unwrap(),
    )
}

fn methods(f: &Fixture) -> Vec<String> {
    f.cloud
        .calls
        .lock()
        .unwrap()
        .iter()
        .map(|(method, _, _)| method.clone())
        .collect()
}

fn addresses(f: &Fixture) -> (String, String) {
    f.core.store.read(|store| {
        let r = store.registration_for(&first_chat()).unwrap();
        (
            r.link_address(&f.core.machine_prefix),
            r.current_address(&f.core.machine_prefix),
        )
    })
}

#[tokio::test]
async fn v1_works_without_a_user_id_or_numeric_sync_count() {
    for receipt in [json!({"synced": true}), json!({})] {
        let f = fixture();
        let mut account = f.cloud.account().unwrap();
        account.user_id = None;
        *f.cloud.account_override.lock().unwrap() = Some(account);
        *f.cloud.put_response.lock().unwrap() = Some(receipt);
        f.negotiate(json!({"claude_session_attestation": 1})).await;

        let answer = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
        assert_eq!(claims(&answer)["v"], 1);
        assert!(f.last_put().get("claude_attestation_version").is_none());
    }
}

#[tokio::test]
async fn failed_refresh_preserves_only_the_unchanged_accepted_v2_sender() {
    for status in [503, 403] {
        let f = fixture();
        add_second_chat(&f);
        rename(&f);
        f.negotiate(capabilities(false)).await;
        f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
        with_first_chat(&f, |r| {
            r.alias.as_mut().unwrap().token = Secret::new("rotated-synthetic-token");
        });

        *f.cloud.failure.lock().unwrap() = Some(CloudError::Status {
            status,
            message: None,
        });
        let previous_calls = methods(&f).len();
        let error = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap_err();
        assert_eq!(error.code, "claude_identity_stale");
        assert_eq!(methods(&f)[previous_calls..], ["PUT"]);
        *f.cloud.failure.lock().unwrap() = None;

        let before = methods(&f);
        let peer = f.ask(IN_UNREGISTERED, call(UNREGISTERED, SEND)).await.unwrap();
        assert_eq!(claims(&peer)["v"], 2);
        assert_eq!(claims(&peer)["agent_id"], "agent-second-chat");
        let error = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap_err();
        assert_eq!(error.code, "claude_identity_stale");
        assert_eq!(methods(&f), before, "the refused snapshot is backed off");
    }
}

#[tokio::test]
async fn rename_exports_the_stable_link_and_only_the_current_native_credential() {
    let f = fixture();
    add_second_chat(&f);
    f.negotiate(json!({"claude_session_attestation":1})).await;
    let old = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
    let original = f.last_put();
    assert!(original.get("claude_attestation_version").is_none());
    rename(&f);
    let legacy = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
    assert_eq!(claims(&old)["v"], 1);
    assert_eq!(claims(&legacy)["v"], 1);
    assert_eq!(f.last_put(), original);

    f.negotiate(capabilities(false)).await;
    let mut request = call(REGISTERED, SEND);
    request["agent_id"] = json!("agent-second-chat");
    request["source_id"] = json!("arbitrary-source");
    let argument_proof = f.ask(IN_REGISTERED, request).await.unwrap();
    assert_eq!(argument_proof["handoff"], false);
    let proof = claims(&argument_proof);
    let body = f.last_put();
    assert_eq!(body["claude_attestation_version"], 2);
    let expected = json!({"kind":"claude", "id":first_chat().id});
    let rows = body["identities"].as_array().unwrap();
    let native: Vec<_> = rows
        .iter()
        .filter(|row| row["native_session"] == expected)
        .collect();
    let (stable, current) = addresses(&f);
    assert_eq!(
        native,
        vec![
            &json!({"name":"renamed-chat","agent_id":"agent-renamed-chat",
        "address":current, "token":ALIAS_TOKEN, "native_session":expected})
        ]
    );
    assert!(
        rows.contains(&json!({"name":"claude-chat","agent_id":"agent-claude-chat",
        "address":stable,"token":"t1"}))
    );
    let other = original["identities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "second-chat")
        .unwrap();
    assert!(rows.contains(other));
    assert!(!rows.iter().any(|row| row["name"] == "previous-chat"));
    assert_eq!(proof["v"], 2);
    assert_eq!(proof["src"], body["source_id"]);
    assert_eq!(proof["sid"], first_chat().id);
    assert_eq!(proof["agent_id"], "agent-renamed-chat");
    assert_eq!(proof["link_address"], stable);
    assert_eq!(
        proof["credential_sha256"],
        URL_SAFE_NO_PAD.encode(Sha256::digest(format!("toolu_01ABC\0{ALIAS_TOKEN}")))
    );
    assert!(!proof.to_string().contains(ALIAS_TOKEN));
    let mut second_call = call(UNREGISTERED, SEND);
    second_call["tool_use_id"] = json!("toolu_second");
    let other_proof = f.ask(IN_UNREGISTERED, second_call).await.unwrap();
    assert_eq!(claims(&other_proof)["agent_id"], "agent-second-chat");
    assert_eq!(claims(&other_proof)["sid"], UNREGISTERED);
    assert_eq!(
        f.ask(IN_UNREGISTERED, call(REGISTERED, SEND))
            .await
            .unwrap_err()
            .code,
        OutboundError::not_a_session().code
    );

    // The broker-facing credential lookup and reply owner used by Desktop
    // delivery still route all three addresses to the one original chat.
    f.core.store.read(|store| {
        for (id, token) in [
            ("agent-claude-chat", "t1"),
            ("agent-renamed-chat", ALIAS_TOKEN),
            ("agent-previous-chat", "synthetic-claude-previous-token"),
        ] {
            let (receiver, credential) = store.credential_for_agent_id(id).unwrap();
            assert_eq!(receiver.target, first_chat());
            assert_eq!(store.reply_owner(&receiver).agent_id, "agent-claude-chat");
            assert_eq!(
                store.link_address_for(&receiver, &f.core.machine_prefix),
                stable
            );
            assert_eq!(credential.expose(), token);
        }
    });

    f.negotiate(capabilities(true)).await;
    let mut handoff_call = call(REGISTERED, SEND);
    handoff_call["tool_use_id"] = json!("toolu_handoff");
    let handoff = f.ask(IN_REGISTERED, handoff_call).await.unwrap();
    assert_eq!(handoff["handoff"], true);
    assert_eq!(
        claims(&handoff)["credential_sha256"],
        URL_SAFE_NO_PAD.encode(Sha256::digest(format!("toolu_handoff\0{ALIAS_TOKEN}")))
    );
    assert_eq!(proof["workspace_id"], WORKSPACE);
    assert_eq!(proof["user_id"], "user-1");
    for reply in [&argument_proof, &handoff, &other_proof] {
        let proof = reply["attestation"].as_str().unwrap();
        let (signed, signature) = proof.rsplit_once('.').unwrap();
        ring::signature::UnparsedPublicKey::new(
            &ring::signature::ED25519,
            URL_SAFE_NO_PAD
                .decode(body["attestation_key"]["public_key"].as_str().unwrap())
                .unwrap(),
        )
        .verify(
            signed.as_bytes(),
            &URL_SAFE_NO_PAD.decode(signature).unwrap(),
        )
        .unwrap();
    }
    let calls = f.cloud.calls.lock().unwrap();
    assert_eq!(
        calls.last().unwrap().2,
        Some(json!({"attestation": handoff["attestation"]}))
    );
    drop(calls);
    if let Ok(path) = std::env::var("RELAY_CLAUDE_ALIAS_FIXTURE") {
        let fixture = json!({
            "synthetic": true, "account": {"workspace_id": WORKSPACE, "user_id":"user-1"},
            "identity_sync": body,
            "sync_receipt": {"synced": rows.len(), "claude_attestation_version": 2, "identities": []},
            "argument_proof": argument_proof["attestation"],
            "handoff": {"attestation": handoff["attestation"]},
            "argument_metadata": {"claudecode/toolUseId": "toolu_01ABC"},
            "handoff_metadata": {"claudecode/toolUseId": "toolu_handoff"},
            "second_chat_metadata": {"claudecode/toolUseId": "toolu_second"},
            "tool": "send_relay_message", "expected_agent_id":"agent-renamed-chat",
            "second_chat_proof": other_proof["attestation"],
        });
        std::fs::write(path, serde_json::to_vec_pretty(&fixture).unwrap()).unwrap();
    }
}

#[tokio::test]
async fn join_only_accepts_the_proven_chats_stable_or_current_address() {
    let f = fixture();
    add_second_chat(&f);
    rename(&f);
    let (stable, current) = addresses(&f);
    let other = f.core.store.read(|store| {
        store
            .registration_named("second-chat")
            .unwrap()
            .link_address(&f.core.machine_prefix)
    });
    for handoff in [false, true] {
        f.negotiate(capabilities(handoff)).await;
        for address in [&stable, &current, &other] {
            let mut join = call(REGISTERED, JOIN);
            join["desktop_address"] = json!(address);
            let result = f.ask(IN_REGISTERED, join).await;
            if address == &other {
                assert_eq!(result.unwrap_err().code, "invalid_request");
            } else {
                let answer = result.unwrap();
                assert_eq!(answer["handoff"], handoff);
                assert_eq!(claims(&answer)["agent_id"], "agent-renamed-chat");
                assert_eq!(claims(&answer)["link_address"], stable);
            }
        }
    }
}

#[tokio::test]
async fn stale_refused_or_skipped_alias_cannot_post_a_proof_and_reconnect_recovers() {
    let f = fixture();
    add_second_chat(&f);
    f.negotiate(capabilities(true)).await;
    f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
    rename(&f);

    let refuse = |status| CloudError::Status {
        status,
        message: None,
    };
    *f.cloud.failure.lock().unwrap() = Some(refuse(503));
    let old_posts = methods(&f).iter().filter(|m| *m == "POST").count();
    for _ in 0..2 {
        let error = f
            .ask(IN_REGISTERED, call(REGISTERED, SEND))
            .await
            .unwrap_err();
        assert_eq!(error.code, "claude_identity_stale");
    }
    assert_eq!(
        methods(&f).iter().filter(|m| *m == "POST").count(),
        old_posts
    );

    *f.cloud.failure.lock().unwrap() = None;
    f.cloud.generation.fetch_add(1, Ordering::SeqCst);
    f.negotiate(capabilities(true)).await;
    *f.cloud.put_response.lock().unwrap() = Some(
        json!({"synced":3, "claude_attestation_version":2, "skipped":[
            {"agent_id":"agent-renamed-chat", "reason":"unverified"}
        ]}),
    );
    let error = f
        .ask(IN_REGISTERED, call(REGISTERED, SEND))
        .await
        .unwrap_err();
    assert_eq!(error.code, "claude_identity_stale");
    assert_eq!(
        methods(&f).iter().filter(|m| *m == "POST").count(),
        old_posts
    );
    let accepted_peer = f
        .ask(IN_UNREGISTERED, call(UNREGISTERED, SEND))
        .await
        .unwrap();
    assert_eq!(accepted_peer["handoff"], true);

    *f.cloud.put_response.lock().unwrap() = None;
    f.cloud.generation.fetch_add(1, Ordering::SeqCst);
    let error = f
        .ask(IN_REGISTERED, call(REGISTERED, SEND))
        .await
        .unwrap_err();
    assert_eq!(error.code, "attestation_unavailable");
    f.negotiate(capabilities(true)).await;
    let proof = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
    assert_eq!(claims(&proof)["agent_id"], "agent-renamed-chat");
}

#[tokio::test]
async fn rotation_during_sync_or_handoff_requires_a_new_call() {
    let f = fixture();
    rename(&f);
    f.negotiate(capabilities(true)).await;
    for method in ["PUT", "POST"] {
        let delay = if method == "PUT" {
            &f.cloud.delay_ms
        } else {
            &f.cloud.post_delay_ms
        };
        delay.store(100, Ordering::SeqCst);
        f.cloud.calls.lock().unwrap().clear();
        let (result, ()) = tokio::join!(f.ask(IN_REGISTERED, call(REGISTERED, SEND)), async {
            tokio::time::timeout(Duration::from_secs(1), async {
                while !methods(&f).iter().any(|m| m == method) {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            with_first_chat(&f, |r| {
                r.alias.as_mut().unwrap().token =
                    Secret::new(format!("synthetic-rotation-{method}"));
            });
        });
        assert_eq!(result.unwrap_err().code, "claude_identity_stale");
        if method == "PUT" {
            assert!(!methods(&f).iter().any(|m| m == "POST"));
        }
        delay.store(0, Ordering::SeqCst);
    }
    let result = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
    assert_eq!(
        claims(&result)["credential_sha256"],
        URL_SAFE_NO_PAD.encode(Sha256::digest(b"toolu_01ABC\0synthetic-rotation-POST"))
    );
}

#[tokio::test]
async fn a_changed_registration_source_or_workspace_needs_its_own_acceptance() {
    let f = fixture();
    f.negotiate(capabilities(false)).await;
    f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();

    with_first_chat(&f, |r| r.registered_at = iso8601_from_seconds(5.0));
    *f.cloud.failure.lock().unwrap() = Some(CloudError::Unreachable);
    let result = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await;
    assert_eq!(result.unwrap_err().code, "claude_identity_stale");
    *f.cloud.failure.lock().unwrap() = None;
    f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();

    let source = "123e4567-e89b-42d3-a456-426614174001";
    f.core
        .store
        .update(|store| {
            store.desktop_sync_source_id = Some(source.into());
        })
        .unwrap();
    *f.cloud.failure.lock().unwrap() = Some(CloudError::Unreachable);
    let result = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await;
    assert_eq!(result.unwrap_err().code, "claude_identity_stale");
    *f.cloud.failure.lock().unwrap() = None;
    let result = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
    assert_eq!(claims(&result)["src"], source);

    f.core
        .store
        .update(|store| {
            store.cloud.as_mut().unwrap().account_workspace_id = Some("other-workspace".into());
        })
        .unwrap();
    let result = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await;
    assert_eq!(result.unwrap_err().code, "claude_identity_stale");
}

#[tokio::test]
async fn an_expired_acceptance_requires_a_successful_refresh_even_for_an_unchanged_sender() {
    let f = fixture();
    f.negotiate(capabilities(false)).await;
    f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(30 * 60)).await;
    *f.cloud.failure.lock().unwrap() = Some(CloudError::Unreachable);
    let result = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await;
    assert_eq!(result.unwrap_err().code, "claude_identity_stale");
    *f.cloud.failure.lock().unwrap() = None;
    assert!(f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.is_ok());
}

#[tokio::test]
async fn a_v2_sync_receipt_must_confirm_the_source_proof_version_and_count() {
    for receipt in [
        json!({"synced": 3}),
        json!({"claude_attestation_version": 2}),
        json!({"claude_attestation_version": 2, "synced": true}),
        json!({"claude_attestation_version": 2, "synced": 0}),
    ] {
        let f = fixture();
        rename(&f);
        f.negotiate(capabilities(true)).await;
        *f.cloud.put_response.lock().unwrap() = Some(receipt);
        let result = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await;
        assert_eq!(result.unwrap_err().code, "claude_identity_stale");
        assert!(!methods(&f).iter().any(|m| m == "POST"));
    }
}

#[tokio::test]
async fn a_slow_identity_refresh_cannot_hold_the_hook_past_its_budget() {
    let f = fixture();
    rename(&f);
    f.negotiate(capabilities(true)).await;
    f.cloud
        .delay_ms
        .store(ROUTE_BUDGET.as_millis() as u64, Ordering::SeqCst);
    tokio::time::pause();
    let start = tokio::time::Instant::now();
    let result = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await;
    assert_eq!(result.unwrap_err().code, "claude_identity_stale");
    assert!(start.elapsed() < ROUTE_BUDGET);
    assert!(methods(&f).iter().any(|m| m == "PUT"));
    assert!(!methods(&f).iter().any(|m| m == "POST"));
}

#[tokio::test]
async fn a_refresh_leaves_a_slow_handoff_time_to_land() {
    // #2283: a refresh took most of the core's budget and left the handoff
    // one second, so a Cloud answering in two denied the call.
    let f = fixture();
    rename(&f);
    f.negotiate(capabilities(true)).await;
    f.cloud.delay_ms.store(3_000, Ordering::SeqCst);
    f.cloud.post_delay_ms.store(5_000, Ordering::SeqCst);
    tokio::time::pause();
    let start = tokio::time::Instant::now();
    let answer = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
    assert_eq!(answer["handoff"], true);
    assert_eq!(f.cloud.posts_stored.load(Ordering::SeqCst), 1);
    assert!(start.elapsed() >= std::time::Duration::from_secs(8));
    assert!(start.elapsed() < ROUTE_BUDGET);
}

#[tokio::test]
async fn a_refresh_using_all_its_time_still_leaves_the_handoff_ten_seconds() {
    // The handoff's share is pinned: at least 10 seconds of the budget stay
    // for Cloud to store the proof, however long the refresh before it took.
    assert!(HANDOFF_RESERVE >= std::time::Duration::from_secs(10));
    assert!(HANDOFF_RESERVE < ROUTE_BUDGET);
    let f = fixture();
    rename(&f);
    f.negotiate(capabilities(true)).await;
    // The refresh ends just inside its allowance; Cloud then takes just
    // under the reserve to store the proof.
    let refresh = ROUTE_BUDGET - HANDOFF_RESERVE - std::time::Duration::from_millis(100);
    f.cloud
        .delay_ms
        .store(refresh.as_millis() as u64, Ordering::SeqCst);
    f.cloud.post_delay_ms.store(
        (HANDOFF_RESERVE - std::time::Duration::from_millis(100)).as_millis() as u64,
        Ordering::SeqCst,
    );
    tokio::time::pause();
    let start = tokio::time::Instant::now();
    let answer = f.ask(IN_REGISTERED, call(REGISTERED, SEND)).await.unwrap();
    assert_eq!(answer["handoff"], true);
    assert_eq!(f.cloud.posts_stored.load(Ordering::SeqCst), 1);
    assert!(start.elapsed() < crate::claude_attestation::hook::TIMEOUT);
}
