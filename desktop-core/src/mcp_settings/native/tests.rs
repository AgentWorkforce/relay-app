use super::*;

fn installer(home: &Path) -> Installer {
    Installer::new(
        home.into(),
        CodexConfig::from_env(&|_| None, home),
        ClaudeConfig::resolve(&|_| None, home, &[]),
        OpenCodeConfig::from_env(&|_| None, home),
    )
}
fn binding(account: &str) -> Binding {
    Binding {
        site_url: "https://agentrelay.com".into(),
        workspace_id: "workspace".into(),
        account_id: account.into(),
    }
}
fn credentials(ids: &[&str]) -> BTreeMap<String, String> {
    ids.iter()
        .map(|id| (id.to_string(), format!("fixture-{id}")))
        .collect()
}

#[test]
fn all_native_providers_share_app_authorization_without_a_helper_or_separate_login() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    let ids = [
        "grok", "opencode", "devin", "muse", "openclaw", "hermes", "pi", "droid",
    ];
    let binding = binding("alice");
    app.select(
        ids.iter().map(|id| id.to_string()).collect(),
        Some(&binding),
        None,
    )
    .unwrap();
    app.set_provider_credentials(&binding, credentials(&ids));
    assert!(app.refresh(Some(&binding)).is_empty());
    assert!(!crate::probe_copy::destination(root.path()).exists());
    let report = app.report(Some(&binding), Some("ready".into()));
    for id in ids {
        let row = report.tools.iter().find(|row| row.id == id).unwrap();
        assert!(row.supported && row.selected && !row.detected, "{id}");
        assert_eq!(row.state, "installed", "{id}: {}", row.detail);
        assert_eq!(row.status_label, "Setup saved");
        assert!(row.can_uninstall, "{id}");
        let report = app.manage(id, "uninstall", Some(&binding), None).unwrap();
        let row = report.tools.iter().find(|row| row.id == id).unwrap();
        assert!(!row.selected && !row.can_uninstall && !row.detected, "{id}");
    }
}

#[test]
fn account_switch_and_signout_never_reuse_provider_grants() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    app.select(vec!["grok".into()], None, None).unwrap();
    let alice = binding("alice");
    app.set_provider_credentials(&alice, credentials(&["grok"]));
    assert!(app.refresh(Some(&alice)).is_empty());
    assert!(!app.refresh(Some(&binding("bob"))).is_empty());
    assert_ne!(
        app.report(Some(&binding("bob")), None)
            .tools
            .iter()
            .find(|row| row.id == "grok")
            .unwrap()
            .state,
        "configured"
    );
    app.refresh(None);
    assert!(
        !app.refresh(Some(&alice)).is_empty(),
        "signout clears cached grants"
    );
}

#[test]
fn selected_provider_without_grant_is_actionable_and_not_configured() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    let binding = binding("alice");
    app.select(vec!["grok".into()], Some(&binding), None)
        .unwrap();
    let warnings = app.refresh(Some(&binding));
    assert!(warnings
        .iter()
        .any(|text| text.contains("could not authorize")));
    let report = app.report(Some(&binding), None);
    assert_eq!(
        report
            .tools
            .iter()
            .find(|row| row.id == "grok")
            .unwrap()
            .state,
        "action_required"
    );
    assert!(!app.native.locations.grok.exists());
}

#[test]
fn opencode_v2_and_legacy_devin_profiles_keep_their_schema() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    fs::create_dir_all(app.native.locations.devin_legacy.parent().unwrap()).unwrap();
    fs::write(
        &app.native.locations.devin_legacy,
        r#"{"theme":"dark","mcpServers":{}}"#,
    )
    .unwrap();
    let opencode = &app.native.locations.opencode[1];
    fs::create_dir_all(opencode.parent().unwrap()).unwrap();
    fs::write(opencode, "{ // personal\n mcp: { servers: {} }}").unwrap();
    let binding = binding("alice");
    app.select(
        vec!["opencode".into(), "devin".into()],
        Some(&binding),
        None,
    )
    .unwrap();
    app.set_provider_credentials(&binding, credentials(&["opencode", "devin"]));
    assert!(app.refresh(Some(&binding)).is_empty());
    let root: Value = json5::from_str(&fs::read_to_string(opencode).unwrap()).unwrap();
    assert_eq!(root["mcp"]["servers"][NAME]["disabled"], false);
    assert!(!app.native.locations.devin.exists());
    let devin: Value =
        serde_json::from_str(&fs::read_to_string(&app.native.locations.devin_legacy).unwrap())
            .unwrap();
    assert_eq!(devin["mcpServers"][NAME]["transport"], "http");
    assert_eq!(devin["theme"], "dark");
}

#[test]
fn primary_conflict_does_not_remove_an_existing_global_alias() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    let lower = &app.native.locations.opencode[0];
    let higher = &app.native.locations.opencode[1];
    fs::create_dir_all(lower.parent().unwrap()).unwrap();
    let old = r#"{"mcp":{"old-relay":{"command":["npx","agent-relay","mcp"]}}}"#;
    let custom = r#"{"mcp":{"agent-relay":{"url":"https://custom.example/mcp"}}}"#;
    fs::write(lower, old).unwrap();
    fs::write(higher, custom).unwrap();
    let binding = binding("alice");
    app.select(vec!["opencode".into()], Some(&binding), None)
        .unwrap();
    app.set_provider_credentials(&binding, credentials(&["opencode"]));
    assert!(!app.refresh(Some(&binding)).is_empty());
    assert_eq!(fs::read_to_string(lower).unwrap(), old);
    assert_eq!(fs::read_to_string(higher).unwrap(), custom);
}

#[test]
fn grok_uses_a_private_per_request_token_file_and_config_does_not_change_on_rotation() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    let alice = binding("alice");
    app.select(vec!["grok".into()], Some(&alice), None).unwrap();
    app.set_provider_credentials(&alice, credentials(&["grok"]));
    assert!(app.refresh(Some(&alice)).is_empty());
    let original = fs::read_to_string(&app.native.locations.grok).unwrap();
    assert!(!original.contains("fixture-grok"));
    assert!(!original.contains("Authorization"));
    assert!(original.contains("bearer_token_file"));
    assert!(original.contains("{{session_id}}"));
    let modified = fs::metadata(&app.native.locations.grok)
        .unwrap()
        .modified()
        .unwrap();
    let path = app.grok_credentials_path();
    assert_eq!(fs::read_to_string(&path).unwrap(), "fixture-grok");
    assert!(private_fs::private_to_owner(
        &fs::metadata(&path).unwrap(),
        0o077
    ));
    app.set_provider_credentials(
        &alice,
        BTreeMap::from([("grok".into(), "rotated-fixture".into())]),
    );
    assert!(app.refresh(Some(&alice)).is_empty());
    assert_eq!(fs::read_to_string(&path).unwrap(), "rotated-fixture");
    assert_eq!(
        fs::read_to_string(&app.native.locations.grok).unwrap(),
        original
    );
    assert_eq!(
        fs::metadata(&app.native.locations.grok)
            .unwrap()
            .modified()
            .unwrap(),
        modified
    );
    let bob = binding("bob");
    app.set_provider_credentials(
        &bob,
        BTreeMap::from([("grok".into(), "bob-fixture".into())]),
    );
    assert!(app.refresh(Some(&bob)).is_empty());
    assert_eq!(fs::read_to_string(&path).unwrap(), "bob-fixture");
    app.clear_provider_credentials();
    assert!(!path.exists());
    app.manage("grok", "uninstall", Some(&bob), None).unwrap();
    assert!(!fs::read_to_string(&app.native.locations.grok)
        .unwrap()
        .contains("bearer_token_file"));
}

#[cfg(unix)]
#[test]
fn grok_token_file_symlinks_are_not_followed() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    let path = app.grok_credentials_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let outside = root.path().join("unrelated");
    fs::write(&outside, "unchanged").unwrap();
    symlink(&outside, &path).unwrap();
    let alice = binding("alice");
    app.select(vec!["grok".into()], Some(&alice), None).unwrap();
    app.set_provider_credentials(&alice, credentials(&["grok"]));
    assert!(!app.refresh(Some(&alice)).is_empty());
    assert_eq!(fs::read_to_string(outside).unwrap(), "unchanged");
    assert!(!app.native.locations.grok.exists());
}

#[test]
fn legacy_app_login_prompts_reauthentication_before_generic_setup_errors() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    let alice = binding("alice");
    app.select(vec!["grok".into()], Some(&alice), None).unwrap();
    app.set_provider_credentials(&alice, BTreeMap::new());
    app.set_provider_login_required(&alice, vec!["grok".into()]);
    assert!(!app.refresh(Some(&alice)).is_empty());
    let report = app.report(Some(&alice), Some("ready".into()));
    let row = report.tools.iter().find(|row| row.id == "grok").unwrap();
    assert_eq!(row.state, "signed_out");
    assert_eq!(row.status_label, "Sign in needed");
    assert_eq!(
        row.detail,
        "Sign in to Agent Relay again to connect this provider on this computer."
    );
    let bob = app.report(Some(&binding("bob")), Some("ready".into()));
    assert_ne!(
        bob.tools.iter().find(|row| row.id == "grok").unwrap().state,
        "signed_out"
    );
    app.clear_provider_credentials();
    assert!(!app.native_login_required("grok", Some(&alice)));
}

#[test]
fn deselect_removes_previous_global_installation_and_preserves_other_servers() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    let alice = binding("alice");
    fs::create_dir_all(app.native.locations.grok.parent().unwrap()).unwrap();
    fs::write(
        &app.native.locations.grok,
        "[mcp_servers.other]\ncommand='other'\n",
    )
    .unwrap();
    app.select(vec!["grok".into(), "hermes".into()], Some(&alice), None)
        .unwrap();
    app.set_provider_credentials(&alice, credentials(&["grok", "hermes"]));
    assert!(app.refresh(Some(&alice)).is_empty());
    app.select(vec!["hermes".into()], Some(&alice), None)
        .unwrap();
    let grok = fs::read_to_string(&app.native.locations.grok).unwrap();
    assert!(grok.contains("mcp_servers.other"));
    assert!(!grok.contains("agent-relay"));
    assert!(!app.grok_credentials_path().exists());
    assert!(fs::read_to_string(&app.native.locations.hermes)
        .unwrap()
        .contains("agent-relay"));
}

#[test]
fn failed_deselection_keeps_invalid_config_and_reports_cleanup_failure() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    let alice = binding("alice");
    app.select(vec!["grok".into()], Some(&alice), None).unwrap();
    app.set_provider_credentials(&alice, credentials(&["grok"]));
    assert!(app.refresh(Some(&alice)).is_empty());
    fs::write(&app.native.locations.grok, "broken = [").unwrap();
    let report = app.select(Vec::new(), Some(&alice), None).unwrap();
    let row = report.tools.iter().find(|row| row.id == "grok").unwrap();
    assert!(!row.selected);
    assert_eq!(row.state, "action_required");
    assert!(row.can_uninstall);
    assert_eq!(
        fs::read_to_string(&app.native.locations.grok).unwrap(),
        "broken = ["
    );
}

#[test]
fn provider_detection_tracks_real_executables_not_installed_mcp_profiles() {
    let root = tempfile::tempdir().unwrap();
    let binary = root
        .path()
        .join(if cfg!(windows) { "grok.exe" } else { "grok" });
    let env =
        |key: &str| (key == "RELAY_DESKTOP_GROK").then(|| binary.to_string_lossy().into_owned());
    let app = installer(root.path()).with_environment(&env);
    fs::create_dir_all(app.native.locations.grok.parent().unwrap()).unwrap();
    fs::write(&app.native.locations.grok, "{}").unwrap();
    assert!(
        !app.native_detected("grok"),
        "a profile is not an installed executable"
    );
    fs::write(&binary, "fixture executable").unwrap();
    private_fs::set_mode(&binary, 0o700).unwrap();
    assert!(
        app.native_detected("grok"),
        "new installations are found without rebuilding the installer"
    );
    fs::remove_file(&binary).unwrap();
    assert!(
        !app.native_detected("grok"),
        "removing the binary removes detection even while its profile remains"
    );
}

#[test]
fn dev_discovers_a_new_original_home_binary_without_an_initial_override() {
    let root = tempfile::tempdir().unwrap();
    let machine = root.path().join("person");
    let private = root.path().join("desktop.dev/mcp/clients");
    let env = |key: &str| {
        (key == "AGENT_RELAY_PROVIDER_DETECTION_HOME")
            .then(|| machine.to_string_lossy().into_owned())
    };
    let mut app = installer(&private).with_environment(&env);
    // Keep this fixture independent of tools installed on the test runner.
    app.native
        .executables
        .directories
        .retain(|path| path.starts_with(&machine));
    assert!(!app.native_detected("grok"));
    let executable =
        machine
            .join(".grok/bin")
            .join(if cfg!(windows) { "grok.exe" } else { "grok" });
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::write(&executable, "new fixture executable").unwrap();
    private_fs::set_mode(&executable, 0o700).unwrap();
    assert!(app.native_detected("grok"));
    assert_eq!(app.native.locations.grok, private.join(".grok/config.toml"));
    assert!(!machine.join(".grok/config.toml").exists());
}

#[test]
fn copilot_reports_detection_and_supports_scoped_provider_setup() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let bin = root.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let system = root.path().join("system");
    // Every install location is under the fixture: the fixed macOS and Linux
    // ones under the detection root, the Windows ones from the environment.
    let env = |key: &str| {
        match key {
            "PATH" => Some(bin.clone()),
            "USERPROFILE" => Some(home.clone()),
            "AGENT_RELAY_PROVIDER_DETECTION_ROOT" => Some(system.clone()),
            "LOCALAPPDATA" => Some(system.join("Local")),
            "ProgramFiles" => Some(system.join("Program Files")),
            _ => None,
        }
        .map(|path| path.display().to_string())
    };
    let app = installer(&home).with_environment(&env);
    let row = |app: &Installer| {
        app.report(Some(&binding("alice")), Some("ready".into()))
            .tools
            .into_iter()
            .find(|tool| tool.id == "copilot")
            .unwrap()
    };
    let missing = row(&app);
    assert_eq!(missing.name, "GitHub Copilot (VS Code)");
    assert!(!missing.detected);
    assert!(
        missing.detail.contains("VS Code: not detected. Checked "),
        "{}",
        missing.detail
    );
    let checked = missing
        .detail
        .split("Checked ")
        .nth(1)
        .unwrap()
        .split(", code")
        .next()
        .unwrap();
    assert!(
        checked
            .split(", ")
            .all(|place| Path::new(place).starts_with(root.path())),
        "{checked}"
    );
    assert!(missing
        .detail
        .contains("GitHub Copilot Chat extension: not detected. Checked "));

    fs::write(
        bin.join(if cfg!(windows) { "code.cmd" } else { "code" }),
        "fixture",
    )
    .unwrap();
    fs::create_dir_all(home.join(".vscode/extensions/github.copilot-chat-0.22.4")).unwrap();
    let found = row(&app);
    assert!(found.detected);
    assert!(copilot::COPILOT_CONNECTION_READY);
    assert_eq!(
        (found.state.as_str(), found.status_label.as_str()),
        ("onboarding_needed", "Choose providers")
    );
    assert!(found.supported && found.can_install && !found.can_repair && !found.can_uninstall);
    assert!(found.detail.starts_with(copilot::REQUIREMENTS));
    assert!(
        found.detail.contains("VS Code: detected at "),
        "{}",
        found.detail
    );
    assert!(found
        .detail
        .contains("GitHub Copilot Chat extension: detected at "));
    app.select(vec!["copilot".into()], Some(&binding("alice")), None)
        .unwrap();
    assert_eq!(app.provider_ids(), vec!["copilot"]);
    assert!(!app.native.locations.copilot.exists());
}
