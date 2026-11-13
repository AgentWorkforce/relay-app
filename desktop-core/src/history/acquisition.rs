//! Session acquisition, the Desktop side: this installation's identity, the
//! grant it holds for each protected session, the grants a batch presents,
//! and the issuer calls that claim, verify and rebind them. An expired
//! grant is never renewed: its sessions stay held, renewal unsupported.
//!
//! Acquisition only applies to the sources the History service enrolled
//! (`acquisition.enrolledSources` in its published limits). For any other
//! source nothing here runs: no issuer call, no `acquisition` entry, no
//! hold. Desktop never enrols or activates anything.
//!
//! The installation and the grants live in [`store`], outside `history.db`,
//! so a local session reset keeps them while the origin rotates; a grant
//! bound to the old origin is then rebound through the issuer before the
//! session's records are presented again.
//!
//! Everything is kept per exact account: site origin, user, org and
//! workspace. Nothing stored for one account is used for another.
//!
//! A grant G and the installation proof are secrets: neither is ever logged
//! or shown. [`grant_sha256`] names a grant where a reference is needed.
pub mod contract;
mod issuer;
mod reconcile;
pub mod store;

pub use issuer::{AcquisitionIssuer, HttpIssuer, IssuerError};
pub use reconcile::{ensure_installation, reconcile, HeldSession, Reconciled};
pub use store::{
    Account, Enrolled, Grant, Installation, Observed, Operation, Reauthorization, Secret, State,
    Store, Watch,
};

use contract::{AcquisitionEntry, GRANT_HASH_DOMAIN, GRANT_PREFIX, PROOF_BYTES};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

/// Why records of an enrolled source are held before they are sent.
pub mod hold_reason {
    /// No grant is stored for the session yet.
    pub const NO_GRANT: &str = "no_grant";
    /// The stored grant is bound to an origin before a local reset.
    pub const ORIGIN_CHANGED: &str = "origin_changed";
    /// The issuer answered `not_authorized`: the signed-in session lacks
    /// `rth:acquire`, which a session gets at its next sign-in. Held
    /// sessions wait for sign-in with session ownership; nothing is paced
    /// or refused meanwhile, and the next sign-in retries.
    pub const SIGN_IN_REQUIRED: &str = "sign_in_required";
    /// The grant expired (`grant_expired`). Renewal is not supported: the
    /// session stays held and its status is refreshed; no new claim and no
    /// new installation is made for it.
    pub const RENEWAL_UNSUPPORTED: &str = "renewal_unsupported";
}

/// The exact account an install delivers to: its site origin, signed-in
/// user, org and workspace, as its validated `config.json` names them.
pub fn account_for(config: &crate::history::delivery_status::Config) -> Account {
    Account {
        site: config.site_url.clone(),
        user: config.account_id.clone(),
        org: config.org_id.clone(),
        workspace: config.workspace_id.clone(),
    }
}

/// What the status says while acquisition waits for a sign-in that carries
/// session ownership (`rth:acquire`).
pub const SIGN_IN_REQUIRED_MESSAGE: &str =
    "Sign in to Agent Relay again to enable session ownership. Held sessions wait and are preserved.";

/// Names the signed-in session an issuer call ran under, so a new sign-in
/// can be told apart without keeping the token. Never the token itself.
pub fn sign_in_fingerprint(access_token: &str) -> String {
    const DOMAIN: &str = "relay-desktop-acquisition-sign-in-v1";
    let joined = [DOMAIN, access_token].join("\u{1f}");
    format!("{:x}", Sha256::digest(joined.as_bytes()))
}

/// Whether acquisition for the exact `account` waits for a sign-in with
/// session ownership (see [`hold_reason::SIGN_IN_REQUIRED`]).
pub fn sign_in_required(store: &Store, account: &Account) -> bool {
    account.exact()
        && store
            .load()
            .is_ok_and(|state| state.reauthorization(account).is_some())
}

/// `lowerhex(SHA-256(join(U+001F, [domain, org, ws, source, session,
/// grant_id, G])))`. Inputs are taken byte for byte: nothing is trimmed or
/// case-folded, and an empty workspace keeps its place.
pub fn grant_sha256(
    org: &str,
    workspace: &str,
    source: &str,
    session_id: &str,
    grant_id: &str,
    grant: &str,
) -> String {
    let joined = [
        GRANT_HASH_DOMAIN,
        org,
        workspace,
        source,
        session_id,
        grant_id,
        grant,
    ]
    .join("\u{1f}");
    format!("{:x}", Sha256::digest(joined.as_bytes()))
}

/// A random (version 4) UUID, lowercase.
pub fn new_uuid() -> anyhow::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes)?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

/// A new installation proof: 32 CSPRNG bytes, base64url without padding.
pub fn new_proof() -> anyhow::Result<Secret> {
    use base64::Engine;
    let mut bytes = [0u8; PROOF_BYTES];
    getrandom::getrandom(&mut bytes)?;
    let proof = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    bytes.iter_mut().for_each(|byte| *byte = 0);
    std::hint::black_box(&bytes);
    Ok(Secret::new(proof))
}

/// A proof in its wire form: 43 base64url characters that decode to 32
/// bytes.
pub fn valid_proof(proof: &str) -> bool {
    use base64::Engine;
    proof.len() == contract::PROOF_LEN
        && base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(proof)
            .is_ok_and(|bytes| bytes.len() == PROOF_BYTES)
}

/// `rhg_` and 64 lowercase hex digits.
pub fn valid_grant(grant: &str) -> bool {
    grant.strip_prefix(GRANT_PREFIX).is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// A canonical lowercase UUID: 8-4-4-4-12 lowercase hex digits.
pub fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => matches!(byte, b'0'..=b'9' | b'a'..=b'f'),
        })
}

/// A positive decimal with no sign and no leading zero, within `u64`.
pub fn decimal_epoch(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('0')
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<u64>().is_ok()
}

/// What a batch carries for acquisition, or the sessions it must hold back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presentation {
    /// Send, with these entries (empty when no record is protected).
    Send(Vec<AcquisitionEntry>),
    /// Records of these sessions may not be sent yet.
    Hold(Vec<HeldSession>),
}

/// The grants a batch presents. `records` are the batch's (source,
/// session) pairs; only those of an `enrolled` source are protected. A
/// protected record without a session, without a usable grant, or whose
/// grant is bound to another origin holds its session.
pub fn present<'a>(
    records: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
    origin_id: &str,
    enrolled: &BTreeSet<String>,
    grant: impl Fn(&str, &str) -> Option<&'a Grant>,
) -> Presentation {
    let mut sessions = BTreeSet::new();
    let mut held = BTreeSet::new();
    for (source, session) in records {
        if !enrolled.contains(source) {
            continue;
        }
        let Some(session) = session else {
            held.insert((
                source.to_owned(),
                String::new(),
                contract::SESSION_REQUIRED.to_owned(),
            ));
            continue;
        };
        sessions.insert((source.to_owned(), session.to_owned()));
    }
    let mut entries = Vec::new();
    for (source, session) in sessions {
        let reason = match grant(&source, &session) {
            Some(grant) if !usable(grant) => hold_reason::NO_GRANT,
            Some(grant) if grant.origin_id != origin_id => hold_reason::ORIGIN_CHANGED,
            Some(grant) => {
                entries.push(AcquisitionEntry {
                    source,
                    session_id: session,
                    grant_id: grant.grant_id.clone(),
                    owner_epoch: grant.owner_epoch.to_string(),
                    grant: grant.grant.expose().to_owned(),
                });
                continue;
            }
            None => hold_reason::NO_GRANT,
        };
        held.insert((source, session, reason.to_owned()));
    }
    if held.is_empty() {
        // Sorted by (source, session) bytes, one per session: `sessions` is
        // an ordered set of owned strings, which compare bytewise.
        Presentation::Send(entries)
    } else {
        Presentation::Hold(
            held.into_iter()
                .map(|(source, session_id, reason)| HeldSession {
                    source,
                    session_id,
                    reason,
                })
                .collect(),
        )
    }
}

fn usable(grant: &Grant) -> bool {
    valid_grant(grant.grant.expose()) && valid_uuid(&grant.grant_id) && grant.owner_epoch > 0
}

/// Check a request's `acquisition` array against its batch: well-formed
/// entries, strictly sorted by (source, sessionId) bytes, each naming a
/// session the batch carries.
pub fn validate_entries<'a>(
    entries: &[AcquisitionEntry],
    batch_sessions: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> bool {
    let sessions: BTreeSet<(&str, &str)> = batch_sessions
        .into_iter()
        .filter_map(|(source, session)| Some((source, session?)))
        .collect();
    entries.iter().all(|entry| {
        valid_grant(&entry.grant)
            && valid_uuid(&entry.grant_id)
            && decimal_epoch(&entry.owner_epoch)
            && sessions.contains(&(entry.source.as_str(), entry.session_id.as_str()))
    }) && entries.windows(2).all(|pair| {
        (pair[0].source.as_bytes(), pair[0].session_id.as_bytes())
            < (pair[1].source.as_bytes(), pair[1].session_id.as_bytes())
    })
}

/// Permanently revoking a grant (`contract::REVOCATIONS_PATH`) is not
/// available from Desktop yet. Excluding a session is a local hold that
/// keeps its grant; a server revoke is permanent and only this explicit
/// operation may ever send one. It is a documented stub.
pub fn revoke_permanently(
    _account: &Account,
    _source: &str,
    _session_id: &str,
) -> Result<(), IssuerError> {
    Err(IssuerError::Unavailable)
}

#[cfg(test)]
mod tests;
