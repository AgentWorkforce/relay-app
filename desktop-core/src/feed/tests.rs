//! The draft contract's fixtures (copied under
//! `desktop-core/tests/fixtures/desktop-feed/`), each signed at run time with a
//! throwaway key, so no signature is committed.

use super::*;
use ed25519_dalek::{Signer, SigningKey};

const KEY_ID: &str = "fk_x7k2q_2026a";

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/desktop-feed/");

pub(super) fn fixture(path: &str) -> String {
    let full = format!("{FIXTURES}{path}");
    std::fs::read_to_string(&full).unwrap_or_else(|error| panic!("{full}: {error}"))
}

fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&[7; 32])
}

fn other_key() -> SigningKey {
    SigningKey::from_bytes(&[9; 32])
}

fn public(key: &SigningKey) -> [u8; 32] {
    key.verifying_key().to_bytes()
}

/// base64url without padding of `key`'s signature over `manifest`'s JCS bytes.
fn sign(key: &SigningKey, manifest: &Value) -> String {
    let bytes = jcs::canonicalize(manifest).expect("canonical manifest");
    URL_SAFE_NO_PAD.encode(key.sign(&bytes).to_bytes())
}

/// An envelope around the manifest's text exactly as given.
fn envelope(manifest_text: &str, signature: &str, key_id: &str) -> String {
    format!(r#"{{"manifest":{manifest_text},"signature":"{signature}","key_id":"{key_id}"}}"#)
}

fn signed(file: &str) -> String {
    let text = fixture(file);
    let value: Value = serde_json::from_str(&text).expect("fixture");
    envelope(&text, &sign(&signing_key(), &value), KEY_ID)
}

fn expected(channel: Channel) -> Expected<'static> {
    Expected {
        profile: "x7k2q",
        channel,
        installed_version: "2026.11.8",
    }
}

fn stable() -> Expected<'static> {
    expected(Channel::Stable)
}

fn result(raw: &str, key: &SigningKey, expected: &Expected<'_>) -> &'static str {
    match verify(raw.as_bytes(), &public(key), expected) {
        Ok(_) => "accept",
        Err(error) => error.code(),
    }
}

fn channel_named(name: &str) -> Channel {
    match name {
        "beta" => Channel::Beta,
        _ => Channel::Stable,
    }
}

#[test]
fn every_manifest_case_reaches_its_client_result() {
    let cases: Value = serde_json::from_str(&fixture("manifest-cases.json")).expect("cases");
    let cases = cases["cases"].as_array().expect("cases array");
    assert_eq!(cases.len(), 27);
    for case in cases {
        let file = case["file"].as_str().expect("file");
        let text = fixture(file.trim_start_matches("./"));
        let value: Value = serde_json::from_str(&text).expect(file);
        let schema_valid = case["schema"] == "valid";
        // A number JCS cannot write has no conformant signature; the strict
        // pass refuses the text before the signature is read.
        let signature = match jcs::canonicalize(&value) {
            Ok(_) => sign(&signing_key(), &value),
            Err(_) => URL_SAFE_NO_PAD.encode([0; 64]),
        };
        assert_eq!(Manifest::from_value(&value).is_ok(), schema_valid, "{file}");
        let request = &case["request"];
        let channel = request["channel"]
            .as_str()
            .or(value["channel"].as_str())
            .unwrap_or("stable");
        let expected = Expected {
            profile: request["profile"].as_str().unwrap_or("x7k2q"),
            channel: channel_named(channel),
            installed_version: "2026.1.0",
        };
        let raw = envelope(&text, &signature, KEY_ID);
        assert_eq!(
            result(&raw, &signing_key(), &expected),
            case["client"],
            "{file}"
        );
    }
}

#[test]
fn every_raw_case_reaches_its_client_result_before_the_signature() {
    let cases: Value = serde_json::from_str(&fixture("raw/cases.json")).expect("cases");
    assert_eq!(cases["placeholder"], "$SIGNATURE");
    let manifest: Value =
        serde_json::from_str(&fixture("manifest/valid-stable.json")).expect("manifest");
    let signature = sign(&signing_key(), &manifest);
    let cases = cases["cases"].as_array().expect("cases array");
    assert_eq!(cases.len(), 7);
    for case in cases {
        let file = case["file"].as_str().expect("file");
        let template = fixture(file);
        assert_eq!(template.matches("$SIGNATURE").count(), 1, "{file}");
        let raw = template.replace("$SIGNATURE", &signature);
        let client = case["client"].as_str().expect("client");
        assert_eq!(result(&raw, &signing_key(), &stable()), client, "{file}");
        let strict = strict_json::check(raw.as_bytes());
        match case["strict"].as_str() {
            Some("ok") => assert_eq!(strict, Ok(()), "{file}"),
            Some("duplicate_name") => {
                assert_eq!(
                    strict,
                    Err(strict_json::StrictJsonError::DuplicateName),
                    "{file}"
                );
                // A last-wins parse reads the valid signed envelope (DF-001).
                let lenient: Value = serde_json::from_str(&raw).expect(file);
                assert_eq!(lenient["manifest"], manifest, "{file}");
                assert_eq!(lenient["key_id"], KEY_ID, "{file}");
            }
            other => panic!("{file}: strict {other:?}"),
        }
    }
}

#[test]
fn every_envelope_fixture_is_refused() {
    let cases: Value = serde_json::from_str(&fixture("manifest-cases.json")).expect("cases");
    let envelopes = cases["envelopes"].as_array().expect("envelopes");
    assert_eq!(envelopes.len(), 8);
    for case in envelopes {
        let file = case["file"].as_str().expect("file");
        assert_eq!(case["schema"], "invalid", "{file}");
        let raw = fixture(file);
        // Its manifest breaks only the manifest schema, which is checked
        // after the (placeholder) signature; signed properly it is
        // schema-invalid.
        let want = if file.ends_with("invalid-manifest-invalid.json") {
            let value: Value = serde_json::from_str(&raw).expect(file);
            let resigned = envelope(
                &value["manifest"].to_string(),
                &sign(&signing_key(), &value["manifest"]),
                KEY_ID,
            );
            assert_eq!(
                result(&resigned, &signing_key(), &stable()),
                "schema_invalid"
            );
            "signature_invalid"
        } else {
            "schema_invalid"
        };
        assert_eq!(result(&raw, &signing_key(), &stable()), want, "{file}");
    }
}

#[test]
fn the_signed_stable_and_beta_manifests_verify() {
    let manifest = verify(
        signed("manifest/valid-stable.json").as_bytes(),
        &public(&signing_key()),
        &stable(),
    )
    .expect("stable verifies");
    assert_eq!(manifest.version, "2026.11.9");
    assert_eq!(manifest.assets.len(), 8);
    assert_eq!(manifest.download_hosts, ["storage.googleapis.com"]);
    let beta = verify(
        signed("manifest/valid-beta.json").as_bytes(),
        &public(&signing_key()),
        &Expected {
            installed_version: "2026.11.10-beta.1",
            ..expected(Channel::Beta)
        },
    )
    .expect("beta verifies");
    assert_eq!(
        beta.notes,
        "- Beta: tray menu rework. Unicode survives JCS: é €.\n"
    );
}

#[test]
fn tampering_another_key_and_another_key_id_fail() {
    let raw = signed("manifest/valid-stable.json");
    let key = signing_key();
    assert_eq!(result(&raw, &key, &stable()), "accept");
    // One byte of the signed content.
    for (from, to) in [
        ("10:04:31Z", "10:04:32Z"),
        ("\"size\": 21311", "\"size\": 21312"),
    ] {
        let tampered = raw.replacen(from, to, 1);
        assert_ne!(tampered, raw);
        assert_eq!(
            result(&tampered, &key, &stable()),
            "signature_invalid",
            "{to}"
        );
    }
    // One bit of the signature.
    let value: Value = serde_json::from_str(&raw).expect("envelope");
    let signature = value["signature"].as_str().expect("signature");
    let mut bytes = URL_SAFE_NO_PAD.decode(signature).expect("base64url");
    bytes[0] ^= 1;
    let flipped = raw.replace(signature, &URL_SAFE_NO_PAD.encode(&bytes));
    assert_eq!(result(&flipped, &key, &stable()), "signature_invalid");
    // The right key_id, signed by another key.
    assert_eq!(result(&raw, &other_key(), &stable()), "signature_invalid");
    // Another profile's key_id is refused before verification, as are
    // another profile's manifests under this one's key.
    for key_id in ["fk_y9p3r_2026a", "fk_x7k2qq_2026a"] {
        let other = raw.replace(KEY_ID, key_id);
        assert_eq!(
            result(&other, &key, &stable()),
            "key_id_refused",
            "{key_id}"
        );
    }
    assert_eq!(
        result(
            &signed("manifest/other-profile-stable.json"),
            &key,
            &stable()
        ),
        "profile_mismatch"
    );
}

#[test]
fn transport_whitespace_and_member_order_do_not_matter() {
    let raw = signed("manifest/valid-stable.json");
    let value: Value = serde_json::from_str(&raw).expect("envelope");
    let pretty = serde_json::to_string_pretty(&value).expect("pretty");
    let compact = value.to_string();
    let reordered = format!(
        r#"{{"key_id":{},"signature":{},"manifest":{}}}"#,
        value["key_id"], value["signature"], value["manifest"]
    );
    let crlf_tabs = format!(
        " \r\n{}\t\n",
        pretty
            .replace("  ", "\t")
            .replace('\n', "\r\n")
            .replace(": ", " :\t")
    );
    for text in [pretty, compact, reordered, crlf_tabs] {
        assert_eq!(result(&text, &signing_key(), &stable()), "accept", "{text}");
    }
}

#[test]
fn the_envelope_is_strict_before_anything_else() {
    let raw = signed("manifest/valid-stable.json");
    let key = signing_key();
    for text in [
        format!("\u{feff}{raw}"),
        format!("{raw}{{}}"),
        raw.replacen("\"x7k2q\"", "\"x7k2q\",\"profile\":\"x7k2q\"", 1),
        raw.replacen("Windows arm64.", "Windows arm64.\\ud800", 1),
        raw.replacen("\"schema\": 1", "\"schema\": 1.0", 1),
        "[]".to_owned(),
        "\"manifest\"".to_owned(),
    ] {
        assert_eq!(result(&text, &key, &stable()), "schema_invalid", "{text}");
    }
}

#[test]
fn the_signature_and_key_id_grammars() {
    let good = URL_SAFE_NO_PAD.encode([0; 64]);
    assert_eq!(good.len(), 86);
    assert!(decode_signature(&good).is_ok());
    for bad in [
        format!("{good}=="),
        good[..85].to_owned(),
        format!("{}B", &good[..85]),
        good.replace('A', "+"),
        format!("{}/A", &good[..84]),
        String::new(),
    ] {
        assert_eq!(
            decode_signature(&bad).err(),
            Some(FeedError::SchemaInvalid),
            "{bad}"
        );
    }
    assert_eq!(key_id_profile("fk_x7k2q_2026a"), Some("x7k2q"));
    for bad in [
        "fk-x7k2q-2026a",
        "fk_X7K2Q_2026a",
        "fk__2026a",
        "fk_x7k2q_",
        "fk_x7k2q_2026_a",
        "fk_x7k2q_abcdefghijklmnopq",
        "fk_x7k2q",
        "",
    ] {
        assert_eq!(key_id_profile(bad), None, "{bad}");
    }
}

#[test]
fn a_version_must_be_newer_than_the_installed_one() {
    let raw = signed("manifest/valid-stable.json");
    for installed in ["2026.11.9", "2026.12.0", "2027.1.0"] {
        let expected = Expected {
            installed_version: installed,
            ..stable()
        };
        assert_eq!(
            result(&raw, &signing_key(), &expected),
            "not_newer",
            "{installed}"
        );
    }
    for installed in ["2026.11.8", "2026.11.9-beta.3", "2025.12.40"] {
        let expected = Expected {
            installed_version: installed,
            ..stable()
        };
        assert_eq!(
            result(&raw, &signing_key(), &expected),
            "accept",
            "{installed}"
        );
    }
}

#[test]
fn the_compiled_key_decodes_from_standard_base64() {
    let key = public(&signing_key());
    let text = STANDARD.encode(key);
    assert_eq!(text.len(), 44);
    assert_eq!(decode_public_key(&text), Some(key));
    assert_eq!(decode_public_key(&URL_SAFE_NO_PAD.encode(key)), None);
    assert_eq!(decode_public_key(&STANDARD.encode([1; 31])), None);
    assert_eq!(decode_public_key(""), None);
}

#[test]
fn every_error_has_a_code_and_a_sentence() {
    let all = [
        FeedError::SchemaInvalid,
        FeedError::SignatureInvalid,
        FeedError::KeyIdRefused,
        FeedError::ProfileMismatch,
        FeedError::ChannelMismatch,
        FeedError::DuplicateAsset,
        FeedError::NotNewer,
        FeedError::NoAsset,
        FeedError::AmbiguousAsset,
        FeedError::AssetNameMismatch,
    ];
    let codes: std::collections::HashSet<_> = all.iter().map(|error| error.code()).collect();
    assert_eq!(codes.len(), all.len());
    for error in [FeedError::SchemaInvalid, FeedError::SignatureInvalid] {
        assert_eq!(error.to_string(), "The update could not be verified.");
    }
    assert!(all.iter().all(|error| error.message().ends_with('.')));
}
