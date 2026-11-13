//! `feed-headers`: prints the headers for the site's signed release feed, for
//! a headless install or a person checking the feed by hand. The feed routes
//! are Cloud's proposed contract (`/cloud/api/v1/desktop/feed/...`), not a
//! stable one. No credentials on disk and no independent login.

use agent_relay_node::relay::daemon::SafeError;
use anyhow::Result;
use serde_json::{json, Map, Value};
use std::io::Write;
use std::time::Duration;

const FAILURE: &str =
    "Agent Relay could not supply release feed authentication. Open the app and check Account health.";

fn headers(
    site: &str,
    workspace: &str,
    account: &str,
    profile: &str,
    call: impl FnOnce(&str, Value) -> Result<Value>,
) -> Result<Value> {
    let response = call(
        "account.feed_headers",
        json!({
            "site_url": site, "workspace_id": workspace, "account_id": account,
            "profile_id": profile,
        }),
    )
    .map_err(|_| SafeError(FAILURE.into()))?;
    let headers = response
        .get("headers")
        .and_then(Value::as_object)
        .filter(|headers| headers.len() == 3)
        .ok_or_else(|| SafeError(FAILURE.into()))?;
    let field = |name: &str| headers.get(name).and_then(Value::as_str);
    let bearer = field("Authorization").is_some_and(|value| {
        value
            .strip_prefix("Bearer ")
            .is_some_and(|token| !token.is_empty() && !token.chars().any(char::is_whitespace))
    });
    if !bearer
        || field("X-Agent-Relay-Profile") != Some(profile)
        || field("X-Agent-Relay-Desktop") != Some("1")
    {
        return Err(SafeError(FAILURE.into()).into());
    }
    Ok(Value::Object(Map::clone(headers)))
}

/// Prints only the HTTP header object, never a bridge result wrapper, from
/// the same user-owned core socket or named pipe as the app.
pub fn print_at(
    socket: Option<&std::path::Path>,
    site: &str,
    workspace: &str,
    account: &str,
    profile: &str,
) -> Result<()> {
    let headers = headers(site, workspace, account, profile, |method, params| {
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

/// `feed-headers --site <url> --workspace <id> --account <id> --profile <id>
/// [--socket <path>]`.
pub fn run(arguments: &[String]) -> i32 {
    let value = |name: &str| {
        arguments
            .iter()
            .position(|argument| argument == name)
            .and_then(|at| arguments.get(at + 1))
    };
    let (Some(site), Some(workspace), Some(account), Some(profile)) = (
        value("--site"),
        value("--workspace"),
        value("--account"),
        value("--profile"),
    ) else {
        eprintln!("feed-headers needs --site, --workspace, --account and --profile.");
        return 2;
    };
    let socket = value("--socket").map(std::path::Path::new);
    match print_at(socket, site, workspace, account, profile) {
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

    fn answer() -> Value {
        json!({"headers":{
            "Authorization":"Bearer fixture-only",
            "X-Agent-Relay-Profile":"x7k2q",
            "X-Agent-Relay-Desktop":"1",
        }, "ignored":"private"})
    }

    #[test]
    fn pins_and_profile_are_sent_and_only_headers_are_returned() {
        let result = headers(
            "https://agentrelay.com",
            "w-1",
            "a-1",
            "x7k2q",
            |method, params| {
                assert_eq!(method, "account.feed_headers");
                assert_eq!(
                    params,
                    json!({"site_url":"https://agentrelay.com", "workspace_id":"w-1",
                    "account_id":"a-1", "profile_id":"x7k2q"})
                );
                Ok(answer())
            },
        )
        .unwrap();
        assert_eq!(result, answer()["headers"]);
    }

    #[test]
    fn failures_and_malformed_headers_never_echo_the_response() {
        let error = headers("site", "w", "a", "x7k2q", |_, _| {
            Err(anyhow::anyhow!("Authorization: Bearer private-fixture"))
        })
        .unwrap_err();
        assert_eq!(error.to_string(), FAILURE);
        let mut wrong_profile = answer();
        wrong_profile["headers"]["X-Agent-Relay-Profile"] = json!("other");
        let mut extra = answer();
        extra["headers"]["X-Extra"] = json!("private-fixture");
        let mut injected = answer();
        injected["headers"]["Authorization"] = json!("Bearer private\r\nInjected: value");
        for response in [
            json!(null),
            json!({"headers":{}}),
            wrong_profile,
            extra,
            injected,
        ] {
            let error = headers("site", "w", "a", "x7k2q", |_, _| Ok(response)).unwrap_err();
            assert_eq!(error.to_string(), FAILURE);
        }
    }
}
