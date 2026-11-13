//! Whether the pipeline admitted an object put into the upload inbox. A few
//! polls of the receiver's admission status follow a PUT that S3 stored:
//! admitted is queued, refused is the terminal refusal a rejected batch
//! gets, and pending (or a poll that did not get through) leaves the batch
//! waiting with its object's key, so a later pass asks again. A receiver
//! with nothing to say about the object (no such route, another contract)
//! leaves it queued: the object is stored, and the pipeline decides on it.
use super::super::{
    check_account, failure_of, guard, http_error, post::http_agent, response_json, retry_after,
    tls::verdict, AcceptanceLevel, DeliveryAcknowledgment, DeliveryFailure, ReceiverFailure,
    Result, StoredUpload,
};
use super::{credentials::CONTRACT, sigv4::uri_encode, Call, Sent};
use chrono::Utc;
use serde::Deserialize;
use std::time::Duration;

/// Polls after a PUT: the first at once, then after [`WAITS`].
const POLLS: usize = 3;
const WAITS: [Duration; 2] = [Duration::from_millis(200), Duration::from_millis(400)];
/// The longest a pending answer's retry hint may hold a send.
const LONGEST_WAIT: Duration = Duration::from_secs(2);
const POLL_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest a pending admission's retry hint may put the next pass off.
const LONGEST_PENDING_WAIT: Duration = Duration::from_secs(300);

/// What the pipeline's admission of the stored object at `key` means for
/// its batch: admitted (or nothing left to wait on) is queued; pending
/// keeps the key on the batch for a later pass, waiting as the receiver
/// asked; refused is the terminal refusal a rejected batch gets, so the
/// batch is set aside and never posted. A session the receiver no longer
/// takes keeps the key; one it refuses (403) forgets it, so a retry after
/// another sign-in stores the object again under that session.
pub(super) fn decide(
    call: &Call<'_>,
    key: &str,
    batch_id: &str,
    revision_ids: &[String],
) -> Sent<DeliveryAcknowledgment> {
    let pending = Some(StoredUpload::Pending(key.to_owned()));
    let failure = match decision(call, key) {
        Ok(Decision::Queued) => return Ok(queued(batch_id, revision_ids)),
        Ok(Decision::Pending(wait)) => ReceiverFailure {
            retry_after_ms: wait.map(|wait| {
                let wait = wait.min(LONGEST_PENDING_WAIT).as_millis() as i64;
                Utc::now().timestamp_millis().saturating_add(wait)
            }),
            upload: pending,
            ..DeliveryFailure::Transient.into()
        },
        Ok(Decision::Refused(refusal)) => {
            eprintln!(
                "upload inbox refused a batch; setting it aside code={}",
                refusal.code.name()
            );
            ReceiverFailure {
                upload: pending,
                ..DeliveryFailure::ReceiverRejected.into()
            }
        }
        Err(error) => {
            let failure = verdict(&error);
            let upload = match failure.failure {
                DeliveryFailure::PermissionDenied => Some(StoredUpload::Forgotten),
                _ => pending,
            };
            ReceiverFailure { upload, ..failure }
        }
    };
    Err(Box::new(failure))
}

/// Every revision of a batch acknowledged as queued: its object is stored
/// and the pipeline took it.
fn queued(batch_id: &str, revision_ids: &[String]) -> DeliveryAcknowledgment {
    DeliveryAcknowledgment {
        batch_id: batch_id.to_owned(),
        accepted_revision_ids: revision_ids.to_vec(),
        unsupported_revision_ids: vec![],
        suppressed_revision_ids: vec![],
        rejected_revision_ids: vec![],
        isolation_frontier: vec![],
        acceptance_level: AcceptanceLevel::Queued,
    }
}

/// What the pipeline has decided on an object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AdmissionStatus {
    Pending,
    Admitted,
    Refused(Refusal),
}

/// Why the pipeline refused an object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub(crate) code: RefusalCode,
}

/// A refusal's code. A code this build does not know is from a newer
/// contract, and is a refusal all the same.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "String")]
pub(crate) enum RefusalCode {
    UploadKeyInvalid,
    UploadWitnessInvalid,
    UploadWitnessExpired,
    UploadTenantMismatch,
    UploadWitnessSeqMismatch,
    UploadDigestMismatch,
    UploadOriginMismatch,
    /// The store requires SSE-KMS and the object was stored without it.
    UploadUnencrypted,
    DeliveryTooLarge,
    UnsupportedEncoding,
    InvalidDelivery,
    UnsupportedProtocol,
    UnsupportedSchema,
    UnsupportedMapping,
    UnsupportedEvidence,
    DeliveryAccountMismatch,
    DeliveryBudgetExceeded,
    DeliveryPrivacySuperseded,
    TenantNotASegment,
    AdmissionInvalid,
    AdmissionFailed,
    Unknown(String),
}

/// Every refusal code `relay-upload-inbox-v1` defines (decision 0004), by
/// the name it is sent as.
const KNOWN_CODES: [(&str, RefusalCode); 21] = [
    ("upload_key_invalid", RefusalCode::UploadKeyInvalid),
    ("upload_witness_invalid", RefusalCode::UploadWitnessInvalid),
    ("upload_witness_expired", RefusalCode::UploadWitnessExpired),
    ("upload_tenant_mismatch", RefusalCode::UploadTenantMismatch),
    (
        "upload_witness_seq_mismatch",
        RefusalCode::UploadWitnessSeqMismatch,
    ),
    ("upload_digest_mismatch", RefusalCode::UploadDigestMismatch),
    ("upload_origin_mismatch", RefusalCode::UploadOriginMismatch),
    ("upload_unencrypted", RefusalCode::UploadUnencrypted),
    ("delivery_too_large", RefusalCode::DeliveryTooLarge),
    ("unsupported_encoding", RefusalCode::UnsupportedEncoding),
    ("invalid_delivery", RefusalCode::InvalidDelivery),
    ("unsupported_protocol", RefusalCode::UnsupportedProtocol),
    ("unsupported_schema", RefusalCode::UnsupportedSchema),
    ("unsupported_mapping", RefusalCode::UnsupportedMapping),
    ("unsupported_evidence", RefusalCode::UnsupportedEvidence),
    (
        "delivery_account_mismatch",
        RefusalCode::DeliveryAccountMismatch,
    ),
    (
        "delivery_budget_exceeded",
        RefusalCode::DeliveryBudgetExceeded,
    ),
    (
        "delivery_privacy_superseded",
        RefusalCode::DeliveryPrivacySuperseded,
    ),
    ("tenant_not_a_segment", RefusalCode::TenantNotASegment),
    ("admission_invalid", RefusalCode::AdmissionInvalid),
    ("admission_failed", RefusalCode::AdmissionFailed),
];

impl From<String> for RefusalCode {
    fn from(code: String) -> Self {
        KNOWN_CODES
            .iter()
            .find(|(name, _)| *name == code)
            .map_or(Self::Unknown(code), |(_, known)| known.clone())
    }
}
impl RefusalCode {
    /// The code as it is logged: a known one by name, an unknown one only
    /// when it is a plain machine code.
    pub(crate) fn name(&self) -> &str {
        match self {
            Self::Unknown(code)
                if !code.is_empty()
                    && code.len() <= 64
                    && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') =>
            {
                code
            }
            Self::Unknown(_) => "unrecognized",
            known => KNOWN_CODES
                .iter()
                .find(|(_, code)| code == known)
                .map_or("unrecognized", |(name, _)| name),
        }
    }
}

/// The admission answer for one object.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Answer {
    contract: String,
    upload_key: String,
    state: String,
    #[serde(default)]
    code: Option<RefusalCode>,
    #[serde(default)]
    retry_after_seconds: Option<u64>,
}

/// One poll's outcome: a decision, a pending one with how long it asked to
/// wait, or no answer worth waiting on.
enum Step {
    Status(AdmissionStatus, Option<Duration>),
    Stop,
}

/// What a poll that got no answer means: the receiver asked to wait, or
/// there is nothing to wait on.
#[derive(Debug)]
enum Unanswered {
    Busy(Option<i64>),
    Stop,
}
impl std::fmt::Display for Unanswered {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("admission status unanswered")
    }
}
impl std::error::Error for Unanswered {}

/// What the polls decided for a batch's stored object.
#[derive(Debug)]
enum Decision {
    /// Admitted, or nothing left to wait on.
    Queued,
    /// Not decided yet; how long the receiver last asked to wait.
    Pending(Option<Duration>),
    Refused(Refusal),
}

/// Poll the admission of the object at `key` up to [`POLLS`] times. A
/// session the receiver no longer takes (401) or refuses (403) fails.
fn decision(call: &Call<'_>, key: &str) -> Result<Decision> {
    let mut asked = None;
    for poll in 0..POLLS {
        let wait = match self::poll(call, key)? {
            Step::Status(AdmissionStatus::Refused(refusal), _) => {
                return Ok(Decision::Refused(refusal))
            }
            Step::Status(AdmissionStatus::Pending, wait) => wait,
            Step::Status(AdmissionStatus::Admitted, _) | Step::Stop => return Ok(Decision::Queued),
        };
        asked = wait;
        let Some(default) = WAITS.get(poll) else {
            break;
        };
        let wait = wait.unwrap_or(*default).min(LONGEST_WAIT);
        match call.budget.remaining() {
            Ok(remaining) if remaining > wait => std::thread::sleep(wait),
            _ => break,
        }
    }
    Ok(Decision::Pending(asked))
}

fn poll(call: &Call<'_>, key: &str) -> Result<Step> {
    let base = call.source.admission_base_url.as_deref();
    if base.is_some_and(|base| !super::allowed(base)) {
        call.note("admission_url");
        return Ok(Step::Stop);
    }
    let query = format!("/v1/delivery/admission?uploadKey={}", uri_encode(key, true));
    let response = super::super::send_authenticated(
        &call.stage,
        |current, timeout| {
            let base = base.unwrap_or(&current.base_url).trim_end_matches('/');
            http_agent()
                .get(&format!("{base}{query}"))
                .timeout(timeout.unwrap_or(POLL_TIMEOUT).min(POLL_TIMEOUT))
                .set("Authorization", &format!("Bearer {}", current.access_token))
                .call()
                .map_err(Box::new)
        },
        unanswered,
        |current| check_account(current, call.account),
        &|| call.budget.remaining(),
    );
    let error = match response {
        Ok(response) => return Ok(answered(response_json(response).ok(), key)),
        Err(error) => guard::retry_unresolved(error),
    };
    match error.downcast_ref::<Unanswered>() {
        Some(Unanswered::Busy(at)) => Ok(Step::Status(AdmissionStatus::Pending, wait_until(*at))),
        Some(Unanswered::Stop) => Ok(Step::Stop),
        None if matches!(
            failure_of(&error),
            Some(DeliveryFailure::AuthenticationRequired | DeliveryFailure::PermissionDenied)
        ) =>
        {
            Err(error)
        }
        // The poll did not get through (transport, the call's budget):
        // undecided, asked again on a later pass.
        None => Ok(Step::Status(AdmissionStatus::Pending, None)),
    }
}

/// 401 and 403 (`upload_tenant_mismatch` for a key that is not this
/// session's) are the session's, mapped as a batch's are; 503 asks to poll
/// again; anything else (404 `upload_unknown`, 400 `invalid_delivery` for a
/// key it cannot read, 501 for a receiver without the route) leaves nothing
/// to wait on, and the stored object stays queued.
fn unanswered(error: ureq::Error) -> anyhow::Error {
    match error {
        ureq::Error::Status(401 | 403, _) => http_error(error, ""),
        ureq::Error::Status(status @ 503, ref response) => {
            Unanswered::Busy(retry_after::of(status, response)).into()
        }
        _ => Unanswered::Stop.into(),
    }
}

/// What a 200 says about the object at `key`. An answer for another
/// contract or another key says nothing.
fn answered(answer: Option<serde_json::Value>, key: &str) -> Step {
    let Some(answer) = answer.and_then(|answer| serde_json::from_value::<Answer>(answer).ok())
    else {
        return Step::Stop;
    };
    if answer.contract != CONTRACT || answer.upload_key != key {
        return Step::Stop;
    }
    let wait = answer.retry_after_seconds.map(Duration::from_secs);
    match (answer.state.as_str(), answer.code) {
        ("admitted", _) => Step::Status(AdmissionStatus::Admitted, None),
        ("refused", Some(code)) => Step::Status(AdmissionStatus::Refused(Refusal { code }), None),
        ("pending", _) => Step::Status(AdmissionStatus::Pending, wait),
        _ => Step::Stop,
    }
}

fn wait_until(at_ms: Option<i64>) -> Option<Duration> {
    let wait = at_ms? - chrono::Utc::now().timestamp_millis();
    u64::try_from(wait).ok().map(Duration::from_millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_known_code_reads_back_as_its_name() {
        for (name, code) in &KNOWN_CODES {
            assert_eq!(RefusalCode::from(name.to_string()), *code);
            assert_eq!(code.name(), *name);
        }
        let newer = RefusalCode::from("upload_newer_reason".to_string());
        assert_eq!(newer, RefusalCode::Unknown("upload_newer_reason".into()));
        assert_eq!(newer.name(), "upload_newer_reason");
        assert_eq!(
            RefusalCode::from("Not A Code\n".to_string()).name(),
            "unrecognized"
        );
    }

    /// `admissionStatus` from the receiver's vectors (see `super::vectors`).
    #[test]
    fn the_receivers_admission_vectors_read_as_their_states() {
        let vectors = super::super::vectors_tests::vectors();
        let key = vectors["objectKey"].as_str().unwrap();
        let status = |state: &str| answered(Some(vectors["admissionStatus"][state].clone()), key);
        assert!(matches!(
            status("pending"),
            Step::Status(AdmissionStatus::Pending, Some(wait)) if wait == Duration::from_secs(15)
        ));
        assert!(matches!(
            status("admitted"),
            Step::Status(AdmissionStatus::Admitted, None)
        ));
        assert!(matches!(
            status("refused"),
            Step::Status(
                AdmissionStatus::Refused(Refusal {
                    code: RefusalCode::DeliveryPrivacySuperseded
                }),
                None
            )
        ));
    }

    #[test]
    fn an_answer_for_another_object_or_contract_says_nothing() {
        let answer = |contract: &str, key: &str, state: &str| {
            serde_json::json!({
                "contract": contract, "uploadKey": key, "state": state,
                "code": "upload_digest_mismatch"
            })
        };
        assert!(matches!(
            answered(Some(answer(CONTRACT, "k", "refused")), "k"),
            Step::Status(AdmissionStatus::Refused(_), _)
        ));
        assert!(matches!(
            answered(Some(answer(CONTRACT, "other", "refused")), "k"),
            Step::Stop
        ));
        assert!(matches!(
            answered(Some(answer("relay-upload-inbox-v2", "k", "refused")), "k"),
            Step::Stop
        ));
        assert!(matches!(
            answered(Some(answer(CONTRACT, "k", "admitted")), "k"),
            Step::Status(AdmissionStatus::Admitted, _)
        ));
    }
}
