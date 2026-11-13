use super::*;

#[cfg(target_os = "macos")]
#[test]
fn observer_failure_rolls_back_new_mcp_and_public_hooks_without_touching_foreign_file() {
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = fixture(home.path()).unwrap();
    let config = hosted(&binding, "failed-synthetic-grant");
    let hooks = app.copilot_config().hooks_file();
    let observer = hooks.with_file_name("agent-relay-desktop-mcp.json");
    let foreign = "{\"personal\":\"concurrently-created\"}";
    let result = app.write_copilot_with_hooks(
        &config,
        |file| file.install(&config),
        || {
            app.set_copilot_hooks(true)?;
            fs::write(&observer, foreign).unwrap();
            Ok(())
        },
    );
    assert!(result.is_err());
    assert!(!hooks.exists());
    assert_eq!(fs::read_to_string(observer).unwrap(), foreign);
    for file in app.copilot_files() {
        assert!(!file.inspect(None).unwrap().present);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn a_failed_mcp_repair_preserves_previously_enabled_hooks_byte_for_byte() {
    for failed in [Provider::CopilotHarness, Provider::Copilot] {
        let home = tempfile::tempdir().unwrap();
        let (app, binding) = fixture(home.path()).unwrap();
        let old = hosted(&binding, "previous-fixture-grant");
        let next = hosted(&binding, "replacement-fixture-grant");
        app.write_copilot(&old).unwrap();
        let hooks = app.copilot_config().hooks_file();
        let original = format!("{}\n", fs::read_to_string(&hooks).unwrap());
        fs::write(&hooks, &original).unwrap();
        let unrelated = hooks.parent().unwrap().join("personal.json");
        fs::write(&unrelated, "personal hooks stay unchanged").unwrap();
        assert!(app.copilot_settings().enabled());
        assert!(app
            .write_copilot_with_hooks(
                &next,
                |file| {
                    file.install(&next)?;
                    if file.provider == failed {
                        Err("synthetic MCP post-write failure".into())
                    } else {
                        Ok(())
                    }
                },
                || panic!("MCP failure must never reach hook setup")
            )
            .is_err());
        assert_eq!(fs::read_to_string(&hooks).unwrap(), original);
        assert!(app.copilot_settings().enabled());
        assert_eq!(
            fs::read_to_string(&unrelated).unwrap(),
            "personal hooks stay unchanged"
        );
        for file in app.copilot_files() {
            assert!(file.inspect(Some(&old)).unwrap().configured);
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn a_postrename_hook_failure_restores_the_exact_previous_legacy_configuration() {
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = fixture(home.path()).unwrap();
    let old = hosted(&binding, "previous-fixture-grant");
    let next = hosted(&binding, "replacement-fixture-grant");
    app.write_copilot(&old).unwrap();
    let hooks = app.copilot_config().hooks_file();
    let mut legacy: Value = serde_json::from_str(&fs::read_to_string(&hooks).unwrap()).unwrap();
    let previous_helper = home.path().join("previous-agent-relay-probe");
    for entries in legacy["hooks"].as_object_mut().unwrap().values_mut() {
        let command = entries[0]["command"].as_str().unwrap().replace(
            app.helper.copilot_executable().to_str().unwrap(),
            previous_helper.to_str().unwrap(),
        );
        entries[0]["command"] = json!(command);
    }
    legacy["hooks"]["sessionStart"][0]["timeoutSec"] = json!(10);
    let original = format!("{}\n", serde_json::to_string_pretty(&legacy).unwrap());
    fs::write(&hooks, &original).unwrap();
    let mutated = std::cell::Cell::new(false);
    assert!(app
        .write_copilot_with_hooks(
            &next,
            |file| file.install(&next),
            || {
                app.set_copilot_hooks(true)?;
                assert!(app.copilot_settings().enabled());
                assert_ne!(fs::read_to_string(&hooks).unwrap(), original);
                mutated.set(true);
                Err("synthetic directory sync failure after hook rename".into())
            }
        )
        .is_err());
    assert!(
        mutated.get(),
        "fault must occur after the core wrote replacement hooks"
    );
    assert_eq!(fs::read_to_string(&hooks).unwrap(), original);
    for file in app.copilot_files() {
        assert!(file.inspect(Some(&old)).unwrap().configured);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn a_postrename_hook_failure_removes_only_a_newly_installed_owned_hook_file() {
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = fixture(home.path()).unwrap();
    let config = hosted(&binding, "failed-fixture-grant");
    let hooks = app.copilot_config().hooks_file();
    fs::create_dir_all(hooks.parent().unwrap()).unwrap();
    let unrelated = hooks.parent().unwrap().join("personal.json");
    fs::write(&unrelated, "personal hooks stay unchanged").unwrap();
    assert!(!hooks.exists());
    let mutated = std::cell::Cell::new(false);
    assert!(app
        .write_copilot_with_hooks(
            &config,
            |file| file.install(&config),
            || {
                app.set_copilot_hooks(true)?;
                assert!(app.copilot_settings().enabled());
                mutated.set(true);
                Err("synthetic directory sync failure after hook rename".into())
            }
        )
        .is_err());
    assert!(
        mutated.get(),
        "fault must occur after the core created the hook file"
    );
    assert!(!hooks.exists());
    assert_eq!(
        fs::read_to_string(&unrelated).unwrap(),
        "personal hooks stay unchanged"
    );
    for file in app.copilot_files() {
        assert!(!file.inspect(None).unwrap().present);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn hook_rollback_preserves_concurrent_foreign_additions_and_reports_the_conflict() {
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = fixture(home.path()).unwrap();
    let config = hosted(&binding, "failed-fixture-grant");
    let hooks = app.copilot_config().hooks_file();
    let changed = std::cell::RefCell::new(String::new());
    let error = app
        .write_copilot_with_hooks(
            &config,
            |file| file.install(&config),
            || {
                app.set_copilot_hooks(true)?;
                let mut current: Value =
                    serde_json::from_str(&fs::read_to_string(&hooks).unwrap()).unwrap();
                current["personalSetting"] = json!("concurrent foreign change");
                *changed.borrow_mut() = current.to_string();
                fs::write(&hooks, changed.borrow().as_bytes()).unwrap();
                Err("synthetic post-write failure".into())
            },
        )
        .unwrap_err();
    assert!(error.contains("rollback left them unchanged"));
    assert_eq!(fs::read_to_string(&hooks).unwrap(), *changed.borrow());
    assert!(
        !app.copilot_settings().enabled(),
        "core ownership must reject extra fields"
    );
    for file in app.copilot_files() {
        assert!(!file.inspect(None).unwrap().present);
    }
}
