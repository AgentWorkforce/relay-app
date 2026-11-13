use super::*;

fn hosted() -> HostedConfig {
    HostedConfig {
        bearer_token_file: None,
        copilot_stdio: None,
        endpoint: "https://agentrelay.com/cloud/api/v1/mcp/shared-sessions".into(),
        headers: BTreeMap::from([("Authorization".into(), "Bearer fixture-grant".into())]),
    }
}

#[test]
fn native_adapters_install_rotate_and_remove_without_changing_other_servers() {
    for (provider, original) in [
        (
            Provider::Cursor,
            "{ // keep\n mcpServers: {other:{command:'other'}}}",
        ),
        (
            Provider::CopilotHarness,
            "{ // keep\n mcpServers: {other:{command:'other'}}, flag:true}",
        ),
        (
            Provider::Copilot,
            "{ // keep\n servers: {other:{command:'other'}}, inputs:[]}",
        ),
        (
            Provider::Grok,
            "# keep\nmodel = 'test'\n[mcp_servers.other]\ncommand = 'other'\n",
        ),
        (
            Provider::OpenCodeV1,
            "{ // keep\n model: 'test', mcp: {other:{type:'local',command:['other']}}}",
        ),
        (
            Provider::OpenCodeV2,
            "{ // keep\n model: 'test', mcp: {servers:{other:{type:'local',command:['other']}}}}",
        ),
        (
            Provider::Devin,
            "{ // keep\n model: 'test', mcpServers: {other:{command:'other'}}}",
        ),
        (
            Provider::Muse,
            "{ // keep\n schema_version:1,model:'test',mcpServers:{other:{command:'other'}}}",
        ),
        (
            Provider::OpenClaw,
            "{ // keep\n model:'test',mcp:{servers:{other:{command:'other'}}}}",
        ),
        (
            Provider::Hermes,
            "# keep\nmodel: test\nmcp_servers:\n  other:\n    command: other\n",
        ),
        (
            Provider::Droid,
            "{ // keep\n mcpServers: {other:{type:'stdio',command:'other',args:[],env:{},disabled:true}}}",
        ),
        (
            Provider::Copilot,
            "{ // keep\n inputs: [], servers: {other:{type:'stdio',command:'other'}}}",
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let file = ProviderFile::new(provider, root.path().join("config"));
        fs::write(&file.path, original).unwrap();
        let config = hosted();
        file.install(&config)
            .unwrap_or_else(|e| panic!("{provider:?}: {e}"));
        assert!(
            file.inspect(Some(&config)).unwrap().configured,
            "{provider:?}"
        );
        let installed = fs::read_to_string(&file.path).unwrap();
        assert!(installed.contains("keep"), "{provider:?}");
        assert!(file.servers(&installed).unwrap().contains_key("other"));
        file.install(&config).unwrap();
        assert_eq!(installed, fs::read_to_string(&file.path).unwrap());
        let mut rotated = config.clone();
        rotated
            .headers
            .insert("Authorization".into(), "Bearer replacement-fixture".into());
        file.install(&rotated).unwrap();
        assert!(!file.inspect(Some(&config)).unwrap().configured);
        file.remove(Some(&config.endpoint)).unwrap();
        assert!(!file.inspect(None).unwrap().present);
        assert!(file
            .servers(&file.read().unwrap())
            .unwrap()
            .contains_key("other"));
    }
}

#[test]
fn empty_configs_install_and_muse_keeps_legacy_schema_unambiguous() {
    for provider in [
        Provider::Cursor,
        Provider::CopilotHarness,
        Provider::Copilot,
        Provider::Grok,
        Provider::OpenCodeV1,
        Provider::OpenCodeV2,
        Provider::Devin,
        Provider::Muse,
        Provider::OpenClaw,
        Provider::Hermes,
        Provider::Droid,
    ] {
        let root = tempfile::tempdir().unwrap();
        let file = ProviderFile::new(provider, root.path().join("sub/config"));
        file.install(&hosted())
            .unwrap_or_else(|e| panic!("{provider:?}: {e}"));
        assert!(file.inspect(Some(&hosted())).unwrap().configured);
    }
    let root = tempfile::tempdir().unwrap();
    let file = ProviderFile::new(Provider::Muse, root.path().join("settings.json"));
    fs::write(
        &file.path,
        r#"{"schema_version":1,"mcp_servers":{"other":{"transport":"stdio","command":"other"}}}"#,
    )
    .unwrap();
    file.install(&hosted()).unwrap();
    assert!(file.inspect(Some(&hosted())).unwrap().configured);
    let actual: Value = serde_json::from_str(&file.read().unwrap()).unwrap();
    assert!(actual.get("mcpServers").is_none());
    assert_eq!(actual["mcp_servers"][NAME]["transport"], "streamable_http");
}

#[test]
fn malformed_and_unrelated_named_servers_fail_closed() {
    for (provider, original) in [
        (Provider::Grok, "invalid ["),
        (Provider::Hermes, "mcp_servers: ["),
        (Provider::Devin, "{broken"),
        (Provider::Muse, r#"{"mcpServers":{},"mcp_servers":{}}"#),
        (
            Provider::OpenClaw,
            r#"{"mcp":{"servers":{"agent-relay":{"url":"https://other.example/mcp"}}}}"#,
        ),
        (Provider::Devin, r#"{"mcpServers":{},"mcpServers":{}}"#),
    ] {
        let root = tempfile::tempdir().unwrap();
        let file = ProviderFile::new(provider, root.path().join("config"));
        fs::write(&file.path, original).unwrap();
        assert!(file.install(&hosted()).is_err(), "{provider:?}");
        assert_eq!(fs::read_to_string(&file.path).unwrap(), original);
    }
}

#[test]
fn recognized_aliases_are_replaced_but_custom_entries_survive_uninstall() {
    let root = tempfile::tempdir().unwrap();
    let file = ProviderFile::new(Provider::Devin, root.path().join("config.json"));
    fs::write(&file.path,r#"{"mcpServers":{"old":{"command":"npx","args":["-y","agent-relay","mcp"]},"other":{"command":"unrelated-agent-relay"}}}"#).unwrap();
    file.install(&hosted()).unwrap();
    assert!(!file
        .servers(&file.read().unwrap())
        .unwrap()
        .contains_key("old"));
    fs::write(
        &file.path,
        r#"{"mcpServers":{"agent-relay":{"url":"https://custom.example/mcp"}}}"#,
    )
    .unwrap();
    let original = file.read().unwrap();
    file.remove(Some(&hosted().endpoint)).unwrap();
    assert_eq!(file.read().unwrap(), original);
}

#[test]
fn path_resolution_uses_only_supplied_environment() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("dev-clients");
    let env = BTreeMap::from([
        ("GROK_HOME", "~/custom-grok"),
        ("XDG_CONFIG_HOME", "~/config"),
        ("OPENCLAW_CONFIG_PATH", "~/claw/settings.json"),
    ]);
    let paths = Locations::from_env(&home, &|key| env.get(key).map(|value| value.to_string()));
    assert_eq!(paths.grok, home.join("custom-grok/config.toml"));
    assert_eq!(paths.muse, home.join("config/muse/settings.json"));
    assert_eq!(paths.openclaw, home.join("claw/settings.json"));
    assert!(paths.opencode.iter().all(|path| path.starts_with(&home)));
}

#[cfg(unix)]
#[test]
fn credential_files_are_private_and_symlink_targets_are_never_changed() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    fs::write(&target, "{}").unwrap();
    let file = ProviderFile::new(Provider::Devin, root.path().join("config"));
    symlink(&target, &file.path).unwrap();
    assert!(file.install(&hosted()).is_err());
    assert_eq!(fs::read_to_string(target).unwrap(), "{}");
    fs::remove_file(&file.path).unwrap();
    file.install(&hosted()).unwrap();
    assert_eq!(
        fs::metadata(&file.path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn signed_out_uninstall_recognizes_owned_development_endpoints() {
    let mut config = hosted();
    config.endpoint = "http://127.0.0.1:3100/cloud/api/v1/mcp/shared-sessions".into();
    for provider in [
        Provider::Grok,
        Provider::Devin,
        Provider::Hermes,
        Provider::Droid,
    ] {
        let root = tempfile::tempdir().unwrap();
        let file = ProviderFile::new(provider, root.path().join("config"));
        file.install(&config).unwrap();
        assert!(file.inspect(None).unwrap().present);
        file.remove(None).unwrap();
        assert!(!file.inspect(None).unwrap().present);
    }
}

#[cfg(unix)]
#[test]
fn parent_symlinks_are_rejected_and_readable_credentials_are_repaired() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), root.path().join("profile")).unwrap();
    let symlinked = ProviderFile::new(Provider::Devin, root.path().join("profile/config.json"));
    assert!(symlinked.install(&hosted()).is_err());
    assert!(!outside.path().join("config.json").exists());
    let file = ProviderFile::new(Provider::Devin, root.path().join("config.json"));
    file.install(&hosted()).unwrap();
    private_fs::set_mode(&file.path, 0o644).unwrap();
    assert!(!file.inspect(Some(&hosted())).unwrap().configured);
    file.install(&hosted()).unwrap();
    assert!(file.inspect(Some(&hosted())).unwrap().configured);
}

#[test]
fn inline_toml_and_yaml_maps_accept_native_installation() {
    for (provider, original) in [
        (
            Provider::Grok,
            "mcp_servers = {other = {command = 'other'}}\n",
        ),
        (
            Provider::Hermes,
            "{model: test, mcp_servers: {other: {command: other}}}\n",
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let file = ProviderFile::new(provider, root.path().join("config"));
        fs::write(&file.path, original).unwrap();
        file.install(&hosted()).unwrap();
        assert!(file.inspect(Some(&hosted())).unwrap().configured);
        assert!(file
            .servers(&file.read().unwrap())
            .unwrap()
            .contains_key("other"));
    }
}

#[test]
fn openclaw_home_is_a_home_override_and_explicit_config_has_highest_precedence() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let mut env = BTreeMap::from([
        (
            "OPENCLAW_HOME",
            root.path().join("alternate").to_string_lossy().into_owned(),
        ),
        ("OPENCLAW_PROFILE", "work".into()),
    ]);
    let resolve = |env: &BTreeMap<&str, String>| {
        Locations::from_env(&home, &|key| env.get(key).cloned()).openclaw
    };
    assert_eq!(
        resolve(&env),
        root.path().join("alternate/.openclaw-work/openclaw.json")
    );
    env.insert(
        "OPENCLAW_STATE_DIR",
        root.path().join("state").to_string_lossy().into_owned(),
    );
    assert_eq!(resolve(&env), root.path().join("state/openclaw.json"));
    env.insert(
        "OPENCLAW_CONFIG_PATH",
        root.path()
            .join("explicit.json")
            .to_string_lossy()
            .into_owned(),
    );
    assert_eq!(resolve(&env), root.path().join("explicit.json"));
}

#[test]
fn copilot_writes_an_http_server_under_vscode_servers() {
    let root = tempfile::tempdir().unwrap();
    let file = ProviderFile::new(Provider::Copilot, root.path().join("mcp.json"));
    let config = hosted();
    file.install(&config).unwrap();
    let written: Value = serde_json::from_str(&fs::read_to_string(&file.path).unwrap()).unwrap();
    let entry = &written["servers"][COPILOT_HISTORY_MCP_SERVER];
    assert_eq!(entry["type"], "http");
    assert_eq!(entry["url"], config.endpoint);
    assert_eq!(entry["headers"]["Authorization"], "Bearer fixture-grant");
    assert!(written.get("mcpServers").is_none());
}

#[test]
fn copilot_config_lives_in_the_vscode_user_directory() {
    let home = Path::new(if cfg!(windows) {
        "C:\\Users\\person"
    } else {
        "/home/person"
    });
    let copilot = Locations::from_env(home, &|_| None).copilot;
    #[cfg(windows)]
    assert_eq!(
        copilot,
        home.join("AppData/Roaming")
            .join("Code")
            .join("User")
            .join("mcp.json")
    );
    #[cfg(target_os = "macos")]
    assert_eq!(
        copilot,
        home.join("Library/Application Support/Code/User/mcp.json")
    );
    #[cfg(not(any(windows, target_os = "macos")))]
    assert_eq!(copilot, home.join(".config/Code/User/mcp.json"));
    let custom = if cfg!(windows) {
        "C:\\Profiles\\Roaming"
    } else {
        "/profiles/config"
    };
    let explicit = Locations::from_env(home, &|key| {
        matches!(key, "APPDATA" | "XDG_CONFIG_HOME").then(|| custom.to_string())
    })
    .copilot;
    if cfg!(target_os = "macos") {
        assert_eq!(explicit, copilot);
    } else {
        assert_eq!(
            explicit,
            Path::new(custom).join("Code").join("User").join("mcp.json")
        );
    }
}

#[test]
fn copilot_owned_legacy_names_migrate_and_foreign_names_are_preserved() {
    for (provider, command) in [
        (Provider::CopilotHarness, None),
        (Provider::Copilot, None),
        (
            Provider::Copilot,
            Some(json!({"command":"npx","args":["-y","agent-relay","mcp"]})),
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let file = ProviderFile::new(provider, root.path().join("config.json"));
        let config = hosted();
        let mut legacy = Map::new();
        legacy.insert(
            NAME.into(),
            command.unwrap_or_else(|| config.entry(provider)),
        );
        legacy.insert("unrelated".into(), json!({"command":"custom"}));
        let key = if provider == Provider::Copilot {
            "servers"
        } else {
            "mcpServers"
        };
        fs::write(&file.path, json!({key:legacy}).to_string()).unwrap();
        assert!(file.inspect(Some(&config)).unwrap().present);
        assert!(!file.inspect(Some(&config)).unwrap().configured);
        file.install(&config).unwrap();
        let written = file.servers(&file.read().unwrap()).unwrap();
        assert_eq!(
            written.get(provider.server_name()),
            Some(&config.entry(provider))
        );
        assert!(!written.contains_key(NAME));
        assert_eq!(written["unrelated"]["command"], "custom");
        file.remove(Some(&config.endpoint)).unwrap();
        assert!(!file.inspect(None).unwrap().present);
        let removed = file.servers(&file.read().unwrap()).unwrap();
        assert!(!removed.contains_key(provider.server_name()));
        assert!(!removed.contains_key(NAME));
        fs::write(
            &file.path,
            json!({key:{NAME:{"command":"custom"}}}).to_string(),
        )
        .unwrap();
        file.install(&config).unwrap();
        assert_eq!(
            file.servers(&file.read().unwrap()).unwrap()[NAME]["command"],
            "custom"
        );
        file.remove(Some(&config.endpoint)).unwrap();
        assert_eq!(
            file.servers(&file.read().unwrap()).unwrap()[NAME]["command"],
            "custom"
        );
    }
}
#[test]
fn copilot_foreign_canonical_name_blocks_legacy_migration_without_mutation() {
    for provider in [Provider::CopilotHarness, Provider::Copilot] {
        let root = tempfile::tempdir().unwrap();
        let file = ProviderFile::new(provider, root.path().join("config.json"));
        let config = hosted();
        let key = if provider == Provider::Copilot {
            "servers"
        } else {
            "mcpServers"
        };
        let original =
            json!({key:{NAME:config.entry(provider),(provider.server_name()):{"command":"custom"}}})
                .to_string();
        fs::write(&file.path, &original).unwrap();
        assert!(file.install(&config).is_err());
        assert_eq!(file.read().unwrap(), original);
    }
}

#[test]
fn droid_writes_an_http_server_and_keeps_other_servers_and_their_keys() {
    let root = tempfile::tempdir().unwrap();
    let file = ProviderFile::new(Provider::Droid, root.path().join(".factory/mcp.json"));
    let other = json!({
        "type": "stdio",
        "command": "other",
        "args": ["--flag"],
        "env": {"KEY": "value"},
        "disabled": true,
    });
    fs::create_dir_all(file.path.parent().unwrap()).unwrap();
    fs::write(
        &file.path,
        json!({"mcpServers": {"other": other}, "theme": "dark"}).to_string(),
    )
    .unwrap();
    let config = hosted();
    file.install(&config).unwrap();
    let written: Value = serde_json::from_str(&fs::read_to_string(&file.path).unwrap()).unwrap();
    let entry = &written["mcpServers"][NAME];
    assert_eq!(entry["type"], "http");
    assert_eq!(entry["url"], config.endpoint);
    assert_eq!(entry["headers"]["Authorization"], "Bearer fixture-grant");
    assert_eq!(written["mcpServers"]["other"], other);
    assert_eq!(written["theme"], "dark");
    file.remove(Some(&config.endpoint)).unwrap();
    let removed: Value = serde_json::from_str(&fs::read_to_string(&file.path).unwrap()).unwrap();
    assert!(removed["mcpServers"].get(NAME).is_none());
    assert_eq!(removed["mcpServers"]["other"], other);
    assert_eq!(removed["theme"], "dark");
}

#[test]
fn droid_config_lives_in_factory_home_and_follows_the_override() {
    let home = Path::new(if cfg!(windows) {
        "C:\\Users\\person"
    } else {
        "/home/person"
    });
    let resolve = |value: Option<&str>| {
        Locations::from_env(home, &|key| {
            (key == "FACTORY_HOME_OVERRIDE")
                .then(|| value.map(str::to_string))
                .flatten()
        })
        .droid
    };
    // Windows has no separate default: `%USERPROFILE%\.factory`, which is the
    // home the app already resolves.
    let default = home.join(".factory").join("mcp.json");
    assert_eq!(resolve(None), default);
    assert_eq!(resolve(Some("  ")), default);
    assert_eq!(resolve(Some("relative/factory")), default);
    let custom = if cfg!(windows) {
        "C:\\Factory"
    } else {
        "/srv/factory"
    };
    assert_eq!(
        resolve(Some(custom)),
        Path::new(custom).join(".factory").join("mcp.json")
    );
    assert_eq!(
        resolve(Some("~/alternate")),
        home.join("alternate").join(".factory").join("mcp.json")
    );
    assert_eq!(resolve(Some("~")), default);
}
