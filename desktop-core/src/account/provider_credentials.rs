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

impl Account {
    /// Called before installation and periodically by the daemon. No user token
    /// enters a provider config: only endpoint-scoped, parent-bound child grants.
    pub(crate) async fn refresh_provider_credentials(
        &self,
        requested: Vec<String>,
        retry_removals: bool,
    ) {
        let _one = self.provider_credentials.lock().await;
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
