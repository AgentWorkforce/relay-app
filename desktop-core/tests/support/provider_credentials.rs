use super::*;

#[tokio::test]
async fn native_provider_uses_scoped_app_grant_and_disconnect_revokes_it() {
    let harness = Harness::start_with(|cloud, _| {
        cloud.state.lock().unwrap().relaycast_url = Some(cloud.site_url.clone());
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"new","flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    let report = client
        .ok("tools.select", json!({"tool_ids":["grok","hermes"]}))
        .await;
    assert!(report["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| matches!(p["id"].as_str(), Some("grok" | "hermes")))
        .all(|p| p["selected"] == true));
    let grants = harness.cloud.state.lock().unwrap().provider_grants.clone();
    assert_eq!(grants.len(), 2);
    let token = grants["grant-grok"]["accessToken"].as_str().unwrap();
    assert!(token.starts_with("cld_at_"));
    assert!(!report.to_string().contains(token));
    let config = std::fs::read_to_string(harness.home.join(".grok/config.toml")).unwrap();
    let parsed: toml_edit::DocumentMut = config.parse().unwrap();
    let token_path = parsed["mcp_servers"]["agent-relay"]["bearer_token_file"]
        .as_str()
        .unwrap();
    assert_eq!(std::fs::read_to_string(token_path).unwrap(), token);
    assert!(!config.contains(token));
    assert!(!config.contains("access-1"));
    assert!(!config.contains("refresh-"));
    client
        .ok("tools.manage", json!({"tool_id":"grok","action":"repair"}))
        .await;
    assert_eq!(
        harness.cloud.state.lock().unwrap().provider_grants["grant-grok"]["accessToken"],
        token
    );
    client
        .ok("tools.select", json!({"tool_ids":["hermes"]}))
        .await;
    assert!(!harness
        .cloud
        .state
        .lock()
        .unwrap()
        .provider_grants
        .contains_key("grant-grok"));
    assert!(
        !std::fs::read_to_string(harness.home.join(".grok/config.toml"))
            .unwrap()
            .contains(token)
    );
    client.ok("account.sign_out", json!({})).await;
    assert!(harness
        .cloud
        .state
        .lock()
        .unwrap()
        .provider_grants
        .is_empty());
    assert!(!harness
        .home
        .join("app-data/mcp-provider-credentials.json")
        .exists());
    harness.stop().await;
}

#[tokio::test]
async fn failed_provider_issuance_retries_same_saved_secret_without_exporting_app_token() {
    let harness = Harness::start_with(|cloud, _| {
        let mut state = cloud.state.lock().unwrap();
        state.relaycast_url = Some(cloud.site_url.clone());
        state.provider_grant_status = 503;
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"new","flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    client
        .ok("tools.select", json!({"tool_ids":["grok"]}))
        .await;
    assert!(!harness.home.join(".grok/config.toml").exists());
    let path = harness.home.join("app-data/mcp-provider-credentials.json");
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let token = saved["grants"]["grok"]["token"].clone();
    harness.cloud.state.lock().unwrap().provider_grant_status = 200;
    client
        .ok("tools.manage", json!({"tool_id":"grok","action":"repair"}))
        .await;
    assert_eq!(
        harness.cloud.state.lock().unwrap().provider_grants["grant-grok"]["accessToken"],
        token
    );
    let config: toml_edit::DocumentMut =
        std::fs::read_to_string(harness.home.join(".grok/config.toml"))
            .unwrap()
            .parse()
            .unwrap();
    let token_path = config["mcp_servers"]["agent-relay"]["bearer_token_file"]
        .as_str()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(token_path).unwrap(),
        token.as_str().unwrap()
    );
    harness.stop().await;
}

#[tokio::test]
async fn offline_disconnect_preserves_retry_material_and_reinstall_recovers() {
    let harness = Harness::start_with(|cloud, _| {
        cloud.state.lock().unwrap().relaycast_url = Some(cloud.site_url.clone());
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"new","flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    client
        .ok("tools.select", json!({"tool_ids":["grok"]}))
        .await;
    let original =
        harness.cloud.state.lock().unwrap().provider_grants["grant-grok"]["accessToken"].clone();
    harness.cloud.state.lock().unwrap().provider_grant_status = 503;
    client
        .ok(
            "tools.manage",
            json!({"tool_id":"grok","action":"uninstall"}),
        )
        .await;
    let path = harness.home.join("app-data/mcp-provider-credentials.json");
    let pending: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(pending["grants"]["grok"]["token"], original);
    assert!(
        !std::fs::read_to_string(harness.home.join(".grok/config.toml"))
            .unwrap()
            .contains(original.as_str().unwrap())
    );
    harness.cloud.state.lock().unwrap().provider_grant_status = 200;
    client
        .ok("tools.manage", json!({"tool_id":"grok","action":"install"}))
        .await;
    assert_eq!(
        harness.cloud.state.lock().unwrap().provider_grants["grant-grok"]["accessToken"],
        original
    );
    // Simulate a crash after successful issuance but before persisting its ID.
    // Removal must recover the same server grant before deleting it.
    let mut lost_reply: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    lost_reply["grants"]["grok"]["id"] = Value::Null;
    std::fs::write(&path, serde_json::to_vec(&lost_reply).unwrap()).unwrap();
    client
        .ok(
            "tools.manage",
            json!({"tool_id":"grok","action":"uninstall"}),
        )
        .await;
    assert!(harness
        .cloud
        .state
        .lock()
        .unwrap()
        .provider_grants
        .is_empty());
    harness.stop().await;
}

#[tokio::test]
async fn older_hosted_server_never_receives_provider_credential_issuance() {
    let harness = Harness::start_with(|cloud, _| {
        let mut state = cloud.state.lock().unwrap();
        state.relaycast_url = Some(cloud.site_url.clone());
        state.mcp_native_identity = false;
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"new","flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    client
        .ok("tools.select", json!({"tool_ids":["grok"]}))
        .await;
    assert!(harness
        .cloud
        .requests(&format!(
            "/cloud/api/v1/workspaces/{WORKSPACE}/mcp/shared-sessions-tokens"
        ))
        .is_empty());
    assert!(!harness.home.join(".grok/config.toml").exists());
    harness.stop().await;
}

#[tokio::test]
async fn legacy_app_login_gets_an_app_signin_action_not_a_per_provider_login() {
    let harness = Harness::start_with(|cloud, _| {
        let mut state = cloud.state.lock().unwrap();
        state.relaycast_url = Some(cloud.site_url.clone());
        state.provider_grant_status = 403;
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"new","flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    let report = client
        .ok("tools.select", json!({"tool_ids":["grok"]}))
        .await;
    let grok = report["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "grok")
        .unwrap();
    assert_eq!(grok["state"], "signed_out");
    assert!(grok["detail"]
        .as_str()
        .unwrap()
        .contains("Sign in to Agent Relay again"));
    assert!(!harness.home.join(".grok/config.toml").exists());
    harness.cloud.state.lock().unwrap().provider_grant_status = 200;
    let report = client
        .ok("tools.manage", json!({"tool_id":"grok","action":"repair"}))
        .await;
    let grok = report["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "grok")
        .unwrap();
    assert_eq!(grok["state"], "installed");
    harness.stop().await;
}

/// A login from before Desktop's consent screen still works for everything
/// else, so the account looked healthy while every provider asked for a
/// sign-in. Account health now asks for one too, and signing in again
/// issues the provider credentials without another provider action.
#[tokio::test]
async fn a_login_refused_provider_credentials_asks_the_account_to_sign_in_again() {
    for reauth_code in [false, true] {
        let harness = Harness::start_with(|cloud, _| {
            let mut state = cloud.state.lock().unwrap();
            state.relaycast_url = Some(cloud.site_url.clone());
            state.provider_grant_status = 403;
            state.provider_grant_reauth_code = reauth_code;
        })
        .await;
        let mut client = harness.client().await;
        client
            .ok("account.sign_in", json!({"sharing":"new","flow":"device"}))
            .await;
        eventually("signed in", || harness.account_state() == "signed_in").await;
        harness.account.check_auth_health().await;
        assert_eq!(
            harness.core.status().account.auth_health.as_deref(),
            Some("ready")
        );
        let report = client
            .ok("tools.select", json!({"tool_ids":["grok"]}))
            .await;
        // `desktop_reauth_required` asks at once; older Cloud's message
        // alone (possibly a token refreshed mid-request) only once a retry
        // with the current token is refused again.
        let issued = harness
            .cloud
            .requests(&format!(
                "/cloud/api/v1/workspaces/{WORKSPACE}/mcp/shared-sessions-tokens"
            ))
            .len();
        assert_eq!(issued, if reauth_code { 1 } else { 2 }, "{reauth_code}");
        assert_eq!(
            tool(&report, "grok")["state"],
            "signed_out",
            "{reauth_code}"
        );
        // The next report shows it too.
        let report = client.ok("tools.status", json!({})).await;
        assert_eq!(
            report["account_health"], "sign_in_required",
            "{reauth_code}"
        );
        let account = harness.core.status().account;
        assert_eq!(account.auth_health.as_deref(), Some("sign_in_required"));
        assert_eq!(
            account.auth_error.as_deref(),
            Some("Sign in again to share your sessions.")
        );
        // A later health check keeps saying so while the grant is refused.
        harness
            .core
            .hub()
            .update(|status| status.account.auth_checked_at = None);
        harness.account.check_auth_health().await;
        assert_eq!(
            harness.core.status().account.auth_health.as_deref(),
            Some("sign_in_required")
        );

        // Signing in again (through consent) issues the grant by itself.
        harness.cloud.state.lock().unwrap().provider_grant_status = 200;
        client
            .ok("account.sign_in", json!({"sharing":"new","flow":"device"}))
            .await;
        eventually("provider credentials issued", || {
            !harness
                .cloud
                .state
                .lock()
                .unwrap()
                .provider_grants
                .is_empty()
        })
        .await;
        eventually("account health ready", || {
            harness.core.status().account.auth_health.as_deref() == Some("ready")
        })
        .await;
        assert_eq!(harness.core.status().account.auth_error, None);
        let report = client.ok("tools.status", json!({})).await;
        assert_ne!(tool(&report, "grok")["state"], "signed_out");
        harness.stop().await;
    }
}

#[tokio::test]
async fn enabling_a_host_installs_detected_providers_before_returning() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"new","flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    std::fs::create_dir_all(harness.home.join(".codex")).unwrap();
    std::fs::create_dir_all(harness.home.join(".claude")).unwrap();
    let helper = agent_relay_desktop_core::probe_copy::destination(&harness.home);
    std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
    std::fs::write(&helper, "fixture executable").unwrap();
    agent_relay_node::platform::fs::set_mode(&helper, 0o700).unwrap();
    client.ok("tools.select", json!({"tool_ids":[]})).await;
    let report = client
        .ok(
            "tools.manage",
            json!({"tool_id":"conductor","action":"enable"}),
        )
        .await;
    for id in ["conductor", "codex", "claude"] {
        assert_eq!(
            report["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["id"] == id)
                .unwrap()["selected"],
            true
        );
    }
    assert!(
        std::fs::read_to_string(harness.home.join(".codex/config.toml"))
            .unwrap()
            .contains("agent-relay")
    );
    assert!(std::fs::read_to_string(harness.home.join(".claude.json"))
        .unwrap()
        .contains("agent-relay"));
    let report = client
        .ok(
            "tools.manage",
            json!({"tool_id":"codex","action":"uninstall"}),
        )
        .await;
    assert!(report["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == "conductor" && row["selected"] == true));
    assert!(report["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|row| row["id"] == "codex" && row["selected"] == false));
    harness.stop().await;
}

/// Puts a usable command-line helper where the installer looks for it, so
/// Cursor's messaging hooks can be installed.
fn usable_helper(home: &std::path::Path) {
    let helper = agent_relay_desktop_core::probe_copy::destination(home);
    std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
    std::fs::write(&helper, "fixture executable").unwrap();
    agent_relay_node::platform::fs::set_mode(&helper, 0o700).unwrap();
}

/// Cursor's `mcp.json` entry for Agent Relay. On macOS and Linux it runs
/// this install's probe as the `cursor-mcp` bridge for `site` and holds no
/// grant; Windows has no Cursor hooks, so it keeps the hosted MCP URL with
/// `grant`.
fn assert_cursor_entry(home: &std::path::Path, site: &str, grant: &str) {
    let installed = std::fs::read_to_string(home.join(".cursor/mcp.json")).unwrap();
    let mcp: Value = json5::from_str(&installed).unwrap();
    let entry = &mcp["mcpServers"]["agent-relay"];
    if cfg!(windows) {
        assert_eq!(
            entry["url"],
            format!("{site}/cloud/api/v1/mcp/shared-sessions")
        );
        assert_eq!(entry["headers"]["Authorization"], format!("Bearer {grant}"));
        return;
    }
    let helper = agent_relay_desktop_core::probe_copy::destination(home);
    assert_eq!(entry["command"], helper.to_str().unwrap(), "{entry}");
    let args: Vec<&str> = entry["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect();
    assert_eq!(
        args[..6],
        [
            "cursor-mcp",
            "--site",
            site,
            "--workspace",
            WORKSPACE,
            "--account"
        ],
        "{entry}"
    );
    assert!(!args[6].is_empty(), "{entry}");
    assert!(
        entry.get("url").is_none() && entry.get("headers").is_none(),
        "{entry}"
    );
    assert!(
        !installed.contains(grant),
        "the bridge entry holds no grant"
    );
}

fn tool<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id)
        .unwrap()
}

/// A gate-only workspace's resolve names `urls.relaygateUrl` and no
/// `relaycastApiKey`. The hosted MCP pipeline does not depend on the key:
/// the app login mints a Cursor grant at the site and Cursor's config points
/// at the site's hosted MCP endpoint with that grant.
#[tokio::test]
async fn a_gate_only_sign_in_writes_cursor_s_hosted_mcp_entry() {
    let harness = Harness::start_with(|cloud, setup| {
        let mut state = cloud.state.lock().unwrap();
        state.relaycast_url = Some(cloud.site_url.clone());
        state.relaygate_url = Some(cloud.site_url.clone());
        state.resolve_without_key = true;
        usable_helper(&setup.home);
    })
    .await;
    let site = harness.cloud.site_url.clone();
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"new","flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    eventually("the gate-only workspace is kept", || {
        harness.core.store.read(|s| {
            s.cloud
                .as_ref()
                .is_some_and(|cloud| cloud.gate_url.as_deref() == Some(site.as_str()))
        })
    })
    .await;
    let report = client
        .ok("tools.select", json!({"tool_ids":["cursor"]}))
        .await;

    // The site advertised the provider-credential protocol, then minted a grant.
    let initialize = harness
        .cloud
        .requests("/cloud/api/v1/mcp/shared-sessions")
        .into_iter()
        .filter(|r| r.body["method"] == "initialize")
        .count();
    assert!(initialize >= 1, "no hosted MCP initialize");
    let issued = harness.cloud.requests(&format!(
        "/cloud/api/v1/workspaces/{WORKSPACE}/mcp/shared-sessions-tokens"
    ));
    assert!(
        issued
            .iter()
            .any(|r| r.method == "POST" && r.body["provider"] == "cursor"),
        "{issued:?}"
    );
    let grant = harness.cloud.state.lock().unwrap().provider_grants["grant-cursor"]["accessToken"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!report.to_string().contains(&grant));

    assert_cursor_entry(&harness.home, &site, &grant);
    let installed = std::fs::read_to_string(harness.home.join(".cursor/mcp.json")).unwrap();
    assert!(!installed.contains("access-"));
    assert!(!installed.contains(ACCOUNT_KEY));

    let cursor = tool(&report, "cursor");
    assert_eq!(cursor["selected"], true, "{cursor}");
    assert!(
        matches!(cursor["state"].as_str(), Some("configured" | "installed")),
        "{cursor}"
    );
    harness.stop().await;
}

/// The resolve is not what the hosted MCP pipeline depends on: one naming
/// neither a relaygate URL nor a key (`ResolveError::NoKey`) leaves no
/// workspace in the store, and the sign-in, the grant and Cursor's entry go
/// ahead all the same (`Account::adopt_workspace` reports a workspace that
/// shares no key and carries on).
#[tokio::test]
async fn a_resolve_with_no_gate_and_no_key_still_writes_cursor_s_hosted_mcp_entry() {
    let harness = Harness::start_with(|cloud, setup| {
        let mut state = cloud.state.lock().unwrap();
        state.relaycast_url = Some(cloud.site_url.clone());
        state.resolve_without_key = true;
        usable_helper(&setup.home);
    })
    .await;
    let site = harness.cloud.site_url.clone();
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"new","flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    assert!(!harness
        .cloud
        .requests(&format!("/cloud/api/v1/workspaces/{WORKSPACE}/resolve"))
        .is_empty());
    assert!(harness.core.store.read(|s| s.cloud.is_none()));
    let report = client
        .ok("tools.select", json!({"tool_ids":["cursor"]}))
        .await;
    let grant = harness.cloud.state.lock().unwrap().provider_grants["grant-cursor"]["accessToken"]
        .as_str()
        .unwrap()
        .to_string();
    assert_cursor_entry(&harness.home, &site, &grant);
    let cursor = tool(&report, "cursor");
    assert!(
        matches!(cursor["state"].as_str(), Some("configured" | "installed")),
        "{cursor}"
    );
    harness.stop().await;
}

/// What does stop Cursor's setup on a gate-only site: a hosted MCP
/// initialize that does not advertise
/// `com.agentrelay/desktop-provider-credentials` version 1. No grant is
/// asked for, Cursor gets no entry, and the row says the provider could not
/// be authorized.
#[tokio::test]
async fn a_gate_only_site_without_the_provider_credential_protocol_writes_no_cursor_entry() {
    let harness = Harness::start_with(|cloud, setup| {
        let mut state = cloud.state.lock().unwrap();
        state.relaycast_url = Some(cloud.site_url.clone());
        state.relaygate_url = Some(cloud.site_url.clone());
        state.resolve_without_key = true;
        state.mcp_native_identity = false;
        usable_helper(&setup.home);
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"new","flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    let report = client
        .ok("tools.select", json!({"tool_ids":["cursor"]}))
        .await;
    assert!(harness
        .cloud
        .requests("/cloud/api/v1/mcp/shared-sessions")
        .iter()
        .any(|r| r.body["method"] == "initialize"));
    assert!(harness
        .cloud
        .requests(&format!(
            "/cloud/api/v1/workspaces/{WORKSPACE}/mcp/shared-sessions-tokens"
        ))
        .is_empty());
    assert!(harness
        .cloud
        .state
        .lock()
        .unwrap()
        .provider_grants
        .is_empty());
    let mcp_path = harness.home.join(".cursor/mcp.json");
    if mcp_path.exists() {
        let mcp: Value = json5::from_str(&std::fs::read_to_string(mcp_path).unwrap()).unwrap();
        assert!(mcp["mcpServers"].get("agent-relay").is_none(), "{mcp}");
    }
    let cursor = tool(&report, "cursor");
    assert_eq!(cursor["state"], "action_required", "{cursor}");
    assert!(
        cursor["detail"]
            .as_str()
            .unwrap()
            .contains("Agent Relay could not authorize this provider."),
        "{cursor}"
    );
    harness.stop().await;
}
