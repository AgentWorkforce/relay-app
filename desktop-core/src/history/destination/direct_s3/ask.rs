//! Asking Control for upload credentials (contract §4). Session (`sts`)
//! keys are asked for first. A 501 `unsupported_by_backend` means the store
//! has no STS: the same ask is made once more, in the same send, with kind
//! `presigned`, naming the batch's one object. That answer is remembered
//! ("presigned-only") for the session that heard it, in memory only, so
//! later sends ask for presigned URLs directly; another session, a session
//! the host replaced and a restart ask for `sts` again.
//!
//! A presigned key names the witness seq, which only an answer tells. The
//! ask names the seq the last answer carried (0 before any): when the
//! answer's `witnessSeq` differs, its URL names another key and is never
//! put to, and the ask is made once more for the key `witnessSeq` names. A
//! second miss posts the batch.
use super::super::{
    check_account, failure_of, guard, hash, http_error,
    post::with_policy_revision,
    rejection::{error_body_bytes, parse_error_body, safe_error_code},
    response_json, retry_after, DeliveryFailure, ReceiverRejection, Result,
};
use super::{
    credentials::{self, Declined, GrantRequest, KeyRequest, Kind},
    http_agent, sigv4, Call, Kept, PresignedOnly, Upload, CONTROL_TIMEOUT, OFF,
};
use chrono::Utc;
use std::sync::Mutex;

/// The generic code a Control router answers for a route it does not have:
/// a build from before the upload-credentials route.
const NO_ROUTE: &str = "not_found";

/// The 501 code that means the store issues presigned URLs only.
const UNSUPPORTED_BY_BACKEND: &str = "unsupported_by_backend";

/// What one ask came to.
enum Asked {
    Kept(Box<Kept>),
    /// 501 `unsupported_by_backend` to an `sts` ask.
    PresignedOnly,
    /// Anything that posts the batch.
    Post,
}

impl Call<'_> {
    /// Credentials for this batch: the kept ones of this session while
    /// fresh and covering its object (unless renewing), else an ask of
    /// Control.
    pub(super) fn grant(&self, upload: &Upload, renew: bool) -> Result<Option<Kept>> {
        let kept = self.remember(|entry| entry.grant.clone());
        if let Some(kept) = kept.filter(|_| !renew) {
            let now = Utc::now();
            let key = upload.key("", kept.grant.witness_seq);
            if kept.of(&self.token, self.epoch)
                && kept.grant.fresh(now)
                && kept.grant.covers(&key, now)
            {
                return Ok(Some(kept));
            }
        }
        let presigned_only = self.remember(|entry| entry.presigned.clone());
        if let Some(only) = presigned_only.filter(|only| only.of(&self.token, self.epoch)) {
            return self.presigned(upload, only.seq.unwrap_or(0));
        }
        match self.ask(upload, Kind::Sts, 0)? {
            Asked::Kept(kept) => Ok(Some(*kept)),
            Asked::PresignedOnly => self.presigned(upload, 0),
            Asked::Post => Ok(None),
        }
    }
    /// Presigned URLs for this batch's object, asked about with `seq`, and
    /// asked once more when the answer's witness names another seq.
    fn presigned(&self, upload: &Upload, seq: u64) -> Result<Option<Kept>> {
        let mut asked = seq;
        for again in [false, true] {
            let Asked::Kept(kept) = self.ask(upload, Kind::Presigned, asked)? else {
                return Ok(None);
            };
            let witness_seq = kept.grant.witness_seq;
            if kept.grant.covers(&upload.key("", witness_seq), Utc::now()) {
                return Ok(Some(*kept));
            }
            if again || witness_seq == asked {
                break;
            }
            asked = witness_seq;
        }
        self.note("urls");
        Ok(None)
    }
    /// Ask Control for `kind` credentials, keeping a usable answer for the
    /// session that asked. An answer that arrives after the host replaced
    /// the session is neither kept nor used.
    fn ask(&self, upload: &Upload, kind: Kind, seq: u64) -> Result<Asked> {
        if !control_allowed(&self.source.api_url) {
            self.note("control_url");
            return Ok(Asked::Post);
        }
        let request = GrantRequest {
            origin_id: &upload.origin_id,
            kind: kind.name(),
            keys: (kind == Kind::Presigned).then(|| {
                [KeyRequest {
                    key: upload.key("", seq),
                    sha256: upload.checksum(),
                }]
            }),
        };
        let Ok(body) = serde_json::to_string(&request) else {
            return Ok(Asked::Post);
        };
        let url = format!(
            "{}/api/v1/workspaces/{}/relayhistory/upload-credentials",
            self.source.api_url.trim_end_matches('/'),
            sigv4::uri_encode(&self.source.workspace_id, true)
        );
        let epoch = self.stage.epoch();
        let revision = crate::history::operator_policy::pass_revision(&self.source.workspace_id);
        let token = Mutex::new(None);
        let response = super::super::send_authenticated(
            &self.stage,
            |current, timeout| {
                *token.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(hash(current.access_token.as_bytes()));
                let request = http_agent()
                    .post(&url)
                    .timeout(timeout.unwrap_or(CONTROL_TIMEOUT).min(CONTROL_TIMEOUT))
                    .set("Authorization", &format!("Bearer {}", current.access_token))
                    .set("Content-Type", "application/json");
                // The pass's policy revision, captured once for every attempt.
                with_policy_revision(request, revision)
                    .send_string(&body)
                    .map_err(Box::new)
            },
            declined,
            |current| check_account(current, self.account),
            &|| self.budget.remaining(),
        );
        let token = token
            .into_inner()
            .unwrap_or_else(|e| e.into_inner())
            .unwrap_or_else(|| self.token.clone());
        let current = epoch == self.epoch && self.stage.epoch() == epoch;
        let answer = match response {
            Ok(response) => response_json(response).ok(),
            Err(error) => {
                return self.declined(guard::retry_unresolved(error), token, kind, current)
            }
        };
        match answer.map(|answer| checked(answer, kind)) {
            Some(Ok(grant)) => {
                if !current {
                    return Ok(Asked::Post);
                }
                let kept = Kept {
                    token,
                    epoch,
                    grant,
                };
                self.remember(|entry| {
                    if let Some(only) = entry.presigned.as_mut().filter(|_| kind == Kind::Presigned)
                    {
                        only.seq = Some(kept.grant.witness_seq);
                    }
                    entry.grant = Some(kept.clone());
                });
                Ok(Asked::Kept(Box::new(kept)))
            }
            Some(Err(cause)) => {
                if kind == Kind::Presigned && cause == "kind" {
                    // Control no longer answers with presigned URLs: the
                    // next send asks for session keys first again.
                    self.remember(|entry| entry.presigned = None);
                }
                self.note(cause);
                Ok(Asked::Post)
            }
            None => {
                self.note("unreadable");
                Ok(Asked::Post)
            }
        }
    }
    /// What a refused ask means: "off" is remembered for the token that
    /// asked, a busy Control's wait is kept, a store without STS is
    /// remembered for the session that asked, a refused session or a
    /// missing workspace route fails the send as a batch POST would, and
    /// anything else posts this batch.
    fn declined(
        &self,
        error: anyhow::Error,
        token: String,
        kind: Kind,
        current: bool,
    ) -> Result<Asked> {
        match error.downcast_ref::<Declined>() {
            Some(Declined::Off) => self.remember(|entry| {
                entry.off = Some(token);
                entry.grant = None;
            }),
            Some(Declined::Busy(at)) => self.remember(|entry| entry.not_before_ms = *at),
            Some(Declined::PresignedOnly) if kind == Kind::Sts && current => {
                self.remember(|entry| {
                    entry.presigned = Some(PresignedOnly {
                        token,
                        epoch: self.epoch,
                        seq: None,
                    });
                });
                return Ok(Asked::PresignedOnly);
            }
            Some(Declined::PresignedOnly) => self.note("status"),
            Some(Declined::Unusable(cause)) => self.note(cause),
            None if matches!(
                failure_of(&error),
                Some(
                    DeliveryFailure::AuthenticationRequired
                        | DeliveryFailure::PermissionDenied
                        | DeliveryFailure::UnsupportedEvidence
                )
            ) =>
            {
                return Err(error)
            }
            None => {
                self.budget.remaining()?;
            }
        }
        Ok(Asked::Post)
    }
}

/// Whether a request carrying the session token may go to Control's API
/// root: the same address rule a History session meets, with an IP literal
/// allowed only on loopback unless the development flag is set.
fn control_allowed(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let flagged = matches!(
        std::env::var("RELAYHISTORY_ALLOW_UNTRUSTED_CLOUD_BASE_URL").as_deref(),
        Ok("1" | "true")
    );
    let host = match parsed.host() {
        Some(url::Host::Ipv4(ip)) => flagged || ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => flagged || ip.is_loopback(),
        Some(url::Host::Domain(_)) => true,
        None => false,
    };
    host && super::super::cloud::require_history_destination(&parsed.origin().ascii_serialization())
        .is_ok()
}

/// Control's refusals (contract §4): 404 `history_upload_direct_off` is
/// "off", and so is a 404 that shows the route does not exist at that API
/// root (a body that is not JSON or names no code, such as a site's HTML
/// page, or a router's generic `not_found`): a site whose Control has no
/// upload-credentials route has no direct uploads, and posting is always
/// safe. A 404 naming any other code is the workspace route's own, failed
/// as a batch POST's 404 is; 429 `rate_limited` posts and keeps its `Retry-After`; 501
/// whose `error` is exactly `unsupported_by_backend` asks again for
/// presigned URLs, any other 501 posts; 503 (`upload_credentials_unavailable`,
/// `privacy_journal_unavailable`) posts this batch only; 401 and 403 are the
/// session's, mapped as a batch POST's are, and a failed request is a batch
/// POST's failure too. Any other status is an answer to post past.
fn declined(error: ureq::Error) -> anyhow::Error {
    match error {
        ureq::Error::Status(404, response) => {
            let body = error_body_bytes(response)
                .as_deref()
                .and_then(parse_error_body);
            // Control's body is `{"error": "<code>", "message": …}`.
            let code = body
                .as_ref()
                .and_then(|body| error_field(body).or_else(|| safe_error_code(body)));
            if matches!(code.as_deref(), None | Some(OFF | NO_ROUTE)) {
                return Declined::Off.into();
            }
            ReceiverRejection {
                failure: DeliveryFailure::UnsupportedEvidence,
                status: 404,
                recovery: None,
                retry_after_ms: None,
                hold: None,
            }
            .into()
        }
        ureq::Error::Status(501, response) => {
            let code = error_body_bytes(response)
                .as_deref()
                .and_then(parse_error_body)
                .as_ref()
                .and_then(error_field);
            match code.as_deref() == Some(UNSUPPORTED_BY_BACKEND) {
                true => Declined::PresignedOnly.into(),
                false => Declined::Unusable("status").into(),
            }
        }
        ureq::Error::Status(status @ 429, ref response) => {
            Declined::Busy(retry_after::of(status, response)).into()
        }
        ureq::Error::Status(503, _) => Declined::Busy(None).into(),
        ureq::Error::Status(401 | 403, _) | ureq::Error::Transport(_) => http_error(error, ""),
        ureq::Error::Status(_, _) => Declined::Unusable("status").into(),
    }
}

/// An answer checked for the `kind` asked. An answer that names another
/// kind is the wrong-kind cause at once, before any of its schema is read.
fn checked(answer: serde_json::Value, kind: Kind) -> Result<credentials::Grant, &'static str> {
    let named = answer.get("kind").and_then(serde_json::Value::as_str);
    if named.is_some_and(|named| named != kind.name()) {
        return Err("kind");
    }
    credentials::parse(answer, kind)
}

/// The JSON string `error` of an error body.
fn error_field(body: &serde_json::Value) -> Option<String> {
    body.get("error")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_of_another_kind_is_the_kind_cause_before_its_schema_is_read() {
        // Nothing else of the schema is present: were it parsed first, the
        // cause would be `kmsKeyId`, not `kind`.
        for (asked, named) in [(Kind::Sts, "presigned"), (Kind::Presigned, "sts")] {
            let answer = serde_json::json!({ "kind": named });
            assert_eq!(checked(answer, asked).unwrap_err(), "kind");
        }
        // An answer that names no kind is left to the schema check.
        let nameless = serde_json::json!({});
        assert_eq!(checked(nameless, Kind::Sts).unwrap_err(), "kmsKeyId");
    }
}
