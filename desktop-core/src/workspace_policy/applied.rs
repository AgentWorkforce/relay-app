//! What the core was handed from the workspace policy: the settings policy
//! and its revision, which `access/me` tells Cloud, and the last failure to
//! hand one over.

use super::WorkspacePolicies;
use agent_relay_node::relay::settings_policy::SettingsPolicy;
use std::sync::atomic::Ordering;

impl WorkspacePolicies {
    /// Records `message` as the latest failure to hand `wanted` over;
    /// answers whether it differs from the one before (worth showing): a
    /// failure for another policy, a lift included, is new even with the same
    /// message.
    pub fn settings_failure_is_new(&self, wanted: &Option<SettingsPolicy>, message: &str) -> bool {
        let mut last = self
            .settings_failure
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let failure = (wanted.clone(), message.to_string());
        let new = last.as_ref() != Some(&failure);
        *last = Some(failure);
        new
    }

    /// Whether `wanted` differs from the settings policy last handed to the
    /// core in this run.
    pub fn settings_changed(&self, wanted: &Option<SettingsPolicy>) -> bool {
        let applied = self
            .applied_settings
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        applied.as_ref() != Some(wanted)
    }

    /// Records `applied` as handed to the core, from `revision`.
    pub fn settings_applied(
        &self,
        applied: Option<SettingsPolicy>,
        revision: Option<(String, i64)>,
    ) {
        *self
            .settings_failure
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        *self
            .applied_settings
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(applied);
        self.revision_applied(revision);
    }

    /// Records that the core holds the settings of `revision` (a workspace
    /// and its policy revision; `None` signed out); a change is due to be
    /// told to Cloud.
    pub fn revision_applied(&self, revision: Option<(String, i64)>) {
        let mut applied = self
            .applied_revision
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if *applied != revision {
            self.report_due.store(revision.is_some(), Ordering::Relaxed);
            *applied = revision;
        }
    }

    /// The policy revision applied for `workspace_id`, if any.
    pub fn applied_revision(&self, workspace_id: &str) -> Option<i64> {
        let applied = self
            .applied_revision
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        applied
            .as_ref()
            .filter(|(workspace, _)| workspace == workspace_id)
            .map(|(_, revision)| *revision)
    }

    /// Whether a newly applied revision is still to be told to Cloud; asking
    /// clears it.
    pub fn take_report_due(&self) -> bool {
        self.report_due.swap(false, Ordering::Relaxed)
    }
}
