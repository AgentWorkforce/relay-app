//! Every wire shape of session acquisition, and nothing else. No other module
//! names a path, a header, a field or a refusal code of the exchange: they
//! read these types and constants.
//!
//! PROPOSED native exchange contract v1 (relayhistory-pipeline): the routes,
//! bodies and refusal codes below bind unless the backend change that
//! registers them says otherwise. The installation proof (addendum P3a) is
//! FINAL for v1.
//!
//! Pinned EXACTLY to relayhistory-pipeline PR 331 (a draft, not final) at
//! head 680d26b8bf7eed0fbc9b6650ef4d4a420e727b7d,
//! docs/acquisition-exchange.md ("Native wire contract", "401 vs 403").
//! At that head:
//!
//! - `403 not_authorized` is the bearer/scope check only, on every native
//!   route: the one 403 a normal sign-in cures ([`NOT_AUTHORIZED`]).
//! - `403 not_owner` and `403 not_installer` (route ownership on rebinds,
//!   and SQL 42501 codes, `not_installer` being the installer-role guard)
//!   are per session; no sign-in cures them ([`NOT_OWNER`],
//!   [`NOT_INSTALLER`]). Since the role guard can refuse a valid grant,
//!   `not_installer` alone never drops one: the scoped status decides.
//! - A status `ownerEpoch` is `null` or a positive decimal string
//!   ([`SessionStatus::owner_epoch`]); claim and rebind success epochs are
//!   always positive ([`ClaimResponse::owner_epoch`],
//!   [`RebindResponse::owner_epoch`]).
//! - A claim whose install has no positive epoch yet is not answered with
//!   one: it is `503` [`ACQUISITION_UNAVAILABLE`] (retryable), which the
//!   reconcile takes as a transport failure: the session is held and the
//!   same immutable operation (same `operationUuid`, same request) is
//!   replayed on the next pass.
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeSet;

/// Registers this installation for the signed-in user.
pub const INSTALLATIONS_PATH: &str = "/v1/acquisition/native/installations";
/// Claims a fresh session for this installation and returns its grant.
pub const CLAIMS_PATH: &str = "/v1/acquisition/native/claims";
/// Permanently revokes a grant. Only the explicit permanent-revoke operation
/// would call it; it is not wired (see [`super::revoke_permanently`]).
pub const REVOCATIONS_PATH: &str = "/v1/acquisition/native/revocations";
/// Binds a held grant to this installation's new origin after a local reset.
pub const REBINDS_PATH: &str = "/v1/acquisition/native/rebinds";
/// Reports who owns a set of sessions of one source.
pub const STATUS_PATH: &str = "/v1/acquisition/native/status";
/// Publishes the enrolled sources, as `acquisition.enrolledSources`.
pub const LIMITS_PATH: &str = "/v1/delivery/limits";

/// The installation proof in the registration body.
pub const PROOF_FIELD: &str = "installationProof";
/// The installation proof on every other native route, beside the Bearer.
pub const PROOF_HEADER: &str = "X-Acquisition-Installation-Proof";
/// The proof is 32 CSPRNG bytes, base64url without padding: 43 characters.
pub const PROOF_BYTES: usize = 32;
pub const PROOF_LEN: usize = 43;

/// Domain separator of the grant digest.
pub const GRANT_HASH_DOMAIN: &str = "relayhistory-acquisition-grant-v1";
/// The prefix of a grant, followed by 64 lowercase hex digits.
pub const GRANT_PREFIX: &str = "rhg_";

/// The error code of a claim refusal and of a writer refusal of a batch.
pub const ACQUISITION_REFUSED: &str = "acquisition_refused";
/// Same `requestUuid`, different proof.
pub const INSTALLATION_PROOF_MISMATCH: &str = "installation_proof_mismatch";
/// Unknown installation, or a proof that does not match it.
pub const INSTALLATION_NOT_PROVEN: &str = "installation_not_proven";
/// 403 from the bearer check, and only from it, on every native route
/// (rebinds included): a valid bearer without `rth:acquire` (or not a
/// desktop user, or without a workspace). The only 403 a new sign-in can
/// cure: Desktop asks for one normal sign-in and retries under it. The
/// service never uses it for a proof or an ownership refusal.
pub const NOT_AUTHORIZED: &str = "not_authorized";
/// 403 on a revocation or rebind of a grant id this installation did not
/// install (unknown, not an install, or another installation's); also an
/// SQL 42501 code (the installer-role guard) on any route, which refuses a
/// valid grant while the service's database role is misconfigured. Per
/// session, never a sign-in. On its own it does not prove the stored grant
/// is not this installation's: a rebind's grant is kept, unpresented, until
/// the scoped status disowns it (then a fresh claim) or shows it still this
/// installation's active grant (then a paced rebind).
pub const NOT_INSTALLER: &str = "not_installer";
/// 503, retryable: the database or transport failed (the operation stays
/// pending at the service), a replay arrived before the operation's
/// backoff or while another sender holds it, or a claim's install has no
/// positive epoch yet. Always the same operation again, never a new one.
pub const ACQUISITION_UNAVAILABLE: &str = "acquisition_unavailable";
pub const INVALID_REQUEST: &str = "invalid_request";

/// Owner states a status reports.
pub const OWNER_ACTIVE: &str = "active";
pub const OWNER_DRAINING: &str = "draining";
pub const OWNER_RELEASED: &str = "released";
pub const OWNER_UNKNOWN: &str = "unknown";

/// Claim outcomes, and rebind outcomes.
pub const OUTCOME_INSTALLED: &str = "installed";
pub const OUTCOME_REBOUND: &str = "rebound";
pub const OUTCOME_REPLAY: &str = "replay";

/// Issuer refusal codes (`reason` of an `acquisition_refused`) that wait on
/// the session's owner: nothing a new operation could change, so the
/// session's status is polled on the hold cadence and only a change of it
/// plans another operation.
pub const OWNER_WAIT_CODES: [&str; 8] = [
    "owner_draining",
    "owner_released",
    "owner_changed",
    "owner_other_principal",
    NOT_OWNER,
    "handover_not_ready",
    "drain_not_ready",
    "revoked_before_install",
];
/// Issuer refusal codes that end an operation even when sent as retryable
/// (spec 4.2: terminal though `40001`). Only `session_busy` and transport
/// failures are retried, by exact replay.
pub const TERMINAL_CODES: [&str; 6] = [
    "stale_revision",
    "revision_cas_lost",
    "predecessor_mismatch",
    "operation_conflict",
    "origin_cas_lost",
    OWNER_UNKNOWN_REFUSAL,
];
/// The issuer's `409 acquisition_refused` reason for a session whose state
/// is unknown, or whose epoch is 0 with prior history. Such a session can
/// hold no live grant and waiting never changes that: only a fresh install
/// writes an owner. Terminal, never an owner wait: a rebind refused so
/// drops that grant and claims the session afresh at once; a claim refused
/// so is refused and paced like any other terminal code.
pub const OWNER_UNKNOWN_REFUSAL: &str = "owner_unknown";

/// Writer refusal codes (per refused session) the reconcile acts on. Every
/// other code, and these when there is nothing local to act with, holds.
pub const GRANT_EXPIRED: &str = "grant_expired";
/// Also the issuer's 403 when its owner check refuses one session (another
/// installation owns it; on a rebind also a grant whose install was refused
/// or is void, or a rebind without the session's G): an owner wait of that
/// session alone, never a sign-in.
pub const NOT_OWNER: &str = "not_owner";
pub const SESSION_REQUIRED: &str = "session_required";
/// Every writer refusal code of v1, for validation of the refused list.
pub const WRITER_REFUSAL_CODES: [&str; 8] = [
    "owner_unknown",
    "owner_draining",
    "owner_released",
    "owner_changed",
    NOT_OWNER,
    "owner_other_principal",
    GRANT_EXPIRED,
    SESSION_REQUIRED,
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationRequest {
    pub request_uuid: String,
    pub origin_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationResponse {
    pub installation_id: String,
    pub origin_id: String,
    #[serde(default)]
    pub issuer_key_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimRequest {
    pub operation_uuid: String,
    pub installation_id: String,
    pub source: String,
    pub session_id: String,
    pub origin_id: String,
}
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimResponse {
    /// `installed` or `replay`.
    pub outcome: String,
    pub grant_id: String,
    /// Always positive. The service never answers a claim without a
    /// positive epoch (it answers `503` [`ACQUISITION_UNAVAILABLE`] and
    /// keeps the install pending), so `0` or a missing epoch is unreadable
    /// here, and the operation is replayed as after a transport failure.
    #[serde(deserialize_with = "epoch")]
    pub owner_epoch: u64,
    #[serde(default)]
    pub revision: Option<serde_json::Value>,
    /// Null in v1; kept optional.
    #[serde(default)]
    pub expires_at: Option<String>,
    pub grant: String,
}
impl std::fmt::Debug for ClaimResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaimResponse")
            .field("outcome", &self.outcome)
            .field("grant_id", &self.grant_id)
            .field("owner_epoch", &self.owner_epoch)
            .field("grant", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RebindRequest {
    pub operation_uuid: String,
    pub installation_id: String,
    pub source: String,
    pub session_id: String,
    pub grant_id: String,
    pub origin_id: String,
    /// The session's grant G (spec 4.7): the proof alone does not rebind.
    pub grant: String,
}
impl std::fmt::Debug for RebindRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RebindRequest")
            .field("source", &self.source)
            .field("session_id", &self.session_id)
            .field("grant_id", &self.grant_id)
            .field("grant", &"<redacted>")
            .finish()
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RebindResponse {
    /// `rebound` or `replay`.
    pub outcome: String,
    #[serde(default)]
    pub revision: Option<serde_json::Value>,
    /// Always positive, as on a claim (else the service answers `503`).
    #[serde(deserialize_with = "epoch")]
    pub owner_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevocationRequest {
    pub operation_uuid: String,
    pub installation_id: String,
    pub source: String,
    pub session_id: String,
    pub target_grant_id: String,
}

/// The 200 body of the native status: `{result:{sessions:[...]}}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StatusEnvelope {
    pub result: StatusResponse,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResponse {
    pub sessions: Vec<SessionStatus>,
}
/// One session of a status answer. Bigint fields arrive as decimal strings
/// (a JSON integer is tolerated); fields this client does not read are
/// ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionStatus {
    pub session_id: String,
    /// [`OWNER_ACTIVE`], [`OWNER_DRAINING`], `released` or `unknown`.
    pub state: String,
    #[serde(default, deserialize_with = "optional_decimal")]
    pub revision: Option<u64>,
    #[serde(default, deserialize_with = "optional_decimal")]
    pub tombstone_revision: Option<u64>,
    /// `None` when the session has no owner. Canonically `null` (never
    /// owned, or a session the authority has no row for) or a positive
    /// decimal string. `"0"` (or `0`) is tolerated, read as no owner and
    /// never as an epoch, but the current backend never produces it.
    #[serde(default, deserialize_with = "status_epoch")]
    pub owner_epoch: Option<u64>,
    #[serde(default)]
    pub owner_kind: Option<String>,
    /// `None` unless `mine`: the grant of another installation is never
    /// named to this one.
    #[serde(default)]
    pub grant_id: Option<String>,
    /// Whether the owner is this installation. `None` when the row leaves
    /// it out (or sends `null`): that is unknown, never "not mine", because
    /// "not mine" with an owner epoch is read as another installation's
    /// owner, which drops a quarantined grant. Only an explicit `false` may
    /// drive that destructive decision.
    #[serde(default)]
    pub mine: Option<bool>,
}
/// The most sessions one status request names.
pub const STATUS_MAX_SESSIONS: usize = 500;
/// The longest request target (path, `?` and query) the service accepts;
/// a longer one is `414 request_uri_too_long`.
pub const STATUS_MAX_TARGET: usize = 8 * 1024;

/// The scoped status request: `GET` [`StatusRequest::target`] with no body,
/// the installation proof in [`PROOF_HEADER`] and the Bearer that scopes the
/// org and workspace, as on every other native route.
///
/// Wire per the DRAFT native wire contract (relayhistory-pipeline PR 331,
/// head 680d26b8bf7eed0fbc9b6650ef4d4a420e727b7d,
/// docs/acquisition-exchange.md "Native wire contract"; a draft, not
/// final): query `installationId`, `source` and one `sessionIds`
/// parameter per id. Ids are opaque and never comma-joined. Build it only
/// with [`status_request`], which enforces both bounds; there is no other
/// shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusRequest {
    pub installation_id: String,
    pub source: String,
    pub session_ids: Vec<String>,
}
impl StatusRequest {
    /// The request target: [`STATUS_PATH`], `?` and the query, every value
    /// percent-encoded (a space as `%20`, never `+`).
    pub fn target(&self) -> String {
        let mut target = format!(
            "{STATUS_PATH}?installationId={}&source={}",
            query_value(&self.installation_id),
            query_value(&self.source)
        );
        for session_id in &self.session_ids {
            target.push_str("&sessionIds=");
            target.push_str(&query_value(session_id));
        }
        target
    }
}
fn query_value(value: &str) -> String {
    // `byte_serialize` writes a space as `+` and a literal `+` as `%2B`, so
    // every `+` it writes is a space.
    url::form_urlencoded::byte_serialize(value.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}
/// The one status request (see [`StatusRequest`]). Duplicate ids are
/// dropped (the first occurrence keeps its place), since the service
/// refuses a request that repeats one. `None` with nothing to ask about,
/// past [`STATUS_MAX_SESSIONS`] distinct ids, or when the encoded target
/// would exceed [`STATUS_MAX_TARGET`] bytes, whichever bound is hit first.
pub fn status_request(
    installation_id: &str,
    source: &str,
    session_ids: &[String],
) -> Option<StatusRequest> {
    let mut seen = BTreeSet::new();
    let session_ids: Vec<String> = session_ids
        .iter()
        .filter(|session_id| seen.insert(session_id.as_str()))
        .cloned()
        .collect();
    let request = (!installation_id.is_empty()
        && !source.is_empty()
        && !session_ids.is_empty()
        && session_ids.len() <= STATUS_MAX_SESSIONS)
        .then(|| StatusRequest {
            installation_id: installation_id.to_owned(),
            source: source.to_owned(),
            session_ids,
        })?;
    (request.target().len() <= STATUS_MAX_TARGET).then_some(request)
}

/// A refusal body: `{error:{code, reason?, retryable?}}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ErrorDetail {
    pub code: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub retryable: bool,
}

/// The writer's 409 for a batch:
/// `{error:{code:"acquisition_refused"}, refused:[{source,sessionId,code}]}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WriterRefusal {
    pub error: WriterRefusalError,
    pub refused: Vec<RefusedSession>,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WriterRefusalError {
    pub code: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefusedSession {
    pub source: String,
    /// `""` with `session_required`: the source's records without a session.
    pub session_id: String,
    pub code: String,
}
/// The refused sessions of a writer 409, or `None` when the body is not an
/// `acquisition_refused` refusal with a well-formed, non-empty list.
pub fn writer_refusal(body: &serde_json::Value) -> Option<Vec<RefusedSession>> {
    let refusal = WriterRefusal::deserialize(body).ok()?;
    (refusal.error.code == ACQUISITION_REFUSED
        && !refusal.refused.is_empty()
        && refusal.refused.iter().all(|refused| {
            WRITER_REFUSAL_CODES.contains(&refused.code.as_str())
                && !refused.source.is_empty()
                && (refused.session_id.is_empty() == (refused.code == SESSION_REQUIRED))
        }))
    .then_some(refusal.refused)
}

/// One entry of a batch claim's `acquisition` array.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcquisitionEntry {
    pub source: String,
    pub session_id: String,
    pub grant_id: String,
    /// A positive decimal string with no leading zeros.
    pub owner_epoch: String,
    pub grant: String,
}
impl std::fmt::Debug for AcquisitionEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcquisitionEntry")
            .field("source", &self.source)
            .field("session_id", &self.session_id)
            .field("grant_id", &self.grant_id)
            .field("owner_epoch", &self.owner_epoch)
            .field("grant", &"<redacted>")
            .finish()
    }
}

/// The sources the server enrolled, from a limits response:
/// `acquisition.enrolledSources`. `None` when the response does not say;
/// an empty set when it says none.
pub fn enrolled_sources(limits: &serde_json::Value) -> Option<BTreeSet<String>> {
    let sources = limits
        .get("acquisition")?
        .get("enrolledSources")?
        .as_array()?;
    sources
        .iter()
        .map(|source| {
            source
                .as_str()
                .filter(|source| !source.is_empty())
                .map(str::to_owned)
        })
        .collect()
}

fn epoch<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    optional_epoch(deserializer)?.ok_or_else(|| serde::de::Error::custom("ownerEpoch required"))
}
/// An owner epoch as a JSON integer or a decimal string, positive.
fn optional_epoch<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let epoch = match value {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(serde_json::Value::Number(number)) => number.as_u64(),
        Some(serde_json::Value::String(text)) if super::decimal_epoch(&text) => text.parse().ok(),
        Some(_) => None,
    };
    epoch
        .filter(|epoch| *epoch > 0)
        .map(Some)
        .ok_or_else(|| serde::de::Error::custom("invalid ownerEpoch"))
}
/// A status `ownerEpoch`: canonically `null` (no owner) or a positive
/// decimal string, read as [`optional_epoch`]. `"0"` (or `0`) is tolerated
/// as no owner only (the current backend answers `null` instead, never
/// `"0"`). Only the status reads it so: a claim or rebind epoch of 0 is
/// still refused, and an owner of `None` never matches a stored grant,
/// whose epoch is positive.
fn status_epoch<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    match value {
        Some(serde_json::Value::String(text)) if text == "0" => Ok(None),
        Some(serde_json::Value::Number(number)) if number.as_u64() == Some(0) => Ok(None),
        value => optional_epoch(value.unwrap_or(serde_json::Value::Null))
            .map_err(serde::de::Error::custom),
    }
}
/// A non-negative bigint (`revision`, `tombstoneRevision`) as a decimal
/// string without leading zeros, or a JSON integer.
fn optional_decimal<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    let decimal = match value {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(serde_json::Value::Number(number)) => number.as_u64(),
        Some(serde_json::Value::String(text)) if text == "0" || super::decimal_epoch(&text) => {
            text.parse().ok()
        }
        Some(_) => None,
    };
    decimal
        .map(Some)
        .ok_or_else(|| serde::de::Error::custom("invalid revision"))
}
