#[path = "build_site_telemetry.rs"]
mod build_site_telemetry;
#[path = "build_version.rs"]
mod build_version;
#[allow(dead_code)]
#[path = "src/history/operator_policy/settings.rs"]
mod policy_settings;
#[path = "src/history/destination/direct_s3/storage_origins.rs"]
mod storage_origins;
#[path = "windows_resources.rs"]
mod windows_resources;

use std::{env, fs, process::Command};

fn git(arguments: &[&str]) -> Option<String> {
    let output = Command::new("git").args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The RelayHistory release session history reports, and the same version
/// the core stamps, for the app's Windows file properties.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    history_version();
    site_telemetry();
    policy_defaults();
    upload_storage_origins();
    // Only Windows executables carry a version; elsewhere nothing here is used.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-env-changed=AGENT_RELAY_PROBE_VERSION");
    println!("cargo:rerun-if-changed=../CHANGELOG.md");
    println!("cargo:rerun-if-changed=windows_resources.rs");
    println!("cargo:rerun-if-changed=build_version.rs");
    // A new commit changes the development version, as in the core's script.
    for arguments in [
        ["rev-parse", "--git-path", "HEAD"],
        ["rev-parse", "--git-path", "index"],
    ] {
        if let Some(path) = git(&arguments) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git(&["rev-parse", "--git-path", &reference]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let revision = git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let changelog = fs::read_to_string("../CHANGELOG.md").expect("desktop changelog");
    let base = changelog
        .lines()
        .find_map(|line| line.strip_prefix("## v"))
        .expect("desktop release heading")
        .trim();
    let version = env::var("AGENT_RELAY_PROBE_VERSION")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| build_version::development_version(base, &revision));
    let product = windows_resources::product_name();
    windows_resources::embed(&version, &product, &format!("{product} Helper"));
}

/// A build for its own site refuses a PostHog host or key
/// (`build_site_telemetry`), on every target. The error names the variables,
/// never their values.
fn site_telemetry() {
    println!("cargo:rerun-if-changed=build_site_telemetry.rs");
    for name in [
        "AGENT_RELAY_SITE_URL",
        "AGENT_RELAY_POSTHOG_HOST",
        "AGENT_RELAY_POSTHOG_API_KEY",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    let read = |name: &str| env::var(name).ok();
    if let Some(error) = build_site_telemetry::refused_posthog_override(
        read("AGENT_RELAY_SITE_URL").as_deref(),
        read("AGENT_RELAY_POSTHOG_HOST").as_deref(),
        read("AGENT_RELAY_POSTHOG_API_KEY").as_deref(),
    ) {
        panic!("{error}");
    }
}

/// The resolved `ai-hist` version (`history::HISTORY_VERSION`). Cargo.lock is
/// the resolved dependency, not the manifest's version range.
fn history_version() {
    println!("cargo:rerun-if-changed=Cargo.lock");
    let lock = fs::read_to_string(
        std::path::PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("Cargo.lock"),
    )
    .unwrap();
    let history = lock
        .split("[[package]]")
        .find_map(|package| {
            let mut name = None;
            let mut version = None;
            for line in package.lines() {
                if let Some(value) = line.strip_prefix("name = \"") {
                    name = value.strip_suffix('"');
                }
                if let Some(value) = line.strip_prefix("version = \"") {
                    version = value.strip_suffix('"');
                }
            }
            (name == Some("ai-hist")).then_some(version).flatten()
        })
        .expect("Cargo.lock must include the resolved ai-hist version");
    println!("cargo:rustc-env=RELAY_HISTORY_VERSION={history}");
}

/// `DESKTOP_POLICY_DEFAULTS`, the upload settings a site profile enforces
/// until its site's policy is read (`history::operator_policy`): checked
/// with the rules a fetched policy passes and handed to the crate as
/// `DESKTOP_POLICY_DEFAULTS_JSON`, compact and sorted, always set (empty
/// for none) so nothing else in the environment can stand in for it. Only
/// a site build (`AGENT_RELAY_SITE_URL` naming a site other than
/// production) takes defaults; production refuses them. A refusal names
/// the variable and the setting, never the value.
fn policy_defaults() {
    println!("cargo:rerun-if-changed=src/history/operator_policy/settings.rs");
    println!(
        "cargo:rerun-if-env-changed={}",
        policy_settings::DEFAULTS_KEY
    );
    println!("cargo:rerun-if-env-changed=AGENT_RELAY_SITE_URL");
    let text = env::var(policy_settings::DEFAULTS_KEY).unwrap_or_default();
    let defaults = match policy_settings::parse_defaults(&text) {
        Ok(defaults) => defaults,
        Err(error) => panic!("{error} (docs/workspace-layout.md, Site profiles)"),
    };
    if defaults.is_some() && !site_build() {
        panic!(
            "{} is set, but this is no site build: only a build whose AGENT_RELAY_SITE_URL names a site other than {PRODUCTION_SITE_URL} takes policy defaults",
            policy_settings::DEFAULTS_KEY,
            PRODUCTION_SITE_URL = policy_settings::PRODUCTION_SITE_URL,
        );
    }
    let canonical = defaults
        .as_ref()
        .map(policy_settings::canonical)
        .unwrap_or_default();
    println!("cargo:rustc-env=DESKTOP_POLICY_DEFAULTS_JSON={canonical}");
}

/// `AGENT_RELAY_HISTORY_UPLOAD_STORAGE_ORIGINS`, the https origins of the
/// storage hosts a site build's direct uploads may PUT to on a private
/// address (`history::destination::direct_s3::private_storage`): checked,
/// canonicalised and handed to the crate as
/// `DESKTOP_HISTORY_UPLOAD_STORAGE_ORIGINS_JSON`, a sorted JSON array, always
/// set (`[]` for none) so nothing else in the environment can stand in for
/// it. Only a site build takes a list. A refusal names the variable and the
/// problem, never the value.
fn upload_storage_origins() {
    println!("cargo:rerun-if-changed=src/history/destination/direct_s3/storage_origins.rs");
    println!("cargo:rerun-if-env-changed={}", storage_origins::KEY);
    println!("cargo:rerun-if-env-changed=AGENT_RELAY_SITE_URL");
    let text = env::var(storage_origins::KEY).unwrap_or_default();
    match storage_origins::compiled(&text, site_build()) {
        Ok(origins) => {
            println!("cargo:rustc-env=DESKTOP_HISTORY_UPLOAD_STORAGE_ORIGINS_JSON={origins}")
        }
        Err(error) => panic!("{error} (docs/workspace-layout.md, Site profiles)"),
    }
}

/// Whether `AGENT_RELAY_SITE_URL` names a site other than production
/// (`policy_settings::is_site_url`).
fn site_build() -> bool {
    policy_settings::is_site_url(&env::var("AGENT_RELAY_SITE_URL").unwrap_or_default())
}
