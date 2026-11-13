//! The core relay ships to this app reports the desktop's build, not its own
//! crate's: `.cargo/config.toml` points relay's build script at this
//! repository's CHANGELOG.md and Git checkout.
#[path = "../build_version.rs"]
mod build_version;

use agent_relay_desktop_core::history::HISTORY_VERSION;
use agent_relay_node::{BUILD_REVISION, VERSION};
use std::process::Command;

#[test]
fn build_identity_is_a_desktop_release_or_git_development_build() {
    if let Some(release) = option_env!("AGENT_RELAY_PROBE_VERSION").filter(|s| !s.is_empty()) {
        assert_eq!(VERSION, release);
    } else {
        let base = include_str!("../../CHANGELOG.md")
            .lines()
            .find_map(|line| line.strip_prefix("## v"))
            .unwrap()
            .trim();
        assert_eq!(
            VERSION,
            build_version::development_version(base, BUILD_REVISION),
            "the core must report the desktop's CalVer, not relay's crate version"
        );
    }
    // This checkout's revision, not the pinned relay commit's.
    let output = Command::new("git")
        .args([
            "-C",
            env!("CARGO_MANIFEST_DIR"),
            "rev-parse",
            "--short=12",
            "HEAD",
        ])
        .output();
    if let Some(output) = output.ok().filter(|output| output.status.success()) {
        assert_eq!(
            BUILD_REVISION,
            String::from_utf8_lossy(&output.stdout).trim()
        );
    }
}

#[test]
fn history_version_matches_the_resolved_dependency() {
    let lock = include_str!("../Cargo.lock");
    assert!(lock.contains(&format!(
        "name = \"ai-hist\"\nversion = \"{HISTORY_VERSION}\""
    )));
}

#[test]
fn development_identity_discards_release_suffixes() {
    for release in [
        "2026.10.5",
        "2026.10.5-rc.1",
        "2026.10.5+build.42",
        "2026.10.5-rc.1+build.42",
    ] {
        assert_eq!(
            build_version::development_version(release, "abc123"),
            "2026.10.5-dev+abc123",
            "release heading: {release}"
        );
    }
}
