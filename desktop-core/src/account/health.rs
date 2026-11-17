//! Live authentication checks shared by the app and the hosted MCP credential helper.

use super::*;

impl Account {
    pub(super) fn core(&self) -> Option<Arc<Core>> {
        self.core.get().and_then(Weak::upgrade)
    }

    /// Stops what the account runs when the daemon stops: a sign-in in
    /// flight and every uploader command.
    pub async fn shutdown(&self) {
        self.abort_auth_health();
        let task = self
            .attempt
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .task
            .take();
        if let Some(task) = task {
            task.abort();
        }
        let _ = self.uploader.runner().cancel_all().await;
    }

    /// Validates the same canonical Cloud credential used by Cloud/MCP requests.
    /// Installation metadata alone never establishes authentication. Checks are
    /// coalesced so UI polling cannot race rotating refresh tokens or flood Cloud.
    pub async fn check_auth_health(&self) {
        self.auth_health(false).await;
    }

    /// At most one supervised check exists, including while Cloud is slow.
    /// Shutdown owns and aborts this task; maintenance never waits for it.
    pub(super) fn schedule_auth_health(self: &Arc<Self>) {
        let mut task = self.auth_task.lock().unwrap_or_else(|p| p.into_inner());
        if task.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        let account = self.clone();
        *task = Some(tokio::spawn(async move {
            account.check_auth_health().await;
            account.refresh_workspace_name().await;
        }));
    }

    pub async fn recheck_auth_health(&self) {
        self.auth_health(true).await;
    }

    pub(super) fn abort_auth_health(&self) {
        if let Some(task) = self
            .auth_task
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            task.abort();
        }
    }

    pub(super) async fn needs_auth_renewal(&self, install: &Install) -> bool {
        self.status().auth_health.as_deref() == Some("sign_in_required")
            || self.provider_sign_in_required()
            || self.must_sign_in_again(&install.target.site_url).await
    }

    async fn auth_health(&self, force: bool) {
        let generation = self.auth_generation.load(Ordering::SeqCst);
        let _one = self.auth_check.lock().await;
        if force && self.auth_generation.load(Ordering::SeqCst) != generation {
            return;
        }
        let before = self.status();
        let now = agent_relay_node::relay::state::now_ms();
        let Some(check) = self.health_context(&before, force, now) else {
            return;
        };
        if before.auth_health.is_none() {
            self.update(|status| status.auth_health = Some("checking".into()));
        }
        let result = self.verify_auth_connection(&check, now).await;
        self.publish_auth_health(&check, result);
    }

    fn health_context(&self, status: &AccountStatus, force: bool, now: i64) -> Option<HealthCheck> {
        if status.state != SignInState::SignedIn {
            return None;
        }
        if !force
            && status
                .auth_checked_at
                .is_some_and(|at| now.saturating_sub(at) < 30_000)
        {
            return None;
        }
        Some(HealthCheck {
            epoch: self.epoch.load(Ordering::SeqCst),
            revision: self.uploader.revision(),
            target: self.uploader.target()?,
        })
    }

    pub(super) fn health_is_current(&self, check: &HealthCheck) -> bool {
        self.epoch.load(Ordering::SeqCst) == check.epoch
            && self.uploader.revision() == check.revision
            && self.uploader.target().as_ref() == Some(&check.target)
    }

    async fn health_identity(
        &self,
        check: &HealthCheck,
        now: i64,
    ) -> Result<(Identity, bool, Secret), CloudError> {
        let profile_due = self
            .workspace_name_retry
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .not_before
            .is_none_or(|at| tokio::time::Instant::now() >= at);
        let analytics = profile_due
            && self
                .telemetry
                .as_ref()
                .is_some_and(|t| t.profile_sync_enabled());
        let (token, identity) = self.desktop_credential(check, now, analytics).await?;
        Ok((identity, profile_due, token))
    }

    fn update_health_profile(&self, check: &HealthCheck, identity: &Identity, profile_due: bool) {
        self.update(|status| {
            if !self.health_is_current(check) {
                return;
            }
            if let Some(name) = identity.current_workspace_name.as_ref().filter(|name| !name.trim().is_empty()) {
                status.workspace_name = Some(name.clone());
            }
            status.workspace_count = identity.workspace_count;
            status.workspace_avatar_url = identity.current_workspace_avatar_url.clone();
        });
        if profile_due {
            let mut retry = self
                .workspace_name_retry
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            retry.failures = 0;
            retry.not_before = Some(tokio::time::Instant::now() + Duration::from_secs(300));
        }
    }

    async fn verify_auth_connection(
        &self,
        check: &HealthCheck,
        now: i64,
    ) -> Result<(), HealthFailure> {
        let (identity, profile_due, token) = self.health_identity(check, now).await?;
        self.update_health_profile(check, &identity, profile_due);
        let core = self.core().ok_or(CloudError::SignedOut)?;
        // Cloud answered and the sign-in is good, but it would not give this
        // computer the workspace: say so, not that the connection failed.
        if let Some(message) = self.workspace_unavailable(check, &core) {
            return Err(HealthFailure::Workspace(message));
        }
        // The shared verifier safely recovers refused account-owned keys.
        // A missing workspace key is a provisioning failure here: the
        // retained Desktop credential has already authenticated successfully.
        let workspace = core
            .verify_workspace()
            .await
            .map_err(|error| match error.data.kind {
                ErrorKind::Unauthorized => CloudError::Rejected,
                _ => CloudError::Unreachable,
            })?;
        let bound = core.store.read(|store| {
            store.cloud.as_ref().is_some_and(|cloud| {
                cloud.account_workspace_id == check.target.workspace_id
                    && cloud.workspace_id == workspace.workspace_id
            })
        });
        if workspace.verification != agent_relay_node::relay::state::Verification::Verified || !bound {
            return Err(CloudError::Unreachable.into());
        }
        Ok(self.probe_hosted_mcp(check, &token).await?)
    }

    /// Records the last workspace resolve for the sign-in `epoch`: a refusal
    /// that is not a connection failure (no shared key, an invalid gate URL,
    /// and the like) is kept, with the text the activity warning shows; a
    /// success or a Cloud failure clears it, so the connection is reported
    /// as it is.
    pub(super) fn note_workspace_resolve<T>(
        &self,
        epoch: u64,
        workspace_id: &str,
        result: &Result<T, crate::cloud_api::ResolveError>,
    ) {
        // A resolve that returns after a sign-out or a switch leaves the
        // newer record alone.
        if self.epoch.load(Ordering::SeqCst) != epoch {
            return;
        }
        let failure = match result {
            Err(error) if !matches!(error, crate::cloud_api::ResolveError::Cloud(_)) => {
                Some(WorkspaceUnavailable {
                    epoch,
                    workspace_id: workspace_id.to_string(),
                    message: error.to_string(),
                })
            }
            _ => None,
        };
        *self
            .workspace_unavailable
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = failure;
    }

    /// Notes a resolve made for sign-in `attempt`, unless a newer sign-in
    /// has started since.
    pub(super) fn note_attempt_resolve<T>(
        &self,
        epoch: u64,
        attempt: u64,
        workspace_id: &str,
        result: &Result<T, crate::cloud_api::ResolveError>,
    ) {
        let current = self.attempt.lock().unwrap_or_else(|p| p.into_inner()).id == attempt;
        if current {
            self.note_workspace_resolve(epoch, workspace_id, result);
        }
    }

    /// Why this computer could not join the checked workspace: the last
    /// resolve's refusal, while the workspace is still not bound.
    fn workspace_unavailable(&self, check: &HealthCheck, core: &Core) -> Option<String> {
        let workspace = check.target.workspace_id.as_deref()?;
        let bound = core.store.read(|store| {
            store
                .cloud
                .as_ref()
                .and_then(|cloud| cloud.account_workspace_id.as_deref())
                == Some(workspace)
        });
        if bound {
            return None;
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        self.workspace_unavailable
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .filter(|failure| {
                failure.epoch == epoch && failure.workspace_id.eq_ignore_ascii_case(workspace)
            })
            .map(|failure| failure.message.clone())
    }

    fn publish_auth_health(&self, check: &HealthCheck, result: Result<(), HealthFailure>) {
        self.update(|status| {
            if status.state != SignInState::SignedIn || !self.health_is_current(check) {
                return;
            }
            let (health, error) = match result {
                Ok(()) => ("ready", None),
                Err(HealthFailure::Workspace(message)) => ("workspace_unavailable", Some(message)),
                Err(HealthFailure::Cloud(CloudError::SignedOut | CloudError::Rejected)) => (
                    "sign_in_required",
                    Some("Sign in again to restore Agent Relay authentication.".into()),
                ),
                Err(HealthFailure::Cloud(CloudError::Status {
                    status: 426,
                    message: Some(message),
                })) => ("offline", Some(message)),
                Err(HealthFailure::Cloud(_)) => (
                    "offline",
                    Some(
                        "Agent Relay could not verify your connection. Retrying automatically."
                            .into(),
                    ),
                ),
            };
            status.auth_health = Some(health.into());
            // The workspace refusal is the more specific cause: keep it.
            status.auth_error = if health != "workspace_unavailable"
                && self.health_environment_conflict()
            {
                Some("Remove CLOUD_API_ACCESS_TOKEN and sign in through Agent Relay to share your Desktop login with MCP.".into())
            } else { error };
            status.auth_checked_at = Some(agent_relay_node::relay::state::now_ms());
            self.auth_generation.fetch_add(1, Ordering::SeqCst);
        });
    }
}

pub(super) struct HealthCheck {
    pub(super) epoch: u64,
    pub(super) revision: u64,
    pub(super) target: Target,
}

/// Why a health check failed: the Cloud connection or sign-in, or a
/// workspace Cloud would not give this computer (`workspace_unavailable`).
pub(super) enum HealthFailure {
    Cloud(CloudError),
    Workspace(String),
}

impl From<CloudError> for HealthFailure {
    fn from(error: CloudError) -> Self {
        HealthFailure::Cloud(error)
    }
}

/// The last workspace resolve's refusal, for one sign-in and workspace.
pub(super) struct WorkspaceUnavailable {
    epoch: u64,
    workspace_id: String,
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKSPACE: &str = "00000000-0000-4000-8000-000000000001";

    fn recorded_epoch(account: &Account) -> Option<u64> {
        let failure = account.workspace_unavailable.lock().unwrap();
        failure.as_ref().map(|failure| failure.epoch)
    }

    #[test]
    fn a_late_resolve_leaves_the_newer_record_alone() {
        let dir = tempfile::tempdir().unwrap();
        let account = Account::new(
            &Default::default(),
            &dir.path().join("app-data"),
            Arc::new(Uploader::new(Arc::new(
                crate::history::uploader::ProcessRunner::new(dir.path().join("no-probe")),
            ))),
        );
        let refused = || Err::<(), _>(crate::cloud_api::ResolveError::NoKey);
        let joined = || Ok::<(), crate::cloud_api::ResolveError>(());
        let old_epoch = account.epoch.load(Ordering::SeqCst);
        let old_attempt = account.attempt.lock().unwrap().id;

        // A sign-out and a new sign-in, whose resolve is refused.
        let epoch = account.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let attempt = old_attempt + 1;
        account.attempt.lock().unwrap().id = attempt;
        account.note_attempt_resolve(epoch, attempt, WORKSPACE, &refused());
        assert_eq!(recorded_epoch(&account), Some(epoch));

        // The old sign-in's resolve returns late, from either path: the
        // newer refusal is still what health reports.
        account.note_attempt_resolve(old_epoch, old_attempt, WORKSPACE, &joined());
        account.note_workspace_resolve(old_epoch, WORKSPACE, &joined());
        assert_eq!(recorded_epoch(&account), Some(epoch));

        // A newer sign-in in the same epoch outdates this one's resolve too.
        account.attempt.lock().unwrap().id = attempt + 1;
        account.note_attempt_resolve(epoch, attempt, WORKSPACE, &joined());
        assert_eq!(recorded_epoch(&account), Some(epoch));

        // The current sign-in's own answer still replaces it.
        account.note_attempt_resolve(epoch, attempt + 1, WORKSPACE, &joined());
        assert_eq!(recorded_epoch(&account), None);
    }
}
