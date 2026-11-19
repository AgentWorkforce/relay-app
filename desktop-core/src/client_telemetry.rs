//! What Desktop tells Cloud about itself for the fleet view: its release and
//! platform in the `User-Agent` of the `access/me` read and of the enrollment
//! key exchange, and on `access/me` the workspace policy revision it applied
//! (`X-Agent-Relay-Policy-Revision`), omitted until one was applied. Other
//! Cloud calls keep relay-node's own `User-Agent`. Cloud records them for
//! the session (relay-control docs/identity.md, "Desktop client telemetry").

use agent_relay_node::relay::cloud_http;
use agent_relay_node::relay::cloud_session::{api_base, status_error, timeout_for, CloudError};
use agent_relay_node::relay::workspace::Secret;
use serde_json::Value;

/// The header naming the workspace policy (v2) revision applied.
pub const POLICY_REVISION_HEADER: &str = "X-Agent-Relay-Policy-Revision";

/// `AgentRelayDesktop/<release> (<platform>)`, the platform one of `macos`,
/// `linux` or `windows`. Cloud reads a release of at most 40 characters of
/// `[0-9A-Za-z.+-]`, so any other character is left out and a longer one cut.
pub fn user_agent() -> String {
    format!(
        "AgentRelayDesktop/{} ({})",
        release(agent_relay_node::VERSION),
        crate::telemetry::platform()
    )
}

fn release(version: &str) -> String {
    version
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
        .take(40)
        .collect()
}

/// The headers an `access/me` read carries: the `User-Agent`, and the
/// applied policy revision when there is one Cloud reads (0 to 2147483647).
pub fn headers(policy_revision: Option<i64>) -> Vec<(&'static str, String)> {
    let mut headers = vec![("User-Agent", user_agent())];
    if let Some(revision) = policy_revision.filter(|r| (0..=i64::from(i32::MAX)).contains(r)) {
        headers.push((POLICY_REVISION_HEADER, revision.to_string()));
    }
    headers
}

/// `cloud_session::api` with [`headers`]: one call to the Cloud API at
/// `site_url`, answering its body or what the failure means.
pub async fn api(
    site_url: &str,
    bearer: &Secret,
    method: &'static str,
    path: &str,
    body: Option<Value>,
    policy_revision: Option<i64>,
) -> Result<Value, CloudError> {
    let url = format!("{}{path}", api_base(site_url));
    let bearer = bearer.expose().to_string();
    let timeout = timeout_for(path);
    let answer = tokio::task::spawn_blocking(move || {
        let headers = headers(policy_revision);
        let headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        cloud_http::send_blocking_with_headers(
            method,
            &url,
            Some(&bearer),
            &headers,
            body.as_ref(),
            timeout,
        )
    })
    .await
    .map_err(|_| CloudError::Unreachable)?
    .map_err(|_| CloudError::Unreachable)?;
    if answer.ok() {
        Ok(answer.body)
    } else {
        Err(status_error(&answer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_agent_names_the_release_and_this_platform() {
        let platform = if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(windows) {
            "windows"
        } else {
            "linux"
        };
        assert_eq!(
            user_agent(),
            format!("AgentRelayDesktop/{} ({platform})", agent_relay_node::VERSION)
        );
    }

    /// The shape Cloud reads: the product first, a release of at most 40
    /// characters of `[0-9A-Za-z.+-]`, one lowercase platform token.
    #[test]
    fn the_user_agent_is_a_shape_cloud_reads() {
        let agent = user_agent();
        assert!(agent.len() <= 512);
        let (version, platform) = agent
            .strip_prefix("AgentRelayDesktop/")
            .and_then(|rest| rest.strip_suffix(')'))
            .and_then(|rest| rest.split_once(" ("))
            .expect("AgentRelayDesktop/<release> (<platform>)");
        assert!(!version.is_empty() && version.len() <= 40, "{version}");
        assert!(version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-')));
        assert!(["macos", "linux", "windows"].contains(&platform), "{platform}");
    }

    #[test]
    fn a_release_cloud_would_ignore_is_trimmed_to_one_it_reads() {
        assert_eq!(release("2026.11.15"), "2026.11.15");
        assert_eq!(release("2026.11.15-dev_local build"), "2026.11.15-devlocalbuild");
        assert_eq!(release(&"9".repeat(50)), "9".repeat(40));
    }

    #[test]
    fn the_revision_is_sent_only_once_one_was_applied() {
        assert_eq!(headers(None), vec![("User-Agent", user_agent())]);
        assert_eq!(
            headers(Some(7)),
            vec![
                ("User-Agent", user_agent()),
                (POLICY_REVISION_HEADER, "7".to_string())
            ]
        );
        assert_eq!(headers(Some(-1)), vec![("User-Agent", user_agent())]);
        assert_eq!(headers(Some(1 << 31)), vec![("User-Agent", user_agent())]);
    }
}
