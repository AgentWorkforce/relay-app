//! Why uploads are not moving. A paused upload used to say only "Upload
//! paused; retrying" with its counts, whether the job was retrying a failure,
//! blocked until a person retries it, or paused by a person, and never named
//! the failure: a job stuck on one batch printed the same line for days.
use super::*;
use crate::history::delivery_status::{delivery_failure_code, delivery_failure_message};

/// A job that is not delivering: its state, the allowlisted code of its
/// failure (never receiver text), its consecutive attempts and when it is
/// tried next.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stall {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<&'static str>,
    pub attempts: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_attempt_ms: Option<i64>,
    pub next_attempt_ms: i64,
}

impl Stall {
    /// The job's stall, or `None` while it is active with no failure.
    pub(super) fn of(status: &delivery::DeliveryStatus) -> Option<Self> {
        if status.state == "active" && status.failure.is_none() {
            return None;
        }
        Some(Self {
            state: status.state.clone(),
            failure_code: status.failure.as_deref().map(|failure| {
                // An unknown code is shown as such, never as its raw text.
                delivery_failure_code(failure).unwrap_or("unknown")
            }),
            attempts: status.attempts,
            last_attempt_ms: status.last_attempt_ms,
            next_attempt_ms: status.next_attempt_ms,
        })
    }

    /// Whether the job waits for a person rather than for its next attempt.
    fn waits_for_a_person(&self) -> bool {
        matches!(self.state.as_str(), "blocked" | "paused")
    }
}

/// What a paused upload is doing, before its counts.
pub(super) fn headline(stall: Option<&Stall>) -> String {
    let Some(stall) = stall else {
        return "Upload paused; retrying".into();
    };
    match (stall.state.as_str(), stall.failure_code) {
        ("paused", _) => "Uploads paused; nothing is uploaded until you resume".into(),
        ("blocked", Some(code)) => {
            format!("Upload blocked ({code}); not retried until a retry or an update")
        }
        ("blocked", None) => "Upload blocked; not retried until a retry or an update".into(),
        (_, Some(code)) => format!(
            "Upload paused; retrying after {code} ({} {})",
            stall.attempts,
            if stall.attempts == 1 {
                "attempt"
            } else {
                "attempts"
            }
        ),
        (_, None) => "Upload paused; retrying".into(),
    }
}

/// The reason and timing after the counts: the failure's own sentence, how
/// long ago the last attempt was, and when the next is due.
pub(super) fn detail(stall: Option<&Stall>, now_ms: i64) -> String {
    let Some(stall) = stall else {
        return String::new();
    };
    let mut parts = Vec::new();
    // A job that waits for a person is not retried, so a failure sentence
    // that promises a retry would contradict the headline: it is left out.
    if let Some(code) = stall
        .failure_code
        .filter(|code| !(stall.waits_for_a_person() && promises_retry(code)))
    {
        parts.push(
            delivery_failure_message(code)
                .trim_end_matches('.')
                .to_string(),
        );
    }
    if let Some(at) = stall.last_attempt_ms {
        parts.push(format!(
            "last attempt {} ago",
            seconds(now_ms.saturating_sub(at))
        ));
    }
    if !stall.waits_for_a_person() && stall.failure_code.is_some() {
        let wait = stall.next_attempt_ms.saturating_sub(now_ms);
        parts.push(if wait > 0 {
            format!("next attempt in {}", seconds(wait))
        } else {
            "next attempt due now".into()
        });
    }
    parts.iter().map(|part| format!(" · {part}")).collect()
}

/// Whether the failure's sentence says Agent Relay retries by itself: the
/// retried failures, and the sentence an unknown code is shown with.
fn promises_retry(code: &str) -> bool {
    matches!(code, "transient" | "rate_limited" | "unknown")
}

fn seconds(ms: i64) -> String {
    format!("{}s", ms.max(0).saturating_add(999) / 1000)
}
