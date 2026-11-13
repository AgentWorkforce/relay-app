//! The signed release feed of a `cloud` build: the verifier for one
//! envelope `{manifest, signature, key_id}` as Cloud serves it.
//!
//! **Proposed contract, not confirmed.** This follows the Cloud team's
//! draft contract (relay-control `docs/desktop-feed.md`, with its manifest
//! and envelope schemas and fixtures, copied under
//! `desktop-core/tests/fixtures/desktop-feed/`), which translates
//! `docs/enterprise-distribution.md`. The Cloud team decides it and it is
//! not confirmed yet, so nothing here is a stable contract: the encodings,
//! the `key_id` shape, the result names and the check order may all change.
//! The GTK updater (`gtk/src/updates/cloud.rs`) and `agent-relay-probe feed
//! verify` (`crate::feed_verify`) share it.
//!
//! [`verify`] runs the design's checks in order, on the raw bytes:
//!
//! 1. Strict I-JSON ([`strict_json`]): a duplicate name at any depth, a lone
//!    surrogate, a byte order mark or anything outside RFC 8259 is
//!    [`FeedError::SchemaInvalid`] before any parse.
//! 2. The envelope: exactly `manifest`, `signature` (base64url without
//!    padding, 86 characters) and `key_id` (`fk_<profile>_<label>`); then
//!    a `key_id` for another profile is [`FeedError::KeyIdRefused`] before
//!    any verification.
//! 3. Ed25519 (RFC 8032) over the RFC 8785 bytes ([`jcs`]) of `manifest`
//!    as received, with the compiled key.
//! 4. The manifest's schema, then its profile, channel, unique assets and
//!    a version newer than the installed one.
//!
//! Asset selection and the download host rule (steps 3 and 4 of the
//! design) are [`Manifest::select`] and [`download_host_allowed`].

pub mod jcs;
pub mod manifest;
pub mod select;
#[cfg(test)]
mod select_tests;
pub mod strict_json;
#[cfg(test)]
mod tests;
mod version;

use std::fmt;

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::Value;

pub use manifest::{Arch, Asset, Channel, Kind, Manifest, Platform};
pub use select::download_host_allowed;
pub use version::{is_newer, version as release_version};

/// Why an envelope was refused. Each variant is one result name of the
/// draft's fixtures ([`FeedError::code`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedError {
    /// `schema_invalid`: not strict I-JSON, or the envelope or manifest
    /// breaks its schema.
    SchemaInvalid,
    /// `signature_invalid`: the signature does not verify with the compiled
    /// key.
    SignatureInvalid,
    /// `key_id_refused`: the envelope names another profile's key.
    KeyIdRefused,
    /// `profile_mismatch`: the manifest is for another profile.
    ProfileMismatch,
    /// `channel_mismatch`: the manifest is for another channel.
    ChannelMismatch,
    /// `duplicate_asset`: two assets share an id or a name.
    DuplicateAsset,
    /// `not_newer`: the version is not newer than the installed one.
    NotNewer,
    /// `no_asset`: no asset for this machine.
    NoAsset,
    /// `ambiguous_asset`: more than one asset for this machine.
    AmbiguousAsset,
    /// `asset_name_mismatch`: the asset's name is not the one this build
    /// expects for the version.
    AssetNameMismatch,
}

impl FeedError {
    /// The result name the draft's fixture cases use.
    pub fn code(self) -> &'static str {
        match self {
            Self::SchemaInvalid => "schema_invalid",
            Self::SignatureInvalid => "signature_invalid",
            Self::KeyIdRefused => "key_id_refused",
            Self::ProfileMismatch => "profile_mismatch",
            Self::ChannelMismatch => "channel_mismatch",
            Self::DuplicateAsset => "duplicate_asset",
            Self::NotNewer => "not_newer",
            Self::NoAsset => "no_asset",
            Self::AmbiguousAsset => "ambiguous_asset",
            Self::AssetNameMismatch => "asset_name_mismatch",
        }
    }

    /// What the update UI says. A manifest that fails verification in any
    /// way is "could not be verified", as the design's table says for
    /// signature and schema failures.
    pub fn message(self) -> &'static str {
        match self {
            Self::NotNewer => "There is no newer update.",
            Self::NoAsset | Self::AmbiguousAsset => "There is no update for this computer.",
            _ => "The update could not be verified.",
        }
    }
}

impl fmt::Display for FeedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for FeedError {}

/// What the build expects of a manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Expected<'a> {
    /// The compiled profile id.
    pub profile: &'a str,
    /// The channel asked for.
    pub channel: Channel,
    /// The running version, which the manifest's must be newer than.
    pub installed_version: &'a str,
}

/// Decodes the compiled `DESKTOP_FEED_PUBLIC_KEY`: standard base64 of the
/// 32-byte Ed25519 public key.
pub fn decode_public_key(text: &str) -> Option<[u8; 32]> {
    STANDARD.decode(text).ok()?.try_into().ok()
}

/// Verifies the envelope `raw` against the compiled `key` and `expected`,
/// in the order the module documentation lists.
pub fn verify(raw: &[u8], key: &[u8; 32], expected: &Expected<'_>) -> Result<Manifest, FeedError> {
    strict_json::check(raw).map_err(|_| FeedError::SchemaInvalid)?;
    let envelope: Value = serde_json::from_slice(raw).map_err(|_| FeedError::SchemaInvalid)?;
    let (manifest, signature, key_id) = envelope_fields(&envelope)?;
    if key_id_profile(key_id) != Some(expected.profile) {
        return Err(FeedError::KeyIdRefused);
    }
    // A number JCS cannot write was refused by the strict pass already.
    let canonical = jcs::canonicalize(manifest).map_err(|_| FeedError::SchemaInvalid)?;
    let key = VerifyingKey::from_bytes(key).map_err(|_| FeedError::SignatureInvalid)?;
    key.verify_strict(&canonical, &signature)
        .map_err(|_| FeedError::SignatureInvalid)?;

    let manifest = Manifest::from_value(manifest)?;
    if manifest.profile != expected.profile {
        return Err(FeedError::ProfileMismatch);
    }
    if manifest.channel != expected.channel {
        return Err(FeedError::ChannelMismatch);
    }
    manifest.check_unique_assets()?;
    if !is_newer(&manifest.version, expected.installed_version) {
        return Err(FeedError::NotNewer);
    }
    Ok(manifest)
}

/// The envelope's three members, with the signature decoded; anything
/// else, or a malformed signature or `key_id`, is schema-invalid.
fn envelope_fields(envelope: &Value) -> Result<(&Value, Signature, &str), FeedError> {
    let object = envelope.as_object().ok_or(FeedError::SchemaInvalid)?;
    let (Some(manifest), Some(Value::String(signature)), Some(Value::String(key_id)), 3) = (
        object.get("manifest"),
        object.get("signature"),
        object.get("key_id"),
        object.len(),
    ) else {
        return Err(FeedError::SchemaInvalid);
    };
    if !manifest.is_object() || key_id_profile(key_id).is_none() {
        return Err(FeedError::SchemaInvalid);
    }
    Ok((manifest, decode_signature(signature)?, key_id))
}

/// `^[A-Za-z0-9_-]{85}[AQgw]$`: 64 bytes of base64url without padding,
/// whose last character carries two data bits and four zero bits.
fn decode_signature(text: &str) -> Result<Signature, FeedError> {
    let alphabet = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-');
    let canonical_tail = matches!(text.as_bytes().last(), Some(b'A' | b'Q' | b'g' | b'w'));
    if text.len() != 86 || !text.bytes().all(alphabet) || !canonical_tail {
        return Err(FeedError::SchemaInvalid);
    }
    let bytes: [u8; 64] = URL_SAFE_NO_PAD
        .decode(text)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(FeedError::SchemaInvalid)?;
    Ok(Signature::from_bytes(&bytes))
}

/// The profile a well-formed `key_id` (`^fk_[a-z0-9]{1,32}_[a-z0-9]{1,16}$`)
/// names.
fn key_id_profile(key_id: &str) -> Option<&str> {
    let (profile, label) = key_id.strip_prefix("fk_")?.split_once('_')?;
    let label_ok = (1..=16).contains(&label.len())
        && label
            .bytes()
            .all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9'));
    (manifest::valid_profile(profile) && label_ok).then_some(profile)
}
