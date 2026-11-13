use super::*;
use agent_relay_node::relay::agents::SettingsExtension;

fn installer(home: &Path) -> Installer {
    Installer::new(
        home.into(),
        CodexConfig::from_env(&|_| None, home),
        ClaudeConfig::resolve(&|_| None, home, &[]),
        OpenCodeConfig::from_env(&|_| None, home),
    )
}

fn executable_helper(home: &Path) {
    let path = crate::probe_copy::destination(home);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "synthetic executable").unwrap();
    agent_relay_node::platform::fs::set_mode(&path, 0o700).unwrap();
}

fn binding() -> Binding {
    Binding {
        site_url: "https://agentrelay.com".into(),
        workspace_id: "workspace".into(),
        account_id: "account".into(),
    }
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// The person's own hooks and relay-node's SessionStart hook.
fn others() -> Value {
    json!({
        "hooks": {
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "audit-bash"}]}],
            "SessionStart": [{"hooks": [{"type": "command", "command": "probe claude-session-hook"}]}],
        },
    })
}

/// A Claude login as v2026.11.6 left it with the setting on: the person's
/// hooks, this build's teammate-context hook, and the saved choice.
fn legacy_login(app: &Installer) -> PathBuf {
    fs::create_dir_all(&app.claude.home).unwrap();
    let settings = app.claude.home.join("settings.json");
    let mut root = others();
    root["hooks"]["SessionStart"]
        .as_array_mut()
        .unwrap()
        .push(json!({"hooks": [{
            "type": "command",
            "command": "'/Apps/Agent Relay.app/probe' teammate-context-hook",
            "timeout": 5,
        }]}));
    fs::write(&settings, root.to_string()).unwrap();
    agent_relay_node::platform::fs::set_mode(&settings, 0o600).unwrap();
    let choice = app.choices.with_file_name("teammate-context-hook.json");
    fs::create_dir_all(choice.parent().unwrap()).unwrap();
    fs::write(
        choice,
        r#"{"on":true}"#,
    )
    .unwrap();
    assert!(app.teammate_hook_present());
    assert!(app.teammate_choice_saved());
    settings
}

fn assert_cleaned(app: &Installer, settings: &Path) {
    assert!(!app.teammate_hook_present());
    assert!(!app.teammate_choice_saved());
    let root = read(settings);
    assert_eq!(
        root["hooks"]["SessionStart"],
        others()["hooks"]["SessionStart"]
    );
    assert_eq!(root["hooks"]["PreToolUse"], others()["hooks"]["PreToolUse"]);
}

#[test]
fn a_signed_out_refresh_removes_the_retired_hook_and_its_choice() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    let settings = legacy_login(&app);
    assert!(app.refresh(None).is_empty());
    assert_cleaned(&app, &settings);
}

#[test]
fn a_refresh_with_no_saved_selection_or_no_helper_still_removes_it() {
    let root = tempfile::tempdir().unwrap();
    // Signed in, but nothing chosen yet and no helper installed: `install`
    // returns before touching any provider.
    let app = installer(root.path());
    let settings = legacy_login(&app);
    assert_eq!(app.selection(), Ok(None));
    let _ = app.refresh(Some(&binding()));
    assert_cleaned(&app, &settings);
}

#[test]
fn a_refresh_with_claude_chosen_removes_it_and_never_puts_it_back() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    executable_helper(root.path());
    let settings = legacy_login(&app);
    app.select(vec!["claude".into()], Some(&binding()), None)
        .unwrap();
    assert!(app.refresh(Some(&binding())).is_empty());
    assert!(!app.teammate_hook_present());
    assert!(!app.teammate_choice_saved());
    // The identity hook went in; the person's own and relay-node's stay.
    let installed = read(&settings);
    assert_eq!(
        installed["hooks"]["SessionStart"],
        others()["hooks"]["SessionStart"]
    );
    assert!(app.refresh(Some(&binding())).is_empty());
    assert_eq!(read(&settings), installed);
}

#[test]
fn the_reclaim_pass_removes_it_and_has_no_setting_of_its_own() {
    let root = tempfile::tempdir().unwrap();
    let app = std::sync::Arc::new(installer(root.path()));
    let settings = legacy_login(&app);
    let cleanup = crate::teammate_context::LegacyCleanup::new(app.clone());
    assert!(cleanup.get().is_empty());
    assert!(cleanup.write_only().is_empty());
    assert!(cleanup.default_on().is_empty());
    assert!(cleanup
        .set("teammate_context_hook", &Value::Bool(true))
        .is_none());
    cleanup.reclaim();
    assert_cleaned(&app, &settings);
    // Again: nothing to change, nothing written.
    let before = fs::read(&settings).unwrap();
    cleanup.reclaim();
    assert_eq!(fs::read(&settings).unwrap(), before);
}

#[test]
fn uninstalling_claude_also_removes_it() {
    let root = tempfile::tempdir().unwrap();
    let app = installer(root.path());
    executable_helper(root.path());
    app.select(vec!["claude".into()], Some(&binding()), None)
        .unwrap();
    let settings = legacy_login(&app);
    app.manage("claude", "uninstall", Some(&binding()), None)
        .unwrap();
    assert!(!app.teammate_hook_present());
    assert_eq!(read(&settings), others());
}
