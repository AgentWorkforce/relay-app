//! Request headers for the site's signed release feed (Cloud's proposed
//! `/cloud/api/v1/desktop/feed/...` routes; not a stable contract yet).
//!
//! The same retained desktop session and pins as `account.mcp_headers`, without
//! the hosted MCP capability check: every signed-in member reads the feed, and
//! Cloud answers 403 `not_entitled` for a workspace that may not.

use super::*;
use crate::contract::account::{FeedHeaders, FeedHeadersParams};

/// The site profile id grammar the release feed verifier also enforces
/// (`gtk/src/updates/feed/manifest.rs`, `valid_profile`).
fn valid_profile(id: &str) -> bool {
    (1..=32).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

impl Account {
    /// `account.feed_headers`. Called again after a 401: the identity check
    /// in [`Account::desktop_credential`] renews a token Cloud rejected.
    pub async fn feed_headers(&self, request: FeedHeadersParams) -> Result<FeedHeaders, RpcError> {
        if !valid_profile(&request.profile_id) {
            return Err(RpcError::new(
                ErrorKind::InvalidParams,
                "The site profile id must be 1 to 32 lowercase letters or digits.",
            ));
        }
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
        if !self.health_is_current(&check) {
            return Err(RpcError::new(
                ErrorKind::NotSignedIn,
                "Agent Relay sign-in changed. Check for updates again.",
            ));
        }
        Ok(FeedHeaders {
            headers: std::collections::BTreeMap::from([
                ("Authorization".into(), format!("Bearer {}", token.expose())),
                ("X-Agent-Relay-Profile".into(), request.profile_id),
                ("X-Agent-Relay-Desktop".into(), "1".into()),
            ]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::valid_profile;

    #[test]
    fn a_profile_id_is_short_lowercase_ascii() {
        for good in ["x7k2q", "a", &"z".repeat(32)] {
            assert!(valid_profile(good), "{good}");
        }
        for bad in ["", "X7K2Q", "x7-k2", "x7 k2", "é", &"z".repeat(33)] {
            assert!(!valid_profile(bad), "{bad}");
        }
    }
}
