//! The parts of the stage `uploader.status` reports: when a busy local
//! history is still a retry, and the sentences that go with each stage.

mod cycle_verdict;
pub(super) use cycle_verdict::{failed_cycle, settled};

use crate::contract::account::Stage;
use serde_json::Value;

pub(super) const LOCAL_STORAGE_FAILURES: [&str; 6] = [
    "retention_limit",
    "database_corrupt",
    "database_unavailable",
    "disk_full",
    "database_busy",
    "permission_denied",
];

/// What a stalled upload with no failure of its own says.
pub(super) const STALLED: &str = "No upload has been confirmed for over five minutes. Agent Relay keeps retrying; queued records are preserved.";

/// When the next attempt of a delivery pass that found local history busy is
/// due, while that is still a retry rather than a failure.
///
/// A busy pass is rescheduled, so until uploads stop moving it shows as an
/// upload with a due time. That holds only while local history has been busy
/// for less than `SUSTAINED_FAILURE_MS` in a row (`busy_since_ms`), and never
/// beside uploads that `already_failing` (stalled, or a retried failure that
/// is sustained): those need attention, busy or not. A report without the
/// streak's start is not bounded, so it is not a retry either.
pub(super) fn busy_retry(
    cycle: &Value,
    error_class: &str,
    already_failing: bool,
    now_ms: i64,
) -> Option<i64> {
    if cycle["ok"] != Value::Bool(false) || error_class != "database_busy" || already_failing {
        return None;
    }
    let busy_since_ms = cycle["delivery"]["busy_since_ms"].as_i64()?;
    if now_ms.saturating_sub(busy_since_ms) >= crate::history::delivery_status::SUSTAINED_FAILURE_MS
    {
        return None;
    }
    cycle["delivery"]["next_attempt_ms"].as_i64()
}

/// Whether a retried failure has gone on long enough to need attention.
pub(super) fn sustained(delivery: &Value, now_ms: i64) -> bool {
    crate::history::delivery_status::retry_failure_is_sustained(
        delivery["attempts"].as_i64().unwrap_or_default(),
        delivery["last_acknowledged_ms"].as_i64(),
        delivery["oldest_pending_ms"].as_i64(),
        now_ms,
    )
}

/// What a delivery conflict says, with `quarantined` records set aside.
pub(super) fn conflict_message(quarantined: i64) -> String {
    if quarantined > 0 {
        format!("{quarantined} receiver-proven {} quarantined. Other records can continue uploading; inspect conflicts for repair.", if quarantined == 1 { "record" } else { "records" })
    } else {
        "History delivery conflict: the receiver detail could not be validated. Records remain queued for repair.".into()
    }
}

/// What a rescheduled busy pass says, its next attempt due at
/// `next_attempt_ms`.
pub(super) fn busy_message(next_attempt_ms: i64, now_ms: i64) -> String {
    let seconds = (next_attempt_ms - now_ms).max(0).saturating_add(999) / 1000;
    if seconds == 0 {
        "Local history is busy. The next upload attempt is due now; queued sessions are preserved."
            .into()
    } else {
        format!("Local history is busy. The next upload attempt is due in about {seconds} s; queued sessions are preserved.")
    }
}

/// The stage of an uploader with nothing failing: uploading what is
/// pending, preparing what is not queued yet, or up to date.
pub(super) fn settled_stage(delivery: &Value, pending: i64) -> Stage {
    if pending > 0 {
        Stage::Uploading
    } else if !delivery["bootstrap_complete"].as_bool().unwrap_or(false)
        || delivery["unqueued_changes"].as_i64().unwrap_or(0) > 0
    {
        Stage::Preparing
    } else {
        Stage::UpToDate
    }
}

/// `message`, followed by how many receiver-proven records remain
/// quarantined when there are any.
pub(super) fn with_quarantined(message: Option<String>, quarantined: i64) -> Option<String> {
    if quarantined <= 0 {
        return message;
    }
    let detail = format!(
        "{quarantined} receiver-proven {} quarantined; inspect conflicts for repair.",
        if quarantined == 1 {
            "record remains"
        } else {
            "records remain"
        }
    );
    Some(match message {
        Some(message) if !message.is_empty() => format!("{message} {detail}"),
        _ => detail,
    })
}
