//! Which builds replace another build of their own release at the
//! command-line copy.
use super::*;
use agent_relay_node::relay::daemon::{Config, Plugins};
use agent_relay_node::relay::paths::{default_socket, CorePaths};
use std::path::Path;

/// The account desktop startup makes for a core on `socket`.
fn account_on(home: &Path, socket: PathBuf) -> Arc<Account> {
    let config = Config {
        paths: CorePaths::for_socket(socket).unwrap(),
        home: home.to_path_buf(),
        app_data_dir: home.join("app-data"),
        env: Default::default(),
        version: "0.0.0-test".into(),
        host_name: "Test Desk.local".into(),
        rediscover_every: Duration::from_secs(3600),
        push_timing: Default::default(),
        delivery_timing: Default::default(),
    };
    with_account(Plugins::default(), &config, false)
        .account
        .unwrap()
        .as_any()
        .downcast::<Account>()
        .unwrap()
}

/// Only a build on the default core socket replaces another build of its
/// own release: a coexisting site profile's core runs on its own socket and
/// shares the copy, so the two never take turns replacing it.
#[test]
fn only_the_default_core_replaces_its_own_release() {
    let dir = tempfile::tempdir().unwrap();
    let copies = !site_choices::development_site(built_in_site_url());
    let profile = account_on(dir.path(), dir.path().join("core.sock"));
    assert_eq!(profile.command_line_copy.is_some(), copies);
    assert!(profile
        .command_line_copy
        .as_ref()
        .is_none_or(|(_, _, replace_same_release)| !replace_same_release));
    let default_core = account_on(dir.path(), default_socket(dir.path()));
    assert_eq!(default_core.command_line_copy.is_some(), copies);
    assert!(default_core
        .command_line_copy
        .as_ref()
        .is_none_or(|(_, _, replace_same_release)| *replace_same_release));
}

/// The copy `command_line_copy` describes reaches `install_if_needed` with
/// its flag: against another build of the same release (same version, other
/// bytes), a coexisting profile's copy leaves it and the default core's
/// replaces it.
#[test]
#[cfg(unix)]
fn the_default_core_flag_reaches_the_installed_copy() {
    let dir = tempfile::tempdir().unwrap();
    let fake = crate::probe_copy::tests::fake;
    let source = fake(dir.path(), "bundled", "2026.11.1-beta.3", None);
    let previous = fake(
        dir.path(),
        "previous",
        "2026.11.1-beta.3",
        Some(&dir.path().join("ran")),
    );
    let destination = dir.path().join("bin/agent-relay-probe");
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::fs::copy(&previous, &destination).unwrap();
    let profile = (source.clone(), destination.clone(), false);
    assert!(!site_choices::install_command_line_copy(
        &profile,
        |_, _| true
    ));
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        std::fs::read(&previous).unwrap()
    );
    let default_core = (source.clone(), destination.clone(), true);
    assert!(site_choices::install_command_line_copy(
        &default_core,
        |_, _| true
    ));
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        std::fs::read(&source).unwrap()
    );
}
