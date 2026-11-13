//! Exercise the real paired installer and the host's name-keyed import boundary.
//! The installed VS Code agent host persists root enablement as
//! `mcpServers#<name>` and overlays forwarded configurations on native ones.
use super::*;

fn ready(app: &Installer, binding: &Binding) {
    app.select(vec!["copilot".into()], Some(binding), None)
        .unwrap();
    app.set_provider_credentials(
        binding,
        BTreeMap::from([("copilot".into(), "namespace-fixture-grant".into())]),
    );
    assert!(app.refresh(Some(binding)).is_empty());
}
fn value(path: &Path) -> Value {
    json5::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}
fn old_http(app: &Installer, binding: &Binding) -> String {
    let path = &app.native.locations.copilot;
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let config = hosted(binding, "previous-fixture-grant");
    let old = json!({"type":"http","url":config.endpoint,"headers":{"Authorization":"Bearer previous-fixture-grant","X-Agent-Relay-Managed-By":"agent-relay-desktop"}});
    let text=format!("{{ // preserve comment\n\"servers\":{{\"agentrelay\":{old},\"foreign\":{{\"command\":\"keep\"}}}},\"inputs\":[]}}");
    fs::write(path, &text).unwrap();
    text
}

#[test]
fn repair_migrates_shipped_http_lineage_and_both_imported_configs_keep_native_dispatch() {
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = fixture(home.path()).unwrap();
    old_http(&app, &binding);
    ready(&app, &binding);
    let native = value(&app.native.locations.copilot_harness);
    let legacy = value(&app.native.locations.copilot);
    let user = native["mcpServers"].as_object().unwrap();
    let forwarded = legacy["servers"].as_object().unwrap();
    assert!(!forwarded.contains_key("agentrelay"));
    assert_eq!(forwarded["relayhistory"]["type"], "http");
    assert_eq!(
        forwarded["relayhistory"]["headers"]["Authorization"],
        "Bearer namespace-fixture-grant"
    );
    // This is the actual host overlay direction. A same-name HTTP server would
    // replace the native command here, even though both files looked valid.
    let mut effective = user.clone();
    effective.extend(forwarded.clone());
    assert_eq!(effective["agentrelay"], user["agentrelay"]);
    if cfg!(target_os = "macos") {
        assert_eq!(effective["agentrelay"]["type"], "stdio");
        assert_eq!(
            effective["agentrelay"]["command"],
            json!(app.copilot_helper())
        );
    }
    let native_name = user.keys().next().unwrap();
    let history_name = forwarded
        .iter()
        .find(|(_, entry)| entry["headers"]["X-Agent-Relay-Managed-By"] == "agent-relay-desktop")
        .unwrap()
        .0;
    let native_enablement = format!("mcpServers#{native_name}");
    let history_enablement = format!("mcpServers#{history_name}");
    let enablement = BTreeMap::from([
        (native_enablement.clone(), true),
        (history_enablement, false),
    ]);
    assert_eq!(enablement.len(), 2);
    assert!(enablement[&native_enablement]);
    assert_eq!(
        crate::copilot_attestation::qualified_tool("agentrelay-send_relay_message"),
        Some("send_relay_message")
    );
    assert_eq!(
        crate::copilot_attestation::qualified_tool("relayhistory-send_relay_message"),
        None
    );
    assert!(legacy["inputs"].is_array());
    assert_eq!(forwarded["foreign"]["command"], "keep");
    assert!(fs::read_to_string(&app.native.locations.copilot)
        .unwrap()
        .contains("preserve comment"));
    assert_eq!(app.provider_ids(), vec!["copilot".to_string()]);
    let report = app.report(Some(&binding), None);
    let row = report.tools.iter().find(|r| r.id == "copilot").unwrap();
    assert!(row.selected);
    assert_eq!(
        row.state,
        if cfg!(target_os = "macos") {
            "installed"
        } else {
            "configured"
        }
    );
    let before: Vec<_> = app
        .copilot_files()
        .iter()
        .map(|f| fs::read(&f.path).unwrap())
        .collect();
    assert!(app.refresh(Some(&binding)).is_empty());
    for (file, bytes) in app.copilot_files().iter().zip(before) {
        assert_eq!(fs::read(&file.path).unwrap(), bytes);
    }
    app.remove("copilot", Some(&binding)).unwrap();
    assert!(!app.copilot_files()[0].inspect(None).unwrap().present);
    let cleaned = value(&app.native.locations.copilot);
    assert_eq!(cleaned["servers"].as_object().unwrap().len(), 1);
    assert_eq!(cleaned["servers"]["foreign"]["command"], "keep");
}

#[test]
fn foreign_new_namespace_or_native_shadow_refuses_setup_without_mutation() {
    for name in ["relayhistory", "agentrelay"] {
        let home = tempfile::tempdir().unwrap();
        let (app, binding) = fixture(home.path()).unwrap();
        let path = &app.native.locations.copilot;
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Even a hand-written URL to the same service is not our owned entry.
        let original=json!({"servers":{name:{"type":"http","url":binding.endpoint().unwrap(),"headers":{"Authorization":"Bearer foreign"}}}}).to_string();
        fs::write(path, &original).unwrap();
        let result = app.write_copilot(&hosted(&binding, "must-not-leak"));
        assert!(result.is_err(), "{name}");
        assert_eq!(fs::read_to_string(path).unwrap(), original);
        assert!(!app.native.locations.copilot_harness.exists());
        if cfg!(target_os = "macos") {
            assert!(!app.copilot_hooks_ready());
        }
    }
}

#[test]
fn a_foreign_native_shadow_invalidates_an_already_configured_legacy_connection() {
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = fixture(home.path()).unwrap();
    ready(&app, &binding);
    let path = &app.native.locations.copilot;
    let mut current = value(path);
    current["servers"]["agentrelay"] = json!({"command":"custom"});
    fs::write(path, current.to_string()).unwrap();
    let original = fs::read(path).unwrap();
    let config = hosted(&binding, "namespace-fixture-grant");
    let inspected = app.copilot_files()[1].inspect(Some(&config)).unwrap();
    assert!(inspected.present);
    assert!(!inspected.configured);
    let errors = app.refresh(Some(&binding));
    assert!(errors
        .iter()
        .any(|error| error.contains("shadow native Copilot")));
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn migration_failure_restores_old_http_bytes_and_foreign_entries_and_never_old_name_as_configured()
{
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = fixture(home.path()).unwrap();
    let original = old_http(&app, &binding);
    let files = app.copilot_files();
    let next = hosted(&binding, "failed-grant");
    assert!(files[1].inspect(Some(&next)).unwrap().present);
    assert!(!files[1].inspect(Some(&next)).unwrap().configured);
    let error = app.write_copilot_with(&next, |file| {
        file.install(&next)?;
        if file.provider == Provider::Copilot {
            return Err("synthetic post-write failure".into());
        }
        Ok(())
    });
    assert!(error.is_err());
    assert_eq!(fs::read_to_string(&files[1].path).unwrap(), original);
    assert!(!files[0].inspect(None).unwrap().present);
    assert!(!files[1].inspect(Some(&next)).unwrap().configured);
}

#[test]
fn cleanup_preserves_foreign_same_endpoint_aliases_while_removing_owned_grants() {
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = fixture(home.path()).unwrap();
    ready(&app, &binding);
    let path = &app.native.locations.copilot;
    let mut current = value(path);
    let foreign = json!({"type":"http","url":binding.endpoint().unwrap(),"headers":{"Authorization":"Bearer foreign"}});
    current["servers"]["personal-history"] = foreign.clone();
    fs::write(path, current.to_string()).unwrap();
    app.remove("copilot", Some(&binding)).unwrap();
    assert_eq!(value(path)["servers"]["personal-history"], foreign);
    assert!(!fs::read_to_string(path)
        .unwrap()
        .contains("namespace-fixture-grant"));
}
