//! Short, revocable leases for native clients that cache HTTP credentials.
//! The app renews the lease; providers keep using the same hosted MCP bearer.
mod file;
mod remote;
#[cfg(test)]
mod tests;

use super::{health::HealthCheck, *};
use crate::mcp_settings::{Binding, Installer};
use file::{Credentials, Grant};
use std::collections::{BTreeMap, BTreeSet};

/// Why account health asks for a sign-in while the login itself still works.
pub(super) const PROVIDER_SIGN_IN_AGAIN: &str = "Sign in again to share your sessions.";

/// Account health with a provider's need for a new app login folded in: a
/// ready login that cannot issue provider credentials needs a sign-in, and
/// that sign-in requirement (and only it) clears once they issue again.
pub(super) fn health_with_provider_sign_in(
    health: Option<&str>,
    error: Option<&str>,
    required: bool,
) -> (Option<String>, Option<String>) {
    match health {
        Some("ready") if required => (
            Some("sign_in_required".into()),
            Some(PROVIDER_SIGN_IN_AGAIN.into()),
        ),
        Some("sign_in_required") if !required && error == Some(PROVIDER_SIGN_IN_AGAIN) => {
            (Some("ready".into()), None)
        }
        _ => (health.map(Into::into), error.map(Into::into)),
    }
}

impl Account {
    /// Called before installation and periodically by the daemon. No user token
    /// enters a provider config: only endpoint-scoped, parent-bound child grants.
    pub(crate) async fn refresh_provider_credentials(
        &self,
        requested: Vec<String>,
        retry_removals: bool,
    ) {
        {
            let _one = self.provider_credentials.lock().await;
            self.refresh_provider_grants(requested, retry_removals).await;
        }
        self.sync_provider_sign_in_health();
    }

    async fn refresh_provider_grants(&self, requested: Vec<String>, retry_removals: bool) {
        let Some(installer) = self.mcp_config.get().cloned() else {
            return;
        };
        let Some((check, binding)) = self.provider_context() else {
            installer.clear_provider_credentials();
            return;
        };
        let selected: BTreeSet<_> = installer
            .provider_ids()
            .into_iter()
            .chain(requested.iter().cloned())
            .filter(|id| Installer::credential_provider(id))
            .collect();
        let path = self.provider_credentials_path();
        let mut saved = match Credentials::load(&path, &binding) {
            Ok(saved) => saved,
            Err(_) => {
                installer.clear_provider_credentials();
                return;
            }
        };
        if selected.is_empty() && saved.grants.is_empty() {
            installer.clear_provider_credentials();
            return;
        }
        let now = agent_relay_node::relay::state::now_ms();
        for (id, grant) in &mut saved.grants {
            if requested.contains(id) || (retry_removals && !selected.contains(id)) {
                grant.retry_after = 0;
            }
        }
        if !saved.needs_network(&selected, now) && self.health_is_current(&check) {
            saved.publish(&installer, &binding, &selected, now);
            return;
        }
        let token = match self.desktop_credential(&check, now, false).await {
            Ok((token, _)) => token,
            Err(error) => {
                if matches!(error, CloudError::Rejected | CloudError::SignedOut) {
                    installer.clear_provider_credentials();
                } else if self.health_is_current(&check) {
                    saved.publish(&installer, &binding, &selected, now);
                }
                return;
            }
        };
        if !self.health_is_current(&check) {
            installer.clear_provider_credentials();
            return;
        }
        // Persist random material before issuing it, so interrupted POST retries
        // reuse the identical credential instead of creating orphaned grants.
        for id in &selected {
            if !saved.grants.contains_key(id) {
                let Ok(grant) = Grant::new() else { return };
                saved.grants.insert(id.clone(), grant);
            }
        }
        if saved.save(&path).is_err() {
            installer.clear_provider_credentials();
            return;
        }
        let answers = remote::refresh(
            &binding,
            &token,
            &saved.installation_id,
            &saved.grants,
            &selected,
            now,
        )
        .await;
        if !self.health_is_current(&check) {
            installer.clear_provider_credentials();
            // New grants expire automatically even if their parent cannot be reached.
            return;
        }
        saved.apply(answers, agent_relay_node::relay::state::now_ms());
        if saved.save(&path).is_err() {
            installer.clear_provider_credentials();
            return;
        }
        saved.publish(
            &installer,
            &binding,
            &selected,
            agent_relay_node::relay::state::now_ms(),
        );
    }

    /// After a sign-in again: issues the grants that waited for a new app
    /// login now, instead of after their retry delay.
    pub(super) async fn retry_provider_sign_in(&self) {
        let (Some(installer), Some((_, binding))) =
            (self.mcp_config.get().cloned(), self.provider_context())
        else {
            return;
        };
        let selected = installer.provider_ids();
        let waiting: Vec<String> = Credentials::load(&self.provider_credentials_path(), &binding)
            .map(|saved| {
                saved
                    .grants
                    .into_iter()
                    .filter(|(id, grant)| grant.needs_app_login && selected.contains(id))
                    .map(|(id, _)| id)
                    .collect()
            })
            .unwrap_or_default();
        if !waiting.is_empty() {
            self.refresh_provider_credentials(waiting, false).await;
        }
    }

    /// Account health says a sign-in is needed while Cloud refuses this
    /// login for provider credentials (a login from before the Desktop
    /// consent screen), so the Account page offers Sign in again instead of
    /// showing a healthy connection; and returns to ready once it is fixed.
    pub(super) fn sync_provider_sign_in_health(&self) {
        // Read before updating: the status lock is held inside `update`.
        let required = self.provider_sign_in_required();
        let status = self.status();
        let next = health_with_provider_sign_in(
            status.auth_health.as_deref(),
            status.auth_error.as_deref(),
            required,
        );
        if status.state != SignInState::SignedIn || next == (status.auth_health, status.auth_error)
        {
            return;
        }
        self.update(|status| {
            if status.state == SignInState::SignedIn {
                (status.auth_health, status.auth_error) = health_with_provider_sign_in(
                    status.auth_health.as_deref(),
                    status.auth_error.as_deref(),
                    required,
                );
            }
        });
    }

    /// A selected provider's grant needs a new app login (an older login
    /// without provider consent), so sign-in renews instead of returning
    /// the existing account.
    pub(super) fn provider_sign_in_required(&self) -> bool {
        let Some(installer) = self.mcp_config.get().cloned() else {
            return false;
        };
        self.provider_context()
            .is_some_and(|(_, binding)| installer.provider_login_required(&binding))
    }

    fn provider_context(&self) -> Option<(HealthCheck, Binding)> {
        if self.status().state != SignInState::SignedIn {
            return None;
        }
        let target = self.uploader.target()?;
        let binding = Binding {
            site_url: validated_base_url(&target.site_url)?,
            workspace_id: target.workspace_id.clone()?,
            account_id: target.account_id.clone()?,
        };
        if !crate::cloud_api::is_workspace_uuid(&binding.workspace_id) {
            return None;
        }
        Some((
            HealthCheck {
                epoch: self.epoch.load(Ordering::SeqCst),
                revision: self.uploader.revision(),
                target,
            },
            binding,
        ))
    }

    fn provider_credentials_path(&self) -> PathBuf {
        self.legacy_session
            .path
            .with_file_name("mcp-provider-credentials.json")
    }

    /// Caller holds the lifecycle lock, before forgetting the parent login.
    pub(super) async fn revoke_provider_credentials(&self) {
        if let Some(installer) = self.mcp_config.get().cloned() {
            installer.clear_provider_credentials();
        }
        let path = self.provider_credentials_path();
        if let Some((_, binding)) = self.provider_context() {
            if let (Ok(saved), Some(session)) =
                (Credentials::load(&path, &binding), self.load_session())
            {
                let revocations = saved
                    .grants
                    .values()
                    .filter_map(|grant| grant.id.as_ref())
                    .map(|id| remote::revoke(&binding, &session.access_token, id));
                // Sign-out never waits indefinitely on an offline server. The
                // parent is revoked by existing sign-out cleanup as well.
                let _ = tokio::time::timeout(
                    Duration::from_secs(5),
                    futures_util::future::join_all(revocations),
                )
                .await;
            }
        }
        let _ = std::fs::remove_file(path);
    }
}
