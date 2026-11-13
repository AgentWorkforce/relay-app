//! Versioned durable transport. The local coordinator persists prepared bytes and owns retries.
use crate::history::acquisition::contract::AcquisitionEntry;
use crate::history::cloud;
use crate::history::delivery::{
    worker::{
        LimitsContext, PreparedBody, Receiver, ReceiverContext, ReceiverFailure, MAX_IN_FLIGHT,
    },
    AcceptanceLevel, ConflictRecovery, DeliveryAcknowledgment, DeliveryFailure, HeldSession,
    HistoryExportBatch, IsolationTask, PreparedPayload, PublishedLimits, StoredUpload,
    MAX_PUBLISHED_BATCH_RECORDS, RETIRED_KINDS, SUPPORTED_KINDS,
};
use crate::history::migration::MigrationStatus;
mod legacy_schedules;
pub use legacy_schedules::{
    legacy_uploader_message, LegacySchedulesHeld, LEGACY_SCHEDULES_UNKNOWN_MESSAGE,
    SCHEDULER_GUARD_TTL,
};
mod command;
use command::{command_answer, command_budget, read_answer, response_json, AnswerFailure};
pub(crate) mod conflict;
pub mod encoding;
mod guard;
mod payload;
use anyhow::Result;
use encoding::{published_encoding, ContentEncoding, WireBody};
pub(crate) use payload::sanitize_payload;
use payload::storable_body;
#[cfg(test)]
use payload::JSON_TEXT_TRUNCATION_MARKER;
mod selection;
use selection::{selection_answer, selection_error};
mod acquisition;
mod stage;
use stage::{SessionCache, Stage};
#[cfg(test)]
mod acquisition_tests;
use acquisition::{entries_for, presented, validate_request, writer_hold};
mod capabilities;
mod rejection;
pub use capabilities::{require_session_deletion, require_session_reset};
mod reset;
pub use reset::{ResetUnsupported, RESET_UNSUPPORTED_MESSAGE};
mod retry_after;
mod transport;
use rejection::{error_body_bytes, parse_error_body, rejection_log, safe_error_code};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeSet, HashSet},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};
use transport::*;
pub const MAPPING_VERSION: &str = "relayhistory-delivery-v1";
pub const MAX_BODY_BYTES: usize = 2_097_152;
/// The protocol's record cap per batch: the most records a batch is prepared
/// with until the receiver publishes another limit.
pub const MAX_BATCH_RECORDS: usize = crate::history::delivery::DEFAULT_BATCH_RECORDS;
const MAX_RESPONSE_BYTES: u64 = 8_388_608;
/// The kinds a batch may carry: every kind the probe drains, and the retired
/// ones too, so a batch an older build prepared before it stopped draining
/// them is still sent rather than blocking its job. No new batch holds one.
static DELIVERABLE_KINDS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    SUPPORTED_KINDS
        .iter()
        .chain(RETIRED_KINDS)
        .copied()
        .collect()
});

#[derive(Debug)]
pub struct TransportFailure(pub DeliveryFailure);
impl std::fmt::Display for TransportFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.code())
    }
}
impl std::error::Error for TransportFailure {}
impl TransportFailure {
    pub fn code(&self) -> &'static str {
        match self.0 {
            DeliveryFailure::Transient => "DELIVERY_TRANSIENT",
            DeliveryFailure::RateLimited => "DELIVERY_RATE_LIMITED",
            DeliveryFailure::AuthenticationRequired => "DELIVERY_AUTHENTICATION_REQUIRED",
            DeliveryFailure::PermissionDenied => "DELIVERY_PERMISSION_DENIED",
            DeliveryFailure::InvalidPayload => "DELIVERY_INVALID_PAYLOAD",
            DeliveryFailure::ReceiverRejected => "DELIVERY_RECEIVER_REJECTED",
            DeliveryFailure::UnsupportedEvidence => "DELIVERY_UNSUPPORTED_EVIDENCE",
            DeliveryFailure::MappingVersionMismatch => "DELIVERY_MAPPING_VERSION_MISMATCH",
            DeliveryFailure::DeliveryConflict => "DELIVERY_CONFLICT",
            DeliveryFailure::LegacyUploaderActive => "DELIVERY_LEGACY_UPLOADER_ACTIVE",
        }
    }
}
/// Maps a refused consent request: 401 needs a new sign-in and 403 is
/// another account's install; anything else is `message`.
fn consent_error(message: &'static str) -> impl Fn(ureq::Error) -> anyhow::Error {
    move |error| match error {
        ureq::Error::Status(401, _) => fail(DeliveryFailure::AuthenticationRequired),
        ureq::Error::Status(403, _) => fail(DeliveryFailure::PermissionDenied),
        _ => anyhow::anyhow!(message),
    }
}

/// The delivery failure an error carries, whether the client decided it
/// (no sign-in, another account) or the server answered with it.
pub fn failure_of(error: &anyhow::Error) -> Option<DeliveryFailure> {
    error
        .downcast_ref::<TransportFailure>()
        .map(|failure| failure.0)
        .or_else(|| error.downcast_ref::<ReceiverRejection>().map(|r| r.failure))
        .or_else(|| tls::configuration_failure(error))
}

fn fail(kind: DeliveryFailure) -> anyhow::Error {
    TransportFailure(kind).into()
}

/// A response the server returned deliberately. Only the numeric status and a
/// tightly validated machine code are retained; response bodies never reach
/// logs, status, or the delivery database.
#[derive(Debug)]
struct ReceiverRejection {
    failure: DeliveryFailure,
    status: u16,
    recovery: Option<ConflictRecovery>,
    retry_after_ms: Option<i64>,
    /// The sessions a writer refused under acquisition: held, not retried.
    hold: Option<Vec<HeldSession>>,
}
impl std::fmt::Display for ReceiverRejection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        TransportFailure(self.failure).fmt(formatter)
    }
}
impl std::error::Error for ReceiverRejection {}
impl ReceiverRejection {
    fn isolatable(&self) -> bool {
        self.failure == DeliveryFailure::ReceiverRejected && matches!(self.status, 400 | 422)
    }
}
fn require(condition: bool, kind: DeliveryFailure) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(fail(kind))
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
/// An expected-account assertion; server authentication remains authoritative.
pub fn account_id(org: &str, workspace: Option<&str>) -> String {
    format!(
        "relayhistory:{}",
        hash(
            serde_json::to_string(&[org, workspace.unwrap_or("")])
                .expect("strings serialize")
                .as_bytes()
        )
    )
}
fn check_account(auth: &cloud::StoredAuth, expected: &str) -> Result<()> {
    let org = auth
        .org_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| fail(DeliveryFailure::AuthenticationRequired))?;
    require(
        account_id(org, auth.workspace_id.as_deref()) == expected,
        DeliveryFailure::PermissionDenied,
    )
}
/// Change server consent after the collector has stopped. The server serializes
/// this with in-flight delivery and retains a fence against delayed retries.
pub fn set_session_uploads(
    base_url: &str,
    expected_account: &str,
    expected_user: &str,
    origin: &str,
    sessions: &[crate::history::delivery::SessionIdentity],
    enabled: bool,
) -> Result<()> {
    let auth = cloud::load_selected_auth(Some(base_url))?
        .ok_or_else(|| fail(DeliveryFailure::AuthenticationRequired))?;
    let url = format!(
        "{}/v1/delivery/sessions/selection",
        cloud::normalized_stage(base_url)?
    );
    // The receiver must enforce both assertions against its authenticated
    // identity. StoredAuth only caches org/workspace, not the uploading user.
    let body = serde_json::json!({"account_id": expected_account, "expected_user_id": expected_user, "origin_id": origin,
        "sessions": sessions, "enabled": enabled}).to_string();
    let agent = cloud::history_agent()
        .redirects(0)
        .timeout(Duration::from_secs(30))
        .build();
    let budget = command_budget(Duration::from_secs(30));
    let response = cloud::send_with_auth_refresh_checked_with_budget(
        &auth,
        |current, remaining| {
            agent
                .post(&url)
                .timeout(remaining.unwrap_or(Duration::from_secs(30)))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .set("Content-Type", "application/json")
                .send_string(&body)
                .map_err(Box::new)
        },
        selection_error,
        |current| check_account(current, expected_account),
        Some(&budget),
    )?;
    selection_answer(response)
}
/// Idempotent reset of the authenticated user's cloud history in this workspace.
/// A service that does not serve reset answers with [`ResetUnsupported`].
pub fn reset_sessions(
    base_url: &str,
    expected_account: &str,
    request_id: &str,
    confirmation: &str,
) -> Result<()> {
    let auth = cloud::load_selected_auth(Some(base_url))?
        .ok_or_else(|| fail(DeliveryFailure::AuthenticationRequired))?;
    let url = format!(
        "{}/v1/delivery/sessions/reset",
        cloud::normalized_stage(base_url)?
    );
    let body = serde_json::json!({"account_id": expected_account, "request_id": request_id,
        "confirmation": confirmation})
    .to_string();
    let agent = cloud::history_agent()
        .redirects(0)
        .timeout(Duration::from_secs(120))
        .build();
    let budget = command_budget(Duration::from_secs(120));
    let response = cloud::send_with_auth_refresh_checked_with_budget(
        &auth,
        |current, remaining| {
            agent
                .post(&url)
                .timeout(remaining.unwrap_or(Duration::from_secs(120)))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .set("Content-Type", "application/json")
                .send_string(&body)
                .map_err(Box::new)
        },
        reset::reset_error,
        |current| check_account(current, expected_account),
        Some(&budget),
    )?;
    let value = command_answer(response)?;
    anyhow::ensure!(
        value["reset"] == true,
        "Cloud did not confirm the session reset"
    );
    Ok(())
}

pub fn selected_account(base_url: Option<&str>) -> Result<String> {
    let auth = cloud::load_selected_auth(base_url)?
        .ok_or_else(|| fail(DeliveryFailure::AuthenticationRequired))?;
    let org = auth
        .org_id
        .as_deref()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| fail(DeliveryFailure::AuthenticationRequired))?;
    Ok(account_id(org, auth.workspace_id.as_deref()))
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    protocol_version: u32,
    batch: HistoryExportBatch,
    /// The grants the batch presents for its protected sessions. Absent when
    /// none is protected, so a body without acquisition is byte for byte the
    /// one this build sent before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    acquisition: Vec<AcquisitionEntry>,
}
/// Check a batch against the protocol, allowing at most `max_records`.
fn validate(batch: &HistoryExportBatch, max_records: usize) -> Result<()> {
    require(
        batch.mapping_version == MAPPING_VERSION,
        DeliveryFailure::MappingVersionMismatch,
    )?;
    require(
        batch.schema_version == 1
            && batch.account_id.starts_with("relayhistory:")
            && !batch.records.is_empty()
            && batch.records.len() <= max_records,
        DeliveryFailure::InvalidPayload,
    )?;
    let mut ids = HashSet::new();
    for record in &batch.records {
        require(
            DELIVERABLE_KINDS.contains(&record.kind.as_str()),
            DeliveryFailure::UnsupportedEvidence,
        )?;
        require(
            record.schema_version == 1
                && record.origin_id == batch.origin_id
                && record.revision > 0
                && record.revision <= 9_007_199_254_740_991
                && !record.record_id.is_empty()
                && !record.revision_id.is_empty()
                && ids.insert(&record.revision_id)
                && match record.operation.as_str() {
                    "delete" => record.payload.is_null(),
                    "upsert" => record.payload.is_object(),
                    _ => false,
                },
            DeliveryFailure::InvalidPayload,
        )?;
    }
    Ok(())
}
/// Prepare exactly once and persist this result in the core before calling
/// send, for a server that accepts the protocol's default record cap.
pub fn prepare(batch: HistoryExportBatch) -> Result<PreparedPayload> {
    prepare_within(batch, MAX_BATCH_RECORDS)
}
/// [`prepare`] for a batch of at most `max_records`. The receiver maps every
/// batch the worker prepared, which holds up to the record limit the server
/// last published (at most [`MAX_PUBLISHED_BATCH_RECORDS`]): a batch prepared
/// under a raised limit before a restart is still mapped after it, before any
/// receipt has published the limit again. The server judges it when it is
/// sent.
pub fn prepare_within(batch: HistoryExportBatch, max_records: usize) -> Result<PreparedPayload> {
    let body = wire_body(batch, &[], max_records)?;
    Ok(PreparedPayload {
        mapping_version: MAPPING_VERSION.into(),
        content_type: "application/json".into(),
        sha256: hash(body.as_bytes()),
        body,
        prepared_by: None,
    })
}
/// The request body [`prepare_within`] prepares, without its digest: the
/// worker hashes the body once, as it stores it.
fn wire_body(
    batch: HistoryExportBatch,
    acquisition: &[AcquisitionEntry],
    max_records: usize,
) -> Result<String> {
    validate(&batch, max_records)?;
    let acquisition = presented(&batch, acquisition)?;
    let body = storable_body(serde_json::to_string(&Request {
        protocol_version: 1,
        batch,
        acquisition,
    })?)?;
    require(
        body.len() <= MAX_BODY_BYTES,
        DeliveryFailure::InvalidPayload,
    )?;
    Ok(body)
}
/// The receiver may add receipt fields; only these decide acknowledgment.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Receipt {
    protocol_version: u32,
    batch_id: String,
    accepted_revision_ids: Vec<String>,
    unsupported_revision_ids: Vec<String>,
    acceptance_level: String,
}
fn receipt(
    value: &serde_json::Value,
    batch: &HistoryExportBatch,
) -> Result<DeliveryAcknowledgment> {
    let result = Receipt::deserialize(value).map_err(|_| fail(DeliveryFailure::InvalidPayload))?;
    require(
        result.protocol_version == 1
            && result.batch_id == batch.batch_id
            && result.acceptance_level == "durable",
        DeliveryFailure::InvalidPayload,
    )?;
    let submitted: HashSet<_> = batch
        .records
        .iter()
        .map(|record| record.revision_id.as_str())
        .collect();
    let mut seen = HashSet::new();
    for id in result
        .accepted_revision_ids
        .iter()
        .chain(&result.unsupported_revision_ids)
    {
        require(
            submitted.contains(id.as_str()) && seen.insert(id),
            DeliveryFailure::InvalidPayload,
        )?;
    }
    Ok(DeliveryAcknowledgment {
        batch_id: result.batch_id,
        accepted_revision_ids: result.accepted_revision_ids,
        unsupported_revision_ids: result.unsupported_revision_ids,
        suppressed_revision_ids: vec![],
        rejected_revision_ids: vec![],
        isolation_frontier: vec![],
        acceptance_level: AcceptanceLevel::Durable,
    })
}
mod tls;
use tls::verdict;
mod post;
use post::{http_agent, post_batch};
mod direct_s3;
pub use direct_s3::DirectUploadSource;

fn http_error(error: ureq::Error, submitted_body: &str) -> anyhow::Error {
    match error {
        ureq::Error::Status(status, response) => {
            let retry_after_ms = retry_after::of(status, &response);
            // Only the typed conflict protocol needs the response body to
            // classify, and 429 and 503 keep its code for the log so a refusal
            // can be told apart from a capacity failure. Other statuses are
            // classified from the status alone, so avoid reading up to 512 KiB
            // for them.
            let bytes = if matches!(status, 409 | 429 | 503) {
                error_body_bytes(response)
            } else {
                None
            };
            let body = bytes.as_deref().and_then(parse_error_body);
            let code = body.as_ref().and_then(safe_error_code);
            let hold = writer_hold(status, body.as_ref(), code.as_deref(), bytes.as_deref());
            let failure = match status {
                409 if hold.is_some() => DeliveryFailure::ReceiverRejected,
                401 => DeliveryFailure::AuthenticationRequired,
                403 => DeliveryFailure::PermissionDenied,
                404 | 405 | 410 | 501 => DeliveryFailure::UnsupportedEvidence,
                408 | 425 | 429 => DeliveryFailure::RateLimited,
                status if status >= 500 => DeliveryFailure::Transient,
                400 | 422 => DeliveryFailure::ReceiverRejected,
                409 if code.as_deref() == Some("delivery_conflict") => {
                    DeliveryFailure::DeliveryConflict
                }
                // The receiver's only 409 is the typed conflict above. Any
                // other 409 is not its verdict on this batch (an edge or a
                // deploy in progress answered instead), so it is retried with
                // backoff rather than blocking every later upload behind it.
                409 => DeliveryFailure::Transient,
                _ => DeliveryFailure::InvalidPayload,
            };
            let recovery = body
                .as_ref()
                .filter(|_| status == 409 && hold.is_none())
                .and_then(|body| {
                    let request: Request = serde_json::from_str(submitted_body).ok()?;
                    conflict::parse(status, body, &request.batch)
                });
            ReceiverRejection {
                failure,
                status,
                recovery,
                retry_after_ms,
                hold,
            }
            .into()
        }
        ureq::Error::Transport(transport) => tls::transport_error(transport),
    }
}
/// State a receiver keeps between batches: its stage session and its last
/// legacy scheduler verdict. Clones share it; `Default` starts empty.
#[derive(Debug, Clone, Default)]
pub struct ReceiverState(Arc<ReceiverStateInner>);
#[derive(Debug, Default)]
struct ReceiverStateInner {
    session: SessionCache,
    schedules: Mutex<Option<(Instant, MigrationStatus)>>,
    /// The last inspection, conclusive or not, for status reports.
    last_schedules: Mutex<Option<MigrationStatus>>,
    /// Replaces `migration::status` (see [`ReceiverState::inspecting_with`]).
    inspector: Option<fn() -> MigrationStatus>,
    /// The batch limits the last receipt published.
    limits: Mutex<PublishedLimits>,
    /// The request-body coding the server published last.
    encoding: Mutex<PublishedEncoding>,
    /// The host has no legacy schedulers to inspect (see [`ReceiverState::hosted`]).
    hosted: bool,
    /// Whether the limits were asked for before any receipt published them.
    limits_asked: std::sync::atomic::AtomicBool,
    /// The sources the server enrolled in acquisition, as its limits last
    /// said (`acquisition.enrolledSources`); unknown until they say. Only
    /// these sources' records present grants or are held for one.
    enrolled: Mutex<Option<BTreeSet<String>>>,
    /// The exact account (site, user, org, workspace) this receiver
    /// delivers for. Only its grants are presented and only its enrolled
    /// sources are kept; without one no grant is presented.
    acquisition_account: Mutex<Option<crate::history::acquisition::Account>>,
    /// Where the installation and grants are kept: the default location
    /// unless a test set one.
    acquisition_store: Option<std::path::PathBuf>,
    /// What direct uploads keep between batches, per credentials source.
    direct: Mutex<direct_s3::Memory>,
}
/// The request-body coding the server accepts, and whether a batch response
/// (a receipt, or a refusal of a coding) published it. A batch response is
/// newer than the limits response asked for alongside it, so that one only
/// fills the coding in until a batch response has spoken.
#[derive(Debug, Default, Clone, Copy)]
struct PublishedEncoding {
    coding: Option<ContentEncoding>,
    from_batch: bool,
}
impl ReceiverState {
    /// State for a hosted sandbox's teardown flush, which has no older managed
    /// uploader that could deliver the same history twice: the legacy
    /// scheduler guard is `clear` without running `launchctl` or `crontab`.
    /// Every other guard is unchanged.
    pub fn hosted() -> Self {
        Self(Arc::new(ReceiverStateInner {
            hosted: true,
            ..Default::default()
        }))
    }
    /// A hosted teardown receiver whose push-only session exists in process
    /// memory only. A transient send may clear the working cache, but the
    /// fixed session remains available for the next bounded pass without a
    /// credential file that a forced teardown could leave behind.
    pub fn hosted_with_session(base_url: String, auth: cloud::StoredAuth) -> Self {
        Self(Arc::new(ReceiverStateInner {
            session: SessionCache::fixed(base_url, auth),
            hosted: true,
            ..Default::default()
        }))
    }
    /// Drop the kept session, so the next send reads the auth store: for
    /// a host that has just stored a new one.
    /// Upload credentials handed out to the old session go with it.
    pub fn forget_session(&self) {
        self.0.session.replace();
        self.0
            .direct
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .forget_grants();
    }
    fn published_limits(&self) -> PublishedLimits {
        *self.0.limits.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn publish(&self, limits: PublishedLimits) {
        *self.0.limits.lock().unwrap_or_else(|e| e.into_inner()) = limits;
    }
    fn request_encoding(&self) -> Option<ContentEncoding> {
        self.0
            .encoding
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .coding
    }
    /// The coding a batch response published: it always stands.
    fn publish_encoding(&self, coding: Option<ContentEncoding>) {
        *self.0.encoding.lock().unwrap_or_else(|e| e.into_inner()) = PublishedEncoding {
            coding,
            from_batch: true,
        };
    }
    /// The coding the limits response published: kept only while no batch
    /// response has published one, since a receipt or refusal that lands
    /// while the limits request is in flight is newer.
    fn publish_asked_encoding(&self, coding: Option<ContentEncoding>) {
        let mut published = self.0.encoding.lock().unwrap_or_else(|e| e.into_inner());
        if !published.from_batch {
            published.coding = coding;
        }
    }
    /// The published limits, asking the receiver for them once, when nothing
    /// has published any yet: otherwise every collector start prepares its
    /// first window of batches at the protocol default until a receipt
    /// arrives. Any failure to ask reads as no limits, and is not retried;
    /// the next receipt publishes them. They are never persisted, since the
    /// server can lower them between runs.
    fn published_limits_or_ask(
        &self,
        ask: impl FnOnce() -> Option<PublishedLimits>,
    ) -> PublishedLimits {
        let known = self.published_limits();
        if known != PublishedLimits::default()
            || self
                .0
                .limits_asked
                .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return known;
        }
        if let Some(asked) = ask() {
            // A receipt that arrived meanwhile is newer and stands.
            let mut limits = self.0.limits.lock().unwrap_or_else(|e| e.into_inner());
            if *limits == PublishedLimits::default() {
                *limits = asked;
            }
        }
        self.published_limits()
    }
}

/// Send one request with the stage session, within `budget`. A
/// command-scoped fixed session is used as is; any other goes through the
/// shared locked reload-and-refresh path on a 401. The session a successful
/// request used is kept, unless the host replaced the session meanwhile,
/// and any failure forgets the cached copy so the next request reads the
/// store again.
fn send_authenticated(
    stage: &Stage<'_>,
    send: impl Fn(
        &cloud::StoredAuth,
        Option<Duration>,
    ) -> std::result::Result<ureq::Response, Box<ureq::Error>>,
    map_error: impl Fn(ureq::Error) -> anyhow::Error,
    check: impl Fn(&cloud::StoredAuth) -> Result<()>,
    budget: &dyn Fn() -> Result<Duration>,
) -> Result<ureq::Response> {
    let epoch = stage.epoch();
    let auth = stage.session()?;
    let used = Mutex::new(None);
    let send = |current: &cloud::StoredAuth, timeout: Option<Duration>| {
        let response = send(current, timeout)?;
        *used.lock().unwrap_or_else(|e| e.into_inner()) = Some(current.clone());
        Ok(response)
    };
    let response = if stage.fixed() {
        cloud::send_with_fixed_auth_checked_with_budget(&auth, send, map_error, check, Some(budget))
    } else {
        cloud::send_with_auth_refresh_checked_with_budget(
            &auth,
            send,
            map_error,
            check,
            Some(budget),
        )
    };
    match (
        &response,
        used.into_inner().unwrap_or_else(|e| e.into_inner()),
    ) {
        (Ok(_), Some(current)) => stage.keep_from(epoch, current),
        _ => stage.forget(),
    }
    response
}

fn send_request(
    stage: Stage<'_>,
    url: &str,
    content_type: &str,
    body: &str,
    batch: &HistoryExportBatch,
    budget: &CallBudget<'_>,
) -> Result<DeliveryAcknowledgment> {
    let stored = WireBody {
        bytes: body.as_bytes(),
        encoding: None,
    };
    // Each attempt, an auth-refresh retry included, sends in the coding the
    // receiver publishes at that moment: a coding the server refused earlier
    // in this send is never posted again. The compressed bytes are kept per
    // coding, so a retry in the same coding does not compress again.
    let compressed: Mutex<Option<(ContentEncoding, Option<Vec<u8>>)>> = Mutex::new(None);
    let encoded = |encoding: ContentEncoding| {
        let mut kept = compressed.lock().unwrap_or_else(|e| e.into_inner());
        if kept.as_ref().map(|(coding, _)| *coding) != Some(encoding) {
            *kept = Some((encoding, encoding.encode(body.as_bytes())));
        }
        kept.as_ref().and_then(|(_, bytes)| bytes.clone())
    };
    // A refused coding with no budget left to resend the stored bytes says
    // nothing about the batch, so it ends the send as the budget does.
    let refused_unresent = std::sync::atomic::AtomicBool::new(false);
    let response = send_authenticated(
        &stage,
        |current, timeout| {
            let timeout = timeout.expect("receiver budget supplies a timeout");
            let post = |wire: WireBody<'_>, timeout: Duration| {
                post_batch(
                    url,
                    current,
                    content_type,
                    wire,
                    timeout,
                    &|| budget.remaining(),
                )
            };
            let Some((encoding, bytes)) = stage
                .published
                .request_encoding()
                .and_then(|encoding| encoded(encoding).map(|bytes| (encoding, bytes)))
            else {
                return post(stored, timeout);
            };
            let wire = WireBody {
                bytes: &bytes,
                encoding: Some(encoding),
            };
            match post(wire, timeout) {
                // A server that no longer decodes the coding it published (a
                // rollback, or a proxy in front of it) refuses the body before
                // reading the batch. The stored bytes go once more as they
                // are, and later attempts and batches stay uncompressed until
                // a receipt publishes a coding again.
                Err(error) if matches!(*error, ureq::Error::Status(400 | 415, _)) => {
                    stage.published.publish_encoding(None);
                    match budget.remaining() {
                        Ok(timeout) => post(stored, timeout),
                        Err(_) => {
                            refused_unresent.store(true, std::sync::atomic::Ordering::SeqCst);
                            Err(error)
                        }
                    }
                }
                result => result,
            }
        },
        |error| http_error(error, body),
        |current| check_account(current, &batch.account_id),
        &|| budget.remaining(),
    );
    let response = response.map_err(guard::retry_unresolved).map_err(|error| {
        if refused_unresent.load(std::sync::atomic::Ordering::SeqCst) {
            budget.remaining().err().unwrap_or(error)
        } else if failure_of(&error).is_some() {
            error
        } else if let Err(stopped) = budget.remaining() {
            stopped
        } else {
            fail(DeliveryFailure::AuthenticationRequired)
        }
    })?;
    budget.remaining()?;
    // Only a batch retries an unreadable answer, so only a batch logs it.
    let response = read_answer(response).map_err(AnswerFailure::logged)?;
    budget.remaining()?;
    let acknowledgment = receipt(&response, batch)?;
    // Each receipt says what the server accepts now; one that says nothing
    // is an older server, which accepts the protocol's defaults and no
    // request-body coding.
    stage.published.publish(published_limits(&response));
    stage.published.enrolled_from(response.get("limits"));
    stage
        .published
        .publish_encoding(response.get("limits").and_then(published_encoding));
    Ok(acknowledgment)
}

/// The longest asking for the published limits may take, a token refresh
/// included; a drain's shorter request deadline or its stop signal ends it
/// sooner.
const LIMITS_TIMEOUT: Duration = Duration::from_secs(5);

/// `GET /v1/delivery/limits` with the stage session, authenticated the way a
/// batch is (see [`send_authenticated`]): the limits the server accepts now,
/// before any batch. `None` on any failure.
fn fetch_limits(stage: Stage<'_>, ctx: &LimitsContext<'_>) -> Option<PublishedLimits> {
    let url = format!(
        "{}/v1/delivery/limits",
        cloud::normalized_stage(&stage.session().ok()?.base_url).ok()?
    );
    let started = Instant::now();
    let budget = LIMITS_TIMEOUT.min(Duration::from_millis(ctx.timeout_ms.max(0) as u64));
    let remaining = || {
        let remaining = budget.saturating_sub(started.elapsed());
        require(
            !(ctx.cancelled)() && !remaining.is_zero(),
            DeliveryFailure::Transient,
        )?;
        Ok(remaining)
    };
    let response = send_authenticated(
        &stage,
        |current, timeout| {
            http_agent()
                .get(&url)
                .timeout(timeout.expect("the limits budget supplies a timeout"))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .call()
                .map_err(Box::new)
        },
        anyhow::Error::new,
        |_| Ok(()),
        &remaining,
    )
    .ok()?;
    let published = response_json(response).ok()?;
    stage
        .published
        .publish_asked_encoding(published_encoding(&published));
    stage.published.enrolled_from(Some(&published));
    let limits = limits_object(&published);
    (limits != PublishedLimits::default()).then_some(limits)
}

/// The batch limits a receipt publishes (`limits: {maxRecords,
/// maxRequestBytes}`), each only when it is a positive integer.
fn published_limits(receipt: &serde_json::Value) -> PublishedLimits {
    receipt
        .get("limits")
        .map_or_else(PublishedLimits::default, limits_object)
}
/// A `{maxRecords, maxRequestBytes}` object, each limit only when it is a
/// positive integer.
fn limits_object(limits: &serde_json::Value) -> PublishedLimits {
    let limit = |name: &str| {
        limits
            .get(name)?
            .as_u64()
            .filter(|value| *value > 0)
            .and_then(|value| usize::try_from(value).ok())
    };
    PublishedLimits {
        max_records: limit("maxRecords"),
        max_request_bytes: limit("maxRequestBytes"),
    }
}

fn complete_receipt(ack: &DeliveryAcknowledgment, batch: &HistoryExportBatch) -> bool {
    let accounted: HashSet<_> = ack
        .accepted_revision_ids
        .iter()
        .chain(&ack.unsupported_revision_ids)
        .map(String::as_str)
        .collect();
    accounted.len() == batch.records.len()
        && batch
            .records
            .iter()
            .all(|record| accounted.contains(record.revision_id.as_str()))
}

pub(crate) fn child_batch(
    parent: &HistoryExportBatch,
    records: &[crate::history::delivery::HistoryExportRecord],
) -> HistoryExportBatch {
    let mut child = parent.clone();
    child.records = records.to_vec();
    let identity = child
        .records
        .iter()
        .map(|record| record.revision_id.as_str())
        .collect::<Vec<_>>();
    child.batch_id = hash(
        &serde_json::to_vec(&(&parent.batch_id, identity)).expect("batch identity serializes"),
    );
    child
}

/// The exact request bytes and semantics handed to the receiver. Older
/// journals can contain JSON text fields that current builds normalize before
/// transport, so every child identity and conflict proof must derive from
/// this representation rather than the retained pre-normalization body.
fn request_for_wire(prepared: &PreparedPayload) -> Result<(String, Request)> {
    wire_request(storable_body(prepared.body.clone())?)
}
/// A body already in wire form, with the request it carries.
fn wire_request(body: String) -> Result<(String, Request)> {
    require(
        body.len() <= MAX_BODY_BYTES,
        DeliveryFailure::InvalidPayload,
    )?;
    let request = serde_json::from_str(&body).map_err(|_| fail(DeliveryFailure::InvalidPayload))?;
    Ok((body, request))
}

const MAX_ISOLATION_REQUESTS: usize = 32;

fn isolatable(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ReceiverRejection>()
        .is_some_and(ReceiverRejection::isolatable)
}

/// A typed conflict can arrive while the receiver is isolating a rejected
/// parent batch. The receiver proof is bound to the exact deterministic child
/// that was sent, while the core lease is bound to the durable parent. Lift
/// only a fully validated child proof into a parent recovery plan; the core
/// will revalidate that plan against the persisted parent bytes under its
/// fenced write transaction.
fn lift_conflict(
    error: anyhow::Error,
    parent: &HistoryExportBatch,
    submitted: &HistoryExportBatch,
) -> anyhow::Error {
    if submitted.batch_id == parent.batch_id {
        return error;
    }
    let Some(rejection) = error.downcast_ref::<ReceiverRejection>() else {
        return error;
    };
    let Some(recovery) = rejection.recovery.as_ref() else {
        return error;
    };
    // A recovery proof scoped to the isolation child that cannot be applied
    // to the durable parent lease is stripped: the receiver's typed
    // delivery-conflict verdict stands, so the worker records a truthful
    // blocked failure instead of attempting parent recovery and abandoning
    // the lease on verification failure.
    ReceiverRejection {
        failure: rejection.failure,
        status: rejection.status,
        recovery: conflict::lift(parent, submitted, recovery),
        retry_after_ms: rejection.retry_after_ms,
        hold: rejection.hold.clone(),
    }
    .into()
}

struct Isolation<'a> {
    stage: Stage<'a>,
    /// The parent's grants; each child presents those of its own sessions.
    acquisition: &'a [AcquisitionEntry],
    url: &'a str,
    content_type: &'a str,
    budget: &'a CallBudget<'a>,
    requests: usize,
    accepted: Vec<String>,
    rejected: Vec<String>,
    frontier: Vec<IsolationTask>,
}
impl Isolation<'_> {
    fn send(&mut self, batch: &HistoryExportBatch) -> Result<DeliveryAcknowledgment> {
        if self.requests >= MAX_ISOLATION_REQUESTS {
            return Err(fail(DeliveryFailure::Transient));
        }
        self.requests += 1;
        let body = wire_body(batch.clone(), self.acquisition, MAX_PUBLISHED_BATCH_RECORDS)?;
        send_request(
            self.stage,
            self.url,
            self.content_type,
            &body,
            batch,
            self.budget,
        )
    }

    fn accept(&mut self, ack: DeliveryAcknowledgment, batch: &HistoryExportBatch) -> Result<()> {
        require(complete_receipt(&ack, batch), DeliveryFailure::Transient)?;
        require(
            ack.unsupported_revision_ids.is_empty()
                && ack.suppressed_revision_ids.is_empty()
                && ack.rejected_revision_ids.is_empty()
                && ack.isolation_frontier.is_empty(),
            DeliveryFailure::UnsupportedEvidence,
        )?;
        self.accepted.extend(ack.accepted_revision_ids);
        Ok(())
    }

    fn batch_for_task(
        parent: &HistoryExportBatch,
        task: &IsolationTask,
    ) -> Result<HistoryExportBatch> {
        let records = task
            .revision_ids
            .iter()
            .map(|revision| {
                parent
                    .records
                    .iter()
                    .find(|record| &record.revision_id == revision)
                    .cloned()
                    .ok_or_else(|| fail(DeliveryFailure::InvalidPayload))
            })
            .collect::<Result<Vec<_>>>()?;
        require(!records.is_empty(), DeliveryFailure::InvalidPayload)?;
        Ok(child_batch(parent, &records))
    }

    /// Drain a durable depth-first search. A rejected node splits locally into
    /// two unknown children; every unfinished node stays in `frontier`, so a
    /// request cap or deadline resumes at a different subtree next time.
    fn drain(&mut self, parent: &HistoryExportBatch) -> Result<()> {
        while let Some(task) = self.frontier.pop() {
            let batch = Self::batch_for_task(parent, &task)?;
            if task.rejected {
                if batch.records.len() == 1 {
                    self.rejected.push(batch.records[0].revision_id.clone());
                    continue;
                }
                let middle = batch.records.len() / 2;
                for records in [&batch.records[middle..], &batch.records[..middle]] {
                    self.frontier.push(IsolationTask {
                        revision_ids: records
                            .iter()
                            .map(|record| record.revision_id.clone())
                            .collect(),
                        rejected: false,
                    });
                }
                continue;
            }
            match self.send(&batch) {
                Ok(ack) => {
                    if let Err(error) = self.accept(ack, &batch) {
                        self.frontier.insert(0, task);
                        return Err(error);
                    }
                }
                Err(error) if isolatable(&error) => self.frontier.push(IsolationTask {
                    revision_ids: task.revision_ids,
                    rejected: true,
                }),
                Err(error) => {
                    // Put the interrupted node at the bottom. If siblings are
                    // available, the durable retry tries one of them first.
                    self.frontier.insert(0, task);
                    return Err(lift_conflict(error, parent, &batch));
                }
            }
        }
        Ok(())
    }
}

fn send_with_budget(
    stage: Stage<'_>,
    prepared: &PreparedPayload,
    budget: &CallBudget<'_>,
    accepted_progress: &[String],
    suppressed_progress: &[String],
    rejected_progress: &[String],
    frontier_progress: &[IsolationTask],
) -> Result<DeliveryAcknowledgment> {
    budget.remaining()?;
    require(
        prepared.mapping_version == MAPPING_VERSION,
        DeliveryFailure::MappingVersionMismatch,
    )?;
    require(
        prepared.body.len() <= MAX_BODY_BYTES
            && prepared.content_type == "application/json"
            && prepared.sha256 == hash(prepared.body.as_bytes()),
        DeliveryFailure::InvalidPayload,
    )?;
    // A persisted body from an older build is immutable in the journal. Its
    // safe wire representation and every isolation child are deterministic,
    // so a crash repeats the same ids and bytes and revision idempotency holds.
    // Parse the exact wire representation too: conflict lift and transactional
    // verification must bind to the same records the receiver saw. A body
    // this build's `prepare` returned, its digest checked above, already is
    // that representation: `storable_body` returns it byte for byte.
    let (body, request) = if prepared.prepared_by_this_build() {
        wire_request(prepared.body.clone())?
    } else {
        request_for_wire(prepared)?
    };
    require(
        request.protocol_version == 1,
        DeliveryFailure::MappingVersionMismatch,
    )?;
    validate_request(&request)?;
    let expected: HashSet<_> = request
        .batch
        .records
        .iter()
        .map(|record| record.revision_id.as_str())
        .collect();
    let accepted_known: HashSet<_> = accepted_progress.iter().map(String::as_str).collect();
    let suppressed_known: HashSet<_> = suppressed_progress.iter().map(String::as_str).collect();
    let rejected_known: HashSet<_> = rejected_progress.iter().map(String::as_str).collect();
    let mut frontier_known = HashSet::new();
    for task in frontier_progress {
        require(
            !task.revision_ids.is_empty()
                && task.revision_ids.iter().all(|revision| {
                    expected.contains(revision.as_str()) && frontier_known.insert(revision.as_str())
                }),
            DeliveryFailure::InvalidPayload,
        )?;
    }
    require(
        accepted_known.len() == accepted_progress.len()
            && suppressed_known.len() == suppressed_progress.len()
            && rejected_known.len() == rejected_progress.len()
            && accepted_known.is_subset(&expected)
            && suppressed_known.is_subset(&expected)
            && rejected_known.is_subset(&expected)
            && accepted_known.is_disjoint(&suppressed_known)
            && accepted_known.is_disjoint(&rejected_known)
            && suppressed_known.is_disjoint(&rejected_known)
            && accepted_known.is_disjoint(&frontier_known)
            && suppressed_known.is_disjoint(&frontier_known)
            && rejected_known.is_disjoint(&frontier_known),
        DeliveryFailure::InvalidPayload,
    )?;
    let known: HashSet<_> = accepted_known
        .union(&suppressed_known)
        .copied()
        .chain(rejected_known.iter().copied())
        .chain(frontier_known.iter().copied())
        .collect();
    let unknown = request
        .batch
        .records
        .iter()
        .filter(|record| !known.contains(record.revision_id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if unknown.is_empty() && frontier_progress.is_empty() {
        // Rejected singletons are provisional until at least one sibling has
        // been accepted. If every revision was rejected, this is a receiver-
        // wide/schema failure: block without advancing or counting skips.
        require(
            !accepted_progress.is_empty(),
            DeliveryFailure::ReceiverRejected,
        )?;
        let mut suppressed = suppressed_progress.to_vec();
        suppressed.extend_from_slice(rejected_progress);
        return Ok(DeliveryAcknowledgment {
            batch_id: request.batch.batch_id,
            accepted_revision_ids: accepted_progress.to_vec(),
            unsupported_revision_ids: vec![],
            suppressed_revision_ids: suppressed,
            rejected_revision_ids: vec![],
            isolation_frontier: vec![],
            acceptance_level: AcceptanceLevel::Durable,
        });
    }
    let auth = stage.session()?;
    guard::require_destination(&auth.base_url)?;
    let url = format!(
        "{}/v1/delivery/batches",
        cloud::normalized_stage(&auth.base_url)?
    );
    let mut isolation = Isolation {
        stage,
        acquisition: &request.acquisition,
        url: &url,
        content_type: &prepared.content_type,
        budget,
        requests: 0,
        accepted: accepted_progress.to_vec(),
        rejected: rejected_progress.to_vec(),
        frontier: frontier_progress.to_vec(),
    };
    let initial_accepted = isolation.accepted.len();
    let initial_rejected = isolation.rejected.len();
    let initial_frontier = isolation.frontier.clone();

    let outcome = if isolation.frontier.is_empty() {
        let pending = if known.is_empty() {
            request.batch.clone()
        } else {
            child_batch(&request.batch, &unknown)
        };
        let pending_body = if known.is_empty() {
            body
        } else {
            wire_body(
                pending.clone(),
                &request.acquisition,
                MAX_PUBLISHED_BATCH_RECORDS,
            )?
        };
        isolation.requests = 1;
        match send_request(
            stage,
            &url,
            &prepared.content_type,
            &pending_body,
            &pending,
            budget,
        ) {
            Ok(ack) => {
                let mut accepted = accepted_progress.to_vec();
                accepted.extend(ack.accepted_revision_ids);
                let mut suppressed = suppressed_progress.to_vec();
                suppressed.extend(ack.suppressed_revision_ids);
                let rejected = if accepted.is_empty() {
                    rejected_progress.to_vec()
                } else {
                    suppressed.extend_from_slice(rejected_progress);
                    vec![]
                };
                return Ok(DeliveryAcknowledgment {
                    batch_id: request.batch.batch_id,
                    accepted_revision_ids: accepted,
                    unsupported_revision_ids: ack.unsupported_revision_ids,
                    suppressed_revision_ids: suppressed,
                    rejected_revision_ids: rejected,
                    isolation_frontier: vec![],
                    acceptance_level: ack.acceptance_level,
                });
            }
            Err(error) if isolatable(&error) => {
                isolation.frontier.push(IsolationTask {
                    revision_ids: pending
                        .records
                        .iter()
                        .map(|record| record.revision_id.clone())
                        .collect(),
                    rejected: true,
                });
            }
            Err(error) => return Err(lift_conflict(error, &request.batch, &pending)),
        }
        isolation.drain(&request.batch)
    } else {
        if !unknown.is_empty() {
            isolation.frontier.insert(
                0,
                IsolationTask {
                    revision_ids: unknown
                        .iter()
                        .map(|record| record.revision_id.clone())
                        .collect(),
                    rejected: false,
                },
            );
        }
        isolation.drain(&request.batch)
    };
    let progressed = isolation.accepted.len() > initial_accepted
        || isolation.rejected.len() > initial_rejected
        || isolation.frontier != initial_frontier;
    let incomplete = match outcome {
        Ok(()) => false,
        Err(error) => {
            let failure = verdict(&error).failure;
            // A partial acknowledgment has no retry-after field. Never turn
            // an authoritative 408/425/429 into generic transient progress;
            // the receiver deduplicates any siblings accepted before it.
            if failure == DeliveryFailure::RateLimited {
                return Err(error);
            }
            let retryable = failure == DeliveryFailure::Transient;
            if !progressed || !retryable {
                return Err(error);
            }
            true
        }
    };
    let isolation_progress = isolation.accepted.len() + isolation.rejected.len();
    if !incomplete {
        require(
            isolation.frontier.is_empty()
                && isolation_progress + suppressed_progress.len() == request.batch.records.len(),
            DeliveryFailure::ReceiverRejected,
        )?;
        require(
            !isolation.accepted.is_empty(),
            DeliveryFailure::ReceiverRejected,
        )?;
    }
    if !isolation.accepted.is_empty() && !isolation.rejected.is_empty() {
        eprintln!(
            "receiver upload records suppressed batch_ref={} count={}",
            hash(request.batch.batch_id.as_bytes()),
            isolation.rejected.len()
        );
    }
    let (suppressed, rejected) = if isolation.accepted.is_empty() {
        (suppressed_progress.to_vec(), isolation.rejected)
    } else {
        let mut suppressed = suppressed_progress.to_vec();
        suppressed.extend(isolation.rejected);
        (suppressed, vec![])
    };
    Ok(DeliveryAcknowledgment {
        batch_id: request.batch.batch_id,
        accepted_revision_ids: isolation.accepted,
        unsupported_revision_ids: vec![],
        suppressed_revision_ids: suppressed,
        rejected_revision_ids: rejected,
        isolation_frontier: isolation.frontier,
        acceptance_level: AcceptanceLevel::Durable,
    })
}

/// The RelayHistory destination as a core delivery receiver: mapping and
/// transport only. Every host — the JSON helper for the SDK, and the probe —
/// runs the same guards through this type instead of reimplementing them, and
/// the core worker owns leases, retries, acknowledgment and compaction.
///
/// `expected_account` and `instance_id` are assertions about the generation a
/// host configured: an unset field asserts nothing. `base_url` selects the
/// stage whose stored auth is used; server authentication stays authoritative.
#[derive(Debug, Clone, Default)]
pub struct RelayHistoryReceiver {
    pub base_url: Option<String>,
    pub expected_account: Option<String>,
    pub instance_id: Option<String>,
    /// The user's explicit statement that they checked older upload schedules
    /// themselves. It never counts as a successful inspection.
    pub acknowledge_uninspected_schedules: bool,
    /// What the receiver keeps between batches (see [`ReceiverState`]).
    pub state: ReceiverState,
    /// Where to ask for upload credentials; a batch goes to the upload
    /// inbox instead of the batch route while they are handed out.
    pub direct_upload: Option<DirectUploadSource>,
}
impl RelayHistoryReceiver {
    /// Consent and destination identity, rechecked before mapping and again
    /// before transport: a legacy uploader can be installed, or a job's
    /// endpoint changed, between a batch being prepared and being sent. The
    /// legacy scheduler verdict is reused for up to [`SCHEDULER_GUARD_TTL`];
    /// the account and instance assertions are checked every time.
    ///
    /// Neither scheduler verdict blocks the job: an `active` older uploader
    /// is its own retried failure, `legacy_uploader_active`, so uploads
    /// resume once it is removed, and an inspection that could not finish is
    /// `transient`, inspected again on the retry. Only the account assertion
    /// is a permission refusal.
    fn admit(&self, batch: &HistoryExportBatch) -> Result<(), ReceiverFailure> {
        match self.state.legacy_scheduler_state() {
            "active" => return Err(DeliveryFailure::LegacyUploaderActive.into()),
            "unknown" if !self.acknowledge_uninspected_schedules => {
                return Err(DeliveryFailure::Transient.into())
            }
            _ => {}
        }
        if self
            .expected_account
            .as_ref()
            .is_some_and(|account| *account != batch.account_id)
        {
            return Err(DeliveryFailure::PermissionDenied.into());
        }
        if self
            .instance_id
            .as_ref()
            .is_some_and(|instance| *instance != batch.instance_id)
        {
            return Err(DeliveryFailure::MappingVersionMismatch.into());
        }
        Ok(())
    }
}
impl Receiver for RelayHistoryReceiver {
    fn mapping_version(&self) -> &str {
        MAPPING_VERSION
    }
    /// Mapping reads only the batch and the scheduler verdict, and sends
    /// share one HTTP agent, session cache and scheduler verdict, each behind
    /// its own lock, so several batches may be mapped and on the wire at
    /// once.
    fn concurrent(&self) -> Option<&(dyn Receiver + Sync)> {
        Some(self)
    }
    fn published_limits(&self, ctx: &LimitsContext<'_>) -> PublishedLimits {
        self.state.published_limits_or_ask(|| {
            fetch_limits(
                Stage {
                    cache: &self.state.0.session,
                    base_url: self.base_url.as_deref(),
                    published: &self.state,
                },
                ctx,
            )
        })
    }
    fn supported_kinds(&self) -> &[&str] {
        &DELIVERABLE_KINDS
    }
    fn supports_tombstones(&self) -> bool {
        true
    }
    fn prepare(
        &self,
        batch: &HistoryExportBatch,
        context: &ReceiverContext<'_>,
    ) -> Result<PreparedBody, ReceiverFailure> {
        let budget = CallBudget::new(context);
        budget.remaining().map_err(|error| verdict(&error))?;
        self.admit(batch)?;
        let acquisition = self.acquisition(batch)?;
        let body = wire_body(batch.clone(), &acquisition, MAX_PUBLISHED_BATCH_RECORDS)
            .map_err(|error| verdict(&error))?;
        budget.remaining().map_err(|error| verdict(&error))?;
        Ok(PreparedBody {
            content_type: "application/json".into(),
            body,
        })
    }
    fn send(
        &self,
        payload: &PreparedPayload,
        batch: &HistoryExportBatch,
        context: &ReceiverContext<'_>,
    ) -> Result<DeliveryAcknowledgment, ReceiverFailure> {
        let budget = CallBudget::new(context);
        budget.remaining().map_err(|error| verdict(&error))?;
        self.admit(batch)?;
        let stage = Stage {
            cache: &self.state.0.session,
            base_url: self.base_url.as_deref(),
            published: &self.state,
        };
        let source = self.direct_upload.as_ref();
        if let Some(sent) = direct_s3::route(stage, source, payload, batch, context, &budget) {
            return sent;
        }
        send_with_budget(
            stage,
            payload,
            &budget,
            context.accepted_revision_ids,
            context.suppressed_revision_ids,
            context.rejected_revision_ids,
            context.isolation_frontier,
        )
        .map_err(|error| verdict(&error))
    }
}

#[cfg(test)]
mod tests {
    use super::rejection::{MAX_ERROR_BODY, UNKNOWN_CONFLICT_EXCERPT};
    use super::*;
    /// A batch prepared before a kind was retired is still sent: the
    /// receiver takes every drained kind and every retired one.
    #[test]
    fn receiver_accepts_retired_kinds_of_batches_already_prepared() {
        let receiver = RelayHistoryReceiver::default();
        for kind in SUPPORTED_KINDS.iter().chain(RETIRED_KINDS) {
            assert!(receiver.supported_kinds().contains(kind), "{kind}");
        }
        assert!(!SUPPORTED_KINDS.contains(&"trajectory"));
        assert!(receiver.supported_kinds().contains(&"trajectory"));
    }
    #[test]
    fn a_batch_response_coding_outlives_a_limits_response_in_flight() {
        // A receipt (or refusal) lands while the limits request is out.
        let state = ReceiverState::default();
        state.publish_encoding(None);
        state.publish_asked_encoding(Some(ContentEncoding::Zstd));
        assert_eq!(state.request_encoding(), None);

        let state = ReceiverState::default();
        state.publish_encoding(Some(ContentEncoding::Gzip));
        state.publish_asked_encoding(None);
        assert_eq!(state.request_encoding(), Some(ContentEncoding::Gzip));

        // The limits response fills the coding in before any batch, and the
        // next receipt replaces it.
        let state = ReceiverState::default();
        state.publish_asked_encoding(Some(ContentEncoding::Zstd));
        assert_eq!(state.request_encoding(), Some(ContentEncoding::Zstd));
        state.publish_encoding(None);
        assert_eq!(state.request_encoding(), None);
    }
    #[test]
    fn conflict_error_body_accepts_protocol_bound_and_rejects_oversize() {
        let valid = serde_json::json!({"error":{"code":"delivery_conflict",
            "message":"conflict","conflicts":vec!["x".repeat(2200);100]}});
        let bytes = serde_json::to_vec(&valid).unwrap();
        assert!(bytes.len() > 65_536);
        assert!(parse_error_body(&bytes).is_some());
        assert!(parse_error_body(&vec![b' '; MAX_ERROR_BODY as usize + 1]).is_none());
    }
    #[test]
    fn typed_http_conflict_uses_the_exact_sanitized_submitted_body() {
        let original = event_batch("before\0after");
        // Model an immutable request retained by an older build before the
        // current wire sanitization was introduced.
        let legacy_body = serde_json::to_string(&Request {
            protocol_version: 1,
            acquisition: vec![],
            batch: original.clone(),
        })
        .unwrap();
        let prepared = PreparedPayload {
            mapping_version: MAPPING_VERSION.into(),
            content_type: "application/json".into(),
            sha256: hash(legacy_body.as_bytes()),
            body: legacy_body,
            prepared_by: None,
        };
        let (wire_body, sent) = request_for_wire(&prepared).unwrap();
        assert_ne!(sent.batch.records[0].payload, original.records[0].payload);
        let body = serde_json::json!({"error":{"code":"delivery_conflict",
            "message":"batch reused","conflict":{"type":"batch_id",
                "originId":sent.batch.origin_id,"batchId":sent.batch.batch_id,
                "submittedDigest":conflict::batch_digest(&sent.batch),
                "currentDigest":"f".repeat(64)}}})
        .to_string();
        let response = ureq::Response::new(409, "Conflict", &body).unwrap();
        let error = http_error(ureq::Error::Status(409, response), &wire_body);
        let rejection = error.downcast_ref::<ReceiverRejection>().unwrap();
        assert_eq!(rejection.failure, DeliveryFailure::DeliveryConflict);
        assert!(rejection.recovery.is_some());
        let response = ureq::Response::new(422, "Invalid", &body).unwrap();
        let error = http_error(ureq::Error::Status(422, response), &wire_body);
        assert!(error
            .downcast_ref::<ReceiverRejection>()
            .unwrap()
            .recovery
            .is_none());
    }
    #[test]
    fn rejection_log_only_reports_a_code_when_the_body_was_inspected() {
        assert_eq!(
            rejection_log(409, Some("delivery_conflict"), Some(b"{}")),
            "receiver rejected upload status=409 code=delivery_conflict"
        );
        assert_eq!(
            rejection_log(409, None, None),
            "receiver rejected upload status=409 code=unknown"
        );
        assert_eq!(
            rejection_log(422, None, None),
            "receiver rejected upload status=422"
        );
    }
    #[test]
    fn a_refusal_logs_its_code_but_never_its_body() {
        assert_eq!(
            rejection_log(429, Some("delivery_busy"), Some(b"{}")),
            "receiver rejected upload status=429 code=delivery_busy"
        );
        assert_eq!(
            rejection_log(503, Some("delivery_unavailable"), Some(b"{}")),
            "receiver rejected upload status=503 code=delivery_unavailable"
        );
        assert_eq!(
            rejection_log(503, None, Some(b"<html>Bad gateway</html>")),
            "receiver rejected upload status=503"
        );
    }
    #[test]
    fn a_refusal_keeps_its_classification_and_retry_hint_with_the_body_read() {
        let refusal = |status, code: &str| {
            let body = format!("{{\"error\":{{\"code\":\"{code}\"}}}}");
            let head = format!("HTTP/1.1 {status} Refused\r\nRetry-After: 7\r\n\r\n{body}");
            let response = head.parse::<ureq::Response>().unwrap();
            // Not a request: a refusal must not need the submission parsed.
            http_error(ureq::Error::Status(status, response), "not json")
        };
        let busy = refusal(429, "delivery_busy");
        let busy = busy.downcast_ref::<ReceiverRejection>().unwrap();
        assert!(matches!(busy.failure, DeliveryFailure::RateLimited));
        assert!(busy.retry_after_ms.is_some());
        assert!(busy.recovery.is_none());
        let unavailable = refusal(503, "delivery_unavailable");
        let unavailable = unavailable.downcast_ref::<ReceiverRejection>().unwrap();
        assert!(matches!(unavailable.failure, DeliveryFailure::Transient));
        assert!(unavailable.recovery.is_none());
    }
    #[test]
    fn an_unrecognized_conflict_logs_a_short_single_line_excerpt() {
        assert_eq!(
            rejection_log(409, None, Some(b"<html>\nConflict</html>")),
            r#"receiver rejected upload status=409 code=unknown body="<html>\nConflict</html>""#
        );
        assert_eq!(
            rejection_log(409, Some("edge_conflict"), Some(b"busy")),
            r#"receiver rejected upload status=409 code=edge_conflict body="busy""#
        );
        let echoed = rejection_log(
            409,
            None,
            Some(b"Authorization: Bearer fake_token_0123456789abcdefghij refused"),
        );
        assert!(echoed.ends_with(r#"body="Authorization: Bearer [redacted] refused""#));
        let long = "x ".repeat(UNKNOWN_CONFLICT_EXCERPT);
        let line = rejection_log(409, None, Some(long.as_bytes()));
        assert!(line.ends_with(&format!("{}…\"", "x ".repeat(UNKNOWN_CONFLICT_EXCERPT / 2))));
        assert!(!line.contains('\n'));
    }
    #[test]
    fn only_a_typed_conflict_blocks_and_any_other_409_is_retried() {
        let wire_body = serde_json::to_string(&Request {
            protocol_version: 1,
            acquisition: vec![],
            batch: event_batch("text"),
        })
        .unwrap();
        for body in [
            "",
            "<html>Conflict</html>",
            r#"{"error":"fixture"}"#,
            r#"{"error":{"code":"edge_conflict"}}"#,
        ] {
            let response = ureq::Response::new(409, "Conflict", body).unwrap();
            let error = http_error(ureq::Error::Status(409, response), &wire_body);
            let rejection = error.downcast_ref::<ReceiverRejection>().unwrap();
            assert_eq!(rejection.failure, DeliveryFailure::Transient, "{body}");
            assert!(rejection.recovery.is_none());
        }
        let response =
            ureq::Response::new(409, "Conflict", r#"{"error":{"code":"delivery_conflict"}}"#)
                .unwrap();
        let error = http_error(ureq::Error::Status(409, response), &wire_body);
        assert_eq!(
            error.downcast_ref::<ReceiverRejection>().unwrap().failure,
            DeliveryFailure::DeliveryConflict
        );
    }
    #[test]
    fn an_unliftable_child_conflict_keeps_the_verdict_without_parent_recovery() {
        let parent = event_batch("parent");
        let submitted = child_batch(&parent, &parent.records);
        let error = ReceiverRejection {
            failure: DeliveryFailure::DeliveryConflict,
            status: 409,
            retry_after_ms: None,
            hold: None,
            recovery: Some(ConflictRecovery {
                // A child-scoped plan that does not bind to the exact child
                // must never be handed to the durable parent lease.
                parent_batch_id: "different-child".into(),
                submitted_digest: conflict::batch_digest(&submitted),
                quarantined: vec![],
                child_batch_id: "unused-child".into(),
            }),
        }
        .into();

        let error = lift_conflict(error, &parent, &submitted);
        let rejection = error.downcast_ref::<ReceiverRejection>().unwrap();
        assert_eq!(rejection.failure, DeliveryFailure::DeliveryConflict);
        assert_eq!(rejection.status, 409);
        assert!(rejection.recovery.is_none());
        assert_eq!(verdict(&error).failure, DeliveryFailure::DeliveryConflict);
    }
    fn event_batch(text: &str) -> HistoryExportBatch {
        serde_json::from_value(serde_json::json!({
            "schema_version": 1, "origin_id": "origin", "batch_id": "batch", "job_id": "job",
            "generation": 1, "destination_id": "relayhistory", "instance_id": "teams-probe",
            "account_id": "relayhistory:account", "mapping_version": MAPPING_VERSION,
            "records": [{
                "schema_version": 1, "kind": "session_event", "operation": "upsert",
                "origin_id": "origin", "record_id": "record", "revision": 1,
                "revision_id": "revision", "session_id": "session", "source": "codex",
                "payload": {"text": text, "nested": [{"a\0b": text}]}
            }]
        }))
        .unwrap()
    }
    #[test]
    fn prepared_bodies_carry_no_nul_the_receiver_would_reject() {
        // A tool result that printed a binary blocked a whole job on one record.
        let prepared = prepare(event_batch("\x7fELF\x02\x01\0\0\0 not stripped")).unwrap();
        assert!(!prepared.body.contains("\\u0000"));
        assert_eq!(prepared.sha256, hash(prepared.body.as_bytes()));
        let request: Request = serde_json::from_str(&prepared.body).unwrap();
        let payload = &request.batch.records[0].payload;
        assert_eq!(
            payload["text"],
            "\x7fELF\x02\x01\u{FFFD}\u{FFFD}\u{FFFD} not stripped"
        );
        assert!(payload["nested"][0]["a\u{FFFD}b"].is_string());
    }
    #[test]
    fn clean_and_legacy_bodies_are_sent_as_prepared_or_scrubbed() {
        let clean = prepare(event_batch("a literal \\u0000 escape is not a NUL")).unwrap();
        assert_eq!(storable_body(clean.body.clone()).unwrap(), clean.body);
        // A batch persisted by an older probe still holds the NUL.
        let legacy = serde_json::to_string(&Request {
            protocol_version: 1,
            acquisition: vec![],
            batch: event_batch("x\0y"),
        })
        .unwrap();
        let sent = storable_body(legacy).unwrap();
        assert!(!sent.contains("\\u0000") && sent.contains("x\u{FFFD}y"));
    }
    /// What lets `send` skip the repair for a body this build prepared:
    /// repairing a prepared body returns it byte for byte, whatever the
    /// preparation had to clean.
    #[test]
    fn a_prepared_body_is_already_in_wire_form() {
        let mut dirty = event_batch("nul \0 and a literal \\u0000");
        dirty.records[0].kind = "tool_call".into();
        dirty.records[0].payload = serde_json::json!({
            "text": "x\0y",
            "args_json": r#"{"command":"before\u0000after","a\u0000b":1,"a�b":2}"#,
            "token_json": r#"{"high":"\ud800","pair":"\ud83d\ude00"}"#,
            "payload_json": r#"{"unfinished":"\u0000""#,
        });
        let mut oversized = event_batch("clean");
        oversized.records[0].kind = "tool_call".into();
        oversized.records[0].payload = serde_json::json!({"args_json": "\\".repeat(600 * 1024)});
        // Its JSON text expands past the body cap and is shortened to fit.
        assert!(prepare(oversized.clone())
            .unwrap()
            .body
            .contains(JSON_TEXT_TRUNCATION_MARKER));
        for batch in [event_batch("clean"), dirty, oversized] {
            let prepared = prepare(batch).unwrap();
            assert_eq!(storable_body(prepared.body.clone()).unwrap(), prepared.body);
        }
    }
    #[test]
    fn a_literal_nul_escape_is_sent_byte_for_byte() {
        // Field order and spacing that re-serialization would change.
        let body = r#"{"z": 1, "a": "a literal \\u0000 escape", "m": {"y": [2, 1], "b": null}}"#;
        assert!(body.contains("\\u0000"));
        assert_eq!(storable_body(body.to_string()).unwrap(), body);
    }
    #[test]
    fn a_scrubbed_key_that_collides_keeps_both_values() {
        let body =
            r#"{"a\u0000b": 1, "a\ufffdb": 2, "a\ufffdb\ufffd1": 3, "c\u0000": 4, "c\ufffd": 5}"#;
        let sent: serde_json::Value =
            serde_json::from_str(&storable_body(body.to_string()).unwrap()).unwrap();
        let map = sent.as_object().unwrap();
        assert_eq!(map.len(), 5);
        // Clean keys keep their names and values.
        assert_eq!(map["a\u{FFFD}b"], 2);
        assert_eq!(map["a\u{FFFD}b\u{FFFD}1"], 3);
        assert_eq!(map["c\u{FFFD}"], 5);
        // Keys that held NUL are renamed to the next free suffix.
        assert_eq!(map["a\u{FFFD}b\u{FFFD}2"], 1);
        assert_eq!(map["c\u{FFFD}\u{FFFD}1"], 4);
    }
    #[test]
    fn json_text_is_cleaned_one_encoding_layer_down_without_false_escapes() {
        let mut batch = event_batch("ordinary text with a literal \\u0000 sequence");
        batch.records[0].kind = "tool_call".into();
        batch.records[0].payload = serde_json::json!({
            "args_json": r#"{"command":"printf '\u0000'","a\u0000b":1,"a�b":2}"#,
            "token_json": r#"{"literal":"\\u0000"}"#,
            "models_json": r#"{"clean":true}"#
        });
        let prepared = prepare(batch).unwrap();
        let request: Request = serde_json::from_str(&prepared.body).unwrap();
        let payload = &request.batch.records[0].payload;
        let args: serde_json::Value =
            serde_json::from_str(payload["args_json"].as_str().unwrap()).unwrap();
        assert_eq!(args["command"], "printf '\u{FFFD}'");
        assert_eq!(args["a\u{FFFD}b"], 2);
        assert_eq!(args["a\u{FFFD}b\u{FFFD}1"], 1);
        assert_eq!(payload["token_json"], r#"{"literal":"\\u0000"}"#);
        assert_eq!(payload["models_json"], r#"{"clean":true}"#);
    }
    #[test]
    fn legacy_json_text_is_scrubbed_on_the_wire_without_changing_identity() {
        let mut batch = event_batch("clean");
        batch.records[0].kind = "tool_call".into();
        batch.records[0].payload =
            serde_json::json!({"args_json": r#"{"command":"before\u0000after"}"#});
        let revision_id = batch.records[0].revision_id.clone();
        let legacy = serde_json::to_string(&Request {
            protocol_version: 1,
            acquisition: vec![],
            batch,
        })
        .unwrap();
        // The embedded escape is itself escaped by the outer request string.
        assert!(legacy.contains(r#"\\u0000"#));
        let sent = storable_body(legacy.clone()).unwrap();
        assert_ne!(sent, legacy);
        let request: Request = serde_json::from_str(&sent).unwrap();
        assert_eq!(request.batch.records[0].revision_id, revision_id);
        let args: serde_json::Value = serde_json::from_str(
            request.batch.records[0].payload["args_json"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(args["command"], "before\u{FFFD}after");
    }
    #[test]
    fn clean_json_text_keeps_the_whole_request_byte_for_byte() {
        let mut batch = event_batch("clean");
        batch.records[0].payload = serde_json::json!({
            "args_json": " { \"literal\" : \"\\\\u0000\", \"pair\" : \"\\ud83d\\ude00\" } "
        });
        let body = serde_json::to_string(&Request {
            protocol_version: 1,
            acquisition: vec![],
            batch,
        })
        .unwrap();
        assert_eq!(storable_body(body.clone()).unwrap(), body);
    }
    #[test]
    fn malformed_json_is_opaque_and_lone_surrogates_are_replaced_in_place() {
        let mut batch = event_batch("clean");
        batch.records[0].payload = serde_json::json!({
            "args_json": r#"{"high":"\ud800","low":"\udc00","pair":"\ud83d\ude00"}"#,
            "payload_json": r#"{"unfinished":"\u0000""#
        });
        let prepared = prepare(batch).unwrap();
        let request: Request = serde_json::from_str(&prepared.body).unwrap();
        let payload = &request.batch.records[0].payload;
        let args: serde_json::Value =
            serde_json::from_str(payload["args_json"].as_str().unwrap()).unwrap();
        assert_eq!(args["high"], "\u{FFFD}");
        assert_eq!(args["low"], "\u{FFFD}");
        assert_eq!(args["pair"], "😀");
        let opaque: String =
            serde_json::from_str(payload["payload_json"].as_str().unwrap()).unwrap();
        assert!(opaque.contains("\\u0000"));
        assert!(!opaque.contains('\0'));
    }
    #[test]
    fn legacy_malformed_json_expansion_is_shortened_to_the_wire_cap() {
        let mut batch = event_batch("clean");
        batch.records[0].kind = "tool_call".into();
        batch.records[0].payload = serde_json::json!({"args_json": "\\".repeat(600 * 1024)});
        let revision_id = batch.records[0].revision_id.clone();
        let legacy = serde_json::to_string(&Request {
            protocol_version: 1,
            acquisition: vec![],
            batch,
        })
        .unwrap();
        assert!(legacy.len() < MAX_BODY_BYTES);
        let sent = storable_body(legacy).unwrap();
        assert!(sent.len() <= MAX_BODY_BYTES);
        let request: Request = serde_json::from_str(&sent).unwrap();
        assert_eq!(request.batch.records[0].revision_id, revision_id);
        let opaque: String = serde_json::from_str(
            request.batch.records[0].payload["args_json"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert!(opaque.len() < 600 * 1024);
        assert!(opaque.ends_with(JSON_TEXT_TRUNCATION_MARKER));
    }
    #[test]
    fn a_receipt_with_fields_this_probe_does_not_know_still_acknowledges() {
        // A receiver that has already stored the batch answers the same way on
        // every retry, so rejecting an additive field blocks delivery for good.
        let batch: HistoryExportBatch = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "origin_id": "origin", "batch_id": "batch", "job_id": "job",
            "generation": 1, "destination_id": "relayhistory", "instance_id": "teams-probe",
            "account_id": "relayhistory:account", "mapping_version": MAPPING_VERSION, "records": []
        }))
        .unwrap();
        let acknowledged = receipt(
            &serde_json::json!({
                "protocolVersion": 1, "batchId": "batch", "acceptedRevisionIds": [],
                "unsupportedRevisionIds": [], "acceptanceLevel": "durable",
                "receiptId": "rhr_0000000000000000000000000000000000000000000000000000000000000000"
            }),
            &batch,
        )
        .unwrap();
        assert_eq!(acknowledged.batch_id, "batch");
    }
    #[test]
    fn a_success_that_is_no_receipt_is_retried() {
        for (status, body) in [
            (202, "{}"),
            (204, ""),
            (200, "<html>Sign in to Wi-Fi</html>"),
        ] {
            let response = ureq::Response::new(status, "OK", body).unwrap();
            let error = response_json(response).unwrap_err();
            assert_eq!(
                verdict(&error),
                DeliveryFailure::Transient.into(),
                "{status}"
            );
        }
    }

    #[test]
    fn a_command_budget_ends_at_its_deadline() {
        let budget = command_budget(Duration::ZERO);
        assert!(budget().is_err());
        assert!(command_budget(Duration::from_secs(1))().is_ok());
    }

    #[test]
    fn only_a_transport_verdict_can_block_the_job() {
        // A lease, storage or consent-recheck error must not be recorded as a
        // nonretryable payload verdict: that blocks delivery until an explicit
        // retry even after the condition clears.
        assert_eq!(
            verdict(&anyhow::anyhow!(
                "Could not inspect older upload schedules."
            )),
            DeliveryFailure::Transient.into()
        );
        assert_eq!(
            verdict(&anyhow::anyhow!("delivery batch missing")),
            DeliveryFailure::Transient.into()
        );
        assert_eq!(
            verdict(
                &anyhow::Error::from(TransportFailure(DeliveryFailure::InvalidPayload))
                    .context("preparing batch")
            ),
            DeliveryFailure::InvalidPayload.into()
        );
    }

    fn schedules(state: &'static str, jobs: &[&'static str]) -> MigrationStatus {
        MigrationStatus {
            state,
            jobs: jobs.to_vec(),
        }
    }

    /// A conclusive scheduler verdict is reused for its time to live, then
    /// inspected again; an `active` one keeps holding every batch while it
    /// is reused, as the retried `legacy_uploader_active`, never a
    /// permission refusal.
    #[test]
    fn the_scheduler_verdict_is_reused_only_for_its_time_to_live() {
        let receiver = RelayHistoryReceiver {
            acknowledge_uninspected_schedules: true,
            ..Default::default()
        };
        let inspections = std::cell::Cell::new(0);
        let inspect = |state| {
            inspections.set(inspections.get() + 1);
            schedules(state, &["launchd-loaded"])
        };
        assert_eq!(
            receiver.state.schedules(|| inspect("active")).state,
            "active"
        );
        assert_eq!(
            receiver.state.schedules(|| inspect("clear")).state,
            "active"
        );
        assert_eq!(inspections.get(), 1);
        // The receiver reuses that verdict for the batch.
        let batch = event_batch("text");
        let failure = receiver.admit(&batch).unwrap_err();
        assert_eq!(failure, DeliveryFailure::LegacyUploaderActive.into());
        assert_ne!(failure, DeliveryFailure::PermissionDenied.into());
        assert!(DeliveryFailure::LegacyUploaderActive.retryable());
        // Once the verdict is older than its time to live, it is inspected again.
        *receiver.state.0.schedules.lock().unwrap() = Some((
            Instant::now()
                .checked_sub(SCHEDULER_GUARD_TTL + Duration::from_secs(1))
                .unwrap(),
            schedules("active", &["launchd-loaded"]),
        ));
        assert_eq!(receiver.state.schedules(|| inspect("clear")).state, "clear");
        assert_eq!(inspections.get(), 2);
        assert!(receiver.admit(&batch).is_ok());
        // Clones share the verdict, so a receiver copied per pass does not
        // inspect again.
        assert_eq!(
            receiver.clone().state.schedules(|| inspect("active")).state,
            "clear"
        );
        assert_eq!(inspections.get(), 2);
    }

    /// An inspection that could not finish (a `launchctl` or `crontab` read
    /// that failed or timed out) is a transient, retried failure. It is not
    /// reused, so the retry inspects again and delivery resumes as soon as an
    /// inspection succeeds. It used to block the job as `permission_denied`
    /// until someone retried by hand.
    #[test]
    fn an_inconclusive_inspection_is_retried_and_never_reused() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn flaky() -> MigrationStatus {
            if CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
                MigrationStatus {
                    state: "unknown",
                    jobs: vec!["launchd-status-unknown"],
                }
            } else {
                MigrationStatus {
                    state: "clear",
                    jobs: vec![],
                }
            }
        }
        let receiver = RelayHistoryReceiver {
            state: ReceiverState::inspecting_with(flaky),
            ..Default::default()
        };
        let batch = event_batch("text");
        let failure = receiver.admit(&batch).unwrap_err();
        assert_eq!(failure, DeliveryFailure::Transient.into());
        assert!(DeliveryFailure::Transient.retryable());
        assert_eq!(
            receiver.state.last_legacy_schedules().unwrap().jobs,
            ["launchd-status-unknown"]
        );
        // The very next attempt inspects again, well inside the time to live.
        assert!(receiver.admit(&batch).is_ok());
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
        assert_eq!(
            receiver.state.last_legacy_schedules().unwrap().state,
            "clear"
        );
        // The clear verdict is the one reused.
        assert!(receiver.admit(&batch).is_ok());
        assert_eq!(CALLS.load(Ordering::SeqCst), 2);
    }

    /// The person's acknowledgement still lets an inconclusive inspection
    /// through, and never an active uploader.
    #[test]
    fn acknowledged_unknown_passes_but_active_does_not() {
        fn unknown() -> MigrationStatus {
            MigrationStatus {
                state: "unknown",
                jobs: vec!["cron-status-unknown"],
            }
        }
        fn active() -> MigrationStatus {
            MigrationStatus {
                state: "active",
                jobs: vec!["managed-cron"],
            }
        }
        let batch = event_batch("text");
        let acknowledged = |inspect| RelayHistoryReceiver {
            acknowledge_uninspected_schedules: true,
            state: ReceiverState::inspecting_with(inspect),
            ..Default::default()
        };
        assert!(acknowledged(unknown).admit(&batch).is_ok());
        assert_eq!(
            acknowledged(active).admit(&batch).unwrap_err(),
            DeliveryFailure::LegacyUploaderActive.into()
        );
        assert!(ReceiverState::hosted().last_legacy_schedules().is_none());
    }

    /// The account assertion is unchanged: a batch for another account is a
    /// permission refusal, which blocks the job.
    #[test]
    fn an_account_mismatch_is_still_permission_denied() {
        fn clear() -> MigrationStatus {
            MigrationStatus {
                state: "clear",
                jobs: vec![],
            }
        }
        let batch = event_batch("text");
        let receiver = RelayHistoryReceiver {
            expected_account: Some(format!("{}-other", batch.account_id)),
            state: ReceiverState::inspecting_with(clear),
            ..Default::default()
        };
        assert_eq!(
            receiver.admit(&batch).unwrap_err(),
            DeliveryFailure::PermissionDenied.into()
        );
        assert!(!DeliveryFailure::PermissionDenied.retryable());
    }

    /// Debug output of a receiver never carries the session it keeps.
    #[test]
    fn a_receiver_never_prints_its_session() {
        let receiver = RelayHistoryReceiver::default();
        receiver.state.0.session.keep(
            None,
            cloud::StoredAuth {
                access_token: "rth_at_secret".into(),
                refresh_token: Some("rth_rt_secret".into()),
                ..Default::default()
            },
        );
        assert!(!format!("{receiver:?}").contains("secret"));
    }

    /// A request that got no response is logged by the kind of failure, and
    /// a TLS refusal or a timeout is named whichever step it ended.
    #[test]
    fn a_transport_failure_is_named_by_its_kind() {
        use std::io::{Error, ErrorKind};
        let tls = || Error::new(ErrorKind::InvalidData, rustls::Error::General("x".into()));
        for (kind, source, cause) in [
            (ureq::ErrorKind::Dns, None, "dns"),
            (
                ureq::ErrorKind::ConnectionFailed,
                Some(Error::from(ErrorKind::ConnectionRefused)),
                "connect",
            ),
            (ureq::ErrorKind::ProxyConnect, None, "connect"),
            (ureq::ErrorKind::ConnectionFailed, Some(tls()), "tls"),
            (ureq::ErrorKind::Io, Some(tls()), "tls"),
            (
                ureq::ErrorKind::ConnectionFailed,
                Some(Error::from(ErrorKind::TimedOut)),
                "timeout",
            ),
            (
                ureq::ErrorKind::Io,
                Some(Error::from(ErrorKind::TimedOut)),
                "timeout",
            ),
            (
                ureq::ErrorKind::Io,
                Some(Error::from(ErrorKind::WouldBlock)),
                "timeout",
            ),
            (
                ureq::ErrorKind::Io,
                Some(Error::from(ErrorKind::ConnectionReset)),
                "io",
            ),
            (ureq::ErrorKind::BadStatus, None, "protocol"),
            (ureq::ErrorKind::BadHeader, None, "protocol"),
            (ureq::ErrorKind::InvalidUrl, None, "other"),
        ] {
            assert_eq!(transport_cause(kind, source.as_ref()), cause, "{kind:?}");
        }
    }

    /// The log line of a real refused connection and a real timeout names
    /// the cause and nothing of the request: no host, path, query or token.
    #[test]
    fn a_logged_transport_failure_carries_no_url_or_credential() {
        let post = |url: &str, timeout: Duration| {
            let error = ureq::AgentBuilder::new()
                .build()
                .post(url)
                .timeout(timeout)
                .set("Authorization", "Bearer secret-bearer")
                .send_bytes(b"{\"private\":\"body\"}")
                .unwrap_err();
            let ureq::Error::Transport(transport) = error else {
                panic!("expected a transport failure, found {error}");
            };
            // The error's own text carries the URL, which is why it is not
            // what the log writes.
            assert!(transport.to_string().contains("127.0.0.1"), "{transport}");
            transport_log(&transport)
        };
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let refused = format!(
            "http://{}/v1/delivery/batches?token=secret-query",
            closed.local_addr().unwrap()
        );
        drop(closed);
        let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let unanswered = format!(
            "http://{}/v1/delivery/batches?token=secret-query",
            silent.local_addr().unwrap()
        );
        let held = std::thread::spawn(move || {
            let (stream, _) = silent.accept().unwrap();
            std::thread::sleep(Duration::from_secs(2));
            drop(stream);
        });
        // A refused loopback connection is immediate on Unix; Windows retries
        // the connect for about two seconds before it reports the refusal,
        // so the refused case waits well past that and is never a timeout.
        for (url, timeout, cause) in [
            (refused, Duration::from_secs(20), "cause=connect"),
            (unanswered, Duration::from_millis(300), "cause=timeout"),
        ] {
            let line = post(&url, timeout);
            assert!(line.ends_with(cause), "{line}");
            for secret in [
                "127.0.0.1",
                "delivery",
                "secret",
                "token",
                "Bearer",
                "private",
                "http",
            ] {
                assert!(!line.contains(secret), "{line}");
            }
        }
        held.join().unwrap();
    }

    /// A send's budget is checked many times; once it has ended, every
    /// check fails the send as transient, and the end is logged once, named
    /// by what ended it.
    #[test]
    fn an_ended_send_budget_fails_every_check_and_is_logged_once() {
        for (timeout, cancelled, cause) in [
            (Duration::ZERO, false, "timeout"),
            (Duration::from_secs(60), true, "cancelled"),
        ] {
            let stop = move || cancelled;
            let budget = CallBudget {
                started: Instant::now(),
                timeout,
                cancelled: &stop,
                reported: std::sync::atomic::AtomicBool::new(false),
            };
            let mut log = Vec::new();
            for _ in 0..3 {
                let error = budget.remaining_logging_to(&mut log).unwrap_err();
                assert_eq!(verdict(&error).failure, DeliveryFailure::Transient);
            }
            assert_eq!(
                String::from_utf8(log).unwrap(),
                format!("receiver request ended by its budget; retrying later cause={cause}\n")
            );
        }
        // A budget with time left and no stop logs nothing.
        let budget = CallBudget {
            started: Instant::now(),
            timeout: Duration::from_secs(60),
            cancelled: &|| false,
            reported: std::sync::atomic::AtomicBool::new(false),
        };
        let mut log = Vec::new();
        assert!(budget.remaining_logging_to(&mut log).is_ok());
        assert!(log.is_empty());
    }
}

#[cfg(test)]
mod consent_error_tests {
    use super::*;

    #[test]
    fn a_refused_consent_request_names_what_to_fix() {
        let status = |code: u16| {
            ureq::Error::Status(code, ureq::Response::new(code, "refused", "").unwrap())
        };
        let mapped = consent_error("Try again.");
        assert_eq!(
            failure_of(&mapped(status(401))),
            Some(DeliveryFailure::AuthenticationRequired)
        );
        assert_eq!(
            failure_of(&mapped(status(403))),
            Some(DeliveryFailure::PermissionDenied)
        );
        let other = mapped(status(503));
        assert_eq!(failure_of(&other), None);
        assert_eq!(other.to_string(), "Try again.");
    }
}
