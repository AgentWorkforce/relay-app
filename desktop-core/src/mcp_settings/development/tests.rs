use super::*;
use agent_relay_node::relay::{daemon::Config, local::LocalConfig, paths::CorePaths};

fn config(root: &Path) -> Config {
    Config {
        home: root.join("person"),
        app_data_dir: root.join("desktop.dev"),
        paths: CorePaths::for_socket(root.join("dev.sock")).unwrap(),
        env: BTreeMap::from([
            (
                "CODEX_HOME".into(),
                root.join("production-codex").to_string_lossy().into_owned(),
            ),
            (
                "CLAUDE_CONFIG_DIR".into(),
                root.join("production-claude")
                    .to_string_lossy()
                    .into_owned(),
            ),
            (
                "CLOUD_API_ACCESS_TOKEN".into(),
                "synthetic-production-token".into(),
            ),
        ]),
        version: "test".into(),
        host_name: "test".into(),
        rediscover_every: std::time::Duration::from_secs(3600),
        push_timing: Default::default(),
        delivery_timing: Default::default(),
    }
}

fn installer(config: &Config, source: &Path) -> Installer {
    let clients = Clients::new(config);
    let mut local = LocalConfig::from_env(&|key| clients.env.get(key).cloned(), &clients.home);
    local.claude.dirs =
        agent_relay_node::relay::claude_dirs::ClaudeDirs::development(&config.app_data_dir);
    Installer::new(clients.home, local.codex, local.claude, local.opencode).with_development(
        &config.app_data_dir,
        &config.paths.socket,
        source,
    )
}

fn binding() -> Binding {
    Binding {
        site_url: "http://localhost:3100".into(),
        workspace_id: "dev-workspace".into(),
        account_id: "dev-account".into(),
    }
}

#[test]
fn a_production_copilot_source_never_replaces_the_private_development_helper() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let source = root.path().join("source-probe");
    fs::write(&source, "synthetic dev executable").unwrap();
    let app = installer(&config, &source);
    let private = app.copilot_helper();
    let app = app.with_copilot_executable(Some(root.path().join("production-probe")));
    assert_eq!(app.copilot_helper(), private);
    assert!(private.starts_with(config.app_data_dir.join("mcp/bin")));
    assert!(!crate::probe_copy::destination(&config.home).exists());
}

#[test]
fn client_profiles_are_private_without_overriding_home_or_copying_credentials() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let clients = Clients::new(&config);
    assert!(clients.safe());
    assert!(clients.home.starts_with(&config.app_data_dir));
    assert!(!clients.env.contains_key("HOME"));
    assert!(!clients.env.contains_key("CLOUD_API_ACCESS_TOKEN"));
    for key in [
        "CLAUDE_CONFIG_DIR",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "GROK_HOME",
        "HERMES_HOME",
        "FACTORY_HOME_OVERRIDE",
        "OPENCLAW_STATE_DIR",
        "PI_CODING_AGENT_DIR",
    ] {
        assert!(
            Path::new(&clients.env[key]).starts_with(&clients.home),
            "{key}"
        );
    }
    assert_eq!(clients.env["CODEX_HOME"], clients.env["CODEX_SQLITE_HOME"]);
    assert!(Path::new(&clients.env["CODEX_HOME"]).starts_with(config.home.join(".agent-relay-dev")));
    assert!(!Path::new(&clients.env["CODEX_HOME"]).starts_with(config.home.join(".codex")));
    let actual = (clients.agent_environment(None))(
        "claude",
        BTreeMap::from([("HOME".into(), config.home.to_string_lossy().into_owned())]),
    );
    assert_eq!(actual["HOME"], config.home.to_string_lossy());
    assert_eq!(
        actual["CLAUDE_CONFIG_DIR"],
        clients.env["CLAUDE_CONFIG_DIR"]
    );
    assert!(!clients.home.exists(), "constructing profiles is read-only");
}

#[test]
fn dev_install_stages_only_its_helper_and_configs_and_pins_the_dev_socket() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let production = config.home.join(".codex/config.toml");
    fs::create_dir_all(production.parent().unwrap()).unwrap();
    fs::write(&production, "production-sentinel").unwrap();
    let source = root.path().join("source-probe");
    fs::write(&source, "synthetic dev executable").unwrap();
    let app = installer(&config, &source);
    assert!(app.refresh(Some(&binding())).is_empty());
    assert!(
        !app.helper.path.exists(),
        "no helper staging before an explicit choice"
    );
    app.select(
        vec!["codex".into(), "claude".into()],
        Some(&binding()),
        None,
    )
    .unwrap();
    assert!(app
        .install(&binding(), &mut |_, _| panic!(
            "fresh isolated profiles have no OAuth aliases"
        ))
        .is_empty());
    assert!(app
        .helper
        .path
        .starts_with(config.app_data_dir.join("mcp/bin")));
    assert_eq!(
        fs::read(&app.helper.path).unwrap(),
        fs::read(source).unwrap()
    );
    let text = fs::read_to_string(app.codex.home.join("config.toml")).unwrap();
    assert!(text.contains("--socket"));
    assert!(text.contains(config.paths.socket.to_str().unwrap()));
    assert!(text.contains("http://localhost:3100/cloud/api/v1/mcp/shared-sessions"));
    assert_eq!(
        fs::read_to_string(production).unwrap(),
        "production-sentinel"
    );
    assert!(!crate::probe_copy::destination(&config.home).exists());
    assert!(!app.codex.home.join("auth.json").exists());
    assert!(app.claude.home.join(".claude.json").is_file());
}

#[cfg(unix)]
#[test]
fn dev_scope_rejects_profile_symlinks_instead_of_writing_production() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let clients = Clients::new(&config);
    fs::create_dir_all(&clients.home).unwrap();
    fs::create_dir_all(&config.home).unwrap();
    std::os::unix::fs::symlink(&config.home, clients.home.join(".codex")).unwrap();
    let app = installer(&config, &root.path().join("unused-probe"));
    assert!(!clients.safe());
    assert!(app.select(vec!["codex".into()], None, None).is_err());
    assert!(!config.app_data_dir.join("tool-integrations.json").exists());
    assert!(!config.home.join("config.toml").exists());
}

#[test]
fn dev_legacy_uninstall_never_touches_normal_openclaw_or_devin_configs() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    let files = [
        config.home.join(".openclaw/openclaw.json"),
        config.home.join(".config/devin/mcp_config.json"),
    ];
    let sentinel = r#"{"mcpServers":{"agent-relay":{"url":"https://agentrelay.com/cloud/api/v1/mcp/shared-sessions"}}}"#;
    for file in &files {
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, sentinel).unwrap();
    }
    let app = installer(&config, &root.path().join("unused-source"));
    assert!(app.home.starts_with(&config.app_data_dir));
    for id in ["openclaw", "devin"] {
        app.manage(id, "uninstall", Some(&binding()), None).unwrap();
    }
    for file in files {
        assert_eq!(fs::read_to_string(file).unwrap(), sentinel);
    }
}

#[test]
fn codex_scope_is_short_stable_and_separate_for_each_dev_install() {
    let home = Path::new("/Users/tester");
    let first = CodexScope::new(
        home,
        Path::new("/Users/tester/Library/Application Support/com.agentrelay.desktop.dev"),
    );
    let repeated = CodexScope::new(
        home,
        Path::new("/Users/tester/Library/Application Support/com.agentrelay.desktop.dev"),
    );
    let another = CodexScope::new(
        home,
        Path::new("/Users/tester/Library/Application Support/another.dev"),
    );
    assert_eq!(first.profile, repeated.profile);
    assert_ne!(first.profile, another.profile);
    assert!(first.profile.starts_with(home.join(".agent-relay-dev")));
    #[cfg(unix)]
    assert!(
        first
            .profile
            .join("app-server-control/app-server-control.sock")
            .as_os_str()
            .len()
            < 104
    );
}

#[test]
fn short_codex_profile_is_owner_private_and_never_copies_normal_auth() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    fs::create_dir_all(config.home.join(".codex")).unwrap();
    let normal_auth = config.home.join(".codex/auth.json");
    fs::write(&normal_auth, "normal-auth-sentinel").unwrap();
    let source = root.path().join("probe");
    fs::write(&source, "dev executable").unwrap();
    let app = installer(&config, &source);
    app.select(vec!["codex".into()], Some(&binding()), None)
        .unwrap();
    assert!(app
        .install(&binding(), &mut |_, _| panic!("no OAuth aliases"))
        .is_empty());
    assert!(app.codex.home.join("config.toml").is_file());
    assert!(!app.codex.home.join("auth.json").exists());
    assert_eq!(
        fs::read_to_string(normal_auth).unwrap(),
        "normal-auth-sentinel"
    );
    assert_eq!(app.codex.home, app.codex.sqlite_home);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&app.codex.home).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}

#[cfg(unix)]
#[test]
fn short_codex_namespace_and_auth_symlinks_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    fs::create_dir_all(&config.home).unwrap();
    let outside = root.path().join("normal-profile");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, config.home.join(".agent-relay-dev")).unwrap();
    assert!(!Clients::new(&config).safe());
    let app = installer(&config, &root.path().join("unused"));
    assert!(app.select(vec!["codex".into()], None, None).is_err());
    assert!(!outside.join("config.toml").exists());
    fs::remove_file(config.home.join(".agent-relay-dev")).unwrap();
    let clients = Clients::new(&config);
    assert!(clients.codex.ensure());
    let auth = config.home.join("normal-auth");
    fs::write(&auth, "normal-auth-sentinel").unwrap();
    std::os::unix::fs::symlink(&auth, clients.codex.profile.join("auth.json")).unwrap();
    assert!(!clients.safe());
    assert!(!clients.codex.ensure());
    assert_eq!(fs::read_to_string(auth).unwrap(), "normal-auth-sentinel");
}

#[cfg(windows)]
#[test]
fn windows_dev_codex_profile_keeps_its_drive_and_owner_namespace() {
    let home = Path::new(r"C:\Users\tester");
    let app_data = Path::new(r"C:\Users\tester\AppData\Roaming\AgentRelay.dev");
    let scope = CodexScope::new(home, app_data);
    assert!(scope.profile.starts_with(home.join(".agent-relay-dev")));
    assert_eq!(
        CodexScope::from_profile(&scope.profile, app_data)
            .unwrap()
            .profile,
        scope.profile
    );
    assert!(CodexScope::from_profile(&home.join(".codex"), app_data).is_none());
}

#[test]
fn dev_claude_folder_and_cloud_choices_persist_without_installing_into_watched_normal_login() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    fs::create_dir_all(&config.app_data_dir).unwrap();
    fs::create_dir_all(&config.home).unwrap();
    let normal = config.home.join(".claude-work");
    fs::create_dir(&normal).unwrap();
    let sentinel = r#"{"mcpServers":{"agent-relay":{"url":"https://agentrelay.com/cloud/api/v1/mcp/shared-sessions"}}}"#;
    let normal_file = normal.join(".claude.json");
    fs::write(&normal_file, sentinel).unwrap();
    let choices =
        agent_relay_node::relay::claude_dirs::ClaudeDirs::development(&config.app_data_dir);
    choices
        .set_added(&[normal.to_string_lossy().into_owned()], &config.home)
        .unwrap();
    choices
        .set_shared(
            normal.to_str().unwrap(),
            false,
            &config.home.join(".claude"),
        )
        .unwrap();
    choices.set_cloud_sessions(true).unwrap();
    let source = root.path().join("source-probe");
    fs::write(&source, "synthetic dev executable").unwrap();
    let app = installer(&config, &source);
    assert!(app.claude.dirs.cloud_sessions());
    assert_eq!(app.claude.dirs.recorded().added, vec![normal.clone()]);
    assert!(
        !app.claude
            .dirs
            .watched(&app.claude.home)
            .iter()
            .find(|row| row.path == normal)
            .unwrap()
            .shared
    );
    app.claude
        .dirs
        .set_shared(normal.to_str().unwrap(), true, &app.claude.home)
        .unwrap();
    app.select(vec!["claude".into()], Some(&binding()), None)
        .unwrap();
    assert!(app
        .install(&binding(), &mut |_, _| panic!(
            "normal login must be excluded"
        ))
        .is_empty());
    app.manage("claude", "uninstall", Some(&binding()), None)
        .unwrap();
    assert_eq!(fs::read_to_string(normal_file).unwrap(), sentinel);
    assert_eq!(app.claude.dirs.recorded().added, vec![normal]);
}

#[cfg(unix)]
#[test]
fn dev_claude_settings_refuse_symlinked_state_and_do_not_follow_running_production_profiles() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    fs::create_dir_all(&config.app_data_dir).unwrap();
    fs::create_dir_all(&config.home).unwrap();
    let sentinel = config.home.join("normal-settings");
    fs::write(&sentinel, "normal-sentinel").unwrap();
    let dirs = agent_relay_node::relay::claude_dirs::ClaudeDirs::development(&config.app_data_dir);
    std::os::unix::fs::symlink(&sentinel, dirs.file.as_ref().unwrap()).unwrap();
    assert!(dirs.set_cloud_sessions(true).is_err());
    assert!(!dirs.record_running(std::slice::from_ref(&config.home)));
    assert_eq!(fs::read_to_string(sentinel).unwrap(), "normal-sentinel");
    assert!(!dirs.cloud_sessions());
}

#[test]
fn unvalidated_dev_helper_is_checking_but_failed_verification_needs_attention() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    fs::create_dir_all(&config.home).unwrap();
    let app = installer(&config, &root.path().join("missing-source"));
    app.select(vec!["codex".into()], Some(&binding()), Some("ready".into()))
        .unwrap();
    let report = app.report(Some(&binding()), Some("ready".into()));
    let codex = report.tools.iter().find(|tool| tool.id == "codex").unwrap();
    assert_eq!(codex.state, "checking");
    assert_eq!(codex.status_label, "Checking");
    assert!(!codex.detail.contains("missing"));
    assert!(!app.helper.ensure());
    let report = app.report(Some(&binding()), Some("ready".into()));
    let codex = report.tools.iter().find(|tool| tool.id == "codex").unwrap();
    assert_eq!(codex.state, "action_required");
    assert!(codex.detail.contains("helper is missing or cannot be run"));
}

#[test]
fn private_directories_reject_parent_components() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("desktop.dev");
    fs::create_dir_all(base.join("mcp/clients/.claude")).unwrap();
    fs::create_dir_all(root.path().join("person/.claude")).unwrap();
    assert!(safe_directory(&base, &base.join("mcp/clients/.claude")));
    let escaping = base
        .join("mcp/clients")
        .join("..")
        .join("..")
        .join("person/.claude");
    assert!(!safe_directory(&base, &escaping));
}

#[test]
fn unselected_leftover_entry_is_not_left_checking() {
    let root = tempfile::tempdir().unwrap();
    let config = config(root.path());
    fs::create_dir_all(&config.home).unwrap();
    let app = installer(&config, &root.path().join("missing-source"));
    let b = binding();
    app.select(Vec::new(), Some(&b), Some("ready".into()))
        .unwrap();
    let helper = app.helper.command(&b).unwrap();
    fs::create_dir_all(&app.codex.home).unwrap();
    fs::write(
        app.codex.home.join("config.toml"),
        super::super::codex::render("", &b.endpoint().unwrap(), &helper, &mut |_| true).unwrap(),
    )
    .unwrap();
    assert!(app.helper.pending());
    let report = app.report(Some(&b), Some("ready".into()));
    let codex = report.tools.iter().find(|tool| tool.id == "codex").unwrap();
    assert!(!codex.selected);
    assert_ne!(codex.state, "checking");
    assert!(codex.can_uninstall);
}

#[test]
fn dev_detects_original_machine_binaries_and_keeps_all_provider_writes_private() {
    let root = tempfile::tempdir().unwrap();
    let mut config = config(root.path());
    let ids = [
        "grok", "opencode", "devin", "muse", "pi", "openclaw", "hermes", "droid",
    ];
    let mut expected = BTreeMap::new();
    for id in ids {
        let file = config.home.join(".local/bin").join(if cfg!(windows) {
            format!("{id}.exe")
        } else {
            id.into()
        });
        config.env.insert(
            format!("RELAY_DESKTOP_{}", id.to_uppercase()),
            file.to_string_lossy().into_owned(),
        );
        expected.insert(id, file);
    }
    for id in ["grok", "opencode", "devin"] {
        fs::create_dir_all(expected[id].parent().unwrap()).unwrap();
        fs::write(&expected[id], "fixture executable").unwrap();
        private_fs::set_mode(&expected[id], 0o700).unwrap();
    }
    let clients = Clients::new(&config);
    let lookup = |key: &str| clients.env.get(key).cloned();
    let app = Installer::new(
        clients.home.clone(),
        CodexConfig::from_env(&lookup, &clients.home),
        ClaudeConfig::resolve(&lookup, &clients.home, &[]),
        OpenCodeConfig::from_env(&lookup, &clients.home),
    )
    .with_environment(&lookup);
    for id in ["grok", "opencode", "devin"] {
        assert!(app.native_detected(id));
        assert_eq!(
            PathBuf::from(&clients.env[&format!("RELAY_DESKTOP_{}", id.to_uppercase())]),
            expected[id]
        );
    }
    for id in ["muse", "pi", "openclaw", "hermes", "droid"] {
        assert!(!app.native_detected(id));
    }
    fs::create_dir_all(clients.home.join(".pi/agent")).unwrap();
    fs::create_dir_all(clients.home.join(".hermes")).unwrap();
    fs::create_dir_all(clients.home.join(".openclaw")).unwrap();
    fs::create_dir_all(clients.home.join(".factory")).unwrap();
    for id in ["pi", "hermes", "openclaw", "droid"] {
        assert!(!app.native_detected(id));
    }
    fs::write(&expected["muse"], "new fixture executable").unwrap();
    private_fs::set_mode(&expected["muse"], 0o700).unwrap();
    assert!(app.native_detected("muse"));
    let locations = &app.native.locations;
    for path in [
        &locations.grok,
        &locations.devin,
        &locations.muse,
        &locations.openclaw,
        &locations.hermes,
        &locations.droid,
        &locations.pi,
    ] {
        assert!(path.starts_with(&clients.home));
        assert!(!path.starts_with(&config.home));
    }
    assert!(!config.home.join(".grok").exists());
    assert_eq!(
        clients.env["AGENT_RELAY_PROVIDER_DETECTION_HOME"],
        config.home.to_string_lossy()
    );
}
