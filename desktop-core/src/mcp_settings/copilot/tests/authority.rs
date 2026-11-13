//! Pending Copilot calls survive refreshes that change nothing and are
//! revoked by every change to the authority they were issued under.
use super::*;

fn installed(home: &Path) -> (Installer, Binding) {
    let (app, binding) = fixture(home).unwrap();
    app.select(vec!["copilot".into()], Some(&binding), None)
        .unwrap();
    app.set_provider_credentials(
        &binding,
        BTreeMap::from([("copilot".into(), "authority-fixture-grant".into())]),
    );
    assert!(app.refresh(Some(&binding)).is_empty());
    (app, binding)
}

fn managed_bytes(app: &Installer) -> Vec<Option<Vec<u8>>> {
    app.copilot_snapshot().unwrap()
}

#[test]
fn unchanged_refresh_and_native_install_keep_the_authority_epoch() {
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = installed(home.path());
    let epoch = app.copilot_epoch();
    let bytes = managed_bytes(&app);
    for _ in 0..3 {
        assert!(app.refresh(Some(&binding)).is_empty());
    }
    assert!(app.install_native(&binding, &["copilot".into()]).is_empty());
    assert_eq!(app.copilot_epoch(), epoch);
    assert_eq!(managed_bytes(&app), bytes);
}

#[test]
fn every_authoritative_change_revokes_pending_calls() {
    let mut cases = vec![
        "binding",
        "credential",
        "file",
        "deleted",
        "selection",
        "uninstall",
        "signout",
    ];
    if cfg!(target_os = "macos") {
        cases.extend(["hooks", "observer"]);
    }
    for change in cases {
        let home = tempfile::tempdir().unwrap();
        let (app, binding) = installed(home.path());
        let epoch = app.copilot_epoch();
        match change {
            "binding" => {
                let other = Binding {
                    account_id: "bob".into(),
                    ..binding.clone()
                };
                app.refresh(Some(&other));
            }
            "credential" => app.set_provider_credentials(
                &binding,
                BTreeMap::from([("copilot".into(), "rotated-fixture-grant".into())]),
            ),
            "file" => {
                fs::write(&app.native.locations.copilot_harness, "{\"mcpServers\":{}}").unwrap();
                assert!(app.refresh(Some(&binding)).is_empty());
            }
            "deleted" => {
                fs::remove_file(&app.native.locations.copilot).unwrap();
                assert!(app.refresh(Some(&binding)).is_empty());
            }
            "selection" => {
                app.select(
                    vec!["copilot".into(), "cursor".into()],
                    Some(&binding),
                    None,
                )
                .unwrap();
                app.refresh(Some(&binding));
            }
            "uninstall" => {
                app.manage("copilot", "uninstall", Some(&binding), None)
                    .unwrap();
            }
            "signout" => {
                app.refresh(None);
            }
            "hooks" => {
                fs::remove_file(app.copilot_config().hooks_file()).unwrap();
                assert!(app.refresh(Some(&binding)).is_empty());
            }
            _ => {
                fs::remove_file(app.copilot_observer_path()).unwrap();
                assert!(app.refresh(Some(&binding)).is_empty());
            }
        }
        assert_ne!(app.copilot_epoch(), epoch, "{change}");
    }
}

#[test]
fn a_failed_write_revokes_even_when_rollback_restores_every_byte() {
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = installed(home.path());
    let epoch = app.copilot_epoch();
    let bytes = managed_bytes(&app);
    let config = hosted(&binding, "replacement-fixture-grant");
    assert!(app
        .write_copilot_with(&config, |_| Err("synthetic write failure".into()))
        .is_err());
    assert_eq!(managed_bytes(&app), bytes);
    assert_ne!(app.copilot_epoch(), epoch);
}

#[test]
fn a_rewrite_that_keeps_the_managed_entry_still_revokes_pending_calls() {
    let home = tempfile::tempdir().unwrap();
    let (app, binding) = installed(home.path());
    let config = hosted(&binding, "authority-fixture-grant");
    let path = app.native.locations.copilot.clone();
    let bytes = managed_bytes(&app);
    let epoch = app.copilot_epoch();
    // The writer reformats the file but leaves the managed entry intact, so
    // only the post-write byte comparison can see the change.
    app.write_copilot_with(&config, |file| {
        file.install(&config)?;
        let mut reformatted = fs::read(&path).unwrap();
        reformatted.extend_from_slice(b"\n\n");
        fs::write(&path, reformatted).unwrap();
        Ok(())
    })
    .unwrap();
    assert_ne!(managed_bytes(&app), bytes);
    assert!(
        app.copilot_proof(Some(&config)).unwrap().configured,
        "the managed entry must still be intact"
    );
    assert_ne!(app.copilot_epoch(), epoch);
}
