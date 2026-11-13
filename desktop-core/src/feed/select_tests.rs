//! Asset selection, download hosts and manifest rules the draft's fixtures
//! do not cover.

use super::tests::fixture;
use super::*;
use serde_json::json;

/// A cloud build's asset prefix.
fn cloud_profile(prefix: &'static str) -> &'static str {
    prefix
}

fn stable_manifest() -> Manifest {
    let value: Value =
        serde_json::from_str(&fixture("manifest/valid-stable.json")).expect("manifest");
    Manifest::from_value(&value).expect("valid")
}

#[test]
fn selects_the_one_asset_and_checks_its_name() {
    let manifest = stable_manifest();
    let profile = cloud_profile("Desktop-x7k2q");
    let pick = |platform, arch, kind, floor| {
        manifest
            .select(profile, platform, arch, kind, floor)
            .map(|asset| asset.name.as_str())
    };
    use {Arch::*, Kind::*, Platform::*};
    for (platform, arch, kind, floor, name) in [
        (
            Linux,
            X64,
            Tarball,
            None,
            "Desktop-x7k2q-2026.11.9-Linux-x64.tar.gz",
        ),
        (
            Linux,
            X64,
            Deb,
            None,
            "Desktop-x7k2q-2026.11.9-Linux-x64.deb",
        ),
        (
            Linux,
            Arm64,
            Probe,
            None,
            "Desktop-x7k2q-2026.11.9-Linux-arm64-probe.tar.gz",
        ),
        (
            Windows,
            X64,
            Setup,
            None,
            "Desktop-x7k2q-2026.11.9-Windows-x64-Setup.exe",
        ),
        (
            Windows,
            Arm64,
            Zip,
            None,
            "Desktop-x7k2q-2026.11.9-Windows-arm64.zip",
        ),
        (
            MacOs,
            Arm64,
            Image,
            Some(15),
            "Desktop-x7k2q-2026.11.9-macOS15-arm64.dmg",
        ),
        (
            MacOs,
            Arm64,
            Image,
            None,
            "Desktop-x7k2q-2026.11.9-macOS-arm64.dmg",
        ),
        (Linux, X64, InstallScript, None, "install.sh"),
    ] {
        assert_eq!(pick(platform, arch, kind, floor), Ok(name));
    }
    assert_eq!(pick(Windows, Arm64, Setup, None), Err(FeedError::NoAsset));
    assert_eq!(pick(Linux, Arm64, Tarball, None), Err(FeedError::NoAsset));
    assert_eq!(pick(MacOs, Arm64, Image, Some(14)), Err(FeedError::NoAsset));
    let other = cloud_profile("Desktop-y9p3r");
    assert_eq!(
        manifest.select(other, Linux, X64, Tarball, None).err(),
        Some(FeedError::AssetNameMismatch)
    );
    let mut doubled = manifest.clone();
    let mut copy = doubled.assets[2].clone();
    copy.id = "a_copy".into();
    copy.name = "Desktop-x7k2q-2026.11.9-Linux-x64.copy.tar.gz".into();
    doubled.assets.push(copy);
    assert_eq!(
        doubled.select(profile, Linux, X64, Tarball, None).err(),
        Some(FeedError::AmbiguousAsset)
    );
}

#[test]
fn a_download_host_is_listed_or_the_site() {
    let manifest = stable_manifest();
    let site = "https://relay.example.com";
    for url in [
        "https://storage.googleapis.com/bucket/object?X-Goog-Signature=ab",
        "https://storage.googleapis.com",
        "https://relay.example.com/cloud/api/v1/desktop/feed/assets/a_01",
    ] {
        assert!(download_host_allowed(url, &manifest, site), "{url}");
    }
    for url in [
        "http://storage.googleapis.com/x",
        "https://storage.googleapis.com:443/x",
        "https://storage.googleapis.com:8443/x",
        "https://user@storage.googleapis.com/x",
        "https://Storage.googleapis.com/x",
        "https://evil.storage.googleapis.com/x",
        "https://storage.googleapis.com.evil.com/x",
        "https://googleapis.com/x",
        "https://10.0.0.7/x",
        "https://[::1]/x",
        "https://storage.googleapis.com%2eevil.com/x",
        "https://relay.example.com:443/x",
        "storage.googleapis.com/x",
        "",
    ] {
        assert!(!download_host_allowed(url, &manifest, site), "{url}");
    }
    assert!(!download_host_allowed(
        "https://relay.example.com/x",
        &manifest,
        "http://relay.example.com"
    ));
    let streamed_only = Manifest {
        download_hosts: Vec::new(),
        ..manifest
    };
    assert!(!download_host_allowed(
        "https://storage.googleapis.com/x",
        &streamed_only,
        site
    ));
    assert!(download_host_allowed(
        "https://relay.example.com/x",
        &streamed_only,
        site
    ));
}

#[test]
fn manifest_rules_beyond_the_fixtures() {
    let base: Value =
        serde_json::from_str(&fixture("manifest/valid-stable.json")).expect("manifest");
    let with = |pointer: &str, value: Value| {
        let mut manifest = base.clone();
        *manifest.pointer_mut(pointer).expect(pointer) = value;
        Manifest::from_value(&manifest).err()
    };
    let invalid = Some(FeedError::SchemaInvalid);
    for (pointer, value) in [
        ("/version", json!("2026.13.1")),
        ("/version", json!("2026.0.1")),
        ("/version", json!("2026.01.1")),
        ("/version", json!("26.11.1")),
        ("/version", json!("2026.11")),
        ("/published_at", json!("2026-02-30T10:00:00Z")),
        ("/published_at", json!("2026-11-20T24:00:00Z")),
        ("/published_at", json!("2026-11-20 10:04:31Z")),
        ("/notes", json!("x".repeat(65_537))),
        ("/download_hosts", json!(["a.com", "a.com"])),
        ("/download_hosts", json!(["localhost"])),
        ("/download_hosts", json!(["-a.com"])),
        ("/download_hosts", json!(["a.com."])),
        (
            "/download_hosts",
            json!(["a.b.c.d.e.f.g.h.i"
                .split('.')
                .map(|l| format!("{l}.com"))
                .collect::<Vec<_>>()]),
        ),
        ("/assets/0/os_floor", json!(9)),
        ("/assets/0/os_floor", json!(100)),
        ("/assets/0/size", json!(0)),
        ("/assets/0/id", json!("b_1")),
        ("/assets/0/name", json!(".hidden")),
        ("/assets/0/name", json!("a..b")),
        ("/schema", json!("1")),
        ("/profile", json!("")),
    ] {
        assert_eq!(with(pointer, value.clone()), invalid, "{pointer} {value}");
    }
    assert_eq!(with("/notes", json!("x".repeat(65_536))), None);
    assert_eq!(with("/published_at", json!("2028-02-29T23:59:60Z")), None);
    let mut repeated = base.clone();
    let first = repeated["assets"][0].clone();
    repeated["assets"]
        .as_array_mut()
        .expect("assets")
        .push(first);
    assert_eq!(Manifest::from_value(&repeated).err(), invalid);
}
