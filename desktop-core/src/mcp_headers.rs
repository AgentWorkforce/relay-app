//! Credential-only helper for clients connecting directly to the hosted MCP.
//! No MCP protocol, credentials on disk, or independent login is provided here.

use agent_relay_node::relay::daemon::SafeError;
use anyhow::Result;
use serde_json::{json, Value};
use std::io::Write;
use std::time::Duration;

const FAILURE: &str = "Agent Relay could not supply hosted MCP authentication. Open the app and check Account health.";

fn headers(
    site: &str,
    workspace: &str,
    account: &str,
    call: impl FnOnce(&str, Value) -> Result<Value>,
) -> Result<Value> {
    let response = call(
        "account.mcp_headers",
        json!({
            "site_url": site, "workspace_id": workspace, "account_id": account,
        }),
    )
    .map_err(|_| SafeError(FAILURE.into()))?;
    let headers = response
        .get("headers")
        .and_then(Value::as_object)
        .filter(|headers| headers.len() == 1)
        .ok_or_else(|| SafeError(FAILURE.into()))?;
    let authorization = headers
        .get("Authorization")
        .and_then(Value::as_str)
        .filter(|value| {
            value
                .strip_prefix("Bearer ")
                .is_some_and(|token| !token.is_empty() && !token.chars().any(char::is_whitespace))
        })
        .ok_or_else(|| SafeError(FAILURE.into()))?;
    Ok(json!({"Authorization": authorization, "X-Agent-Relay-Desktop": "1"}))
}

/// Prints only the hosted HTTP header object, never a bridge result wrapper.
/// CorePaths selects the same user-owned core socket/named pipe as the app.
/// The RPC pins origin, workspace and account; the core alone renews the token.
pub fn print(site: &str, workspace: &str, account: &str) -> Result<()> {
    print_at(None, site, workspace, account)
}

pub fn print_at(
    socket: Option<&std::path::Path>,
    site: &str,
    workspace: &str,
    account: &str,
) -> Result<()> {
    let headers = headers(site, workspace, account, |method, params| {
        agent_relay_node::relay::cli::call(socket, method, params, Duration::from_secs(30))
    })?;
    let mut bytes = serde_json::to_vec(&headers).map_err(|_| SafeError(FAILURE.into()))?;
    bytes.push(b'\n');
    std::io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|_| SafeError(FAILURE.into()))?;
    Ok(())
}

/// `mcp-headers --site <url> --workspace <id> --account <id> [--socket <path>]`.
pub fn run(arguments: &[String]) -> i32 {
    let value = |name: &str| {
        arguments
            .iter()
            .position(|argument| argument == name)
            .and_then(|at| arguments.get(at + 1))
    };
    let (Some(site), Some(workspace), Some(account)) =
        (value("--site"), value("--workspace"), value("--account"))
    else {
        eprintln!("mcp-headers needs --site, --workspace and --account.");
        return 2;
    };
    let socket = value("--socket").map(std::path::Path::new);
    match print_at(socket, site, workspace, account) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_workspace_and_site_are_pinned_and_only_headers_are_returned() {
        let result = headers("https://agentrelay.com", "workspace-1", "account-1", |method, params| {
            assert_eq!(method, "account.mcp_headers");
            assert_eq!(params, json!({"site_url":"https://agentrelay.com", "workspace_id":"workspace-1", "account_id":"account-1"}));
            Ok(json!({"headers":{"Authorization":"Bearer fixture-only"}, "ignored":"private"}))
        }).unwrap();
        assert_eq!(
            result,
            json!({"Authorization":"Bearer fixture-only", "X-Agent-Relay-Desktop":"1"})
        );
    }

    #[test]
    fn rpc_failures_never_expose_raw_errors_or_credentials() {
        let error = headers("site", "workspace", "account", |_, _| {
            Err(anyhow::anyhow!("Authorization: Bearer private-fixture"))
        })
        .unwrap_err();
        assert_eq!(error.to_string(), FAILURE);
    }

    #[test]
    fn malformed_headers_are_refused_without_echoing_the_response() {
        for response in [
            json!(null),
            json!({"headers":{}}),
            json!({"headers":{"Authorization":7}}),
            json!({"headers":{"Authorization":"Basic fixture"}}),
            json!({"headers":{"Authorization":"Bearer "}}),
            json!({"headers":{"Authorization":"Bearer fixture", "X-Extra":"private-fixture"}}),
            json!({"headers":{"Authorization":"Bearer private-fixture\r\nInjected: value"}}),
        ] {
            let error = headers("site", "workspace", "account", |_, _| Ok(response)).unwrap_err();
            assert_eq!(error.to_string(), FAILURE);
        }
    }
}
