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
    }
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
