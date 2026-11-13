//! `AGENT_RELAY_HISTORY_UPLOAD_STORAGE_ORIGINS`, the storage hosts a site
//! build's direct uploads may PUT to on a private address (a store on the
//! organization's private DNS), as `build.rs` checks it and hands it to the
//! crate. `build.rs` includes this file by path, so it depends on nothing
//! but `std` and `serde_json`; the crate compiles it for its tests.
//!
//! Messages name the variable and the problem, never a value.

use serde_json::Value;
use std::collections::BTreeSet;

/// The build-time variable, named in its messages.
pub const KEY: &str = "AGENT_RELAY_HISTORY_UPLOAD_STORAGE_ORIGINS";

/// Why the list was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OriginsError {
    /// Not JSON.
    NotJson,
    /// JSON, but not an array of strings.
    NotArray,
    /// An entry that is not an https origin.
    NotHttps,
    /// An entry with user info.
    Credentials,
    /// An entry with a path other than `/`, a query or a fragment.
    NotOrigin,
    /// An entry whose host is no DNS name (an IP literal included: those
    /// are never held to the address rule, so listing one means nothing).
    Host,
    /// An entry whose port is no number from 1 to 65535.
    Port,
    /// A non-empty list outside a site build.
    NotSiteBuild,
}

impl std::fmt::Display for OriginsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let problem = match self {
            OriginsError::NotJson => "is not valid JSON",
            OriginsError::NotArray => "must be a JSON array of strings",
            OriginsError::NotHttps => "may list only https origins",
            OriginsError::Credentials => "has an origin with user info",
            OriginsError::NotOrigin => {
                "has an entry with a path, query or fragment; list origins only"
            }
            OriginsError::Host => "has an origin whose host is no DNS name",
            OriginsError::Port => "has an origin whose port is no number from 1 to 65535",
            OriginsError::NotSiteBuild => {
                "is set, but this is no site build: only a build whose AGENT_RELAY_SITE_URL names a site other than https://agentrelay.com takes upload storage origins"
            }
        };
        write!(f, "{KEY} {problem}")
    }
}

impl std::error::Error for OriginsError {}

/// The list read: empty (or only whitespace) is none; anything else must
/// be a JSON array of origins, each canonicalised ([`canonical_origin`]),
/// duplicates folded.
pub fn parse(text: &str) -> Result<BTreeSet<String>, OriginsError> {
    if text.trim().is_empty() {
        return Ok(BTreeSet::new());
    }
    let value: Value = serde_json::from_str(text).map_err(|_| OriginsError::NotJson)?;
    let Value::Array(entries) = value else {
        return Err(OriginsError::NotArray);
    };
    entries
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .ok_or(OriginsError::NotArray)
                .and_then(canonical_origin)
        })
        .collect()
}

/// `raw` as `https://host[:port]`: scheme and host lowercased, the default
/// port dropped, one trailing `/` allowed. Anything else is refused: another
/// scheme, user info, a path, query or fragment, an IP literal, a host that
/// is no DNS name, a port out of range.
pub fn canonical_origin(raw: &str) -> Result<String, OriginsError> {
    let raw = raw.trim();
    let (scheme, rest) = raw.split_once("://").ok_or(OriginsError::NotHttps)?;
    if !scheme.eq_ignore_ascii_case("https") {
        return Err(OriginsError::NotHttps);
    }
    if rest.contains(['?', '#']) {
        return Err(OriginsError::NotOrigin);
    }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if !path.is_empty() {
        return Err(OriginsError::NotOrigin);
    }
    if authority.contains('@') {
        return Err(OriginsError::Credentials);
    }
    if authority.starts_with('[') {
        return Err(OriginsError::Host);
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port_number(port)?)),
        None => (authority, None),
    };
    let host = host.to_ascii_lowercase();
    if !dns_name(&host) {
        return Err(OriginsError::Host);
    }
    Ok(match port {
        Some(port) if port != 443 => format!("https://{host}:{port}"),
        _ => format!("https://{host}"),
    })
}

/// A port as written: digits only, 1 to 65535.
fn port_number(port: &str) -> Result<u16, OriginsError> {
    port.bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| port.parse::<u16>().ok())
        .flatten()
        .filter(|&port| port != 0)
        .ok_or(OriginsError::Port)
}

/// A lowercase DNS name: labels of letters, digits and inner hyphens, no
/// empty label (a trailing dot included), and a last label that is no
/// number (decimal or `0x` hex), which a URL reads as an IPv4 address.
fn dns_name(host: &str) -> bool {
    let label = |label: &str| {
        (1..=63).contains(&label.len())
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    let last = host.rsplit('.').next().unwrap_or_default();
    let number = last.bytes().all(|b| b.is_ascii_digit())
        || last
            .strip_prefix("0x")
            .is_some_and(|hex| hex.bytes().all(|b| b.is_ascii_hexdigit()));
    host.len() <= 253 && host.split('.').all(label) && !number
}

/// What `build.rs` hands the crate as `DESKTOP_HISTORY_UPLOAD_STORAGE_ORIGINS_JSON`:
/// the list read, refused when it is not empty and this is no site build,
/// as a compact sorted JSON array (`[]` for none).
pub fn compiled(text: &str, site_build: bool) -> Result<String, OriginsError> {
    let origins = parse(text)?;
    if !origins.is_empty() && !site_build {
        return Err(OriginsError::NotSiteBuild);
    }
    serde_json::to_string(&origins).map_err(|_| OriginsError::NotArray)
}
