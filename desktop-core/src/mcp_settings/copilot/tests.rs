use super::*;
use std::collections::BTreeMap;
fn fixture(home: &Path) -> std::io::Result<(Installer, Binding)> {
    let command = crate::probe_copy::destination(home);
    fs::create_dir_all(
        command
            .parent()
            .ok_or_else(|| std::io::Error::other("fixture helper has no parent"))?,
    )?;
    fs::write(&command, "synthetic executable")?;
    private_fs::set_mode(&command, 0o700)?;
    let app = Installer::new(
        home.into(),
        CodexConfig::from_env(&|_| None, home),
        ClaudeConfig::resolve(&|_| None, home, &[]),
        OpenCodeConfig::from_env(&|_| None, home),
    )
    .with_copilot_executable(Some(command));
    app.set_copilot_attestation_source(std::sync::Arc::new(|| true));
    let binding = Binding {
        site_url: "https://agentrelay.com".into(),
        workspace_id: "workspace".into(),
        account_id: "alice".into(),
    };
    Ok((app, binding))
}

mod authority;
mod namespaces;
mod owned_helper;
#[test]
fn one_choice_installs_both_mcp_formats_hooks_and_revokes_them_together() {
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = fixture(home.path()).unwrap();
    for (path, text) in [
        (
            &app.native.locations.copilot_harness,
            "{ // keep modern\n\"mcpServers\":{\"other\":{\"command\":\"other\"}}}",
        ),
        (
            &app.native.locations.copilot,
            "{ // keep vscode\n\"servers\":{\"other\":{\"command\":\"other\"}},\"inputs\":[]}",
        ),
    ] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    app.select(vec!["copilot".into()], Some(&binding), None)
        .unwrap();
    app.set_provider_credentials(
        &binding,
        BTreeMap::from([("copilot".into(), "copilot-fixture-grant".into())]),
    );
    assert!(app.refresh(Some(&binding)).is_empty());
    let modern: Value =
        json5::from_str(&fs::read_to_string(&app.native.locations.copilot_harness).unwrap())
            .unwrap();
    let legacy: Value =
        json5::from_str(&fs::read_to_string(&app.native.locations.copilot).unwrap()).unwrap();
    assert_eq!(
        modern["mcpServers"][COPILOT_MCP_SERVER]["type"],
        if cfg!(target_os = "macos") {
            "stdio"
        } else {
            "http"
        }
    );
    assert_eq!(
        modern["mcpServers"][COPILOT_MCP_SERVER]["tools"],
        json!(["*"])
    );
    if cfg!(target_os = "macos") {
        assert!(modern["mcpServers"][COPILOT_MCP_SERVER]
            .get("headers")
            .is_none());
        assert_eq!(
            modern["mcpServers"][COPILOT_MCP_SERVER]["command"],
            json!(app.copilot_helper())
        );
        assert_eq!(
            modern["mcpServers"][COPILOT_MCP_SERVER]["args"][0],
            "copilot-mcp"
        );
        assert!(!modern["mcpServers"][COPILOT_MCP_SERVER]
            .to_string()
            .contains("copilot-fixture-grant"));
    } else {
        assert_eq!(
            modern["mcpServers"][COPILOT_MCP_SERVER]["headers"]["Authorization"],
            "Bearer copilot-fixture-grant"
        );
    }
    assert_eq!(
        legacy["servers"][COPILOT_HISTORY_MCP_SERVER]["type"],
        "http"
    );
    assert_eq!(
        legacy["servers"][COPILOT_HISTORY_MCP_SERVER]["headers"]["Authorization"],
        "Bearer copilot-fixture-grant"
    );
    assert!(legacy["inputs"].is_array());
    assert!(app.copilot_hooks_ready());
    if cfg!(target_os = "macos") {
        let path = app
            .copilot_config()
            .hooks_file()
            .with_file_name("agent-relay-desktop-mcp.json");
        let observer: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(observer["hooks"].as_object().unwrap().len(), 1);
        assert!(observer["hooks"]["preMcpToolCall"][0]["command"]
            .as_str()
            .unwrap()
            .contains("copilot-mcp-observe"));
        assert!(app.copilot_observer().enabled());
    }
    let report = app.report(Some(&binding), Some("ready".into()));
    let row = report.tools.iter().find(|row| row.id == "copilot").unwrap();
    assert!(row.supported && row.selected);
    assert_eq!(
        row.state,
        if cfg!(target_os = "macos") {
            "installed"
        } else {
            "configured"
        }
    );
    if !cfg!(target_os = "macos") {
        assert_eq!(row.status_label, "Connected");
        assert!(!row.detail.contains("availability refreshes automatically"));
        assert!(row.detail.contains("unavailable on this platform"));
        assert!(!app
            .native
            .locations
            .copilot_harness
            .parent()
            .unwrap()
            .join("hooks/agent-relay.json")
            .exists());
    }
    assert!(row.detail.contains(if cfg!(target_os = "macos") {
        "Start a new VS Code chat"
    } else {
        "Start a new chat"
    }));
    app.set_copilot_attestation_source(std::sync::Arc::new(|| false));
    let unavailable = app.report(Some(&binding), Some("ready".into()));
    assert_eq!(
        unavailable
            .tools
            .iter()
            .find(|row| row.id == "copilot")
            .unwrap()
            .name,
        "GitHub Copilot (VS Code)"
    );
    assert_eq!(
        unavailable
            .tools
            .iter()
            .find(|row| row.id == "copilot")
            .unwrap()
            .state,
        if cfg!(target_os = "macos") {
            "action_required"
        } else {
            "configured"
        }
    );
    app.set_copilot_attestation_source(std::sync::Arc::new(|| true));
    app.manage("copilot", "uninstall", Some(&binding), None)
        .unwrap();
    assert!(!app.copilot_settings().enabled());
    for file in app.copilot_files() {
        assert!(!file.inspect(None).unwrap().present);
        assert!(fs::read_to_string(file.path).unwrap().contains("keep"));
    }
}
#[test]
fn a_conflict_in_either_harness_prevents_partial_installation() {
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = fixture(home.path()).unwrap();
    let bad = &app.native.locations.copilot;
    fs::create_dir_all(bad.parent().unwrap()).unwrap();
    let original = r#"{"servers":{"agentrelay":{"command":"custom"}}}"#;
    fs::write(bad, original).unwrap();
    app.select(vec!["copilot".into()], Some(&binding), None)
        .unwrap();
    app.set_provider_credentials(
        &binding,
        BTreeMap::from([("copilot".into(), "fixture-grant".into())]),
    );
    assert!(!app.refresh(Some(&binding)).is_empty());
    assert!(!app.native.locations.copilot_harness.exists());
    assert!(!app.copilot_settings().enabled());
    assert_eq!(fs::read_to_string(bad).unwrap(), original);
}

fn hosted(binding: &Binding, credential: &str) -> HostedConfig {
    HostedConfig {
        endpoint: binding.endpoint().unwrap(),
        headers: BTreeMap::from([("Authorization".into(), format!("Bearer {credential}"))]),
        bearer_token_file: None,
        copilot_stdio: None,
    }
}
#[test]
fn an_error_after_the_second_write_restores_both_preexisting_owned_configurations() {
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = fixture(home.path()).unwrap();
    let previous = hosted(&binding, "previous-fixture-grant");
    let replacement = hosted(&binding, "replacement-fixture-grant");
    let files = app.copilot_files();
    let mut originals = Vec::new();
    for file in &files {
        fs::create_dir_all(file.path.parent().unwrap()).unwrap();
        fs::write(
            &file.path,
            if file.provider == Provider::Copilot {
                r#"{"servers":{"other":{"command":"keep"}},"setting":"keep"}"#
            } else {
                r#"{"mcpServers":{"other":{"command":"keep"}},"setting":"keep"}"#
            },
        )
        .unwrap();
        file.install(&previous).unwrap();
        originals.push(fs::read_to_string(&file.path).unwrap());
    }
    let result = app.write_copilot_with(&replacement, |file| {
        file.install(&replacement)?;
        if file.provider == Provider::Copilot {
            Err("synthetic post-write inspection failure".into())
        } else {
            Ok(())
        }
    });
    assert!(result.is_err());
    for (file, original) in files.iter().zip(originals) {
        assert_eq!(fs::read_to_string(&file.path).unwrap(), original);
        assert!(file.inspect(Some(&previous)).unwrap().configured);
        assert!(!fs::read_to_string(&file.path)
            .unwrap()
            .contains("replacement-fixture-grant"));
    }
    assert!(!app.copilot_settings().enabled());
}
#[test]
fn rollback_preserves_concurrent_foreign_servers_while_removing_the_failed_grant() {
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = fixture(home.path()).unwrap();
    let config = hosted(&binding, "failed-fixture-grant");
    let result = app.write_copilot_with(&config, |file| {
        file.install(&config)?;
        if file.provider == Provider::Copilot {
            let mut current: Value =
                json5::from_str(&fs::read_to_string(&file.path).unwrap()).unwrap();
            current["servers"]["foreign"] = json!({"command":"keep"});
            current["setting"] = json!("changed concurrently");
            fs::write(&file.path, current.to_string()).unwrap();
            Err("synthetic post-write failure".into())
        } else {
            Ok(())
        }
    });
    assert!(result.is_err());
    let restored: Value =
        json5::from_str(&fs::read_to_string(&app.native.locations.copilot).unwrap()).unwrap();
    assert_eq!(restored["servers"]["foreign"]["command"], "keep");
    assert_eq!(restored["setting"], "changed concurrently");
    for file in app.copilot_files() {
        assert!(!file.inspect(None).unwrap().present);
    }
}
#[test]
fn one_broken_document_does_not_skip_removing_its_siblings_credential() {
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = fixture(home.path()).unwrap();
    let config = hosted(&binding, "cleanup-fixture-grant");
    app.write_copilot(&config).unwrap();
    let broken = &app.native.locations.copilot_harness;
    fs::write(broken, "broken JSON").unwrap();
    assert!(app.remove("copilot", Some(&binding)).is_err());
    assert_eq!(fs::read_to_string(broken).unwrap(), "broken JSON");
    assert!(!app.copilot_files()[1].inspect(None).unwrap().present);
}
#[cfg(target_os = "macos")]
#[test]
fn a_foreign_hook_file_cannot_leave_mcp_credentials_behind_on_install_or_uninstall() {
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = fixture(home.path()).unwrap();
    let config = hosted(&binding, "hook-fixture-grant");
    let hooks = app
        .native
        .locations
        .copilot_harness
        .parent()
        .unwrap()
        .join("hooks/agent-relay.json");
    fs::create_dir_all(hooks.parent().unwrap()).unwrap();
    fs::write(&hooks, r#"{"foreign":"keep"}"#).unwrap();
    assert!(app.write_copilot(&config).is_err());
    for file in app.copilot_files() {
        assert!(!file.inspect(None).unwrap().present);
        file.install(&config).unwrap();
    }
    assert!(app.remove("copilot", Some(&binding)).is_err());
    for file in app.copilot_files() {
        assert!(!file.inspect(None).unwrap().present);
    }
    assert_eq!(fs::read_to_string(hooks).unwrap(), r#"{"foreign":"keep"}"#);
}

#[test]
fn cleanup_scope_escape_is_a_hard_stop_before_either_file_is_changed() {
    let home = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let (app, binding) = fixture(home.path()).unwrap();
    let app = app
        .with_environment(&|key| {
            (key == "COPILOT_HOME").then(|| outside.path().display().to_string())
        })
        .with_development(
            &home.path().join("app-data"),
            &home.path().join("dev.sock"),
            &home.path().join("source"),
        );
    let config = hosted(&binding, "scope-fixture-grant");
    let files = app.copilot_files();
    for file in &files {
        file.install(&config).unwrap();
    }
    let originals: Vec<_> = files
        .iter()
        .map(|f| fs::read_to_string(&f.path).unwrap())
        .collect();
    assert!(app
        .remove("copilot", Some(&binding))
        .unwrap_err()
        .contains("outside its private profile"));
    for (file, original) in files.iter().zip(originals) {
        assert_eq!(fs::read_to_string(&file.path).unwrap(), original);
    }
}
#[test]
fn rollback_does_not_touch_a_concurrent_install_in_an_unattempted_file() {
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = fixture(home.path()).unwrap();
    let config = hosted(&binding, "concurrent-fixture-grant");
    let files = app.copilot_files();
    let result = app.write_copilot_with(&config, |file| {
        assert_eq!(file.provider, Provider::CopilotHarness);
        file.install(&config)?;
        // Another setup finishes the second file after validation, before this
        // attempt has reached it. This attempt must never roll that file back.
        files[1].install(&config)?;
        Err("synthetic first-file post-write failure".into())
    });
    assert!(result.is_err());
    assert!(!files[0].inspect(None).unwrap().present);
    assert!(files[1].inspect(Some(&config)).unwrap().configured);
}

#[test]
fn failed_second_write_restores_both_owned_legacy_namespace_entries() {
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = fixture(home.path()).unwrap();
    let old = hosted(&binding, "previous-fixture-grant");
    let next = hosted(&binding, "replacement-fixture-grant");
    let files = app.copilot_files();
    let mut originals = Vec::new();
    for (index, file) in files.iter().enumerate() {
        file.install(&old).unwrap();
        let original = fs::read_to_string(&file.path).unwrap().replace(
            if file.provider == Provider::Copilot {
                COPILOT_HISTORY_MCP_SERVER
            } else {
                COPILOT_MCP_SERVER
            },
            NAME,
        );
        fs::write(&file.path, &original).unwrap();
        originals.push(original);
        assert!(
            !file.inspect(Some(&old)).unwrap().configured,
            "legacy {index}"
        );
    }
    assert!(app
        .write_copilot_with(&next, |file| {
            file.install(&next)?;
            if file.path == files[1].path {
                Err("synthetic post-write failure".into())
            } else {
                Ok(())
            }
        })
        .is_err());
    for (file, original) in files.iter().zip(originals) {
        assert_eq!(fs::read_to_string(&file.path).unwrap(), original);
        assert!(!file.inspect(Some(&next)).unwrap().configured);
    }
}

#[cfg(target_os = "macos")]
mod hook_transaction;
