//! What `with_account` keys on the build: the site the core is built for
//! and whether a telemetry project was built in. A release profile (the
//! default one or a customer's; docs/workspace-layout.md, "Site profiles")
//! is any build for a site that is not a development one.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// The choices a built core makes at start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SiteChoices {
    /// Whether telemetry reports to the build's project: when the build
    /// has one (`telemetry::posthog`: a production build needs
    /// `AGENT_RELAY_POSTHOG_API_KEY` at build time; a site build reports to
    /// its own site's collector, and `AGENT_RELAY_POSTHOG_HOST` and
    /// `AGENT_RELAY_POSTHOG_API_KEY` are refused when it is made), whatever
    /// the site.
    pub telemetry_project: bool,
    /// Whether the command-line copy of this binary is kept: any release
    /// profile, no probe named in the environment (`RELAY_DESKTOP_PROBE`, a
    /// development run), and not a development version (a `dev`
    /// prerelease, such as a from-source build's `2026.11.3-dev+abc123`).
    pub command_line_copy: bool,
}

/// Whether this build's core is the default one, on the default core
/// socket. Only such a build replaces another build of its own release, at
/// the command-line copy (`probe_copy`) and at a workspace's collector
/// (`delivery_status::outdated`): a site profile built to coexist
/// (`DESKTOP_COEXIST`) runs its own core on its own socket and shares both,
/// so two builds of one release never take turns replacing them.
pub(super) fn default_core(config: &agent_relay_node::relay::daemon::Config) -> bool {
    config.paths.socket == agent_relay_node::relay::paths::default_socket(&config.home)
}

/// This binary, where its command-line copy goes, and whether this build
/// replaces another build of its own release there: only when its core is
/// the default one (`default_core`).
pub(super) fn command_line_copy(
    executable: PathBuf,
    config: &agent_relay_node::relay::daemon::Config,
) -> (PathBuf, PathBuf, bool) {
    let destination = crate::probe_copy::destination(&config.home);
    (executable, destination, default_core(config))
}

/// Brings the command-line copy `command_line_copy` describes up to date,
/// on a blocking thread, without waiting for it.
pub(super) fn keep_command_line_copy(copy: (PathBuf, PathBuf, bool)) {
    tokio::task::spawn_blocking(move || {
        install_command_line_copy(&copy, crate::probe_copy::signed_like)
    });
}

/// Installs the command-line copy `command_line_copy` describes, replacing
/// another build of this release only when it says so; `trusts` is
/// `probe_copy::signed_like` outside tests. Whether it installed.
pub(super) fn install_command_line_copy(
    (source, destination, replace_same_release): &(PathBuf, PathBuf, bool),
    trusts: impl Fn(&std::path::Path, &std::path::Path) -> bool,
) -> bool {
    crate::probe_copy::install_if_needed(source, destination, *replace_same_release, trusts)
}

/// Whether `site` is a development site: the core's build accepts plain
/// `http` for `localhost` and `127.0.0.1` only (relay's `build_site.rs`), so
/// an `http` origin is a developer's own Cloud, never a release profile's.
pub(super) fn development_site(site: &str) -> bool {
    site.starts_with("http://")
}

/// The choices for a core built for `site` with (`telemetry_key`) or
/// without a telemetry project (`telemetry::posthog`), run with `env`, as
/// the core `version`.
pub(super) fn for_build(
    site: &str,
    telemetry_key: bool,
    env: &BTreeMap<String, String>,
    version: &str,
) -> SiteChoices {
    SiteChoices {
        telemetry_project: telemetry_key,
        command_line_copy: !development_site(site)
            && !env.contains_key("RELAY_DESKTOP_PROBE")
            && !crate::probe_copy::Release::parse(version)
                .is_some_and(|release| release.is_development()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_relay_node::relay::links::DEFAULT_SITE_URL;

    const CUSTOMER_SITE: &str = "https://relay.example";

    #[test]
    fn a_release_profile_keeps_the_command_line_copy() {
        for site in [DEFAULT_SITE_URL, CUSTOMER_SITE] {
            assert!(!development_site(site), "{site}");
            assert!(
                for_build(site, true, &BTreeMap::new(), "2026.11.3").command_line_copy,
                "{site}"
            );
            // A probe named in the environment is a development run: no copy.
            let named = BTreeMap::from([("RELAY_DESKTOP_PROBE".to_string(), "/x".to_string())]);
            assert!(
                !for_build(site, true, &named, "2026.11.3").command_line_copy,
                "{site}"
            );
        }
    }

    #[test]
    fn a_development_version_keeps_no_copy() {
        let env = BTreeMap::new();
        assert!(!for_build(DEFAULT_SITE_URL, true, &env, "2026.11.3-dev+abc123").command_line_copy);
        assert!(for_build(DEFAULT_SITE_URL, true, &env, "2026.11.3").command_line_copy);
        assert!(for_build(DEFAULT_SITE_URL, true, &env, "2026.11.3-rc.1").command_line_copy);
    }

    #[test]
    fn a_development_site_keeps_no_copy() {
        for site in ["http://localhost:3100", "http://127.0.0.1"] {
            assert!(development_site(site), "{site}");
            assert!(
                !for_build(site, true, &BTreeMap::new(), "2026.11.3").command_line_copy,
                "{site}"
            );
        }
    }

    #[test]
    fn telemetry_reports_only_with_a_key_built_in_whatever_the_site() {
        for site in [DEFAULT_SITE_URL, CUSTOMER_SITE, "http://localhost:3100"] {
            assert!(
                for_build(site, true, &BTreeMap::new(), "2026.11.3").telemetry_project,
                "{site}"
            );
            assert!(
                !for_build(site, false, &BTreeMap::new(), "2026.11.3").telemetry_project,
                "{site}"
            );
        }
    }
}
