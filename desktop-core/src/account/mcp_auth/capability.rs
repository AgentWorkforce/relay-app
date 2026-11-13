//! What this sign-in learned about the hosted MCP route: a short-lived,
//! account-generation scoped proof of the hosted identity protocol, and
//! whether the route was found absent on the site.
use super::*;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(in crate::account) struct Capability {
    verified: Option<Verified>,
    /// The health check that found the site without a hosted MCP route, so
    /// the Activity notice is given once per finding: a sign-out or a new
    /// attachment is a new finding, and only the route answering again
    /// forgets this one (`probe_hosted_mcp`).
    pub(super) absent: Option<Finding>,
}

struct Verified {
    epoch: u64,
    revision: u64,
    target: Target,
    checked: Instant,
}

#[derive(PartialEq)]
pub(super) struct Finding {
    epoch: u64,
    revision: u64,
    target: Target,
}

impl Finding {
    pub(super) fn of(check: &HealthCheck) -> Self {
        Finding {
            epoch: check.epoch,
            revision: check.revision,
            target: check.target.clone(),
        }
    }
}

impl Account {
    pub(in crate::account) async fn require_native_mcp(
        &self,
        check: &HealthCheck,
        token: &Secret,
    ) -> Result<(), RpcError> {
        let mut cached = self.mcp_capability.lock().await;
        if cached
            .as_ref()
            .and_then(|c| c.verified.as_ref())
            .is_some_and(|c| {
                c.epoch == check.epoch
                    && c.revision == check.revision
                    && c.target == check.target
                    && c.checked.elapsed() < Duration::from_secs(60)
            })
        {
            return Ok(());
        }
        let site = &check.target.site_url;
        let reply = super::hosted_read(site, token, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"agent-relay-desktop-capability","version":agent_relay_node::VERSION}
        }})).await.map_err(|error| if super::hosted_mcp_absent(site, &error) {
            RpcError::new(ErrorKind::Refused, super::hosted_mcp_absent_message(site))
        } else {
            RpcError::new(ErrorKind::CloudFailed, "Hosted Agent Relay identity support could not be verified. Try again later.")
        })?;
        validate_native_capability(&reply)
            .map_err(|error| RpcError::new(ErrorKind::CloudFailed, error.to_string()))?;
        if !self.health_is_current(check) {
            return Err(RpcError::new(
                ErrorKind::NotSignedIn,
                "Agent Relay sign-in changed. Reconnect this MCP configuration.",
            ));
        }
        cached.get_or_insert_with(Capability::default).verified = Some(Verified {
            epoch: check.epoch,
            revision: check.revision,
            target: check.target.clone(),
            checked: Instant::now(),
        });
        Ok(())
    }
}

/// Both exported credentials and public health require the same hosted protocol.
pub(super) fn validate_native_capability(reply: &Value) -> Result<(), CloudError> {
    if reply["result"]["capabilities"]["experimental"]["com.agentrelay/native-session-identity"]
        ["version"]
        .as_u64()
        == Some(1)
    {
        Ok(())
    } else {
        Err(CloudError::Status { status: 426, message: Some("Hosted Agent Relay needs an update before this desktop can safely connect provider chats.".into()) })
    }
}
