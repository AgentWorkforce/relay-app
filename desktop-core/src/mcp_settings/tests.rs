use super::*;

fn binding(account: &str) -> Binding {
    Binding {
        site_url: "https://agentrelay.com".into(),
        workspace_id: "workspace-1".into(),
        account_id: account.into(),
    }
}
fn helper(account: &str) -> String {
    binding(account)
        .helper(Path::new("/home/me/Agent Relay/probe"))
        .unwrap()
}

#[test]
fn codex_direct_hosted_install_logs_out_only_relay_once_and_preserves_comments() {
    let endpoint = binding("alice").endpoint().unwrap();
    let mut logouts = Vec::new();
    let configured = codex::render(
        "# personal\nmodel = 'example'\n[mcp_servers.other]\ncommand = 'other'\n",
        &endpoint,
        &helper("alice"),
        &mut |alias| {
            logouts.push(alias.to_string());
            true
        },
    )
    .unwrap();
    assert!(configured.starts_with("# personal\nmodel = 'example'"));
    assert!(configured.contains("command = 'other'"));
    assert!(
        logouts.is_empty(),
        "fresh installs have no existing OAuth alias to log out"
    );
    assert!(!configured.contains("Authorization"));
    assert!(
        codex::render(&configured, &endpoint, &helper("alice"), &mut |_| panic!(
            "managed config is idempotent"
        ))
        .is_none()
    );
    let changed = codex::render(&configured, &endpoint, &helper("bob"), &mut |_| {
        panic!("account switch does not touch OAuth cache")
    })
    .unwrap();
    let parsed = changed.parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(
        parsed["mcp_servers"][NAME]["http_headers_helper"].as_str(),
        Some(helper("bob").as_str())
    );
}

#[test]
fn existing_hosted_alias_migrates_without_duplicate_or_unrelated_secret_changes() {
    let endpoint = binding("alice").endpoint().unwrap();
    let text = format!("[mcp_servers.relay_alias]\nurl = '{endpoint}'\nenabled = true\nbearer_token_env_var = 'OLD_TOKEN'\nhttp_headers = {{ Authorization = 'old-private-token', X-Custom = 'keep-me' }}\n[mcp_servers.unrelated]\nurl = 'https://example.com/mcp'\nhttp_headers = {{ Authorization = 'unrelated-secret' }}\n");
    let mut called = Vec::new();
    let configured = codex::render(&text, &endpoint, &helper("alice"), &mut |alias| {
        called.push(alias.to_string());
        true
    })
    .unwrap();
    assert_eq!(called, ["relay_alias"]);
    assert!(!configured.contains("old-private-token"));
    assert!(!configured.contains("OLD_TOKEN"));
    assert!(configured.contains("unrelated-secret"));
    assert!(!configured.contains("keep-me"));
    assert!(!configured.contains("bearer_token_env_var"));
    assert!(configured.contains("[mcp_servers.agent-relay]"));
    assert!(codex::render(&text, &endpoint, &helper("alice"), &mut |_| false).is_none());
}

#[test]
fn custom_same_name_and_invalid_config_are_never_overwritten_or_logged_out() {
    let endpoint = binding("alice").endpoint().unwrap();
    for text in [
        "broken = [",
        "[mcp_servers.agent-relay]\nurl = 'https://other.example/mcp'\n",
        "mcp_servers = false",
    ] {
        assert!(
            codex::render(text, &endpoint, &helper("alice"), &mut |_| panic!(
                "unrelated credentials cannot be logged out"
            ))
            .is_none()
        );
    }
    assert!(json_config::render("{broken", &endpoint, &helper("alice")).is_none());
    assert!(json_config::render(
        r#"{"mcpServers":{"agent-relay":{"url":"https://other.example/mcp"}}}"#,
        &endpoint,
        &helper("alice")
    )
    .is_none());
}

#[test]
fn claude_installs_direct_http_and_rotates_all_identity_pins() {
    let endpoint = binding("alice").endpoint().unwrap();
    let text = format!(
        r#"{{"theme":"dark","mcpServers":{{"relayAlias":{{"type":"http","url":"{endpoint}","headers":{{"authorization":"old-secret","X-Custom":"keep"}}}},"other":{{"headers":{{"Authorization":"unrelated"}}}}}}}}"#
    );
    let configured = json_config::render(&text, &endpoint, &helper("alice")).unwrap();
    let value: Value = serde_json::from_str(&configured).unwrap();
    assert_eq!(value["mcpServers"][NAME]["type"], "http");
    assert_eq!(
        value["mcpServers"][NAME]["headers"]["X-Custom"],
        Value::Null
    );
    assert_eq!(
        value["mcpServers"]["other"]["headers"]["Authorization"],
        "unrelated"
    );
    assert!(!configured.contains("old-secret"));
    assert!(json_config::render(&configured, &endpoint, &helper("alice")).is_none());
    let mut next = binding("bob");
    next.site_url = "https://preview.agentrelay.com".into();
    next.workspace_id = "workspace-2".into();
    let changed = json_config::render(
        &configured,
        &next.endpoint().unwrap(),
        &next
            .helper(Path::new("/home/me/Agent Relay/probe"))
            .unwrap(),
    )
    .unwrap();
    let parsed: Value = serde_json::from_str(&changed).unwrap();
    assert_eq!(
        parsed["mcpServers"][NAME]["headersHelper"],
        next.helper(Path::new("/home/me/Agent Relay/probe"))
            .unwrap()
    );
    assert_eq!(parsed["mcpServers"][NAME]["url"], next.endpoint().unwrap());
}

#[test]
fn installer_signed_out_is_inert_and_opencode_is_reported_without_modification() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let codex = CodexConfig::from_env(&|_| None, home);
    let claude = ClaudeConfig::resolve(&|_| None, home, &[]);
    let opencode = OpenCodeConfig::from_env(&|_| None, home);
    fs::create_dir_all(&opencode.config_dir).unwrap();
    let file = opencode.config_dir.join("opencode.json");
    fs::write(&file, "{}").unwrap();
    let installer = Installer::new(home.into(), codex, claude, opencode);
    assert!(installer.refresh(None).is_empty());
    let command = crate::probe_copy::destination(home);
    fs::create_dir_all(command.parent().unwrap()).unwrap();
    fs::write(&command, "test helper").unwrap();
    let report = installer.report(Some(&binding("alice")), None);
    assert_eq!(
        report
            .tools
            .iter()
            .find(|t| t.id == "opencode")
            .unwrap()
            .state,
        "onboarding_needed"
    );
    assert_eq!(fs::read_to_string(file).unwrap(), "{}");
}

#[test]
fn invalid_binding_never_becomes_a_shell_command() {
    let mut invalid = binding("alice;unsafe");
    assert!(invalid.helper(Path::new("/probe")).is_none());
    invalid = binding("alice");
    invalid.site_url = "https://example.com/redirect".into();
    assert!(invalid.endpoint().is_none());
}

#[cfg(windows)]
#[test]
fn windows_helper_paths_cannot_expand_powershell_expressions() {
    for path in [
        r"C:\Users\$(expression)\probe.exe",
        r"C:\Users\`expression\probe.exe",
    ] {
        assert!(binding("alice").helper(Path::new(path)).is_none());
    }
}

#[cfg(unix)]
#[test]
fn files_remain_private_and_symlinks_are_untouched() {
    use std::os::unix::fs::MetadataExt;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("config.json");
    files::update(&path, |_| Some("{}".into())).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    let link = root.path().join("link.json");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    files::update(&link, |_| Some("unsafe".into())).unwrap();
    assert_eq!(fs::read_to_string(path).unwrap(), "{}");
}

#[test]
fn explicit_selection_repairs_known_disabled_entries() {
    let endpoint = binding("alice").endpoint().unwrap();
    let text = format!("[mcp_servers.agent-relay]\nurl = '{endpoint}'\nenabled = false\n");
    let repaired = codex::render(&text, &endpoint, &helper("alice"), &mut |_| true).unwrap();
    assert!(!repaired.contains("enabled = false"));
    let text =
        format!(r#"{{"mcpServers":{{"relayAlias":{{"url":"{endpoint}","disabled":true}}}}}}"#);
    let repaired = json_config::render(&text, &endpoint, &helper("alice")).unwrap();
    assert!(!repaired.contains("disabled"));
}

#[test]
fn codex_inline_server_tables_support_hosted_helpers() {
    let endpoint = binding("alice").endpoint().unwrap();
    let configured = codex::render(
        "mcp_servers = { other = { command = 'other' } }\n",
        &endpoint,
        &helper("alice"),
        &mut |_| panic!(),
    )
    .unwrap();
    assert!(configured.parse::<toml_edit::DocumentMut>().is_ok());
}

#[test]
fn codex_messaging_approval_is_scoped_and_preserves_explicit_choices() {
    let endpoint = binding("alice").endpoint().unwrap();
    let configured = codex::render(
        "approval_policy = 'never'\n",
        &endpoint,
        &helper("alice"),
        &mut |_| true,
    )
    .unwrap();
    let parsed = configured.parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(parsed["approval_policy"].as_str(), Some("never"));
    let server = &parsed["mcp_servers"][NAME];
    assert_eq!(
        server["tools"]["check_relay_inbox"]["approval_mode"].as_str(),
        Some("approve")
    );
    assert_eq!(server["tools"].as_table_like().unwrap().len(), 10);
    assert_eq!(
        server["tools"]["send_relay_message"]["approval_mode"].as_str(),
        Some("approve")
    );
    assert_eq!(
        server["tools"]["list_relay_spawn_targets"]["approval_mode"].as_str(),
        Some("approve")
    );
    assert_eq!(
        server["tools"]["get_relay_spawn_status"]["approval_mode"].as_str(),
        Some("approve")
    );
    assert_eq!(
        server["tools"]["spawn_relay_agent"]["approval_mode"].as_str(),
        Some("approve")
    );
    assert_eq!(
        server["tools"]["release_relay_agent"]["approval_mode"].as_str(),
        Some("approve")
    );
    assert!(server["tools"].get("subscribe_relay_channel").is_none());
    for policy in ["approval_policy = 'on-request'\n", ""] {
        let configured = codex::render(policy, &endpoint, &helper("alice"), &mut |_| true).unwrap();
        let parsed = configured.parse::<toml_edit::DocumentMut>().unwrap();
        let server = &parsed["mcp_servers"][NAME];
        assert_eq!(
            server["tools"]["list_relay_spawn_targets"]["approval_mode"].as_str(),
            Some("approve")
        );
        assert_eq!(
            server["tools"]["get_relay_spawn_status"]["approval_mode"].as_str(),
            Some("approve")
        );
        assert!(server["tools"].get("spawn_relay_agent").is_none());
        assert!(server["tools"].get("release_relay_agent").is_none());
        assert_eq!(server["tools"].as_table_like().unwrap().len(), 8);
    }
    let switched = configured.replace(
        "approval_policy = 'never'",
        "approval_policy = 'on-request'",
    );
    let switched = codex::render(&switched, &endpoint, &helper("alice"), &mut |_| true).unwrap();
    let switched = switched.parse::<toml_edit::DocumentMut>().unwrap();
    let server = &switched["mcp_servers"][NAME];
    assert!(server["tools"].get("spawn_relay_agent").is_none());
    assert!(server["tools"].get("release_relay_agent").is_none());
    assert_eq!(
        server["tools"]["list_relay_spawn_targets"]["approval_mode"].as_str(),
        Some("approve")
    );
    assert_eq!(server["tools"].as_table_like().unwrap().len(), 8);
    let profile_never = "profile = 'work'\napproval_policy = 'on-request'\n[profiles.work]\napproval_policy = 'never'\n";
    let profile_never =
        codex::render(profile_never, &endpoint, &helper("alice"), &mut |_| true).unwrap();
    let profile_never = profile_never.parse::<toml_edit::DocumentMut>().unwrap();
    assert!(profile_never["mcp_servers"][NAME]["tools"]
        .get("spawn_relay_agent")
        .is_none());
    assert!(profile_never["mcp_servers"][NAME]["tools"]
        .get("release_relay_agent")
        .is_none());
    let mixed = "approval_policy = 'never'\n[profiles.safe]\napproval_policy = 'on-request'\n[profiles.fast]\napproval_policy = 'never'\n";
    let mixed = codex::render(mixed, &endpoint, &helper("alice"), &mut |_| true).unwrap();
    let mixed = mixed.parse::<toml_edit::DocumentMut>().unwrap();
    assert!(mixed["mcp_servers"][NAME]["tools"]
        .get("spawn_relay_agent")
        .is_none());
    assert!(mixed["mcp_servers"][NAME]["tools"]
        .get("release_relay_agent")
        .is_none());
    assert_eq!(
        mixed["mcp_servers"][NAME]["tools"]["list_relay_spawn_targets"]["approval_mode"].as_str(),
        Some("approve")
    );
    let all_never = "approval_policy = 'never'\n[profiles.work]\napproval_policy = 'never'\n[profiles.other]\nmodel = 'gpt'\n";
    let all_never = codex::render(all_never, &endpoint, &helper("alice"), &mut |_| true).unwrap();
    let all_never = all_never.parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(
        all_never["mcp_servers"][NAME]["tools"]["spawn_relay_agent"]["approval_mode"].as_str(),
        Some("approve")
    );
    assert_eq!(
        all_never["mcp_servers"][NAME]["tools"]["release_relay_agent"]["approval_mode"].as_str(),
        Some("approve")
    );
    let profile_prompt = "profile = 'work'\napproval_policy = 'never'\n[profiles.work]\napproval_policy = 'on-request'\n";
    let profile_prompt =
        codex::render(profile_prompt, &endpoint, &helper("alice"), &mut |_| true).unwrap();
    let profile_prompt = profile_prompt.parse::<toml_edit::DocumentMut>().unwrap();
    assert!(profile_prompt["mcp_servers"][NAME]["tools"]
        .get("spawn_relay_agent")
        .is_none());
    assert!(profile_prompt["mcp_servers"][NAME]["tools"]
        .get("release_relay_agent")
        .is_none());
    let six = format!(
        "approval_policy = 'never'\n[mcp_servers.agent-relay]\nurl = '{endpoint}'\nhttp_headers_helper = {:?}\n\
         [mcp_servers.agent-relay.tools.check_relay_inbox]\napproval_mode = 'approve'\n\
         [mcp_servers.agent-relay.tools.send_relay_message]\napproval_mode = 'approve'\n\
         [mcp_servers.agent-relay.tools.join_relay]\napproval_mode = 'approve'\n\
         [mcp_servers.agent-relay.tools.leave_relay]\napproval_mode = 'approve'\n\
         [mcp_servers.agent-relay.tools.relay_status]\napproval_mode = 'approve'\n\
         [mcp_servers.agent-relay.tools.read_relay_conversation]\napproval_mode = 'approve'\n",
        helper("alice"),
    );
    let upgraded = codex::render(&six, &endpoint, &helper("alice"), &mut |_| true).unwrap();
    let upgraded = upgraded.parse::<toml_edit::DocumentMut>().unwrap();
    let server = &upgraded["mcp_servers"][NAME];
    assert_eq!(
        server["tools"]["check_relay_inbox"]["approval_mode"].as_str(),
        Some("approve")
    );
    assert_eq!(
        server["tools"]["spawn_relay_agent"]["approval_mode"].as_str(),
        Some("approve")
    );
    assert_eq!(
        server["tools"]["release_relay_agent"]["approval_mode"].as_str(),
        Some("approve")
    );
    let prompted = format!(
        "approval_policy = 'never'\n[mcp_servers.agent-relay]\nurl = '{endpoint}'\nhttp_headers_helper = {:?}\n\
         [mcp_servers.agent-relay.tools.spawn_relay_agent]\napproval_mode = 'prompt'\n",
        helper("alice"),
    );
    let kept = codex::render(&prompted, &endpoint, &helper("alice"), &mut |_| true).unwrap();
    let kept = kept.parse::<toml_edit::DocumentMut>().unwrap();
    assert_eq!(
        kept["mcp_servers"][NAME]["tools"]["spawn_relay_agent"]["approval_mode"].as_str(),
        Some("prompt")
    );
    assert_eq!(
        kept["mcp_servers"][NAME]["tools"]["release_relay_agent"]["approval_mode"].as_str(),
        Some("approve")
    );
    for policy in [
        "default_tools_approval_mode = 'prompt'\n",
        "disabled_tools = ['check_relay_inbox']\n[ mcp_servers.agent-relay.tools.check_relay_inbox ]\napproval_mode = 'prompt'\n",
    ] {
        let original = format!("[mcp_servers.agent-relay]\nurl = '{endpoint}'\nhttp_headers_helper = {:?}\n{policy}", helper("alice"));
        let repaired = codex::render(&original, &endpoint, &helper("bob"), &mut |_| true).unwrap();
        let before = original.parse::<toml_edit::DocumentMut>().unwrap();
        let after = repaired.parse::<toml_edit::DocumentMut>().unwrap();
        for key in ["default_tools_approval_mode", "disabled_tools"] {
            assert_eq!(before["mcp_servers"][NAME].get(key).map(ToString::to_string), after["mcp_servers"][NAME].get(key).map(ToString::to_string));
        }
        if let Some(tool) = before["mcp_servers"][NAME].get("tools").and_then(|tools| tools.get("check_relay_inbox")) {
            assert_eq!(tool["approval_mode"].as_str(), after["mcp_servers"][NAME]["tools"]["check_relay_inbox"]["approval_mode"].as_str());
        }
    }
}

/// A helper in the earlier `<executable> relay mcp-headers` form is this
/// build's, so setup rewrites it in the current form.
#[test]
fn an_earlier_relay_mcp_headers_helper_is_replaced() {
    let endpoint = binding("alice").endpoint().unwrap();
    let current = helper("alice");
    // Windows quotes every word of the command.
    let earlier = if current.contains(" \"mcp-headers\"") {
        current.replacen(" \"mcp-headers\"", " \"relay\" \"mcp-headers\"", 1)
    } else {
        current.replacen(" mcp-headers", " relay mcp-headers", 1)
    };
    assert_ne!(earlier, current);
    assert!(managed_helper(&earlier, &current));
    let text = serde_json::to_string(&serde_json::json!({
        "mcpServers": {NAME: {"type": "http", "url": endpoint, "headersHelper": earlier}}
    }))
    .unwrap();
    let rendered: Value = serde_json::from_str(&json_config::render(&text, &endpoint, &current).unwrap()).unwrap();
    assert_eq!(rendered["mcpServers"][NAME]["headersHelper"], current);
    assert!(!managed_helper("/usr/bin/echo relay mcp-headers --site x", &current));
    // The form Windows writes, checked on every platform.
    assert!(managed_helper(
        r#""C:\probe.exe" "relay" "mcp-headers" "--site" "x""#,
        r#""C:\probe.exe" "mcp-headers" "--site" "x""#
    ));
}

#[test]
fn amp_connections_url_appends_the_cloud_app_once() {
    for site in [
        "https://agentrelay.com",
        "https://agentrelay.com/",
        "https://agentrelay.com/cloud",
        " https://agentrelay.com/cloud/ ",
    ] {
        assert_eq!(
            amp_connections_url(site).as_deref(),
            Some("https://agentrelay.com/cloud/dashboard/settings#connections"),
            "{site}"
        );
    }
    assert_eq!(
        amp_connections_url("http://localhost:8787").as_deref(),
        Some("http://localhost:8787/cloud/dashboard/settings#connections")
    );
    for invalid in ["", "file:///tmp", "http://example.com", "https://agentrelay.com/other"] {
        assert_eq!(amp_connections_url(invalid), None, "{invalid}");
    }
}

#[test]
fn providers_list_amp_and_copilot_in_name_order() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let installer = Installer::new(
        home.into(),
        CodexConfig::from_env(&|_| None, home),
        ClaudeConfig::resolve(&|_| None, home, &[]),
        OpenCodeConfig::from_env(&|_| None, home),
    );
    let names: Vec<String> = installer
        .report(None, None)
        .tools
        .into_iter()
        .map(|tool| tool.name)
        .collect();
    assert_eq!(names.first().map(String::as_str), Some("Amp"));
    let position = |name: &str| names.iter().position(|n| n == name).unwrap();
    let copilot = position("GitHub Copilot (VS Code)");
    assert!(position("Dot") < copilot && copilot < position("Grok"), "{names:?}");
    assert_eq!(names.len(), manager::TOOLS.len());
}
