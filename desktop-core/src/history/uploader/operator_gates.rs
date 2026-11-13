//! The operator upload policy's gates on the person's uploader changes:
//! pausing and resuming, the sharing mode, deleting sessions from Cloud, and
//! what `uploader.status` reports of the policy.

use super::*;
use crate::history::operator_policy::OperatorPolicy;

impl Uploader {
    /// The person's pause or resume; refused while the operator runs
    /// uploads, and uploads the operator policy withholds are not resumed.
    /// The person's own pause while withheld clears the withheld flag, so a
    /// later read of the policy leaves it paused.
    pub async fn pause(&self, paused: bool) -> Result<Value, RpcError> {
        let workspace_id = self.workspace_id();
        if let Some(policy) = self.operator_policy() {
            policy.check_pause(workspace_id.as_deref())?;
        }
        if !paused {
            if let Some(policy) = self.operator_policy() {
                policy.check_resume(workspace_id.as_deref())?;
            }
        }
        let value = self.pause_unchecked(paused).await?;
        // Crash window: if the core dies after the probe paused and before
        // the flag is written, the flag survives and the next read of the
        // policy resumes uploads the person paused.
        if paused {
            if let (Some(policy), Some(workspace_id)) = (self.operator_policy(), workspace_id) {
                policy.set_withheld(&workspace_id, false);
            }
        }
        Ok(value)
    }

    /// Pauses or resumes uploads whatever the operator policy says: the
    /// policy's own withholding.
    pub(crate) async fn pause_unchecked(&self, paused: bool) -> Result<Value, RpcError> {
        let name = if paused { "pause" } else { "resume" };
        self.control(name, timeouts::PAUSE).await
    }

    /// Uses the operator policy, from the account.
    pub(crate) fn set_operator_policy(&self, policy: Arc<OperatorPolicy>) {
        let _ = self.operator_policy.set(policy);
    }

    pub(crate) fn operator_policy(&self) -> Option<&OperatorPolicy> {
        self.operator_policy.get().map(|policy| policy.as_ref())
    }

    /// The attached install's workspace.
    pub(super) fn workspace_id(&self) -> Option<String> {
        self.target().and_then(|target| target.workspace_id)
    }

    /// The attached install, for deleting its sessions from Cloud: refused
    /// when the operator does not let the person delete.
    pub(super) fn attached_for_delete(&self) -> Result<Target, RpcError> {
        let target = self.attached()?;
        if let Some(policy) = self.operator_policy() {
            policy.check_delete(target.workspace_id.as_deref())?;
        }
        Ok(target)
    }

    /// The settings the operator enforces on the attached install.
    pub fn enforced_settings(&self) -> std::collections::BTreeMap<String, String> {
        self.operator_policy()
            .map(|policy| policy.enforced_settings(self.workspace_id().as_deref()))
            .unwrap_or_default()
    }

    /// Whether the attached install's uploads wait for the first read of
    /// the operator policy.
    pub fn uploads_withheld(&self) -> bool {
        self.operator_policy()
            .is_some_and(|policy| policy.withholds(self.workspace_id().as_deref()))
    }

    /// The person's sharing choice, refused when the operator enforces
    /// another.
    pub async fn sharing_set(&self, mode: SharingMode) -> Result<Value, RpcError> {
        if let Some(policy) = self.operator_policy() {
            policy.check_sharing(self.workspace_id().as_deref(), mode)?;
        }
        self.sharing_set_unchecked(mode).await
    }

    /// Sets the sharing mode whatever the operator policy says: the policy
    /// applying itself, or giving back the person's own mode.
    pub(crate) async fn sharing_set_unchecked(&self, mode: SharingMode) -> Result<Value, RpcError> {
        let target = self.attached()?;
        let _changing = self.changing();
        let value = self
            .runner
            .run(command::sharing_set(&target, mode), None)
            .await?;
        let mut install = self.install.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(install) = install.as_mut().filter(|install| install.target == target) {
            install.sharing_mode = Some(mode);
        }
        Ok(json!({ "sharing_mode": value["sharing_mode"].as_str().unwrap_or(mode.as_str()) }))
    }
}
