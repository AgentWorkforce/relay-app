//! The canonical transcript, its HMAC and the checks on the raw values a
//! challenge exchange carries (`docs/service-callers-auth.md` section 3).

use super::AUTH_PARAMS_MAX_BYTES;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// The fields of the canonical transcript (section 3), in order.
pub(crate) struct Transcript<'a> {
    pub(crate) version: u8,
    pub(crate) domain: &'a [u8],
    pub(crate) challenge_id: &'a [u8],
    pub(crate) nonce: &'a [u8],
    pub(crate) caller_id: &'a [u8],
    pub(crate) site_url: &'a [u8],
    pub(crate) workspace_id: &'a [u8],
    pub(crate) user_id: &'a [u8],
    pub(crate) issued_at_ms: u64,
    pub(crate) expires_at_ms: u64,
}

impl Transcript<'_> {
    /// The version byte, then each field as a big-endian u16 length and its
    /// bytes; the times as 8-byte big-endian integers.
    pub(crate) fn bytes(&self) -> Vec<u8> {
        fn field(out: &mut Vec<u8>, value: &[u8]) {
            let length = u16::try_from(value.len()).unwrap_or(u16::MAX);
            out.extend_from_slice(&length.to_be_bytes());
            out.extend_from_slice(&value[..usize::from(length)]);
        }
        let mut out = vec![self.version];
        for value in [
            self.domain,
            self.challenge_id,
            self.nonce,
            self.caller_id,
            self.site_url,
            self.workspace_id,
            self.user_id,
        ] {
            field(&mut out, value);
        }
        field(&mut out, &self.issued_at_ms.to_be_bytes());
        field(&mut out, &self.expires_at_ms.to_be_bytes());
        out
    }
}

#[cfg(test)]
thread_local! {
    /// HMAC computations on this thread, for the timing-class test.
    pub(crate) static HMAC_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// HMAC-SHA256 (RFC 2104) with a 32-byte key, over `sha2`.
pub(crate) fn hmac(key: &[u8; 32], message: &[u8]) -> [u8; 32] {
    #[cfg(test)]
    HMAC_CALLS.with(|calls| calls.set(calls.get() + 1));
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for (index, byte) in key.iter().enumerate() {
        ipad[index] ^= byte;
        opad[index] ^= byte;
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// 1 when `left` equals `right`, else 0, in time independent of where
/// they differ.
pub(super) fn same_mac(left: &[u8; 32], right: &[u8; 32]) -> u8 {
    let differ = left
        .iter()
        .zip(right)
        .fold(0u8, |differ, (a, b)| differ | (a ^ b));
    // 1 exactly when `differ` is 0.
    (((u16::from(differ)).wrapping_sub(1) >> 8) & 1) as u8
}

/// `K`: the 32 bytes the stored lowercase hex `SecretDigest.sha256` names.
pub(crate) fn key_of(sha256: &str) -> Option<[u8; 32]> {
    let bytes = sha256.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    };
    let mut key = [0u8; 32];
    for (index, pair) in bytes.chunks(2).enumerate() {
        key[index] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(key)
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// `sc_` and 16 lowercase hex characters.
pub(crate) fn is_caller_id(value: &str) -> bool {
    value
        .strip_prefix("sc_")
        .is_some_and(|hex| lower_hex(hex, 16))
}

/// `ch_` and 32 lowercase hex characters.
pub(crate) fn is_challenge_id(value: &str) -> bool {
    value
        .strip_prefix("ch_")
        .is_some_and(|hex| lower_hex(hex, 32))
}

/// The `authenticate` params, checked on the raw value: at most
/// [`AUTH_PARAMS_MAX_BYTES`], an object of exactly `challenge_id` and
/// `authenticator`, each in its format. `None` is `invalid_input`.
pub(crate) fn parse_authenticate(raw: &Value) -> Option<(String, [u8; 32])> {
    if serde_json::to_vec(raw).ok()?.len() > AUTH_PARAMS_MAX_BYTES {
        return None;
    }
    let object = raw.as_object()?;
    if object.len() != 2 {
        return None;
    }
    let challenge_id = object.get("challenge_id")?.as_str()?;
    let authenticator = object.get("authenticator")?.as_str()?;
    if !is_challenge_id(challenge_id) || authenticator.len() != 43 {
        return None;
    }
    let mac: [u8; 32] = URL_SAFE_NO_PAD
        .decode(authenticator)
        .ok()?
        .try_into()
        .ok()?;
    Some((challenge_id.to_string(), mac))
}

/// Whether the raw params carry a phase-1 credential key.
pub(crate) fn has_legacy_keys(raw: &Value) -> bool {
    raw.as_object().is_some_and(|object| {
        object.contains_key("caller_secret") || object.contains_key("caller_name")
    })
}
