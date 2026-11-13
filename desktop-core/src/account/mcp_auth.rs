//! Credential-only integration with the existing hosted MCP server.

mod capability;
mod feed_auth;
pub(super) use capability::Capability;
use capability::Finding;

use super::health::HealthCheck;
use super::*;
use crate::contract::account::{McpHeaders, McpHeadersParams};

impl Account {
    pub(super) fn prepare_sign_out(&self) {
        if let Some(installer) = self.mcp_config.get().cloned() {
            installer.invalidate_runtime();
        }
        self.stop_attempt(false);
    }

    pub async fn mcp_headers(&self, request: McpHeadersParams) -> Result<McpHeaders, RpcError> {
        let check = self
            .pinned_context(
                &request.site_url,
                &request.workspace_id,
                &request.account_id,
            )
            .ok_or_else(|| {
                RpcError::new(
                    ErrorKind::NotSignedIn,
                    "Sign in to the configured Agent Relay account and workspace first.",
                )
            })?;
        let (token, _) = self
            .desktop_credential(&check, agent_relay_node::relay::state::now_ms(), false)
            .await
            .map_err(|_| {
                RpcError::new(
                    ErrorKind::NotSignedIn,
                    "Agent Relay needs you to sign in again.",
                )
            })?;
        self.require_native_mcp(&check, &token).await?;
        if !self.health_is_current(&check) {
            return Err(RpcError::new(
                ErrorKind::NotSignedIn,
                "Agent Relay sign-in changed. Reconnect this MCP configuration.",
            ));
        }
        Ok(McpHeaders {
            headers: std::collections::BTreeMap::from([(
                "Authorization".into(),
                format!("Bearer {}", token.expose()),
            )]),
        })
    }

    /// The signed-in context, when the caller's site, workspace and account
    /// pins name it.
    pub(super) fn pinned_context(
        &self,
        site_url: &str,
        workspace_id: &str,
        account_id: &str,
    ) -> Option<HealthCheck> {
        if self.status().state != SignInState::SignedIn {
            return None;
        }
        let check = HealthCheck {
            epoch: self.epoch.load(Ordering::SeqCst),
            revision: self.uploader.revision(),
            target: self.uploader.target()?,
        };
        let site = validated_base_url(site_url)?;
        (site == check.target.site_url
            && check.target.workspace_id.as_deref() == Some(workspace_id)
            && check.target.account_id.as_deref() == Some(account_id))
        .then_some(check)
    }

    /// Uses only this desktop's retained session, never the independent CLI or environment.
    pub(super) async fn desktop_credential(
        &self,
        check: &HealthCheck,
        now: i64,
        analytics: bool,
    ) -> Result<(Secret, Identity), CloudError> {
        // A concurrent collector may rotate the same session while whoami answers.
        for _ in 0..2 {
            let session = self
                .load_session()
                .filter(|s| s.site_url == check.target.site_url)
                .ok_or(CloudError::Rejected)?;
            if self.environment_conflicts_with(&session.access_token) {
                return Err(CloudError::Rejected);
            }
            let token = if access_is_fresh(&session, FRESH_MARGIN, now) {
                session.access_token
            } else {
                self.renew_for_health(&check.target.site_url, &session.access_token)
                    .await?
                    .access_token
            };
            let (token, identity) = self
                .confirm_desktop_identity(check, token, analytics)
                .await?;
            if self.environment_conflicts_with(&token) {
                return Err(CloudError::Rejected);
            }
            if !self.health_is_current(check) {
                return Err(CloudError::SignedOut);
            }
            if self.load_session().is_some_and(|s| s.access_token == token) {
                return Ok((token, identity));
            }
        }
        Err(CloudError::Unreachable)
    }

    pub(super) fn environment_conflicts_with(&self, token: &Secret) -> bool {
        self.env_token
            .as_ref()
            .is_some_and(|environment| environment != token)
    }

    pub(super) fn health_environment_conflict(&self) -> bool {
        self.env_token.as_ref().is_some_and(|environment| {
            self.load_session()
                .is_none_or(|session| session.access_token != *environment)
        })
    }

    async fn confirm_desktop_identity(
        &self,
        check: &HealthCheck,
        token: Secret,
        analytics: bool,
    ) -> Result<(Secret, Identity), CloudError> {
        let site = &check.target.site_url;
        let (token, identity) = match cloud_session::whoami_with_analytics(site, &token, analytics)
            .await
        {
            Err(CloudError::Rejected) => {
                let renewed = self.renew_for_health(site, &token).await?;
                let identity =
                    cloud_session::whoami_with_analytics(site, &renewed.access_token, analytics)
                        .await?;
                (renewed.access_token, identity)
            }
            result => (token, result?),
        };
        if check.target.account_id.as_deref() != Some(identity.user_id.as_str())
            || check.target.workspace_id != identity.current_workspace_id
        {
            return Err(CloudError::Rejected);
        }
        Ok((token, identity))
    }

    pub(super) async fn probe_hosted_mcp(
        &self,
        check: &HealthCheck,
        token: &Secret,
    ) -> Result<(), CloudError> {
        let current = self.load_session().filter(|s| s.access_token != *token);
        let token = if current.is_some() {
            self.desktop_credential(check, agent_relay_node::relay::state::now_ms(), false)
                .await?
                .0
        } else {
            token.clone()
        };
        let site = &check.target.site_url;
        let result = match probe_tools(site, &token).await {
            Err(CloudError::Rejected) => {
                self.renew_for_health(site, &token).await?;
                let (renewed, _) = self
                    .desktop_credential(check, agent_relay_node::relay::state::now_ms(), false)
                    .await?;
                probe_tools(site, &renewed).await
            }
            result => result,
        };
        // A self-hosted site may have no hosted MCP: the connection is
        // verified all the same, and hosted MCP is reported unavailable
        // (`require_native_mcp`), not hidden behind an unverified connection.
        // Only a current check speaks for this sign-in (as
        // `publish_auth_health` keeps its result): one that outlived a
        // sign-out, or finished after a newer one, neither notices nor
        // forgets.
        match result {
            Err(error) if hosted_mcp_absent(site, &error) => {
                if self.health_is_current(check) {
                    let mut state = self.mcp_capability.lock().await;
                    let state = state.get_or_insert_with(Capability::default);
                    let finding = Finding::of(check);
                    if state.absent.as_ref() != Some(&finding) {
                        state.absent = Some(finding);
                        if let Some(core) = self.core() {
                            core.hub
                                .activity(Level::Info, hosted_mcp_absent_message(site));
                        }
                    }
                }
                Ok(())
            }
            // Only the route answering again forgets the notice; an outage
            // in between must not make it repeat.
            Ok(()) => {
                if self.health_is_current(check) {
                    if let Some(state) = self.mcp_capability.lock().await.as_mut() {
                        state.absent = None;
                    }
                }
                Ok(())
            }
            result => result,
        }
    }
}

/// Whether `error` from the hosted MCP route means the site has none: a
/// 404 beside a site other than production, which always has it.
pub(super) fn hosted_mcp_absent(site: &str, error: &CloudError) -> bool {
    site != agent_relay_node::relay::links::DEFAULT_SITE_URL
        && matches!(error, CloudError::Status { status: 404, .. })
}

pub(super) fn hosted_mcp_absent_message(site: &str) -> String {
    format!("Hosted Agent Relay MCP is not available on {site}, so provider chats that use it are not available on this site.")
}

#[cfg(test)]
mod absent_tests {
    use super::*;

    #[test]
    fn a_404_means_no_hosted_mcp_only_beside_another_site() {
        let missing = CloudError::Status {
            status: 404,
            message: None,
        };
        assert!(hosted_mcp_absent("https://gcp-proof.example", &missing));
        assert!(!hosted_mcp_absent(
            agent_relay_node::relay::links::DEFAULT_SITE_URL,
            &missing
        ));
        for error in [
            CloudError::Unreachable,
            CloudError::Rejected,
            CloudError::Status {
                status: 503,
                message: None,
            },
            CloudError::Status {
                status: 426,
                message: Some("update".into()),
            },
        ] {
            assert!(
                !hosted_mcp_absent("https://gcp-proof.example", &error),
                "{error}"
            );
        }
        assert!(hosted_mcp_absent_message("https://gcp-proof.example")
            .contains("https://gcp-proof.example"));
    }
}

// Discovery never invokes tools and therefore cannot create a hosted identity.
pub(super) async fn probe_tools(site: &str, token: &Secret) -> Result<(), CloudError> {
    let initialize = json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params": {
            "protocolVersion":"2025-06-18", "capabilities":{},
            "clientInfo":{"name":"agent-relay-desktop-health", "version":agent_relay_node::VERSION}}});
    let initialized = hosted_read(site, token, initialize).await?;
    capability::validate_native_capability(&initialized)?;
    if !initialized["result"]["serverInfo"].is_object() {
        return Err(CloudError::Unreachable);
    }
    // A site that answered initialize has the route: a 404 on tools/list
    // is a failure of that site, never "no hosted MCP here".
    let tools = hosted_read(
        site,
        token,
        json!({"jsonrpc":"2.0", "id":2, "method":"tools/list", "params":{}}),
    )
    .await
    .map_err(|error| match error {
        CloudError::Status { status: 404, .. } => CloudError::Unreachable,
        error => error,
    })?;
    let advertised = tools["result"]["tools"]
        .as_array()
        .ok_or(CloudError::Unreachable)?;
    let required = ["search_shared_sessions", "send_relay_message"];
    if !required.iter().all(|name| {
        advertised
            .iter()
            .any(|tool| tool["name"].as_str() == Some(name))
    }) {
        return Err(CloudError::Unreachable);
    }
    Ok(())
}
pub(super) async fn hosted_read(
    site: &str,
    token: &Secret,
    body: Value,
) -> Result<Value, CloudError> {
    let url = format!("{}/mcp/shared-sessions", cloud_session::api_base(site));
    let token = token.clone();
    let result = tokio::task::spawn_blocking(move || {
        agent_relay_node::relay::cloud_http::send_raw_blocking(
            "POST",
            &url,
            Some(token.expose()),
            &[
                ("Accept", "application/json, text/event-stream"),
                ("Content-Type", "application/json"),
                ("MCP-Protocol-Version", "2025-06-18"),
            ],
            Some(body.to_string().as_bytes()),
            cloud_session::REQUEST_TIMEOUT,
        )
    })
    .await
    .map_err(|_| CloudError::Unreachable)?
    .map_err(|_| CloudError::Unreachable)?;
    if matches!(result.status, 401 | 403) {
        return Err(CloudError::Rejected);
    }
    // A site without the hosted MCP route answers 404; told apart from an
    // outage so a self-hosted site can lack it (`hosted_mcp_absent`).
    if result.status == 404 {
        return Err(CloudError::Status {
            status: 404,
            message: None,
        });
    }
    if !(200..300).contains(&result.status) {
        return Err(CloudError::Unreachable);
    }
    let reply: Value = serde_json::from_slice(&result.body).map_err(|_| CloudError::Unreachable)?;
    if !reply["error"].is_null() {
        return Err(CloudError::Unreachable);
    }
    Ok(reply)
}
