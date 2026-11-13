#[path = "../desktop-core/build_version.rs"]
mod build_version;
#[path = "../desktop-core/windows_resources.rs"]
mod windows_resources;

use std::{env, fs, path::PathBuf, process::Command};

fn git(arguments: &[&str]) -> Option<String> {
    let output = Command::new("git").args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The build's site profile, as far as it is the app's own
/// (docs/workspace-layout.md, "Site profiles"): the product name it shows,
/// the GitHub repository its updater reads releases from, what those
/// releases' artifact names start with, and the mark it draws. Each comes
/// from the variable `scripts/build-gtk.sh` (and `build-windows.sh`)
/// exports, the default profile's when unset, and reaches the crate as a
/// compile-time variable (`env!`, `src/build_profile.rs`) or, for the mark,
/// as `$OUT_DIR/mark.svg`. The site and link scheme are the core's
/// (`AGENT_RELAY_SITE_URL`, `AGENT_RELAY_URL_SCHEME`), read by its build.
fn profile() -> String {
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let product = windows_resources::product_name();
    println!("cargo:rustc-env=DESKTOP_PRODUCT_NAME={product}");
    let read = |name: &str| {
        println!("cargo:rerun-if-env-changed={name}");
        env::var(name)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    let is_name = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
    };
    let is_repository = |value: &str| {
        matches!(value.split_once('/'), Some((owner, name)) if is_name(owner) && is_name(name) && !name.contains('/'))
    };
    let prefix = read("DESKTOP_ASSET_PREFIX").unwrap_or_else(|| "AgentRelay".into());
    assert!(
        prefix == "AgentRelay"
            || prefix.strip_prefix("Desktop-").is_some_and(|id| {
                !id.is_empty()
                    && id.len() <= 40
                    && id
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                    && !id.starts_with('-')
                    && !id.ends_with('-')
                    && !id.contains("--")
            }),
        "DESKTOP_ASSET_PREFIX must be AgentRelay or Desktop-<id>, got: {prefix}"
    );
    println!("cargo:rustc-env=DESKTOP_ASSET_PREFIX={prefix}");
    // The profile's opaque id is the one its asset prefix carries; the
    // default profile has none.
    let derived = prefix.strip_prefix("Desktop-").unwrap_or_default().to_owned();
    let profile_id = read("DESKTOP_PROFILE_ID").unwrap_or_else(|| derived.clone());
    assert!(
        profile_id == derived,
        "DESKTOP_PROFILE_ID must be the id in DESKTOP_ASSET_PREFIX ({prefix}), got: {profile_id}"
    );
    println!("cargo:rustc-env=DESKTOP_PROFILE_ID={profile_id}");
    // Where the updater reads releases (docs/workspace-layout.md, "Site
    // profiles"): `github:<owner>/<name>`, by default the releases repository,
    // or `cloud`, the profile's own site, which then needs the key its
    // release records are signed with.
    let repository = read("RELEASES_REPOSITORY");
    let feed = read("RELEASE_FEED").unwrap_or_else(|| {
        format!(
            "github:{}",
            repository
                .as_deref()
                .unwrap_or("AgentWorkforce/relay-desktop-releases")
        )
    });
    let key = read("DESKTOP_FEED_PUBLIC_KEY").unwrap_or_default();
    let repository = if feed == "cloud" {
        assert!(
            repository.is_none(),
            "RELEASES_REPOSITORY must be unset for a cloud RELEASE_FEED"
        );
        assert!(
            !profile_id.is_empty(),
            "a cloud RELEASE_FEED needs a site profile (DESKTOP_ASSET_PREFIX Desktop-<id>)"
        );
        let site = read("AGENT_RELAY_SITE_URL").unwrap_or_default();
        assert!(
            !site.is_empty() && site.trim_end_matches('/') != "https://agentrelay.com",
            "a cloud RELEASE_FEED needs the profile's own AGENT_RELAY_SITE_URL"
        );
        assert!(
            is_feed_key(&key),
            "a cloud RELEASE_FEED needs DESKTOP_FEED_PUBLIC_KEY, a 32-byte Ed25519 key in base64"
        );
        String::new()
    } else {
        let named = feed
            .strip_prefix("github:")
            .filter(|named| is_repository(named))
            .unwrap_or_else(|| {
                panic!("RELEASE_FEED must be github:<owner>/<name> or cloud, got: {feed}")
            });
        assert!(
            repository
                .as_deref()
                .is_none_or(|repository| repository.eq_ignore_ascii_case(named)),
            "RELEASE_FEED ({feed}) must name RELEASES_REPOSITORY"
        );
        assert!(
            key.is_empty(),
            "DESKTOP_FEED_PUBLIC_KEY is only for a cloud RELEASE_FEED"
        );
        repository.unwrap_or_else(|| named.to_owned())
    };
    assert!(
        repository.is_empty() || is_repository(&repository),
        "RELEASES_REPOSITORY must be owner/name, got: {repository}"
    );
    println!("cargo:rustc-env=RELEASES_REPOSITORY={repository}");
    println!("cargo:rustc-env=RELEASE_FEED={feed}");
    println!("cargo:rustc-env=DESKTOP_FEED_PUBLIC_KEY={key}");
    println!("cargo:rerun-if-env-changed=DESKTOP_ICON");
    let mark = env::var_os("DESKTOP_ICON")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("../assets/agent-relay-mark.svg"));
    println!("cargo:rerun-if-changed={}", mark.display());
    let data =
        fs::read(&mark).unwrap_or_else(|error| panic!("DESKTOP_ICON {}: {error}", mark.display()));
    assert!(
        String::from_utf8_lossy(&data).contains("<svg"),
        "DESKTOP_ICON must be an SVG: {}",
        mark.display()
    );
    fs::write(out.join("mark.svg"), data).expect("write the mark");
    product
}

/// Whether `key` is a 32-byte key in standard, padded base64: 43 characters
/// whose last carries no stray bits, then `=`.
fn is_feed_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    let base64 = |c: &u8| c.is_ascii_alphanumeric() || *c == b'+' || *c == b'/';
    bytes.len() == 44
        && bytes[..42].iter().all(base64)
        && b"AEIMQUYcgkosw048".contains(&bytes[42])
        && bytes[43] == b'='
}

/// The core crates `Cargo.lock` resolved, for About > Diagnostics
/// (`src/models/core_crates.rs`, `CoreCrates`): the commit of agent-relay-node
/// and the version of relaycast, as `$OUT_DIR/crates.json` in the shape
/// `scripts/apply-crate-overrides.sh --json` writes. These are the pins or
/// an internal build's overrides; `{}` when the lockfile has neither.
fn crates() {
    println!("cargo:rerun-if-changed=Cargo.lock");
    let lock = fs::read_to_string("Cargo.lock").unwrap_or_default();
    let mut node = None;
    let mut relaycast = None;
    for package in lock.split("[[package]]").skip(1) {
        let field = |key: &str| {
            package.lines().find_map(|line| {
                let value = line.trim().strip_prefix(key)?.trim_start();
                let value = value.strip_prefix('=')?.trim();
                Some(value.trim_matches('"').to_owned())
            })
        };
        match field("name").as_deref() {
            Some("agent-relay-node") => {
                node = field("source")
                    .and_then(|source| source.rsplit_once('#').map(|(_, rev)| rev.to_owned()))
                    .filter(|rev| rev.len() == 40 && rev.chars().all(|c| c.is_ascii_hexdigit()));
            }
            Some("relaycast") => {
                relaycast = field("version").filter(|version| {
                    version
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || ".-+".contains(c))
                });
            }
            _ => {}
        }
    }
    let json = match (node, relaycast) {
        (Some(rev), Some(version)) => format!(
            r#"{{"agent-relay-node":{{"rev":"{rev}"}},"relaycast":{{"version":"{version}"}}}}"#
        ),
        _ => "{}".to_owned(),
    };
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out.join("crates.json"), json).expect("write crates.json");
}

/// The same version the core stamps, for the app's Windows file properties.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let product = profile();
    crates();
    // Only Windows executables carry a version; elsewhere nothing below is used.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    println!("cargo:rerun-if-env-changed=AGENT_RELAY_PROBE_VERSION");
    println!("cargo:rerun-if-changed=../CHANGELOG.md");
    println!("cargo:rerun-if-changed=../desktop-core/windows_resources.rs");
    println!("cargo:rerun-if-changed=../desktop-core/build_version.rs");
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
    windows_resources::embed(&version, &product, &product);
}
