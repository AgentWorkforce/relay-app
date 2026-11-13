use super::*;

#[test]
fn production_copilot_requires_its_explicit_absolute_executable() {
    for executable in [None, Some(PathBuf::from("relative-probe"))] {
        let home = tempfile::tempdir().unwrap();
        let (app, _) = fixture(home.path()).unwrap();
        let app = app.with_copilot_executable(executable);
        assert!(app.copilot_helper().as_os_str().is_empty());
        assert!(!app.helper.copilot_usable());
        #[cfg(target_os = "macos")]
        {
            assert!(app.set_copilot_hooks(true).is_err());
            assert!(!app.copilot_config().hooks_file().exists());
        }
    }
    let home = tempfile::tempdir().unwrap();
    let (app, _) = fixture(home.path()).unwrap();
    let app = app.with_copilot_executable(Some(home.path().join("missing-current-helper")));
    assert!(!app.helper.copilot_usable());
}

#[cfg(target_os = "macos")]
#[test]
fn repair_uses_the_current_core_and_preserves_a_stale_global_cli_and_foreign_hooks() {
    for global_exists in [true, false] {
        let home = tempfile::tempdir().unwrap();
        let (app, binding) = fixture(home.path()).unwrap();
        let global = crate::probe_copy::destination(home.path());
        let stale = b"#!/bin/sh\nexit 2\n";
        if global_exists {
            fs::write(&global, stale).unwrap();
        } else {
            fs::remove_file(&global).unwrap();
        }
        CopilotSettings::new(&app.copilot_config(), global.clone())
            .set_enabled(true)
            .unwrap();
        let hook = app.copilot_config().hooks_file();
        let foreign = hook.parent().unwrap().join("orca.json");
        let foreign_bytes = b"{\"version\":1,\"hooks\":{\"preToolUse\":[]}}\n";
        fs::write(&foreign, foreign_bytes).unwrap();

        let current = home.path().join("Current App/Agent Relay Helper");
        fs::create_dir_all(current.parent().unwrap()).unwrap();
        fs::write(&current, "current helper fixture").unwrap();
        private_fs::set_mode(&current, 0o700).unwrap();
        let app = app.with_copilot_executable(Some(current.clone()));
        app.set_provider_credentials(
            &binding,
            BTreeMap::from([("copilot".into(), "copilot-fixture-grant".into())]),
        );
        let report = app
            .manage("copilot", "repair", Some(&binding), Some("ready".into()))
            .unwrap();
        let row = report.tools.iter().find(|row| row.id == "copilot").unwrap();
        assert!(row.selected);
        assert_eq!(row.state, "installed");

        assert_eq!(app.copilot_helper(), current);
        assert!(app.copilot_hooks_ready());
        let value: Value = serde_json::from_slice(&fs::read(&hook).unwrap()).unwrap();
        assert_eq!(value["hooks"].as_object().unwrap().len(), 4);
        for entries in value["hooks"].as_object().unwrap().values() {
            let command = entries[0]["command"].as_str().unwrap();
            assert!(command.contains(current.to_str().unwrap()));
            assert!(!command.contains(global.to_str().unwrap()));
        }
        assert_eq!(fs::read(&foreign).unwrap(), foreign_bytes);
        assert_eq!(
            fs::read(&global).ok(),
            global_exists.then(|| stale.to_vec())
        );
    }
}
