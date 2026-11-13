use super::*;

const MCP_PATH: &str = "/cloud/api/v1/mcp/shared-sessions";

fn override_response(
    h: &Harness,
    path: &str,
    token: &str,
    operation: &str,
    status: u16,
    value: Value,
) {
    h.cloud.state.lock().unwrap().response_overrides.insert(
        (path.into(), token.into(), operation.into()),
        (status, value),
    );
}

fn native_discovery(h: &Harness, token: &str, tools: &[&str]) {
    override_response(
        h,
        MCP_PATH,
        token,
        "initialize",
        200,
        json!({"jsonrpc":"2.0","id":1,"result":{
            "serverInfo":{"name":"fixture","version":"1"},"capabilities":{"experimental":{
                "com.agentrelay/native-session-identity":{"version":1},"com.agentrelay/desktop-provider-credentials":{"version":1}
            }}
        }}),
    );
    override_response(
        h,
        MCP_PATH,
        token,
        "tools/list",
        200,
        json!({"jsonrpc":"2.0","id":2,"result":{"tools":tools.iter().map(|name|json!({"name":name})).collect::<Vec<_>>()}}),
    );
}

fn row<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id)
        .unwrap()
}

async fn signed_in() -> Harness {
    let harness = Harness::start_with(|cloud, _| {
        cloud.state.lock().unwrap().relaycast_url = Some(cloud.site_url.clone());
    })
    .await;
    harness
        .client()
        .await
        .ok("account.sign_in", json!({"sharing":"new","flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    harness.account.recheck_auth_health().await;
    harness
}

#[tokio::test]
async fn native_connection_uses_scoped_discovery_and_refreshes_cache_and_host_state() {
    let harness = signed_in().await;
    let mut client = harness.client().await;
    client
        .ok("tools.select", json!({"tool_ids":["grok","paseo"]}))
        .await;
    let token = harness.cloud.state.lock().unwrap().provider_grants["grant-grok"]["accessToken"]
        .as_str()
        .unwrap()
        .to_string();
    native_discovery(
        &harness,
        &token,
        &["search_shared_sessions", "send_relay_message"],
    );
    let report = client.ok("tools.check", json!({})).await;
    assert_eq!(row(&report, "grok")["state"], "configured");
    assert_eq!(row(&report, "paseo")["state"], "configured");
    assert!(row(&report, "paseo")["detail"]
        .as_str()
        .unwrap()
        .contains("Grok"));
    let requests = || {
        harness
            .cloud
            .requests(MCP_PATH)
            .into_iter()
            .filter(|r| r.bearer.as_deref() == Some(&token))
            .collect::<Vec<_>>()
    };
    assert!(requests().iter().any(|r| r.body["method"] == "initialize"));
    assert!(requests().iter().any(|r| r.body["method"] == "tools/list"));
    assert!(requests()
        .iter()
        .all(|r| matches!(r.body["method"].as_str(), Some("initialize" | "tools/list"))));
    let count = requests().len();
    assert_eq!(
        row(&client.ok("tools.status", json!({})).await, "grok")["state"],
        "configured"
    );
    assert_eq!(
        requests().len(),
        count,
        "unchanged status reuses provider proof"
    );
    for missing in [vec!["search_shared_sessions"], vec!["send_relay_message"]] {
        native_discovery(&harness, &token, &missing);
        assert_eq!(
            row(&client.ok("tools.status", json!({})).await, "grok")["state"],
            if missing[0] == "search_shared_sessions" {
                "configured"
            } else {
                "installed"
            }
        );
        let checked = client.ok("tools.check", json!({})).await;
        assert_eq!(row(&checked, "grok")["state"], "installed");
        assert_eq!(
            row(&checked, "grok")["status_label"],
            "Connection not verified"
        );
    }
    override_response(
        &harness,
        MCP_PATH,
        &token,
        "initialize",
        403,
        json!({"error":"rejected"}),
    );
    let rejected = client.ok("tools.check", json!({})).await;
    assert_eq!(row(&rejected, "grok")["state"], "installed");
    assert!(row(&rejected, "grok")["detail"]
        .as_str()
        .unwrap()
        .contains("rejected"));
    // On a site without a hosted MCP route (this fake is not production), the
    // provider's saved connection cannot be verified at all: Tools says so
    // under the same label instead of asking to check again later.
    override_response(
        &harness,
        MCP_PATH,
        &token,
        "initialize",
        404,
        json!({"error":"no such route"}),
    );
    let absent = client.ok("tools.check", json!({})).await;
    assert_eq!(row(&absent, "grok")["state"], "installed");
    assert_eq!(
        row(&absent, "grok")["status_label"],
        "Connection not verified"
    );
    let detail = row(&absent, "grok")["detail"].as_str().unwrap().to_string();
    assert!(
        detail.starts_with("Hosted Agent Relay MCP is not available on"),
        "{detail}"
    );
    assert!(!detail.contains("Check again"), "{detail}");

    let binding = agent_relay_desktop_core::mcp_settings::Binding {
        site_url: harness.cloud.site_url.clone(),
        workspace_id: WORKSPACE.into(),
        account_id: USER.into(),
    };
    let installer = harness.account.mcp_config.get().unwrap();
    let rotated = "cld_at_rotated_provider_fixture";
    installer.set_provider_credentials(&binding, BTreeMap::from([("grok".into(), rotated.into())]));
    installer.refresh(Some(&binding));
    native_discovery(
        &harness,
        rotated,
        &["search_shared_sessions", "send_relay_message"],
    );
    assert_eq!(
        row(&client.ok("tools.status", json!({})).await, "grok")["state"],
        "configured"
    );
    assert!(harness
        .cloud
        .requests(MCP_PATH)
        .iter()
        .any(|r| r.bearer.as_deref() == Some(rotated)));
    client.ok("tools.select", json!({"tool_ids":[]})).await;
    let off = client.ok("tools.status", json!({})).await;
    assert_eq!(row(&off, "grok")["selected"], false);
    assert_ne!(row(&off, "grok")["state"], "configured");
    client
        .ok("tools.select", json!({"tool_ids":["grok"]}))
        .await;
    let reselected = harness.cloud.state.lock().unwrap().provider_grants["grant-grok"]
        ["accessToken"]
        .as_str()
        .unwrap()
        .to_string();
    native_discovery(
        &harness,
        &reselected,
        &["search_shared_sessions", "send_relay_message"],
    );
    let before_signout = client.ok("tools.check", json!({})).await;
    assert_eq!(row(&before_signout, "grok")["selected"], true);
    assert_eq!(row(&before_signout, "grok")["state"], "configured");
    client.ok("account.sign_out", json!({})).await;
    assert_ne!(
        row(&client.ok("tools.status", json!({})).await, "grok")["state"],
        "configured"
    );
    harness.stop().await;
}

#[tokio::test]
async fn capy_workspace_proof_checks_bound_api_and_obeys_cache_failures_and_deadline() {
    let harness = signed_in().await;
    let token = harness.store()["cloudSession"]["accessToken"]
        .as_str()
        .unwrap()
        .to_string();
    let roster = format!("/cloud/api/v1/workspaces/{WORKSPACE}/sessions/agents");
    let projects = format!("/cloud/api/v1/workspaces/{WORKSPACE}/capy/projects");
    override_response(
        &harness,
        &roster,
        &token,
        "GET",
        200,
        json!({"unavailable":false,"agents":[],"connectors":[]}),
    );
    override_response(
        &harness,
        &projects,
        &token,
        "GET",
        200,
        json!({"projects":[]}),
    );
    let mut client = harness.client().await;
    let connected = client.ok("tools.check", json!({})).await;
    assert_eq!(row(&connected, "capy")["state"], "configured");
    assert!(row(&connected, "capy")["detail"]
        .as_str()
        .unwrap()
        .contains("Capy’s API"));
    let count = harness.cloud.requests(&projects).len();
    assert!(count > 0);
    assert!(harness
        .cloud
        .requests(&projects)
        .iter()
        .all(|r| r.method == "GET" && r.bearer.as_deref() == Some(&token)));
    client.ok("tools.status", json!({})).await;
    assert_eq!(harness.cloud.requests(&projects).len(), count);
    for (status, label) in [
        (409, "Workspace connection not configured"),
        (403, "Connection not verified"),
        (503, "Connection not verified"),
    ] {
        override_response(
            &harness,
            &projects,
            &token,
            "GET",
            status,
            json!({"error":"fixture"}),
        );
        let checked = client.ok("tools.check", json!({})).await;
        assert_eq!(row(&checked, "capy")["status_label"], label);
        assert_ne!(row(&checked, "capy")["state"], "configured");
    }
    let gate = Arc::new(tokio::sync::Notify::new());
    harness
        .cloud
        .state
        .lock()
        .unwrap()
        .response_gates
        .insert(projects.clone(), gate.clone());
    override_response(
        &harness,
        &projects,
        &token,
        "GET",
        200,
        json!({"projects":[]}),
    );
    let start = std::time::Instant::now();
    let slow = client.ok("tools.check", json!({})).await;
    assert!(
        start.elapsed() < Duration::from_secs(15),
        "optional Capy proof exceeded status budget"
    );
    assert_eq!(
        row(&slow, "capy")["status_label"],
        "Connection not verified"
    );
    gate.notify_waiters();
    harness.stop().await;
}

#[tokio::test]
async fn capy_proof_deadline_does_not_cancel_rotating_login_credentials() {
    let harness = signed_in().await;
    let access = harness.store()["cloudSession"]["accessToken"]
        .as_str()
        .unwrap()
        .to_string();
    let roster = format!("/cloud/api/v1/workspaces/{WORKSPACE}/sessions/agents");
    let projects = format!("/cloud/api/v1/workspaces/{WORKSPACE}/capy/projects");
    override_response(
        &harness,
        &roster,
        &access,
        "GET",
        200,
        json!({"unavailable":false,"agents":[],"connectors":[]}),
    );
    override_response(
        &harness,
        &projects,
        &access,
        "GET",
        401,
        json!({"error":"expired"}),
    );
    let gate = Arc::new(tokio::sync::Notify::new());
    harness.cloud.state.lock().unwrap().refresh_gate = Some(gate.clone());
    let refreshes = harness
        .cloud
        .requests("/cloud/api/v1/auth/token/refresh")
        .len();
    let mut client = harness.client().await;
    let report = client.ok("tools.check", json!({})).await;
    assert_eq!(
        row(&report, "capy")["status_label"],
        "Connection not verified"
    );
    assert_eq!(
        harness
            .cloud
            .requests("/cloud/api/v1/auth/token/refresh")
            .len(),
        refreshes + 1
    );
    assert_eq!(harness.store()["cloudSession"]["accessToken"], access);
    harness.cloud.state.lock().unwrap().refresh_gate = None;
    gate.notify_waiters();
    eventually("rotated login saved after optional proof timed out", || {
        harness.store()["cloudSession"]["accessToken"]
            .as_str()
            .is_some_and(|token| token != access)
    })
    .await;
    assert_eq!(harness.account_state(), "signed_in");
    let next = harness.store()["cloudSession"]["accessToken"]
        .as_str()
        .unwrap()
        .to_string();
    override_response(
        &harness,
        &roster,
        &next,
        "GET",
        200,
        json!({"unavailable":false,"agents":[],"connectors":[]}),
    );
    override_response(
        &harness,
        &projects,
        &next,
        "GET",
        200,
        json!({"projects":[]}),
    );
    assert_eq!(
        row(&client.ok("tools.check", json!({})).await, "capy")["state"],
        "configured"
    );
    assert_eq!(
        harness
            .cloud
            .requests("/cloud/api/v1/auth/token/refresh")
            .len(),
        refreshes + 1
    );
    harness.stop().await;
}
