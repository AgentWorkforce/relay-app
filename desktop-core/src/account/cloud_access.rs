//! `Account` as the daemon's `CloudAccess`: every Cloud call made as the
//! person goes through `request`, which refreshes the kept session and
//! retries once when Cloud refuses it. The node's `relay_node_credential`
//! hook answers from memory (`relay_node_credential`).

use super::*;
use crate::client_telemetry;

impl CloudAccess for Account {
    fn may_sync_identities(&self) -> bool {
        self.identity_sync_allowed()
    }

    fn identity_sync_generation(&self) -> u64 {
        self.attempt
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .id
    }

    fn agent_owner(&self) -> Option<AgentOwner> {
        let install = self.uploader.install()?;
        Some(AgentOwner {
            user_id: install.target.account_id?,
            workspace_id: install.target.workspace_id,
        })
    }

    fn account(&self) -> Option<CloudAccount> {
        let identity = self
            .identity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(install) = self.uploader.install() {
            return Some(CloudAccount {
                site_url: install.target.site_url,
                workspace_id: install.target.workspace_id?,
                user_id: install.target.account_id.or(identity.map(|i| i.user_id)),
            });
        }
        let session = self.load_session()?;
        let workspace = self.core()?.store.read(|s| {
            s.cloud
                .as_ref()
                .and_then(|c| c.account_workspace_id.clone())
        })?;
        Some(CloudAccount {
            site_url: session.site_url,
            workspace_id: workspace,
            user_id: identity.map(|i| i.user_id),
        })
    }

    fn credential(&self, refused: Option<Secret>) -> BoxFuture<'_, Option<(String, Secret)>> {
        Box::pin(self.pinned_credential(refused))
    }

    fn device_token(&self) -> BoxFuture<'_, Option<Secret>> {
        Box::pin(self.device_bearer())
    }

    fn relay_node_credential(&self) -> Result<cloud_session::RelayNodeCredential, Option<String>> {
        self.held_relay_node_credential()
    }

    fn request(
        &self,
        method: &'static str,
        path: String,
        body: Option<Value>,
    ) -> BoxFuture<'_, Result<Value, CloudError>> {
        Box::pin(async move {
            let account = self.account().ok_or(CloudError::SignedOut)?;
            let site = account.site_url;
            let workspace = &account.workspace_id;
            let telemetry = (path == crate::history::operator_policy::access_me_path(workspace))
                .then(|| {
                    self.operator_policy
                        .workspace_policies()
                        .applied_revision(workspace)
                });
            let answer = async {
                let (bearer, kept) = self.request_bearer(&site).await?;
                match send(&site, &bearer, method, &path, body.clone(), telemetry).await {
                    Err(CloudError::Rejected) if kept => {
                        // Revoked or expired server-side despite looking fresh:
                        // renew once; a session Cloud refuses is forgotten, one
                        // it could not answer for is kept.
                        let renewed = self.renew(&site, &bearer).await?;
                        let retried =
                            send(&site, &renewed.access_token, method, &path, body, telemetry)
                                .await;
                        if matches!(retried, Err(CloudError::Rejected)) {
                            self.note_refused(&renewed);
                        }
                        retried
                    }
                    other => other,
                }
            }
            .await;
            self.note_cloud_answer(&answer);
            answer
        })
    }
}

/// One call as the person. `access/me` (`telemetry` set, with the workspace
/// policy revision applied, if any) tells Cloud the release, platform and
/// revision for the fleet view; every other call goes as before.
async fn send(
    site: &str,
    bearer: &Secret,
    method: &'static str,
    path: &str,
    body: Option<Value>,
    telemetry: Option<Option<i64>>,
) -> Result<Value, CloudError> {
    match telemetry {
        Some(revision) => client_telemetry::api(site, bearer, method, path, body, revision).await,
        None => cloud_session::api(site, bearer, method, path, body).await,
    }
}
