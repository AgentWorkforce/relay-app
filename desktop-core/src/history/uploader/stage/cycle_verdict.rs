//! How `derive` reads a failed collector cycle: local storage that needs
//! the person, a capture-side failure while delivery is healthy, or an
//! uploader that is offline.
use super::{settled_stage, LOCAL_STORAGE_FAILURES};
use crate::contract::account::Stage;
use serde_json::Value;

/// Capture classes the person fixes: a remote connector to set up or sign
/// in to again.
const CAPTURE_NEEDS_PERSON: [&str; 2] = ["connector_auth_expired", "connector_not_configured"];

/// The capture class of a cycle that failed on its capture side alone, with
/// a healthy delivery (its side of the cycle ok, no failure code); `None`
/// for any other cycle.
fn capture_only_class<'a>(cycle: &'a Value, failure_code: &str) -> Option<&'a str> {
    let failed = |side: &Value| side["ok"] == Value::Bool(false);
    let delivery_healthy = !failed(&cycle["delivery"]) && failure_code.is_empty();
    (failed(cycle) && failed(&cycle["capture"]) && delivery_healthy)
        .then(|| cycle["capture"]["error_class"].as_str().unwrap_or_default())
}

/// The stage of a failed cycle, `None` where the cycle did not fail or its
/// failure is a transient capture one that leaves the stage to delivery
/// (stalled, preparing or up to date). A healthy delivery is never
/// `Offline` because capture failed: a capture class the person fixes (a
/// connector or local storage) needs attention, and every other capture
/// class is transient and shows its sentence while records upload. A cycle
/// whose delivery side failed stays `Offline`.
pub(in super::super) fn failed_cycle(
    cycle: &Value,
    failure_code: &str,
    stalled: bool,
    pending: i64,
) -> Option<(Stage, Option<String>)> {
    if cycle["ok"] != Value::Bool(false) {
        return None;
    }
    let message = cycle["message"].as_str().map(str::to_string);
    let Some(capture_class) = capture_only_class(cycle, failure_code) else {
        return Some((Stage::Offline, message));
    };
    if CAPTURE_NEEDS_PERSON.contains(&capture_class)
        || LOCAL_STORAGE_FAILURES.contains(&capture_class)
    {
        Some((Stage::NeedsAttention, message))
    } else if pending > 0 && !stalled {
        Some((Stage::Uploading, message))
    } else {
        None
    }
}

/// The sentence an idle stage (preparing or up to date) carries after a
/// transient capture failure, so a failure that repeats on every pass stays
/// visible while nothing is pending.
fn idle_message(cycle: &Value, failure_code: &str) -> Option<String> {
    capture_only_class(cycle, failure_code)?;
    cycle["message"].as_str().map(str::to_string)
}

/// The stage of an uploader with nothing failing ([`settled_stage`]); an
/// idle one (preparing or up to date) carries [`idle_message`].
pub(in super::super) fn settled(
    delivery: &Value,
    pending: i64,
    cycle: &Value,
    failure_code: &str,
) -> (Stage, Option<String>) {
    match settled_stage(delivery, pending) {
        Stage::Uploading => (Stage::Uploading, None),
        idle => (idle, idle_message(cycle, failure_code)),
    }
}
