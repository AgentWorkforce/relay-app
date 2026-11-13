use super::*;
use crate::teammate_context::SETTING;
use agent_relay_node::relay::agents::SettingsExtension;
use agent_relay_node::relay::daemon::SessionStartContext;
use agent_relay_node::relay::store::SessionKind;

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

/// A Claude login holding the person's own hooks and relay-node's
/// SessionStart hook, chosen in the provider chooser.
fn claude_chosen(root: &Path) -> (std::sync::Arc<Installer>, PathBuf, Value) {
    let app = std::sync::Arc::new(installer(root));
    executable_helper(root);
    fs::create_dir_all(&app.claude.home).unwrap();
    let settings = app.claude.home.join("settings.json");
    let others = json!({
        "hooks": {
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "audit-bash"}]}],
            "SessionStart": [{"hooks": [{"type": "command", "command": "probe claude-session-hook"}]}],
        },
    });
    fs::write(&settings, others.to_string()).unwrap();
    agent_relay_node::platform::fs::set_mode(&settings, 0o600).unwrap();
    app.select(vec!["claude".into()], Some(&binding()), None)
        .unwrap();
    assert!(app.refresh(Some(&binding())).is_empty());
    (app, settings, others)
}

fn read(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn off_by_default_even_once_claude_is_chosen_until_the_person_turns_it_on() {
    let root = tempfile::tempdir().unwrap();
    let (app, settings, others) = claude_chosen(root.path());
    let setting = crate::teammate_context::Setting::new(app.clone());
    // Write-only: settings.get never carries it.
    assert!(setting.get().is_empty());
    assert_eq!(setting.write_only(), &[SETTING]);
    // No choice yet: nothing written, and no default pass turns it on.
    assert_eq!(app.report(None, None).teammate_context, Some(false));
    assert!(setting.default_on().is_empty());
    assert_eq!(
        read(&settings)["hooks"]["SessionStart"],
        others["hooks"]["SessionStart"]
    );
    assert_eq!(setting.set(SETTING, &Value::Bool(true)), Some(Ok(())));
    assert_eq!(app.report(None, None).teammate_context, Some(true));
    assert!(app.teammate_hook_present());
}

#[test]
fn an_off_chosen_before_the_default_pass_stays_off() {
    let root = tempfile::tempdir().unwrap();
    let (app, settings, others) = claude_chosen(root.path());
    let setting = crate::teammate_context::Setting::new(app.clone());
    app.set_teammate_context(false).unwrap();
    assert_eq!(setting.set(SETTING, &Value::Bool(true)), Some(Ok(())));
    assert_eq!(app.report(None, None).teammate_context, Some(false));
    assert_eq!(
        read(&settings)["hooks"]["SessionStart"],
        others["hooks"]["SessionStart"]
    );
    // An agent may turn it off through the write-only key.
    app.set_teammate_context(true).unwrap();
    assert_eq!(setting.set(SETTING, &Value::Bool(false)), Some(Ok(())));
    assert!(!app.teammate_hook_present());
}

#[test]
fn on_adds_one_session_start_hook_beside_the_others_and_off_takes_only_it_out() {
    let root = tempfile::tempdir().unwrap();
    let (app, settings, others) = claude_chosen(root.path());
    let before = read(&settings);
    app.set_teammate_context(true).unwrap();
    let command = app.teammate_hook_command().unwrap();
    assert!(command.ends_with(" teammate-context-hook"), "{command}");
    let on = read(&settings);
    let starts = on["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(starts.len(), 2, "{on}");
    assert_eq!(starts[0], others["hooks"]["SessionStart"][0]);
    assert_eq!(starts[1]["hooks"][0]["command"], command);
    assert!(starts[1].get("matcher").is_none(), "{on}");
    // The identity hook and the person's own PreToolUse hook are untouched.
    assert_eq!(on["hooks"]["PreToolUse"], before["hooks"]["PreToolUse"]);

    // Again, and on refresh: unchanged.
    app.set_teammate_context(true).unwrap();
    assert!(app.refresh(Some(&binding())).is_empty());
    assert_eq!(read(&settings), on);

    app.set_teammate_context(false).unwrap();
    assert_eq!(read(&settings), before);
    // A refresh while off does not put it back.
    assert!(app.refresh(Some(&binding())).is_empty());
    app.reclaim_teammate_context();
    assert_eq!(read(&settings), before);
}

#[test]
fn a_lost_hook_comes_back_on_reclaim_and_uninstalling_claude_removes_it() {
    let root = tempfile::tempdir().unwrap();
    let (app, settings, others) = claude_chosen(root.path());
    app.set_teammate_context(true).unwrap();
    let on = read(&settings);
    fs::write(&settings, others.to_string()).unwrap();
    app.reclaim_teammate_context();
    assert_eq!(
        read(&settings)["hooks"]["SessionStart"],
        on["hooks"]["SessionStart"]
    );

    app.manage("claude", "uninstall", Some(&binding()), None)
        .unwrap();
    assert_eq!(read(&settings), others);
    // Claude no longer chosen: a reclaim leaves the file alone.
    app.reclaim_teammate_context();
    assert_eq!(read(&settings), others);
}

#[test]
fn the_setting_refuses_anything_but_on_or_off() {
    let root = tempfile::tempdir().unwrap();
    let setting =
        crate::teammate_context::Setting::new(std::sync::Arc::new(installer(root.path())));
    assert!(setting.refuse(SETTING, &json!("yes")).is_some());
    assert!(setting.refuse(SETTING, &Value::Bool(false)).is_none());
    assert!(setting.refuse("telemetry", &json!("yes")).is_none());
    assert!(matches!(setting.set(SETTING, &json!(1)), Some(Err(_))));
    assert!(setting.set("telemetry", &Value::Bool(true)).is_none());
    // Never on by default. (Whether the setting shows without a chosen
    // provider depends on what this computer has installed, which
    // detection reads from PATH and the editors, so it is not asserted.)
    assert!(setting.default_on().is_empty());
}

#[test]
fn a_chooser_save_changes_no_file_and_the_selection_refresh_applies_it() {
    let root = tempfile::tempdir().unwrap();
    let (app, settings, others) = claude_chosen(root.path());
    app.save_teammate_context(true).unwrap();
    assert_eq!(
        read(&settings)["hooks"]["SessionStart"],
        others["hooks"]["SessionStart"]
    );
    app.select(vec!["claude".into()], Some(&binding()), None)
        .unwrap();
    assert!(app.refresh(Some(&binding())).is_empty());
    assert!(app.teammate_hook_present());
    app.save_teammate_context(false).unwrap();
    assert!(app.teammate_hook_present());
    assert!(app.refresh(Some(&binding())).is_empty());
    assert!(!app.teammate_hook_present());
}

#[test]
fn the_core_tells_every_other_provider_while_claude_keeps_its_own_hook() {
    let root = tempfile::tempdir().unwrap();
    let (app, _settings, _others) = claude_chosen(root.path());
    let context = crate::teammate_context::Context::new(app.clone());
    let others = [SessionKind::Codex, SessionKind::Copilot, SessionKind::Devin];
    // Off by default: nobody is told.
    for kind in others.iter().copied().chain([SessionKind::ClaudeTerminal]) {
        assert_eq!(context.context(kind), None, "{kind:?}");
    }
    app.set_teammate_context(true).unwrap();
    for kind in others {
        assert_eq!(
            context.context(kind).as_deref(),
            Some(crate::teammate_context::GUIDANCE),
            "{kind:?}"
        );
    }
    // Claude Code gets it from its own hook, never twice through the core,
    // so it still arrives with the core's claude_session_hook off.
    assert_eq!(context.context(SessionKind::ClaudeTerminal), None);
    assert!(app.teammate_hook_present());
    app.set_teammate_context(false).unwrap();
    assert_eq!(context.context(SessionKind::Codex), None);
}

#[test]
fn any_reached_provider_offers_the_setting() {
    for id in crate::teammate_context::PROVIDERS {
        let root = tempfile::tempdir().unwrap();
        let app = installer(root.path());
        executable_helper(root.path());
        let _ = app.select(vec![id.to_string()], Some(&binding()), None);
        assert_eq!(app.report(None, None).teammate_context, Some(false), "{id}");
    }
}
