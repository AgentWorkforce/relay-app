//! `feed verify`'s argument parsing and results, on the draft's valid
//! stable fixture signed at run time with a throwaway key.

use super::*;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};

const MANIFEST: &str = include_str!("../tests/fixtures/desktop-feed/manifest/valid-stable.json");

fn arguments(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

fn key() -> SigningKey {
    SigningKey::from_bytes(&[3; 32])
}

/// An envelope signed over the fixture, carrying `change`d manifest.
fn envelope_with(key_id: &str, change: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut manifest: Value = serde_json::from_str(MANIFEST).expect("fixture");
    let canonical = feed::jcs::canonicalize(&manifest).expect("canonical");
    let signature = URL_SAFE_NO_PAD.encode(key().sign(&canonical).to_bytes());
    change(&mut manifest);
    json!({"manifest": manifest, "signature": signature, "key_id": key_id})
        .to_string()
        .into_bytes()
}

fn envelope(key_id: &str) -> Vec<u8> {
    envelope_with(key_id, |_| {})
}

fn request(installed: Option<&str>) -> Request {
    Request {
        profile: "x7k2q".into(),
        channel: Channel::Stable,
        installed_version: installed.map(str::to_owned),
        public_key: STANDARD.encode(key().verifying_key().to_bytes()),
        file: None,
    }
}

#[test]
fn parses_the_options_and_reads_stdin_for_a_dash() {
    let parsed = parse(&arguments(&[
        "verify",
        "--profile",
        "x7k2q",
        "--channel",
        "beta",
        "--installed-version",
        "2026.11.1",
        "--public-key",
        "a2V5",
        "-",
    ]));
    assert_eq!(
        parsed,
        Ok(Parsed::Run(Request {
            profile: "x7k2q".into(),
            channel: Channel::Beta,
            installed_version: Some("2026.11.1".into()),
            public_key: "a2V5".into(),
            file: None,
        }))
    );
    assert_eq!(parse(&arguments(&["--help"])), Ok(Parsed::Help));
    assert_eq!(parse(&arguments(&["verify", "--help"])), Ok(Parsed::Help));
}

#[test]
fn usage_errors_are_refused() {
    for items in [
        &[][..],
        &["check"][..],
        &["verify", "--profile", "x7k2q"][..],
        &["verify", "--public-key", "a2V5"][..],
        &[
            "verify",
            "--profile",
            "x7k2q",
            "--public-key",
            "a2V5",
            "--channel",
            "nightly",
        ][..],
        &[
            "verify",
            "--profile",
            "x7k2q",
            "--public-key",
            "a2V5",
            "--other",
        ][..],
        &[
            "verify",
            "--profile",
            "x7k2q",
            "--public-key",
            "a2V5",
            "a",
            "b",
        ][..],
        &["verify", "--profile"][..],
    ] {
        assert!(parse(&arguments(items)).is_err(), "{items:?}");
    }
}

#[test]
fn a_signed_envelope_verifies_and_lists_its_assets() {
    let (line, verified) = check(&envelope("fk_x7k2q_2026a"), &request(Some("2026.11.8")));
    assert!(verified, "{line}");
    assert_eq!(line["ok"], true);
    assert_eq!(line["version"], "2026.11.9");
    let assets = line["assets"].as_array().expect("assets");
    assert!(assets.iter().any(|asset| asset["kind"] == "probe"
        && asset["platform"] == "Linux"
        && asset["name"] == "Desktop-x7k2q-2026.11.9-Linux-arm64-probe.tar.gz"));
    assert!(line["download_hosts"].is_array());
    // No installed version: any release is accepted.
    assert!(check(&envelope("fk_x7k2q_2026a"), &request(None)).1);
}

#[test]
fn refusals_name_their_code() {
    let code = |raw: &[u8], request: &Request| {
        let (line, verified) = check(raw, request);
        assert!(!verified);
        assert_eq!(line["ok"], false);
        line["code"].as_str().unwrap_or_default().to_owned()
    };
    let tampered = envelope_with("fk_x7k2q_2026a", |manifest| {
        manifest["assets"][0]["size"] = json!(1);
    });
    assert_eq!(code(&tampered, &request(None)), "signature_invalid");
    assert_eq!(
        code(&envelope("fk_other_2026a"), &request(None)),
        "key_id_refused"
    );
    assert_eq!(
        code(&envelope("fk_x7k2q_2026a"), &request(Some("2026.11.9"))),
        "not_newer"
    );
    let mut bad_key = request(None);
    bad_key.public_key = "c2hvcnQ=".into();
    assert_eq!(
        code(&envelope("fk_x7k2q_2026a"), &bad_key),
        "public_key_invalid"
    );
    let mut beta = request(None);
    beta.channel = Channel::Beta;
    assert_eq!(code(&envelope("fk_x7k2q_2026a"), &beta), "channel_mismatch");
}
