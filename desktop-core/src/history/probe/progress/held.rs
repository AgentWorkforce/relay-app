//! Records held for acquisition: pending, but not this computer's to send
//! until their session's ownership is settled.

use super::*;

pub(super) fn is_zero(value: &i64) -> bool {
    *value == 0
}

impl Progress {
    /// The phase's line, naming the records that wait for ownership while
    /// the phase is about uploading.
    pub(super) fn line(&self) -> String {
        let line = self.phase_line();
        if self.records_held > 0 && !matches!(self.phase.as_str(), "scanning" | "capture_paused") {
            format!("{line} · {} waiting for ownership", self.records_held)
        } else {
            line
        }
    }
}
