//! Local capture while sessions are prepared (`status.account.progress`).

use super::{capitalized, grouped};
use agent_relay_node::relay::state::CaptureProgress;

/// Whether local history is being read now.
pub fn capture_is_active(capture: &CaptureProgress) -> bool {
    matches!(capture.phase.as_str(), "scanning" | "capture_paused")
}

pub(super) fn capture_is_paused(capture: &CaptureProgress) -> bool {
    capture.phase == "capture_paused"
}

pub fn capture_headline(capture: &CaptureProgress) -> &'static str {
    if capture_is_paused(capture) {
        "History preparation paused · Retrying"
    } else {
        "Reading local history"
    }
}

/// `Codex: 5 of 10 files processed · 2 sessions captured locally.`
pub fn capture_detail(capture: &CaptureProgress) -> String {
    let processed = grouped(capture.processed_files);
    let count = match capture.total_files {
        Some(total) => format!("{processed} of {} files processed", grouped(total)),
        None => format!("{processed} files processed"),
    };
    let source = if capture.source.is_empty() {
        "Local history".to_string()
    } else {
        capitalized(&capture.source)
    };
    format!(
        "{source}: {count} · {} sessions captured locally.",
        grouped(capture.sessions_captured)
    )
}

/// How far capture is, when it reports a bounded total.
pub fn capture_fraction(capture: &CaptureProgress) -> Option<f64> {
    let total = capture.total_files.filter(|total| *total > 0)?;
    Some((capture.processed_files as f64 / total as f64).clamp(0.0, 1.0))
}
