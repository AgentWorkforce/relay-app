//! Session batches put straight into the workspace's upload inbox when
//! Control hands out upload credentials. The batch's zstd bytes become one
//! create-only object; once S3 stores it, the receiver's admission decides
//! the batch: admitted acknowledges it as queued, refused sets it aside as
//! a batch History rejects, and pending keeps the object's key on the batch
//! so a later pass (after a restart too) asks again instead of putting it
//! once more. Whenever Control declines or an answer cannot be used, the
//! send returns `None` and the batch is posted to the receiver as before.
//! Once a PUT is made, any answer but 200 or 412 leaves the batch for a
//! retry under the same key (the contract's rule).
//!
//! What Control answers (its contract): this build asks for session (`sts`)
//! credentials first, and for presigned URLs once Control answers 501
//! `unsupported_by_backend` (a store without STS; see `ask`). Control
//! answers with the kind asked for or an error, so an answer of another
//! kind is unusable and the batch is posted. A 404
//! `history_upload_direct_off` keeps the POST path and is remembered for
//! the session token that asked, as is a 404 showing the API root has no
//! such route (not JSON, no code, or a router's `not_found`: a site whose
//! Control predates direct uploads); a 404 naming any other code is the
//! workspace route's own and fails the send as the sync path fails a 404. A 503 or 429 posts
//! that batch only (a 429 keeps its `Retry-After`).
//!
//! Credentials, presigned URLs and the witness live in this process's
//! memory only, per receiver and for the session they were asked with
//! (its token and the stage's session generation, so a session the host
//! replaced takes them along), and are asked for again after a restart: they are never written
//! to delivery-queue.db (a job's config or its batches) or history.db, as a
//! delivery job's config holds no authentication material. Only the object
//! key, which names the object and carries no authority, is kept on a
//! batch. The session they are asked with stays in the stage's auth file.
//! Before every PUT the stage's current session must still be the one the
//! credentials were asked with, and its account the batch's. Nothing here
//! is logged but fixed causes: never a token, credential or witness.
//!
//! A site build may name storage origins its PUTs may reach on a private
//! address (`private_storage`); a PUT to one goes through storage-only
//! agents, and nothing else changes.
mod admission;
mod ask;
mod credentials;
mod private_storage;
mod sigv4;
mod storage;
#[cfg(test)]
mod storage_origins;
#[cfg(test)]
mod vectors_tests;

use super::{
    check_account, cloud, hash,
    post::{fresh_agent, http_agent, resend_once},
    request_for_wire,
    tls::verdict,
    validate_request, wire_request, CallBudget, ContentEncoding, DeliveryAcknowledgment,
    HistoryExportBatch, PreparedPayload, ReceiverContext, ReceiverFailure, Result, Stage,
    MAPPING_VERSION,
};
use admission::decide;
use base64::Engine;
use chrono::Utc;
use credentials::Grant;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, time::Duration};
use storage::{put, unstored, Put};

/// The longest a credentials request may take of a send's budget.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest object key Control takes below the served prefix.
const MAX_RELATIVE_KEY: usize = 512;
/// The only 404 code that means direct upload is off.
const OFF: &str = "history_upload_direct_off";

/// Where a receiver asks for upload credentials: Control's API root and the
/// workspace, and where admission is asked after (the receiver's own base
/// URL when unset). Holds no credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectUploadSource {
    pub api_url: String,
    pub workspace_id: String,
    pub admission_base_url: Option<String>,
}

/// What a receiver remembers about each source between sends: whether
/// Control said the feature is off, when it asked to wait, and the last
/// credentials it handed out. Process memory only, so a restart asks again.
#[derive(Default)]
pub(super) struct Memory(HashMap<(String, String), Entry>);
impl Memory {
    /// The host replaced the session: no credentials asked with the old one
    /// are kept, nor what Control said about presigned URLs to it.
    pub(super) fn forget_grants(&mut self) {
        for entry in self.0.values_mut() {
            entry.grant = None;
            entry.presigned = None;
        }
    }
}
impl std::fmt::Debug for Memory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Memory")
            .field("sources", &self.0.len())
            .finish_non_exhaustive()
    }
}
#[derive(Default)]
struct Entry {
    /// The digest of the session token Control said "off" to: another token
    /// (a refreshed or new sign-in) asks again.
    off: Option<String>,
    /// The epoch-ms time a busy Control asked the next request to wait for.
    not_before_ms: Option<i64>,
    /// The last credentials and the session they were asked with: another
    /// token (another user, too) or a replaced session asks again.
    grant: Option<Kept>,
    /// Control answered an `sts` ask of this session 501
    /// `unsupported_by_backend`: later asks are for presigned URLs.
    presigned: Option<PresignedOnly>,
    /// Whether the one log line for an unusable source was written.
    warned: bool,
}

/// Credentials and the session that obtained them: the digest of its
/// token and the stage's session generation then.
#[derive(Clone)]
struct Kept {
    token: String,
    epoch: u64,
    grant: Grant,
}
impl Kept {
    fn of(&self, token: &str, epoch: u64) -> bool {
        self.token == token && self.epoch == epoch
    }
}

/// A store without STS, as Control told a session (the digest of its token
/// and the stage's session generation then), and the witness seq its last
/// presigned answer carried, which the next presigned key names.
#[derive(Clone)]
struct PresignedOnly {
    token: String,
    epoch: u64,
    seq: Option<u64>,
}
impl PresignedOnly {
    fn of(&self, token: &str, epoch: u64) -> bool {
        self.token == token && self.epoch == epoch
    }
}

/// One batch as an object: its zstd bytes and what names it.
struct Upload {
    body: Vec<u8>,
    sha256: [u8; 32],
    origin_id: String,
    batch_id: String,
    account_id: String,
    revision_ids: Vec<String>,
}
impl Upload {
    /// The batch a payload carries, when it can go as an object: a body this
    /// receiver could post, without acquisition grants (only the receiver
    /// checks those), from an origin that is a valid key segment, and
    /// coded within the largest object the receiver admits, checked before
    /// any credentials are asked for. Anything else is posted, and the POST
    /// path reports what is wrong with it.
    fn of(payload: &PreparedPayload) -> Option<Self> {
        if payload.mapping_version != MAPPING_VERSION
            || payload.content_type != "application/json"
            || payload.sha256 != hash(payload.body.as_bytes())
        {
            return None;
        }
        let (body, request) = if payload.prepared_by_this_build() {
            wire_request(payload.body.clone())
        } else {
            request_for_wire(payload)
        }
        .ok()?;
        let postable = request.protocol_version == 1 && validate_request(&request).is_ok();
        if !postable || !request.acquisition.is_empty() || !origin_valid(&request.batch.origin_id) {
            return None;
        }
        let body = coded(body.as_bytes())?;
        let upload = Self {
            sha256: Sha256::digest(&body).into(),
            body,
            origin_id: request.batch.origin_id,
            batch_id: request.batch.batch_id,
            account_id: request.batch.account_id,
            revision_ids: request
                .batch
                .records
                .into_iter()
                .map(|record| record.revision_id)
                .collect(),
        };
        relative_key_valid(&upload.key("", u64::MAX)).then_some(upload)
    }
    /// The object's key under `prefix` for a witness of `seq`.
    fn key(&self, prefix: &str, seq: u64) -> String {
        object_key(prefix, &self.origin_id, &self.batch_id, &self.sha256, seq)
    }
    /// `x-amz-checksum-sha256`: the base64 of the bytes' raw SHA-256.
    fn checksum(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.sha256)
    }
}

/// The key of a batch's object: the upload inbox prefix as Control served
/// it, then `o=<origin>/b=<batch>/d=<digest>-w<seq>.json.zst`, where batch
/// and digest are the first 32 hex digits of the SHA-256 of the batch id
/// and of the object bytes, and `seq` is the witness's journal seq.
fn object_key(
    prefix: &str,
    origin_id: &str,
    batch_id: &str,
    sha256: &[u8; 32],
    seq: u64,
) -> String {
    let batch = sigv4::hex(&Sha256::digest(batch_id.as_bytes()));
    let digest = sigv4::hex(sha256);
    format!(
        "{prefix}o={origin_id}/b={}/d={}-w{seq}.json.zst",
        &batch[..32],
        &digest[..32]
    )
}

/// An origin id the receiver takes (decision 0004: `[A-Za-z0-9._-]{1,128}`,
/// not starting with a dot). Control's key rule is wider, so its `o=`
/// segment always passes there too.
fn origin_valid(origin_id: &str) -> bool {
    (1..=128).contains(&origin_id.len())
        && !origin_id.starts_with('.')
        && origin_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

/// A key relative to the served prefix that Control's key rule takes: at
/// most 512 characters, each `/`-separated segment matching
/// `^[A-Za-z0-9][A-Za-z0-9._=-]{0,159}$`.
fn relative_key_valid(key: &str) -> bool {
    (1..=MAX_RELATIVE_KEY).contains(&key.len())
        && key.split('/').all(|segment| {
            (1..=160).contains(&segment.len())
                && segment
                    .bytes()
                    .next()
                    .is_some_and(|b| b.is_ascii_alphanumeric())
                && segment
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._=-".contains(&b))
        })
}

/// Whether a PUT may go to the storage URL `url` (an `endpoint`-built or a
/// presigned one): https to any host, and plain http only to loopback
/// (`localhost` or a loopback IP literal) unless the development flag is
/// set, as a History session's bearer is sent. A URL with userinfo is
/// never put to. A local certificate override refuses the connection, as
/// it does for History.
fn allowed(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    matches!(parsed.scheme(), "http" | "https")
        && parsed.host().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && cloud::require_secure_transport(&parsed.origin().ascii_serialization()).is_ok()
}

/// One send's view of its source.
struct Call<'a> {
    stage: Stage<'a>,
    source: &'a DirectUploadSource,
    account: &'a str,
    budget: &'a CallBudget<'a>,
    /// The digest of the stage's session token when the send began.
    token: String,
    /// The stage's session generation when the send began.
    epoch: u64,
}

/// The zstd object for a wire body, or `None` when it is larger than the
/// largest object the receiver admits. `Upload::of` checks this before a
/// credentials ask, so an oversized batch is posted without spending one.
fn coded(body: &[u8]) -> Option<Vec<u8>> {
    let body = ContentEncoding::Zstd.encode(body)?;
    u64::try_from(body.len())
        .is_ok_and(|len| len <= credentials::MAX_OBJECT_BYTES)
        .then_some(body)
}

/// What a direct send ends in; the failure is boxed, as it is large.
pub(super) type Sent<T> = std::result::Result<T, Box<ReceiverFailure>>;

/// The direct route for a send, or `None` to post the batch. A batch whose
/// object an earlier pass stored waits on admission only: it is never put
/// or posted again. Otherwise only a first send goes direct: a retry
/// carries an earlier verdict that the batch route resumes from.
pub(super) fn route(
    stage: Stage<'_>,
    source: Option<&DirectUploadSource>,
    payload: &PreparedPayload,
    batch: &HistoryExportBatch,
    context: &ReceiverContext<'_>,
    budget: &CallBudget<'_>,
) -> Option<std::result::Result<DeliveryAcknowledgment, ReceiverFailure>> {
    if let Some(key) = context.upload_key {
        return Some(resume(stage, source, key, batch, budget).map_err(|failure| *failure));
    }
    let first = context.accepted_revision_ids.is_empty()
        && context.suppressed_revision_ids.is_empty()
        && context.rejected_revision_ids.is_empty()
        && context.isolation_frontier.is_empty();
    let source = source.filter(|_| first)?;
    let sent = send(stage, source, payload, budget).transpose()?;
    Some(sent.map_err(|failure| *failure))
}

/// Put the batch into the upload inbox. `Some` acknowledgment when S3
/// stored it and the pipeline took it; `None` to post it instead; a
/// failure the POST path would also have failed with, or, once the object
/// is stored, one that keeps its key on the batch (pending) or sets the
/// batch aside (refused).
fn send(
    stage: Stage<'_>,
    source: &DirectUploadSource,
    payload: &PreparedPayload,
    budget: &CallBudget<'_>,
) -> Sent<Option<DeliveryAcknowledgment>> {
    let Some(upload) = Upload::of(payload) else {
        return Ok(None);
    };
    let epoch = stage.epoch();
    let Ok(session) = stage.session() else {
        return Ok(None);
    };
    let call = Call {
        token: hash(session.access_token.as_bytes()),
        epoch,
        stage,
        source,
        account: &upload.account_id,
        budget,
    };
    if !call.usable() {
        return Ok(None);
    }
    let Some(key) = call
        .store(&upload)
        .map_err(|error| Box::new(verdict(&error)))?
    else {
        return Ok(None);
    };
    decide(&call, &key, &upload.batch_id, &upload.revision_ids).map(Some)
}

/// Ask again about the object an earlier send stored for `batch` under
/// `key`: nothing is put and no credentials are asked for. Admission is
/// asked where the source names, else at the receiver's own base URL.
fn resume(
    stage: Stage<'_>,
    source: Option<&DirectUploadSource>,
    key: &str,
    batch: &HistoryExportBatch,
    budget: &CallBudget<'_>,
) -> Sent<DeliveryAcknowledgment> {
    let receiver_only = DirectUploadSource {
        api_url: String::new(),
        workspace_id: String::new(),
        admission_base_url: None,
    };
    let epoch = stage.epoch();
    let session = stage.session().map_err(|error| Box::new(verdict(&error)))?;
    let call = Call {
        token: hash(session.access_token.as_bytes()),
        epoch,
        stage,
        source: source.unwrap_or(&receiver_only),
        account: &batch.account_id,
        budget,
    };
    let revisions: Vec<String> = batch
        .records
        .iter()
        .map(|record| record.revision_id.clone())
        .collect();
    decide(&call, key, &batch.batch_id, &revisions)
}

impl Call<'_> {
    fn remember<T>(&self, change: impl FnOnce(&mut Entry) -> T) -> T {
        let mut memory = self
            .stage
            .published
            .0
            .direct
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let source = (
            self.source.api_url.clone(),
            self.source.workspace_id.clone(),
        );
        change(memory.0.entry(source).or_default())
    }
    /// Log, once per source, why a batch is posted instead.
    fn note(&self, cause: &str) {
        if !self.remember(|entry| std::mem::replace(&mut entry.warned, true)) {
            eprintln!("direct upload unavailable; posting batches instead cause={cause}");
        }
    }
    /// Whether to ask at all: not while Control's "off" stands for this
    /// session token, nor while it asked to wait. Another token, or a
    /// session the host replaced, drops what was kept for the last one.
    fn usable(&self) -> bool {
        let now = Utc::now().timestamp_millis();
        self.remember(|entry| {
            if entry.off.as_ref().is_some_and(|off| *off != self.token) {
                entry.off = None;
            }
            if entry
                .grant
                .as_ref()
                .is_some_and(|kept| !kept.of(&self.token, self.epoch))
            {
                entry.grant = None;
            }
            if entry
                .presigned
                .as_ref()
                .is_some_and(|only| !only.of(&self.token, self.epoch))
            {
                entry.presigned = None;
            }
            entry.off.is_none() && !entry.not_before_ms.is_some_and(|at| at > now)
        })
    }
    /// Store the object, renewing the credentials once when S3 says they
    /// expired or the session changed under them: its key, `None` to post
    /// the batch, or an error that keeps it for a retry. Every PUT, kept
    /// credentials' included, first checks the session as a POST would.
    fn store(&self, upload: &Upload) -> Result<Option<String>> {
        for renew in [false, true] {
            let Some(kept) = self.grant(upload, renew)? else {
                return Ok(None);
            };
            if !self.bound(&kept)? {
                self.remember(|entry| {
                    if entry
                        .grant
                        .as_ref()
                        .is_some_and(|held| held.of(&kept.token, kept.epoch))
                    {
                        entry.grant = None;
                    }
                });
                if renew {
                    return Ok(None);
                }
                continue;
            }
            let grant = kept.grant;
            if u64::try_from(upload.body.len()).map_or(true, |len| len > grant.max_object_bytes) {
                return Ok(None);
            }
            let key = upload.key(&grant.key_prefix, grant.witness_seq);
            match put(self, &grant, upload, &key)? {
                Put::Stored => return Ok(Some(key)),
                Put::Unsent => return Ok(None),
                Put::Expired => {}
            }
        }
        Err(unstored(403, None))
    }
    /// Whether `kept` may put now: the stage's current session must be the
    /// batch account's, as a POST checks, and the one the credentials were
    /// asked with (the same token and session generation).
    fn bound(&self, kept: &Kept) -> Result<bool> {
        let session = self.stage.session()?;
        check_account(&session, self.account)?;
        let token = hash(session.access_token.as_bytes());
        Ok(kept.of(&token, self.stage.epoch()) && kept.epoch == self.epoch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_object_over_the_receiver_ceiling_is_never_coded_for_upload() {
        // Hex of hash-chained bytes halves under zstd: about 2.6 MiB of
        // object from 5.2 MiB of text, over the ceiling.
        let mut block = [0u8; 32];
        let mut text = |blocks: usize| -> Vec<u8> {
            (0..blocks)
                .flat_map(|_| {
                    block = Sha256::digest(block).into();
                    sigv4::hex(&block).into_bytes()
                })
                .collect()
        };
        let large = text(85_000);
        let object = ContentEncoding::Zstd.encode(&large).unwrap();
        assert!(u64::try_from(object.len()).unwrap() > credentials::MAX_OBJECT_BYTES);
        assert!(coded(&large).is_none());
        let small = text(1_024);
        let object = coded(&small).unwrap();
        assert_eq!(zstd::bulk::decompress(&object, small.len()).unwrap(), small);
    }

    #[test]
    fn an_object_key_is_the_served_prefix_then_origin_batch_digest_and_seq() {
        let sha256: [u8; 32] = Sha256::digest(b"zstd frame").into();
        let key = object_key(
            "fixture-inbox/ws=ws-fixture/u=u1/",
            "origin-1",
            "batch-1",
            &sha256,
            7,
        );
        let batch = sigv4::hex(&Sha256::digest(b"batch-1"));
        let digest = sigv4::hex(&sha256);
        assert_eq!(
            key,
            format!(
                "fixture-inbox/ws=ws-fixture/u=u1/o=origin-1/b={}/d={}-w7.json.zst",
                &batch[..32],
                &digest[..32]
            )
        );
        // The longest key under the prefix meets Control's key rule.
        let origin = "o".repeat(128);
        assert!(relative_key_valid(&object_key(
            "",
            &origin,
            "batch-1",
            &sha256,
            u64::MAX
        )));
    }

    #[test]
    fn an_origin_is_checked_by_the_receiver_rule() {
        assert!(origin_valid("origin_1.a-b"));
        assert!(origin_valid(&"a".repeat(128)));
        for origin in [
            "",
            ".hidden",
            "a:b",
            "a=b",
            "a/b",
            "a b",
            "é",
            &"a".repeat(129),
        ] {
            assert!(!origin_valid(origin), "{origin:?}");
        }
    }

    #[test]
    fn a_relative_key_is_bounded_by_length_and_segment_rule() {
        let segment = format!("a{}", "b".repeat(159));
        let long = [segment.as_str(); 3].join("/");
        let longest = format!("{long}/{}", "c".repeat(29));
        assert_eq!(longest.len(), 512);
        assert!(relative_key_valid(&longest));
        assert!(!relative_key_valid(&format!("{longest}c")));
        assert!(relative_key_valid("o=a.b/b=1/d=2-w3.json.zst"));
        assert!(!relative_key_valid(&format!("a{}", "b".repeat(160))));
        for key in ["", "/a", "a//b", "a/", ".a", "-a", "a b", "a/é", "o=a:b"] {
            assert!(!relative_key_valid(key), "{key:?}");
        }
    }

    #[test]
    fn storage_is_https_on_any_host_or_http_on_loopback() {
        assert!(allowed("http://127.0.0.1:9000/bucket/key"));
        assert!(allowed("http://[::1]:9000"));
        assert!(allowed("http://localhost:9000"));
        assert!(allowed(
            "https://objects.example.test:9000/relayhistory/b/k?X-Amz-Signature=s"
        ));
        assert!(allowed("https://10.0.0.5:9000"));
        assert!(!allowed("http://10.0.0.5:9000"));
        assert!(!allowed("http://storage.example.com"));
        assert!(!allowed("http://127.0.0.1.example.com"));
        assert!(!allowed("https://user:pass@objects.example.test"));
        assert!(!allowed("ftp://objects.example.test"));
        assert!(!allowed("not a url"));
    }
}
