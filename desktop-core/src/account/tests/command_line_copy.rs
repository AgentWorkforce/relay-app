//! Which builds keep the shared command-line copy, and which replace
//! another build of their own release there. Three cases: the stock app
//! (default core) keeps it and replaces its own release; a site profile on
//! its own socket that is not built to coexist (it takes over Agent Relay)
//! keeps it, upgrading but never replacing its own release; a site profile
//! built to coexist never writes it.
use super::*;
use agent_relay_node::relay::daemon::{Config, Plugins};
use agent_relay_node::relay::paths::{default_socket, CorePaths};
use std::path::Path;

fn config(home: &Path, socket: PathBuf, version: &str) -> Config {
    Config {
        paths: CorePaths::for_socket(socket).unwrap(),
        home: home.to_path_buf(),
        app_data_dir: home.join("app-data"),
        env: Default::default(),
        version: version.into(),
        host_name: "Test Desk.local".into(),
        rediscover_every: Duration::from_secs(3600),
        push_timing: Default::default(),
        delivery_timing: Default::default(),
    }
}

/// The account desktop startup makes for a core on `socket`. The test
/// binary runs from no app bundle, so it is never built to coexist.
fn account_on(home: &Path, socket: PathBuf) -> Arc<Account> {
    with_account(
        Plugins::default(),
        &config(home, socket, "0.0.0-test"),
        false,
    )
    .account
    .unwrap()
    .as_any()
    .downcast::<Account>()
    .unwrap()
}

/// Stock and a site profile not built to coexist, as desktop startup makes
/// them: both keep the shared copy, and only the stock app's default core
/// replaces another build of its own release there.
#[test]
fn the_stock_app_and_a_profile_not_built_to_coexist_keep_the_shared_copy() {
    let dir = tempfile::tempdir().unwrap();
    let copies = !site_choices::development_site(built_in_site_url());
    let shared = crate::probe_copy::destination(dir.path());
    let profile = account_on(dir.path(), dir.path().join("core.sock"));
    assert_eq!(profile.command_line_copy.is_some(), copies);
    assert!(profile
        .command_line_copy
        .as_ref()
        .is_none_or(|(_, destination, replace)| *destination == shared && !replace));
    let stock = account_on(dir.path(), default_socket(dir.path()));
    assert_eq!(stock.command_line_copy.is_some(), copies);
    assert!(stock
        .command_line_copy
        .as_ref()
        .is_none_or(|(_, destination, replace)| *destination == shared && *replace));
}

/// The three cases, for a build newer than any copy installed: stock and a
/// profile not built to coexist keep the shared copy (only stock replacing
/// its own release); a site profile built to coexist has none to keep.
#[test]
fn only_a_profile_built_to_coexist_never_writes_the_shared_copy() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let shared = crate::probe_copy::destination(home);
    let newer = home.join("newer-probe");
    let stock = config(home, default_socket(home), "2099.1.1");
    let profile = config(home, home.join("core.profile.sock"), "2099.1.1");
    for coexists in [false, true] {
        // The default core is never built to coexist (build-app.sh refuses
        // --coexist for it); were the signal wrong, it still keeps the copy.
        assert_eq!(
            site_choices::command_line_copy(newer.clone(), &stock, coexists),
            Some((newer.clone(), shared.clone(), true)),
            "stock, coexists {coexists}"
        );
    }
    // A site profile on its own socket, not built to coexist.
    assert_eq!(
        site_choices::command_line_copy(newer.clone(), &profile, false),
        Some((newer.clone(), shared.clone(), false))
    );
    // A site profile built to coexist.
    assert_eq!(site_choices::command_line_copy(newer, &profile, true), None);
}

/// The copy each case describes reaches `install_if_needed`: against an
/// older copy both stock and a site profile not built to coexist upgrade it;
/// against another build of the same release (same version, other bytes)
/// the profile leaves it and stock replaces it.
#[test]
#[cfg(unix)]
fn a_profile_not_built_to_coexist_upgrades_but_never_replaces_its_own_release() {
    let dir = tempfile::tempdir().unwrap();
    let fake = crate::probe_copy::tests::fake;
    let destination = dir.path().join("bin/agent-relay-probe");
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    let source = fake(dir.path(), "bundled", "2026.11.1-beta.3", None);
    let older = fake(dir.path(), "older", "2026.11.1-beta.2", None);
    let same_release = fake(
        dir.path(),
        "previous",
        "2026.11.1-beta.3",
        Some(&dir.path().join("ran")),
    );
    for (case, replace_same_release) in [("profile", false), ("stock", true)] {
        let copy = (source.clone(), destination.clone(), replace_same_release);
        // An older copy: upgraded.
        std::fs::copy(&older, &destination).unwrap();
        assert!(
            site_choices::install_command_line_copy(&copy, |_, _| true),
            "{case}"
        );
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            std::fs::read(&source).unwrap(),
            "{case}"
        );
        // Another build of the same release: only stock replaces it.
        std::fs::copy(&same_release, &destination).unwrap();
        assert_eq!(
            site_choices::install_command_line_copy(&copy, |_, _| true),
            replace_same_release,
            "{case}"
        );
        let kept = if replace_same_release {
            &source
        } else {
            &same_release
        };
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            std::fs::read(kept).unwrap(),
            "{case}"
        );
    }
}
