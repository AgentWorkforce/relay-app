//! `scripts/build-linux.sh` builds the tree `linux_app::install` describes,
//! and the app's updater installs that tarball: the app finds its core, the
//! systemd unit runs the tree's core, and the launcher entry runs the tree's
//! app. Stand-in scripts take the place of the built binaries.
//!
//! Linux packaging only: Windows builds its zip with `scripts/build-windows.sh`.
#![cfg(unix)]

use agent_relay_app::install::{
    self, APP, ICON, LAUNCHER_ENTRY, PROBE, TARBALL_UPDATER_LINK, UNIT,
};
use agent_relay_app::updates::{self, CommandOutput, FetchError, Host, Release};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const VERSION: &str = "2026.10.9";

fn stand_in(path: &Path) {
    fs::write(path, "#!/bin/sh\n").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

/// Runs the packaging script from outside the repository; returns `--out`.
fn package(directory: &Path) -> PathBuf {
    package_version(directory, VERSION)
}

fn package_version(directory: &Path, version: &str) -> PathBuf {
    let built = build_linux(directory, version, "https://agentrelay.com", "agentrelay");
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    directory.join("out")
}

/// `scripts/build-linux.sh` on stand-in executables, the core beside the
/// sidecars `scripts/build-probe.sh` writes naming the site and link scheme
/// it was built for.
fn build_linux(directory: &Path, version: &str, site: &str, scheme: &str) -> std::process::Output {
    build_linux_with(
        directory,
        version,
        (site, scheme),
        ("https://agentrelay.com", "agentrelay"),
    )
}

/// `build_linux` with the core's and the app's records given separately.
fn build_linux_with(
    directory: &Path,
    version: &str,
    core: (&str, &str),
    app: (&str, &str),
) -> std::process::Output {
    let (app_path, probe) = (directory.join("app"), directory.join("probe"));
    stand_in(&app_path);
    stand_in(&probe);
    for (name, (site, scheme)) in [("probe", core), ("agent-relay", app)] {
        fs::write(
            directory.join(format!("{name}-site.txt")),
            format!("{site}\n"),
        )
        .unwrap();
        fs::write(
            directory.join(format!("{name}-url-scheme.txt")),
            format!("{scheme}\n"),
        )
        .unwrap();
    }
    let app = app_path;
    let out = directory.join("out");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/build-linux.sh");
    Command::new("bash")
        .arg(script)
        .args(["--version", version, "--arch", "x64", "--app"])
        .arg(&app)
        .arg("--probe")
        .arg(&probe)
        .arg("--out")
        .arg(&out)
        .current_dir(directory)
        .output()
        .unwrap()
}

/// The package is Agent Relay for production: a core built for another
/// site, or opening another scheme's links, is refused, naming which.
#[test]
fn a_core_built_for_another_site_or_scheme_is_not_packaged() {
    let directory = tempfile::tempdir().unwrap();
    let refused = build_linux(
        directory.path(),
        VERSION,
        "http://localhost:3100",
        "agentrelay",
    );
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("is built for http://localhost:3100, not https://agentrelay.com"),
        "{stderr}"
    );
    assert!(!directory.path().join("out").exists());
    let refused = build_linux(
        directory.path(),
        VERSION,
        "https://agentrelay.com",
        "agentrelay-dev",
    );
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("opens agentrelay-dev:// links, not agentrelay://"),
        "{stderr}"
    );
    assert!(!directory.path().join("out").exists());
    // The app is checked as the core is, by the records build-gtk.sh
    // writes beside it.
    let refused = build_linux_with(
        directory.path(),
        VERSION,
        ("https://agentrelay.com", "agentrelay"),
        ("http://localhost:3100", "agentrelay"),
    );
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("is built for http://localhost:3100, not https://agentrelay.com")
            && stderr.contains("scripts/build-gtk.sh"),
        "{stderr}"
    );
    assert!(!directory.path().join("out").exists());
}

fn no_env(_: &str) -> Option<OsString> {
    None
}

#[test]
#[cfg(target_os = "linux")]
fn beta_debian_versions_sort_before_stable_and_keep_versioned_filenames() {
    let directory = tempfile::tempdir().unwrap();
    let out = package_version(directory.path(), "2026.10.9-beta.2");
    let deb = out.join("AgentRelay-2026.10.9-beta.2-Linux-x64.deb");
    assert!(deb.exists());
    assert!(out
        .join("AgentRelay-2026.10.9-beta.2-Linux-x64.tar.gz")
        .exists());
    if Command::new("dpkg-deb").arg("--version").output().is_ok() {
        let version = Command::new("dpkg-deb")
            .arg("-f")
            .arg(&deb)
            .arg("Version")
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8(version.stdout).unwrap().trim(),
            "2026.10.9~beta.2"
        );
        assert!(Command::new("dpkg")
            .args(["--compare-versions", "2026.10.9~beta.2", "lt", "2026.10.9"])
            .status()
            .unwrap()
            .success());
    }
}

#[test]
fn the_package_is_the_tree_the_app_expects() {
    let directory = tempfile::tempdir().unwrap();
    let out = package(directory.path());
    let tree = out.join("AgentRelay-linux-x64");

    for path in [APP, PROBE, LAUNCHER_ENTRY, ICON, UNIT] {
        assert!(tree.join(path).is_file(), "{path} is packaged");
    }
    for path in [APP, PROBE] {
        assert!(is_executable(&tree.join(path)), "{path} is executable");
    }
    // The tree left beside the packages is the .deb's: the npm CLI's
    // `agent-relay` name is the tarball's alone.
    assert!(!tree.join(TARBALL_UPDATER_LINK).exists());
    let app = tree.join(APP).canonicalize().unwrap();
    assert_eq!(
        install::bundled_probe(&app, &no_env),
        Some(tree.canonicalize().unwrap().join(PROBE))
    );

    let unit = fs::read_to_string(tree.join(UNIT)).unwrap();
    assert!(
        unit.lines()
            .any(|line| line == format!("ExecStart=/{PROBE} serve --headless")),
        "{unit}"
    );
    let entry = fs::read_to_string(tree.join(LAUNCHER_ENTRY)).unwrap();
    let desktop_name = Path::new(APP).file_name().unwrap().to_str().unwrap();
    assert!(
        entry
            .lines()
            .any(|line| line == format!("Exec={desktop_name} %u")),
        "{entry}"
    );
    let icon = Path::new(ICON).file_stem().unwrap().to_str().unwrap();
    assert!(
        entry.lines().any(|line| line == format!("Icon={icon}")),
        "{entry}"
    );

    // Where `dpkg-deb` runs (Linux CI), the package holds the same tree and
    // depends on GTK 4, not Python.
    if Command::new("dpkg-deb").arg("--version").output().is_ok() {
        let deb = out.join(format!("AgentRelay-{VERSION}-Linux-x64.deb"));
        let field = |name: &str| {
            let output = Command::new("dpkg-deb")
                .arg("-f")
                .arg(&deb)
                .arg(name)
                .output()
                .unwrap();
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        assert_eq!(field("Version"), VERSION);
        assert_eq!(
            field("Depends"),
            "libc6 (>= 2.34), libgtk-4-1 (>= 4.6), librsvg2-common"
        );
        let listing = Command::new("dpkg-deb")
            .arg("-c")
            .arg(&deb)
            .output()
            .unwrap();
        let listing = String::from_utf8(listing.stdout).unwrap();
        for path in [APP, PROBE, LAUNCHER_ENTRY, ICON, UNIT] {
            assert!(listing.contains(&format!("./{path}")), "{path}:\n{listing}");
        }
        assert!(
            !listing.contains(&format!("./{TARBALL_UPDATER_LINK} ")),
            "{listing}"
        );
        // Running user managers reload the unit, whose core may have moved.
        let postinst = Command::new("dpkg-deb")
            .arg("-I")
            .arg(&deb)
            .arg("postinst")
            .output()
            .unwrap();
        let postinst = String::from_utf8_lossy(&postinst.stdout);
        assert!(postinst.contains("daemon-reload"));
        // Launch-at-login entries naming the old app move with the upgrade.
        assert!(postinst.contains("Exec=/usr/bin/agent-relay-desktop --background"));
    }
}

#[test]
fn the_updater_installs_the_packaged_tarball() {
    let directory = tempfile::tempdir().unwrap();
    let out = package(directory.path());
    let tarball = fs::read(out.join(format!("AgentRelay-{VERSION}-Linux-x64.tar.gz"))).unwrap();
    let published =
        fs::read(out.join(format!("AgentRelay-{VERSION}-Linux-x64.tar.gz.sha256"))).unwrap();
    assert_eq!(
        updates::checksum(&String::from_utf8_lossy(&published)),
        Some(
            Sha256::digest(&tarball)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        )
    );

    let home = directory.path().join("home");
    let fetch = |url: &str| -> Result<Vec<u8>, FetchError> {
        Ok(if url.ends_with(".sha256") {
            published.clone()
        } else {
            tarball.clone()
        })
    };
    let run = |_: &[&str], _: Duration| {
        Ok(CommandOutput {
            code: Some(0),
            ..CommandOutput::default()
        })
    };
    let host = Host {
        home: home.clone(),
        env: &no_env,
        fetch: &fetch,
        run: &run,
        request: &|_| Err(FetchError::Unreachable("no feed".into())),
        feed: None,
    };
    let release = Release {
        version: VERSION.into(),
        tarball_url: "https://example.invalid/AgentRelay.tar.gz".into(),
        tarball_checksum_url: "https://example.invalid/AgentRelay.tar.gz.sha256".into(),
        deb_url: None,
        deb_checksum_url: None,
        page_url: "https://example.invalid/release".into(),
        feed: None,
    };
    let root = home.join(".local/lib/agent-relay");
    let unit = updates::default_unit_path(&home);
    let installed = updates::install_tarball(&release, &root, &unit, &host).unwrap();

    let tree = root.join(VERSION).join("AgentRelay-linux-x64");
    assert_eq!(installed, tree.join(APP));
    // Earlier releases' updaters find and relaunch the app by this link.
    assert_eq!(
        tree.join(TARBALL_UPDATER_LINK).canonicalize().unwrap(),
        tree.join(APP).canonicalize().unwrap()
    );
    assert_eq!(
        install::bundled_probe(&installed, &no_env),
        Some(tree.join(PROBE))
    );
    let unit = fs::read_to_string(unit).unwrap();
    assert!(
        unit.lines().any(|line| line
            == format!(
                "ExecStart={} serve --headless",
                tree.join(PROBE).display()
            )),
        "{unit}"
    );
    let entry =
        fs::read_to_string(home.join(".local/share/applications/agent-relay.desktop")).unwrap();
    assert!(
        entry
            .lines()
            .any(|line| line == format!("Exec={} %u", tree.join(APP).display())),
        "{entry}"
    );
}
