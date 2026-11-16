//! The one-line, human-readable progress report for each phase, printed to
//! the collector's log.
use super::*;

impl Progress {
    /// The line for the current phase, before any held-records suffix.
    pub(super) fn phase_line(&self) -> String {
        match self.phase.as_str() {
            "scanning" => match self.total_files {
                Some(total) => format!(
                    "Reading {} session files: {} of {} processed · {} sessions captured",
                    self.source, self.processed_files, total, self.sessions_captured
                ),
                None => format!(
                    "Scanning {}… {} sessions captured",
                    self.source, self.sessions_captured
                ),
            },
            "capture_paused" => format!(
                "Local capture paused; retrying. {} session files processed, {} sessions captured",
                self.processed_files, self.sessions_captured
            ),
            "paused" => format!(
                "{}. {} records uploaded, {} queued{}{}",
                stall::headline(self.stall.as_ref()),
                self.records_uploaded,
                self.records_queued,
                if self.records_quarantined > 0 {
                    format!(", {} quarantined", self.records_quarantined)
                } else {
                    String::new()
                },
                stall::detail(self.stall.as_ref(), chrono::Utc::now().timestamp_millis()),
            ),
            "conflict" => format!(
                "Delivery conflict: {} records quarantined, {} queued; other records continue uploading. Inspect delivery status for repair.",
                self.records_quarantined, self.records_queued
            ),
            "watching" => format!(
                "Up to date: {} records uploaded. Watching for new sessions.",
                self.records_uploaded
            ),
            _ => format!(
                "Uploading: {} records received, {} queued{}",
                self.records_uploaded,
                self.records_queued,
                if self.backlog_complete {
                    ""
                } else {
                    " · preparing remaining records"
                }
            ),
        }
    }
}
