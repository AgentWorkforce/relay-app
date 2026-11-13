use super::*;

pub(super) struct NativeFixture;
impl crate::copilot_dispatch::peer::Verifier for NativeFixture {
    fn runtime(
        &self,
        _: &crate::mcp_settings::Installer,
        _: &crate::mcp_settings::Binding,
        pid: i32,
        uid: u32,
    ) -> Result<agent_relay_node::relay::process::ProcessInfo, &'static str> {
        if pid != IN_REGISTERED + 10 {
            return Err("fixture_peer");
        }
        Ok(agent_relay_node::relay::process::ProcessInfo {
            pid: 103,
            parent: 1,
            uid,
            start: 5,
        })
    }
    fn held(
        &self,
        _: &crate::mcp_settings::Installer,
        runtime: agent_relay_node::relay::process::ProcessInfo,
        sid: &str,
    ) -> Result<crate::copilot_dispatch::intent::Peer, &'static str> {
        if sid != REGISTERED.to_ascii_lowercase() {
            return Err("fixture_sid");
        }
        Ok(crate::copilot_dispatch::intent::Peer {
            runtime,
            hold: crate::copilot_dispatch::intent::FileIdentity {
                device: 1,
                inode: 2,
                birth_ns: 3,
            },
        })
    }
    fn hook(
        &self,
        i: &crate::mcp_settings::Installer,
        pid: i32,
        uid: u32,
        sid: &str,
    ) -> Result<crate::copilot_dispatch::intent::Peer, &'static str> {
        if pid != IN_REGISTERED {
            return Err("fixture_hook");
        }
        self.held(
            i,
            agent_relay_node::relay::process::ProcessInfo {
                pid: 103,
                parent: 1,
                uid,
                start: 5,
            },
            sid,
        )
    }
}

struct CopilotDirectory;
impl SessionDirectory for CopilotDirectory {
    fn sessions(&self) -> Vec<DiscoveredSession> {
        Vec::new()
    }
    fn copilot_hook(&self, pid: i32, input: &Value) -> Option<Value> {
        let sid = match pid {
            IN_REGISTERED => REGISTERED.to_ascii_lowercase(),
            IN_UNREGISTERED => UNREGISTERED.into(),
            _ => return None,
        };
        (input["sessionId"] == sid && input["relayEvent"] == "before").then(|| json!({}))
    }
}

#[tokio::test]
async fn copilot_route_issues_only_verified_registered_hook_intents_for_the_current_account_generation(
) {
    let f = fixture_with(|_| Arc::new(CopilotDirectory));
    let target = Target::new(SessionKind::Copilot, REGISTERED.to_ascii_lowercase());
    f.core
        .store
        .update(|store| {
            assert!(store.add(
                registration("copilot-chat", target.clone()),
                Secret::new("t3")
            ));
        })
        .unwrap();
    f.core.rediscover_workspace();
    assert!(f.core.workspace().is_some());
    f.negotiate(json!({"native_session_identity":1,"copilot_session_attestation":1}))
        .await;
    assert!(f.installer.copilot_attestation());
    let input = json!({"sessionId":target.id,"relayEvent":"before","toolName":"agentrelay-send_relay_message",
        "toolArgs":{"to":"peer","text":"hello","_agent_relay_attestation":"forged"}});
    let request = Request {
        method: "POST".into(),
        path: "/copilot/hook".into(),
        body: serde_json::to_vec(&input).unwrap(),
        ..Default::default()
    };
    let uid = agent_relay_node::relay::peer::current_uid();
    let answer = f
        .outbound
        .handle(IN_REGISTERED, uid, &request)
        .await
        .unwrap();
    assert!(answer.get("permissionDecision").is_none());
    let ticket = answer["modifiedArgs"][attestation::FIELD].as_str().unwrap();
    assert!(ticket.starts_with("copilot-intent-v1."));
    let call = f.dispatch.call(ticket, std::time::Instant::now()).unwrap();
    assert_eq!(call.owner.workspace, WORKSPACE);
    assert_eq!(call.owner.user, "user-1");
    assert_eq!(call.sid, target.id);
    assert_eq!(call.args["to"], "peer");
    assert_eq!(call.args["text"], "hello");
    assert!(call.args["idempotency_key"].is_string());
    assert!(call.args.get(attestation::FIELD).is_none());
    assert_eq!(
        f.outbound
            .handle(9999, uid, &request)
            .await
            .unwrap_err()
            .status,
        403
    );
    assert_eq!(
        f.outbound
            .handle(IN_REGISTERED, uid + 1, &request)
            .await
            .unwrap_err()
            .status,
        403
    );
    let mut borrowed = request.clone();
    let mut bad = input.clone();
    bad["sessionId"] = json!(UNREGISTERED);
    borrowed.body = serde_json::to_vec(&bad).unwrap();
    assert_eq!(
        f.outbound
            .handle(IN_REGISTERED, uid, &borrowed)
            .await
            .unwrap_err()
            .status,
        403
    );
    f.cloud
        .generation
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    assert!(!f.installer.copilot_attestation());
    assert_eq!(
        f.outbound
            .handle(IN_REGISTERED, uid, &request)
            .await
            .unwrap_err()
            .status,
        409
    );
}

pub(crate) async fn registered_copilot() -> anyhow::Result<Fixture> {
    let f = fixture_with(|_| Arc::new(CopilotDirectory));
    let added = f.core.store.update(|store| {
        store.add(
            registration(
                "copilot-chat",
                Target::new(SessionKind::Copilot, REGISTERED.to_ascii_lowercase()),
            ),
            Secret::new("t3"),
        )
    })?;
    anyhow::ensure!(added, "fixture registration already exists");
    f.core.rediscover_workspace();
    anyhow::ensure!(f.core.workspace().is_some(), "fixture workspace missing");
    f.negotiate(json!({"native_session_identity":1,"copilot_session_attestation":1}))
        .await;
    Ok(f)
}
pub(crate) fn hook_request(tool: &str, args: Value) -> anyhow::Result<Request> {
    Ok(Request {
        method: "POST".into(),
        path: "/copilot/hook".into(),
        body: serde_json::to_vec(
            &json!({"sessionId":REGISTERED.to_ascii_lowercase(),"relayEvent":"before","toolName":tool,"toolArgs":args}),
        )?,
        ..Default::default()
    })
}
#[tokio::test]
async fn joins_refuse_another_name_and_sign_matching_or_absent_names_without_description(
) -> anyhow::Result<()> {
    let f = registered_copilot().await?;
    let uid = agent_relay_node::relay::peer::current_uid();
    let refused = hook_request(
        "agentrelay-join_relay",
        json!({"name":"another-chat","description":"must not rename"}),
    )?;
    assert_eq!(
        f.outbound
            .handle(IN_REGISTERED, uid, &refused)
            .await
            .unwrap_err()
            .status,
        409
    );
    for mut args in [
        json!({"name":"copilot-chat","description":"do not forward","desktop_address":"copilot-chat@fixture"}),
        json!({"description":"also removed","desktop_address":"copilot-chat@fixture"}),
    ] {
        let request = hook_request("agentrelay-join_relay", args.clone())?;
        let answer = f
            .outbound
            .handle(IN_REGISTERED, uid, &request)
            .await
            .unwrap();
        args.as_object_mut().unwrap().remove("name");
        args.as_object_mut().unwrap().remove("description");
        assert!(answer["modifiedArgs"].get("name").is_none());
        assert!(answer["modifiedArgs"].get("description").is_none());
        let ticket = answer["modifiedArgs"][attestation::FIELD].as_str().unwrap();
        let call = f.dispatch.call(ticket, std::time::Instant::now()).unwrap();
        assert_eq!(call.args, args);
        assert_eq!(call.tool, "join_relay");
    }
    assert_eq!(
        f.core.store.read(|store| store
            .registration_for(&Target::new(
                SessionKind::Copilot,
                REGISTERED.to_ascii_lowercase()
            ))
            .unwrap()
            .display_name()
            .to_owned()),
        "copilot-chat"
    );
    Ok(())
}
#[tokio::test]
async fn non_relay_tools_get_no_proof_and_oversized_args_or_missing_registrations_cannot_sign(
) -> anyhow::Result<()> {
    let f = registered_copilot().await?;
    let uid = agent_relay_node::relay::peer::current_uid();
    for name in [
        "other-server-read",
        "agent-relay-send_relay_message",
        "agentrelay-relay-send_relay_message",
        "agentrelay-send_relay_message-extra",
        "agentrelay-unknown_tool",
    ] {
        let ordinary = hook_request(name, json!({"path":"fixture"}))?;
        assert_eq!(
            f.outbound
                .handle(IN_REGISTERED, uid, &ordinary)
                .await
                .unwrap(),
            json!({})
        );
    }
    let oversized = hook_request(
        "agentrelay-send_relay_message",
        json!({"text":"x".repeat(32*1024)}),
    )?;
    assert_eq!(
        f.outbound
            .handle(IN_REGISTERED, uid, &oversized)
            .await
            .unwrap_err()
            .status,
        409
    );
    let valid = hook_request(
        "agentrelay-send_relay_message",
        json!({"text":"small fixture"}),
    )?;
    assert!(
        f.outbound.handle(IN_REGISTERED, uid, &valid).await.unwrap()["modifiedArgs"]
            [attestation::FIELD]
            .is_string()
    );
    f.core.store.update(|store| {
        for registration in &mut store.registrations {
            if registration.target.kind == SessionKind::Copilot {
                registration.missing_since = Some("2026-11-06T00:00:00Z".into());
            }
        }
    })?;
    assert_eq!(
        f.outbound
            .handle(IN_REGISTERED, uid, &valid)
            .await
            .unwrap_err()
            .code,
        OutboundError::not_registered().code
    );
    Ok(())
}
