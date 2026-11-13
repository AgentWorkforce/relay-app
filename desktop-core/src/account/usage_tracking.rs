//! The account's upload and sign-in usage events.

use super::*;
use crate::telemetry::{
    LegacyScheduleJobs, TelemetryAccount, TelemetryFailure, TelemetryOperation, TelemetryOutcome,
};

impl Account {
    /// Records an explicit sign-in cancellation, then stops the attempt.
    pub fn cancel_sign_in(self: &std::sync::Arc<Self>) {
        if self.busy() {
            self.track(
                TelemetryOperation::SignIn,
                TelemetryOutcome::Cancelled,
                None,
            );
        }
        self.cancel();
    }

    /// Update success is the last RPC before a hosted core can be released.
    /// Give its sender a bounded chance to finish before acknowledging it.
    pub(crate) async fn record_usage(&self, event: TelemetryEvent) {
        let Some(telemetry) = self.telemetry.clone() else {
            return;
        };
        telemetry.record(event);
        if event.operation == TelemetryOperation::UpdateInstall
            && event.outcome == TelemetryOutcome::Succeeded
        {
            let _ = tokio::task::spawn_blocking(move || {
                telemetry.flush(std::time::Duration::from_secs(2))
            })
            .await;
        }
    }

    /// Records that uploads started failing for `failure`, with what the
    /// older-uploader check found, when telemetry is on.
    pub(crate) fn track_upload_failure(&self, failure: TelemetryFailure, jobs: LegacyScheduleJobs) {
        if let Some(telemetry) = &self.telemetry {
            let event =
                TelemetryEvent::new(TelemetryOperation::UploadHealth, TelemetryOutcome::Failed);
            telemetry.record(event.failure(failure).legacy_schedule_jobs(jobs));
        }
    }

    /// Records one operation, when telemetry is on.
    pub fn track(
        &self,
        operation: TelemetryOperation,
        outcome: TelemetryOutcome,
        failure: Option<TelemetryFailure>,
    ) {
        if let Some(telemetry) = &self.telemetry {
            let mut event = TelemetryEvent::new(operation, outcome);
            if let Some(failure) = failure {
                event = event.failure(failure);
            }
            telemetry.record(event);
        }
    }

    /// Records a completed sign-in, when telemetry is on.
    pub(super) fn track_sign_in_completed(&self) {
        if let Some(telemetry) = &self.telemetry {
            telemetry.sign_in_completed();
        }
    }
}

/// The analytics account of `install`: its signed-in account, when it has
/// one.
pub(super) fn telemetry_account(install: &Install) -> Option<TelemetryAccount> {
    install
        .target
        .account_id
        .clone()
        .map(|user_id| TelemetryAccount {
            user_id,
            organization_id: install.org_id.clone(),
            workspace_id: install.target.workspace_id.clone(),
        })
}
