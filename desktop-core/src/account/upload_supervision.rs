//! Supervision's report of the uploader's health as usage telemetry.

use super::*;

impl Account {
    /// Reports when a running uploader starts failing, fails for another
    /// reason, or recovers; returns the failure it reports now.
    pub(super) fn track_upload_health(
        &self,
        status: &Value,
        reported: Option<TelemetryFailure>,
    ) -> Option<TelemetryFailure> {
        match upload_health(status) {
            None => reported,
            Some(Some(failure)) => {
                if reported != Some(failure) {
                    self.track_upload_failure(failure, upload_legacy_schedule_jobs(status));
                }
                Some(failure)
            }
            Some(None) => {
                if reported.is_some() {
                    self.track(
                        TelemetryOperation::UploadHealth,
                        TelemetryOutcome::Succeeded,
                        None,
                    );
                }
                None
            }
        }
    }
}
