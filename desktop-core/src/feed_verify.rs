//! `feed verify`: checks one release feed envelope with the verifier the
//! GTK updater uses ([`crate::feed`]), for the headless install scripts.
//! An update run of `linux/install.sh` or `windows/install.ps1` hands the
//! manifest it fetched to the probe already installed, so every update is
//! verified by a binary the machine already trusts.
//!
//! **Proposed contract, not confirmed.** The envelope is the Cloud team's
//! draft (relay-control `docs/desktop-feed.md`); its encodings and result
//! names may change.
//!
//! One line of JSON on stdout: `{"ok":true,"version":..,"download_hosts":
//! [..],"assets":[..]}`, or `{"ok":false,"code":..,"message":..}`. Exit 0
//! when verified, 1 when refused, 2 for a usage error.

use std::io::{Read, Write};

use serde_json::{json, Value};

use crate::feed::{self, Asset, Channel, Expected, FeedError};

/// The largest envelope read, as the install scripts cap the download.
pub const MAX_ENVELOPE: u64 = 1024 * 1024;

const USAGE: &str = "\
Usage: agent-relay-probe feed verify --profile <id> --public-key <base64>
         [--channel stable|beta] [--installed-version <version>] [<file> | -]

Verifies a release feed envelope ({manifest, signature, key_id}) read from
<file>, or from stdin when it is - or absent, against the profile's Ed25519
public key (standard base64 of 32 bytes). The channel defaults to stable.
With --installed-version the release must be newer (code not_newer).

Prints one line of JSON: {\"ok\":true,\"version\":...,\"download_hosts\":[...],
\"assets\":[...]} or {\"ok\":false,\"code\":...,\"message\":...}.
Exit status: 0 verified, 1 refused, 2 usage.
";

/// What the command line asked for.
#[derive(Debug, PartialEq, Eq)]
struct Request {
    profile: String,
    channel: Channel,
    installed_version: Option<String>,
    public_key: String,
    file: Option<String>,
}

/// What a run decides, before it is printed.
#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    Help,
    Run(Request),
}

fn parse(arguments: &[String]) -> Result<Parsed, String> {
    let Some((first, rest)) = arguments.split_first() else {
        return Err("feed needs a subcommand: verify".into());
    };
    match first.as_str() {
        "-h" | "--help" | "help" => return Ok(Parsed::Help),
        "verify" => {}
        other => return Err(format!("feed has no subcommand {other:?}; it has verify")),
    }
    let (mut profile, mut channel, mut installed, mut key, mut file) =
        (None, Channel::Stable, None, None, None);
    let mut items = rest.iter();
    while let Some(item) = items.next() {
        let mut value = |name: &str| {
            items
                .next()
                .filter(|value| !value.is_empty())
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match item.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--profile" => profile = Some(value("--profile")?),
            "--public-key" => key = Some(value("--public-key")?),
            "--installed-version" => installed = Some(value("--installed-version")?),
            "--channel" => {
                channel = match value("--channel")?.as_str() {
                    "stable" => Channel::Stable,
                    "beta" => Channel::Beta,
                    _ => return Err("--channel must be stable or beta".into()),
                }
            }
            flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
            path if file.is_none() => file = Some(path.to_owned()),
            _ => return Err("feed verify reads one envelope".into()),
        }
    }
    let (Some(profile), Some(public_key)) = (profile, key) else {
        return Err("feed verify needs --profile and --public-key".into());
    };
    Ok(Parsed::Run(Request {
        profile,
        channel,
        installed_version: installed,
        public_key,
        file: file.filter(|path| path != "-"),
    }))
}

/// Why an envelope could not be checked at all, apart from the verifier's
/// own results.
fn refusal(code: &str, message: &str) -> Value {
    json!({"ok": false, "code": code, "message": message})
}

fn asset_json(asset: &Asset) -> Value {
    let mut value = json!({
        "id": asset.id, "name": asset.name, "platform": asset.platform.name(),
        "arch": asset.arch.name(), "kind": asset.kind.name(), "size": asset.size,
        "sha256": asset.sha256,
    });
    if let (Some(floor), Some(object)) = (asset.os_floor, value.as_object_mut()) {
        object.insert("os_floor".into(), json!(floor));
    }
    value
}

/// Verifies `raw` for `request`: the JSON line and whether it verified.
fn check(raw: &[u8], request: &Request) -> (Value, bool) {
    let Some(key) = feed::decode_public_key(&request.public_key) else {
        return (
            refusal(
                "public_key_invalid",
                "The release feed's public key is not the base64 of 32 bytes.",
            ),
            false,
        );
    };
    // Without an installed version (a first install, or another product's
    // core) any well-formed version is accepted.
    let installed = request.installed_version.as_deref().unwrap_or("0.0.0");
    let expected = Expected {
        profile: &request.profile,
        channel: request.channel,
        installed_version: installed,
    };
    match feed::verify(raw, &key, &expected) {
        Ok(manifest) => (
            json!({
                "ok": true,
                "version": manifest.version,
                "download_hosts": manifest.download_hosts,
                "assets": manifest.assets.iter().map(asset_json).collect::<Vec<_>>(),
            }),
            true,
        ),
        Err(error) => (refusal(error.code(), message(error)), false),
    }
}

fn message(error: FeedError) -> &'static str {
    match error {
        FeedError::NotNewer => "The release feed has no release newer than the installed one.",
        FeedError::KeyIdRefused | FeedError::ProfileMismatch => {
            "The release feed's manifest is for another profile."
        }
        FeedError::ChannelMismatch => "The release feed's manifest is for another channel.",
        FeedError::SignatureInvalid => "The release feed's manifest signature does not verify.",
        _ => "The release feed's manifest could not be verified.",
    }
}

fn read_envelope(file: Option<&str>) -> Result<Vec<u8>, Value> {
    let mut raw = Vec::new();
    let read = match file {
        Some(path) => std::fs::File::open(path)
            .and_then(|stream| stream.take(MAX_ENVELOPE + 1).read_to_end(&mut raw)),
        None => std::io::stdin()
            .lock()
            .take(MAX_ENVELOPE + 1)
            .read_to_end(&mut raw),
    };
    match read {
        Err(_) => Err(refusal(
            "unreadable",
            "The release feed's envelope could not be read.",
        )),
        Ok(_) if raw.len() as u64 > MAX_ENVELOPE => Err(refusal(
            "too_large",
            "The release feed's envelope is larger than 1 MiB.",
        )),
        Ok(_) => Ok(raw),
    }
}

/// `feed verify ...` and `feed --help`; the exit status.
pub fn run(arguments: &[String]) -> i32 {
    let request = match parse(arguments) {
        Ok(Parsed::Help) => {
            print!("{USAGE}");
            return 0;
        }
        Ok(Parsed::Run(request)) => request,
        Err(problem) => {
            eprint!("{problem}\n\n{USAGE}");
            return 2;
        }
    };
    let (line, verified) = match read_envelope(request.file.as_deref()) {
        Ok(raw) => check(&raw, &request),
        Err(line) => (line, false),
    };
    let mut out = std::io::stdout().lock();
    if writeln!(out, "{line}").is_err() {
        return 1;
    }
    i32::from(!verified)
}

#[cfg(test)]
#[path = "feed_verify_tests.rs"]
mod tests;
