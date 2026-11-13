use super::*;

fn installer(home: &Path) -> Installer {
    Installer::new(
        home.into(),
        CodexConfig::from_env(&|_| None, home),
        ClaudeConfig::resolve(&|_| None, home, &[]),
        OpenCodeConfig::from_env(&|_| None, home),
    )
}

fn executable_helper(home: &Path) -> PathBuf {
    let path = crate::probe_copy::destination(home);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "synthetic executable").unwrap();
    agent_relay_node::platform::fs::set_mode(&path, 0o700).unwrap();
    path
}
fn binding() -> Binding {
    Binding {
        site_url: "https://agentrelay.com".into(),
        workspace_id: "workspace".into(),
        account_id: "account".into(),
    }
}

#[test]
fn status_and_refresh_without_choices_never_install() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    fs::create_dir_all(&app.codex.home).unwrap();
    assert!(!app.report(Some(&binding()), None).onboarding_complete);
    assert!(app.refresh(Some(&binding())).is_empty());
    assert!(!app.codex.home.join("config.toml").exists());
    assert!(!app.choices.exists());
}

#[test]
fn not_now_persists_across_restart_and_signout() {
    let root = tempfile::tempdir().unwrap();
    installer(root.path())
        .select(Vec::new(), Some(&binding()), None)
        .unwrap();
    let restarted = installer(root.path());
    assert!(restarted.report(None, None).onboarding_complete);
    assert!(restarted
        .report(None, None)
        .tools
        .iter()
        .all(|t| !t.selected));
    assert!(restarted.refresh(Some(&binding())).is_empty());
}

#[test]
fn uninstall_persists_opt_out_even_if_configuration_cannot_be_removed() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    fs::create_dir_all(&app.codex.home).unwrap();
    fs::write(app.codex.home.join("config.toml"), "invalid = [").unwrap();
    app.select(vec!["codex".into()], None, None).unwrap();
    app.manage("codex", "uninstall", None, None).unwrap();
    let restarted = installer(root.path());
    assert!(restarted.refresh(Some(&binding())).is_empty());
    assert!(
        !restarted
            .report(None, None)
            .tools
            .iter()
            .find(|t| t.id == "codex")
            .unwrap()
            .selected
    );
    assert_eq!(
        fs::read_to_string(app.codex.home.join("config.toml")).unwrap(),
        "invalid = ["
    );
}

#[test]
fn repair_failure_stays_selected_and_reports_action_needed() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    fs::create_dir_all(&app.codex.home).unwrap();
    fs::write(app.codex.home.join("config.toml"), "invalid = [").unwrap();
    let report = app
        .manage("codex", "repair", Some(&binding()), Some("healthy".into()))
        .unwrap();
    let tool = report.tools.iter().find(|t| t.id == "codex").unwrap();
    assert!(tool.selected);
    assert_eq!(tool.state, "action_required");
    assert_eq!(report.account_health.as_deref(), Some("healthy"));
}

#[test]
fn canonical_migration_dedupes_project_aliases_without_touching_custom_origins() {
    let text = r#"{"mcpServers":{"old":{"url":"https://agentrelay.com/cloud/api/v1/mcp/shared-sessions"},"custom":{"url":"https://custom.example/mcp"}},"projects":{"/project":{"mcpServers":{"legacy":{"command":"/bin/agent-relay-probe","args":["relay","mcp"]},"custom":{"command":"unrelated","args":["agent-relay"]}}}}}"#;
    let b = binding();
    let rendered = json_config::render(
        text,
        &b.endpoint().unwrap(),
        &b.helper(Path::new("/probe")).unwrap(),
    )
    .unwrap();
    let root: Value = serde_json::from_str(&rendered).unwrap();
    assert!(root["mcpServers"][NAME].is_object());
    assert!(root["mcpServers"]["old"].is_null());
    assert!(root["mcpServers"]["custom"].is_object());
    assert!(root["projects"]["/project"]["mcpServers"]["legacy"].is_null());
    assert!(root["projects"]["/project"]["mcpServers"]["custom"].is_object());
}

#[test]
fn legacy_recognition_is_precise_for_real_npm_and_bundled_commands() {
    for (command, args) in [
        ("agent-relay", vec!["mcp"]),
        ("/bin/agent-relay-probe", vec!["relay", "mcp"]),
        ("npx", vec!["-y", "agent-relay@1.2.3", "mcp"]),
        ("node", vec!["/package/agent-relay-mcp.js"]),
    ] {
        assert!(recognized::legacy(command, &args));
    }
    for (command, args) in [
        ("agent-relay", vec!["send"]),
        ("npx", vec!["unrelated-agent-relay", "mcp"]),
        ("node", vec!["/package/custom-agent-relay-mcp.js"]),
        ("sh", vec!["-c", "agent-relay mcp"]),
    ] {
        assert!(!recognized::legacy(command, &args));
    }
    assert!(recognized::json(
        &json!({"command":["npx","-y","agent-relay","mcp"]}),
        None,
        None
    ));
}

#[test]
fn unsupported_installs_are_rejected_and_configured_is_never_live_verified() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    executable_helper(root.path());
    assert!(app
        .select(vec!["unknown-provider".into()], None, None)
        .is_err());
    assert!(app
        .manage("unknown-provider", "install", None, None)
        .is_err());
    fs::create_dir_all(&app.codex.home).unwrap();
    let b = binding();
    let helper = b
        .helper(&crate::probe_copy::destination(root.path()))
        .unwrap();
    fs::write(
        app.codex.home.join("config.toml"),
        codex::render("", &b.endpoint().unwrap(), &helper, &mut |_| true).unwrap(),
    )
    .unwrap();
    let report = app.report(Some(&b), None);
    let tool = report.tools.iter().find(|t| t.id == "codex").unwrap();
    assert_eq!(tool.state, "configured");
    assert!(tool.detail.contains("not been verified"));
}

#[test]
fn configuration_proof_rejects_duplicates_project_overrides_and_competing_auth() {
    let b = binding();
    let endpoint = b.endpoint().unwrap();
    let helper = b.helper(Path::new("/probe")).unwrap();
    let root =
        json!({"mcpServers": {NAME: {"type":"http", "url":endpoint, "headersHelper":helper}}});
    assert!(verification::json(&root, Some(&endpoint), Some(&helper)));
    for extra in [
        json!({"headers":{"aUtHoRiZaTiOn":"synthetic"}}),
        json!({"oauth":{}}),
        json!({"command":"other"}),
    ] {
        let mut changed = root.clone();
        changed["mcpServers"][NAME]
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(!verification::json(
            &changed,
            Some(&endpoint),
            Some(&helper)
        ));
    }
    let mut duplicate = root.clone();
    duplicate["mcpServers"]["alias"] = json!({"url":endpoint});
    assert!(!verification::json(
        &duplicate,
        Some(&endpoint),
        Some(&helper)
    ));
    let mut project = root;
    project["projects"] =
        json!({"/project":{"mcpServers":{"old":{"command":"agent-relay","args":["mcp"]}}}});
    assert!(!verification::json(
        &project,
        Some(&endpoint),
        Some(&helper)
    ));
    let mut server = toml_edit::Table::new();
    server["url"] = toml_edit::value(endpoint.as_str());
    server["http_headers_helper"] = toml_edit::value(helper.as_str());
    let mut server = toml_edit::Item::Table(server);
    assert!(verification::codex(&server, Some(&endpoint), Some(&helper)));
    server["bearer_token_env_var"] = toml_edit::value("SYNTHETIC");
    assert!(!verification::codex(
        &server,
        Some(&endpoint),
        Some(&helper)
    ));
}

#[test]
fn legacy_stdio_uninstall_does_not_need_an_installed_codex_cli() {
    let root = tempfile::tempdir().unwrap();
    let mut app = installer(root.path());
    app.codex.executable = None;
    fs::create_dir_all(&app.codex.home).unwrap();
    let path = app.codex.home.join("config.toml");
    fs::write(&path, "[mcp_servers.legacy]\ncommand = 'agent-relay'\nargs = ['mcp']\n[mcp_servers.other]\ncommand = 'other'\n").unwrap();
    let report = app.manage("codex", "uninstall", None, None).unwrap();
    let tool = report.tools.iter().find(|tool| tool.id == "codex").unwrap();
    assert!(!tool.can_uninstall);
    assert!(!fs::read_to_string(&path).unwrap().contains("legacy"));
    assert!(fs::read_to_string(path).unwrap().contains("other"));
}

#[test]
fn lifecycle_clears_stale_failure_only_after_configuration_proof() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    let b = binding();
    app.select(vec!["codex".into()], Some(&b), None).unwrap();
    executable_helper(root.path());
    fs::create_dir_all(&app.codex.home).unwrap();
    let path = app.codex.home.join("config.toml");
    fs::write(
        &path,
        "[mcp_servers.agent-relay]\nurl = 'https://unrelated.example/mcp'\n",
    )
    .unwrap();
    app.failures
        .lock()
        .unwrap()
        .insert("codex".into(), "synthetic previous failure".into());
    app.refresh(Some(&b));
    assert!(app.failures.lock().unwrap().contains_key("codex"));
    fs::write(&path, "").unwrap();
    app.refresh(Some(&b));
    assert!(!app.failures.lock().unwrap().contains_key("codex"));
    assert_eq!(
        app.report(Some(&b), None)
            .tools
            .iter()
            .find(|tool| tool.id == "codex")
            .unwrap()
            .state,
        "configured"
    );
}

#[test]
fn configured_claude_with_ready_login_is_not_reported_as_healthy_messaging() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    executable_helper(root.path());
    let b = binding();
    app.select(vec!["claude".into()], Some(&b), None).unwrap();
    fs::create_dir_all(&app.claude.home).unwrap();
    let helper = b
        .helper(&crate::probe_copy::destination(root.path()))
        .unwrap();
    fs::write(
        root.path().join(".claude.json"),
        json_config::render("", &b.endpoint().unwrap(), &helper).unwrap(),
    )
    .unwrap();
    let claude = |app: &Installer| {
        app.report(Some(&b), Some("ready".into()))
            .tools
            .into_iter()
            .find(|tool| tool.id == "claude")
            .unwrap()
    };
    // The MCP entry without the chat identity hook needs repair.
    assert_eq!(claude(&app).status_label, "Setup needed");
    let settings = app.claude.home.join("settings.json");
    let hook = app.claude_hook_command().unwrap();
    assert!(app.edit_claude_hook(&settings, Some(&hook)));
    let report = claude(&app);
    assert_eq!(report.state, "action_required");
    assert_eq!(report.status_label, "Messaging unavailable");
    assert!(report
        .detail
        .contains("does not verify Claude chat identity yet"));
    assert!(report.can_repair && report.can_uninstall);
    // Once Cloud verifies Claude proofs, the setup is configured.
    app.set_claude_attestation_source(std::sync::Arc::new(|| true));
    assert_eq!(claude(&app).state, "configured");
}

#[test]
fn claude_setup_installs_repairs_and_removes_the_identity_hook_with_the_mcp_entry() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    executable_helper(root.path());
    let b = binding();
    fs::create_dir_all(&app.claude.home).unwrap();
    let settings = app.claude.home.join("settings.json");
    let others = serde_json::json!({
        "crossSessionInbound": "accept",
        "hooks": {
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "audit-bash"}]}],
            "SessionStart": [{"hooks": [{"type": "command", "command": "probe claude-session-hook"}]}],
        },
    });
    fs::write(&settings, others.to_string()).unwrap();
    agent_relay_node::platform::fs::set_mode(&settings, 0o600).unwrap();
    app.select(vec!["claude".into()], Some(&b), None).unwrap();
    assert!(app.refresh(Some(&b)).is_empty());
    let read = || serde_json::from_slice::<Value>(&fs::read(&settings).unwrap()).unwrap();
    let hook = app.claude_hook_command().unwrap();
    assert!(hook.ends_with(" claude-attestation-hook"), "{hook}");
    let installed = read();
    assert!(claude_hook::current(&installed, &hook), "{installed}");
    assert_eq!(installed["crossSessionInbound"], "accept");
    assert_eq!(
        installed["hooks"]["SessionStart"],
        others["hooks"]["SessionStart"]
    );
    assert_eq!(
        installed["hooks"]["PreToolUse"][0],
        others["hooks"]["PreToolUse"][0]
    );
    assert_eq!(
        installed["hooks"]["PreToolUse"][1]["matcher"],
        crate::claude_attestation::MATCHER
    );
    let report = app.report(Some(&b), None);
    let claude = report.tools.iter().find(|t| t.id == "claude").unwrap();
    assert_eq!(claude.status_label, "Messaging unavailable");

    // A deleted hook is reported and repair puts it back.
    fs::write(&settings, others.to_string()).unwrap();
    assert!(!app.claude_hook_configured());
    let report = app.report(Some(&b), None);
    let claude = report.tools.iter().find(|t| t.id == "claude").unwrap();
    assert_eq!(claude.status_label, "Setup needed");
    app.manage("claude", "repair", Some(&b), None).unwrap();
    assert!(claude_hook::current(&read(), &hook));

    app.manage("claude", "uninstall", Some(&b), None).unwrap();
    assert_eq!(read(), others);
    let global: Value =
        serde_json::from_slice(&fs::read(root.path().join(".claude.json")).unwrap()).unwrap();
    assert!(global["mcpServers"].get(NAME).is_none(), "{global}");
}

#[test]
fn missing_or_nonexecutable_helper_is_never_reported_as_configured() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    let b = binding();
    app.select(vec!["codex".into()], Some(&b), None).unwrap();
    let command = crate::probe_copy::destination(root.path());
    let helper = b.helper(&command).unwrap();
    fs::create_dir_all(&app.codex.home).unwrap();
    fs::write(
        app.codex.home.join("config.toml"),
        codex::render("", &b.endpoint().unwrap(), &helper, &mut |_| true).unwrap(),
    )
    .unwrap();
    let report = app.report(Some(&b), Some("ready".into()));
    let codex = report.tools.iter().find(|tool| tool.id == "codex").unwrap();
    assert_eq!(codex.state, "action_required");
    assert!(codex.detail.contains("helper is missing or cannot be run"));
    executable_helper(root.path());
    assert!(
        app.proof("codex", Some(&b.endpoint().unwrap()), Some(&helper))
            .1
    );
    #[cfg(unix)]
    {
        agent_relay_node::platform::fs::set_mode(&command, 0o600).unwrap();
        assert!(
            !app.proof("codex", Some(&b.endpoint().unwrap()), Some(&helper))
                .1
        );
        assert!(app
            .report(Some(&b), None)
            .tools
            .iter()
            .find(|tool| tool.id == "codex")
            .unwrap()
            .detail
            .contains("cannot be run"));
    }
}

#[test]
fn uninstall_cleans_known_unshared_claude_folders_without_installing_into_them() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    let b = binding();
    executable_helper(root.path());
    let extra = root.path().join("other-claude");
    fs::create_dir_all(&extra).unwrap();
    app.claude
        .dirs
        .set_added(&[extra.to_string_lossy().into_owned()], root.path())
        .unwrap();
    app.claude
        .dirs
        .set_shared(&extra.to_string_lossy(), false, &app.claude.home)
        .unwrap();
    let helper = b
        .helper(&crate::probe_copy::destination(root.path()))
        .unwrap();
    fs::write(
        root.path().join(".claude.json"),
        json_config::render("", &b.endpoint().unwrap(), &helper).unwrap(),
    )
    .unwrap();
    let extra_file = extra.join(".claude.json");
    fs::write(&extra_file, "{}").unwrap();
    let hook = app.claude_hook_command().unwrap();
    assert!(app.edit_claude_hook(&app.claude.home.join("settings.json"), Some(&hook)));
    assert!(
        app.proof("claude", Some(&b.endpoint().unwrap()), Some(&helper))
            .1,
        "an unshared folder does not need an MCP installation"
    );
    fs::write(&extra_file, json!({"mcpServers":{"old-relay":{"url":b.endpoint().unwrap()},"custom":{"url":"https://unrelated.example/mcp"}}}).to_string()).unwrap();
    // A hook left in a folder since unshared is removed with the rest.
    let extra_settings = extra.join("settings.json");
    let with_hook = claude_hook::render(
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo"}]}]}}"#,
        Some(&hook),
    )
    .unwrap();
    fs::write(&extra_settings, with_hook).unwrap();
    agent_relay_node::platform::fs::set_mode(&extra_settings, 0o600).unwrap();
    let report = app.manage("claude", "uninstall", Some(&b), None).unwrap();
    let left: Value = serde_json::from_slice(&fs::read(&extra_settings).unwrap()).unwrap();
    assert_eq!(
        left,
        json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo"}]}]}})
    );
    assert!(
        !report
            .tools
            .iter()
            .find(|tool| tool.id == "claude")
            .unwrap()
            .can_uninstall
    );
    let remaining: Value = serde_json::from_slice(&fs::read(extra_file).unwrap()).unwrap();
    assert!(remaining["mcpServers"]["old-relay"].is_null());
    assert!(remaining["mcpServers"]["custom"].is_object());
    assert!(app
        .claude
        .dirs
        .watched(&app.claude.home)
        .iter()
        .any(|folder| folder.path == extra && !folder.shared));
}

#[test]
fn large_normal_json_never_uses_the_relaxed_parser() {
    let projects: serde_json::Map<String, Value> = (0..5000).map(|index| {
        (format!("/synthetic/project-{index}"), json!({"allowedTools":[],"history":[{"display":"synthetic project preference"}],"mcpServers":{}}))
    }).collect();
    let text = serde_json::to_string(&json!({"projects":projects,"mcpServers":{}})).unwrap();
    assert!(text.len() > 350_000);
    for relaxed in [false, true] {
        let parsed = inventory::parse_with_fallback(&text, relaxed, |_| {
            panic!("valid JSON must never enter the JSON5 parser")
        });
        assert_eq!(parsed.unwrap()["projects"].as_object().unwrap().len(), 5000);
    }
    assert!(inventory::parse_with_fallback("{broken", false, |_| panic!(
        "strict JSON files must not invoke JSON5"
    ))
    .is_none());
    assert_eq!(
        inventory::parse_with_fallback("{relaxed: true}", true, |text| json5::from_str(text).ok()),
        Some(json!({"relaxed":true}))
    );
}

#[test]
fn manual_provider_choices_survive_restart_without_installing_local_configuration() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    for id in ["capy", "replicas"] {
        let report = app.manage(id, "enable", Some(&binding()), None).unwrap();
        let tool = report.tools.iter().find(|tool| tool.id == id).unwrap();
        assert!(tool.selected);
        assert!(!tool.supported);
        if id != "capy" {
            assert!(!tool.detected);
        }
        assert!(!tool.can_uninstall);
        assert!(matches!(
            tool.state.as_str(),
            "external_setup" | "unavailable"
        ));
    }
    let restarted = installer(root.path());
    assert!(restarted.refresh(Some(&binding())).is_empty());
    let report = restarted.report(None, None);
    for id in ["capy", "replicas"] {
        assert!(
            report
                .tools
                .iter()
                .find(|tool| tool.id == id)
                .unwrap()
                .selected
        );
    }
    assert!(!crate::probe_copy::destination(root.path()).exists());
    assert!(!app.codex.home.exists());
    assert!(!app.claude.home.exists());
    assert!(!app.opencode.config_dir.exists());
    restarted
        .manage("replicas", "disable", Some(&binding()), None)
        .unwrap();
    let report = installer(root.path()).report(None, None);
    assert!(
        !report
            .tools
            .iter()
            .find(|tool| tool.id == "replicas")
            .unwrap()
            .selected
    );
    assert!(
        report
            .tools
            .iter()
            .find(|tool| tool.id == "capy")
            .unwrap()
            .selected
    );
}

#[test]
fn catalog_is_alphabetical_and_manual_supported_enable_defers_setup() {
    let root = tempfile::tempdir().unwrap();
    let mut app = installer(root.path());
    app.codex.executable = None;
    let report = app
        .manage("codex", "enable", Some(&binding()), None)
        .unwrap();
    let tool = report.tools.iter().find(|tool| tool.id == "codex").unwrap();
    assert!(tool.selected);
    assert!(!tool.detected);
    assert_ne!(tool.state, "configured");
    assert!(!app.codex.home.join("config.toml").exists());
    assert!(!crate::probe_copy::destination(root.path()).exists());
    assert!(report.tools.windows(2).all(|pair| {
        (pair[0].name.to_lowercase(), &pair[0].id) < (pair[1].name.to_lowercase(), &pair[1].id)
    }));
    assert!(app.manage("codex", "disable", None, None).is_err());
    app.manage("codex", "uninstall", None, None).unwrap();
    assert!(
        !installer(root.path())
            .report(None, None)
            .tools
            .iter()
            .find(|tool| tool.id == "codex")
            .unwrap()
            .selected
    );
    assert!(app
        .manage("unknown-provider", "enable", None, None)
        .is_err());
}

#[test]
fn a_new_choice_replaces_damaged_provider_choices() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    fs::create_dir_all(app.choices.parent().unwrap()).unwrap();
    fs::write(&app.choices, "{not json").unwrap();
    assert!(app.selection().is_err());
    app.manage("codex", "enable", None, None).unwrap();
    assert_eq!(app.selection().unwrap(), Some(vec!["codex".to_string()]));
    fs::write(&app.choices, "{not json").unwrap();
    app.select(Vec::new(), None, None).unwrap();
    assert_eq!(app.selection().unwrap(), Some(Vec::new()));
}
