use super::*;
fn config(helper: PathBuf) -> HostedConfig {
    let binding = crate::mcp_settings::Binding {
        site_url: "https://agentrelay.com".into(),
        workspace_id: "synthetic-workspace".into(),
        account_id: "synthetic-user".into(),
    };
    HostedConfig {
        endpoint: binding.endpoint().unwrap(),
        headers: BTreeMap::from([(
            "Authorization".into(),
            "Bearer synthetic-provider-grant".into(),
        )]),
        bearer_token_file: None,
        copilot_stdio: Some(CopilotStdio {
            command: helper,
            args: crate::copilot_dispatch::peer::arguments(&binding, false),
        }),
        cursor_stdio: None,
    }
}

#[test]
fn native_timeout_outlasts_the_bridge_without_changing_other_transports() {
    let hosted = config(PathBuf::from("/synthetic/helper"));
    let native = hosted.entry(Provider::CopilotHarness);
    assert_eq!(native["timeout"], 180_000);
    assert!(
        native["timeout"].as_u64().unwrap() as u128
            > crate::copilot_dispatch::LOCAL_RESPONSE_BUDGET.as_millis()
    );
    assert!(hosted.entry(Provider::Copilot).get("timeout").is_none());
    let mut http = hosted;
    http.copilot_stdio = None;
    assert!(http
        .entry(Provider::CopilotHarness)
        .get("timeout")
        .is_none());
}

#[test]
fn legacy_owned_stdio_migrates_timeout_idempotently_and_rolls_back_exactly() {
    let home = tempfile::tempdir().unwrap();
    let helper = home.path().join("managed-helper");
    let hosted = config(helper.clone());
    let file = ProviderFile::new(Provider::CopilotHarness, home.path().join("config.json"))
        .with_copilot_helper(helper);
    let mut legacy = hosted.entry(Provider::CopilotHarness);
    legacy.as_object_mut().unwrap().remove("timeout");
    let original = json!({"mcpServers":{"agentrelay":legacy,
        "other":{"command":"other","timeout":12}},"userSetting":true})
    .to_string();
    fs::write(&file.path, &original).unwrap();
    private_fs::set_mode(&file.path, 0o600).unwrap();
    let inspection = file.inspect(Some(&hosted)).unwrap();
    assert!(inspection.present && !inspection.configured);
    let backup = file.copilot_backup(&hosted).unwrap();
    file.install(&hosted).unwrap();
    assert!(file.inspect(Some(&hosted)).unwrap().configured);
    let installed = fs::read_to_string(&file.path).unwrap();
    let root: Value = serde_json::from_str(&installed).unwrap();
    assert_eq!(root["mcpServers"]["agentrelay"]["timeout"], 180_000);
    assert_eq!(
        root["mcpServers"]["other"],
        json!({"command":"other","timeout":12})
    );
    assert_eq!(root["userSetting"], true);
    assert!(private_fs::private_to_owner(
        &fs::metadata(&file.path).unwrap(),
        0o077
    ));
    file.install(&hosted).unwrap();
    assert_eq!(fs::read_to_string(&file.path).unwrap(), installed);
    backup.restore(&hosted).unwrap();
    assert_eq!(fs::read_to_string(&file.path).unwrap(), original);
}

#[test]
fn timeout_is_part_of_exact_entry_verification_not_an_ownership_marker() {
    let home = tempfile::tempdir().unwrap();
    let helper = home.path().join("managed-helper");
    let hosted = config(helper.clone());
    let expected = hosted.entry(Provider::CopilotHarness);
    for timeout in [json!(60_000), json!("180000"), Value::Null] {
        let mut changed = expected.clone();
        changed["timeout"] = timeout;
        assert!(!entry_matches(&changed, &expected));
    }
    let file = ProviderFile::new(Provider::CopilotHarness, home.path().join("config.json"))
        .with_copilot_helper(helper);
    let mut foreign = expected;
    foreign["command"] = json!(home.path().join("foreign-helper"));
    foreign.as_object_mut().unwrap().remove("timeout");
    let original = json!({"mcpServers":{"agentrelay":foreign}}).to_string();
    fs::write(&file.path, &original).unwrap();
    assert!(file.install(&hosted).is_err());
    assert_eq!(fs::read_to_string(&file.path).unwrap(), original);
}

#[test]
fn hardlinked_stdio_command_preserves_exact_binding_and_refuses_copies() {
    let home = tempfile::tempdir().unwrap();
    let main = home.path().join("helper main");
    let alias = home.path().join("helper alias");
    let copy = home.path().join("helper copy");
    fs::write(&main, "synthetic helper").unwrap();
    fs::hard_link(&main, &alias).unwrap();
    fs::copy(&main, &copy).unwrap();
    let hosted = config(alias.clone());
    let file = ProviderFile::new(Provider::CopilotHarness, home.path().join("config.json"))
        .with_copilot_helper(alias);
    let mut entry = hosted.entry(Provider::CopilotHarness);
    entry["command"] = json!(main);
    let original = json!({"mcpServers":{"agentrelay":entry}}).to_string();
    fs::write(&file.path, &original).unwrap();
    private_fs::set_mode(&file.path, 0o600).unwrap();
    let inspection = file.inspect(Some(&hosted)).unwrap();
    assert!(inspection.present && inspection.configured);
    assert_eq!(fs::read_to_string(&file.path).unwrap(), original);
    let mut changed = entry.clone();
    changed["args"][2] = json!("https://other.example");
    fs::write(
        &file.path,
        json!({"mcpServers":{"agentrelay":changed}}).to_string(),
    )
    .unwrap();
    assert!(!file.inspect(Some(&hosted)).unwrap().configured);
    changed = entry;
    changed["command"] = json!(copy);
    fs::write(
        &file.path,
        json!({"mcpServers":{"agentrelay":changed}}).to_string(),
    )
    .unwrap();
    let inspection = file.inspect(Some(&hosted)).unwrap();
    assert!(!inspection.present && !inspection.configured);
    assert!(file.install(&hosted).is_err());
}
#[test]
fn copied_marker_does_not_own_foreign_command_or_authorize_its_replacement() {
    let home = tempfile::tempdir().unwrap();
    let current = home.path().join("managed-current-helper");
    let hosted = config(current.clone());
    let file = ProviderFile::new(Provider::CopilotHarness, home.path().join("config.json"))
        .with_copilot_helper(current);
    let mut foreign = hosted.entry(Provider::CopilotHarness);
    foreign["command"] = json!(home.path().join("foreign-helper"));
    let original =
        json!({"mcpServers":{"agentrelay":foreign,"other":{"command":"other"}}}).to_string();
    fs::write(&file.path, &original).unwrap();
    assert!(!file.inspect(Some(&hosted)).unwrap().present);
    assert!(file.install(&hosted).is_err());
    file.remove(Some(&hosted.endpoint)).unwrap();
    assert_eq!(fs::read_to_string(&file.path).unwrap(), original);
}
#[test]
fn current_and_deleted_old_managed_helpers_migrate_remove_and_rollback_without_foreign_loss() {
    let home = tempfile::tempdir().unwrap();
    let hosted = config(home.path().join("current-helper"));
    let file = ProviderFile::new(Provider::CopilotHarness, home.path().join("config.json"))
        .with_copilot_helper(hosted.copilot_stdio.as_ref().unwrap().command.clone());
    let mut old = hosted.entry(Provider::CopilotHarness);
    old["command"] = json!(home.path().join(".local/bin/agent-relay-probe"));
    let original =
        json!({"mcpServers":{"agent-relay":old,"other":{"command":"other"}}}).to_string();
    fs::write(&file.path, &original).unwrap();
    let backup = file.copilot_backup(&hosted).unwrap();
    file.install(&hosted).unwrap();
    assert!(file.inspect(Some(&hosted)).unwrap().configured);
    let mut current: Value =
        serde_json::from_str(&fs::read_to_string(&file.path).unwrap()).unwrap();
    let mut foreign = hosted.entry(Provider::CopilotHarness);
    foreign["command"] = json!(home.path().join("foreign-helper"));
    current["mcpServers"]["concurrent-foreign"] = foreign.clone();
    fs::write(&file.path, current.to_string()).unwrap();
    backup.restore(&hosted).unwrap();
    let restored: Value = serde_json::from_str(&fs::read_to_string(&file.path).unwrap()).unwrap();
    assert_eq!(restored["mcpServers"]["agent-relay"], old);
    assert_eq!(restored["mcpServers"]["concurrent-foreign"], foreign);
    file.remove(Some(&hosted.endpoint)).unwrap();
    let result: Value = serde_json::from_str(&fs::read_to_string(&file.path).unwrap()).unwrap();
    assert_eq!(result["mcpServers"]["concurrent-foreign"], foreign);
    assert!(result["mcpServers"].get("agent-relay").is_none());
}

#[test]
fn cursor_helper_entries_are_managed_only_in_their_exact_shape() {
    let home = tempfile::tempdir().unwrap();
    let helper = home.path().join("helper");
    fs::write(&helper, "synthetic helper").unwrap();
    let binding = crate::mcp_settings::Binding {
        site_url: "https://agentrelay.com".into(),
        workspace_id: "synthetic-workspace".into(),
        account_id: "synthetic-user".into(),
    };
    let endpoint = binding.endpoint().unwrap();
    let socket = home.path().join("relay.sock");
    let entry = |args: Vec<String>| json!({"command": helper, "args": args});
    let managed = |entry: &Value| managed_cursor_stdio(entry, Some(&endpoint), Some(&helper));
    let plain = crate::cursor_dispatch::arguments(&binding, false, None);
    assert!(managed(&entry(plain.clone())));
    assert!(managed(&entry(crate::cursor_dispatch::arguments(
        &binding,
        true,
        Some(&socket)
    ))));
    assert!(!managed_cursor_stdio(
        &entry(plain.clone()),
        Some("https://agentrelay.com/elsewhere"),
        Some(&helper)
    ));
    let mut typed = entry(plain.clone());
    typed["type"] = json!("stdio");
    assert!(!managed(&typed), "Cursor's entry has no other members");
    let mut relative = plain.clone();
    relative.extend(["--socket".into(), "relay.sock".into()]);
    assert!(!managed(&entry(relative)));
    let mut reordered = plain.clone();
    reordered.swap(1, 3);
    reordered.swap(2, 4);
    assert!(!managed(&entry(reordered)));
    let mut extra = plain;
    extra.push("--verbose".into());
    assert!(!managed(&entry(extra)));
    assert!(!managed(
        &json!({"command": "helper", "args": crate::cursor_dispatch::arguments(&binding, false, None)})
    ));
}
