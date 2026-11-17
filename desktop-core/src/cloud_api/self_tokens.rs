//! The desktop's own relay-node token on a gate-only install:
//! `POST|DELETE <site>/cloud/api/v1/workspaces/{id}/relay-node/self-tokens`,
//! with the Desktop session's bearer. Control mints a node-scoped `sh_wk_`
//! bound to one node name and shows its value once.
//!
//! The mint is sent here rather than through `cloud_http`: its 429 names its
//! wait in `Retry-After`, which `cloud_http` does not read, and its answer
//! carries the token, which is never written to the request log.

use agent_relay_node::relay::cloud_http;
use agent_relay_node::relay::cloud_session::api_base;
use agent_relay_node::relay::websocket_tls::{client_config, platform_root_environment_is_safe};
use agent_relay_node::relay::workspace::{validated_base_url, Secret, NODE_KEY_PREFIX};
use serde_json::{json, Value};
use std::io::Read;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(15);
/// A mint answer is a few hundred bytes; anything far larger is not one.
const MAX_BODY: u64 = 64 * 1024;
/// A 429 without a readable `Retry-After` waits this long.
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(30);

/// A relay-node token Control minted for this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelfToken {
    /// Control's id for the token, which its `DELETE` takes.
    pub id: String,
    pub node_name: String,
    pub expires_at: Option<String>,
    /// The `sh_wk_` value, shown once.
    pub token: Secret,
}

/// Why Control minted no token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MintError {
    /// Control could not be reached or answered with something unreadable,
    /// or failed (5xx): try again later.
    Unreachable,
    /// The session was refused (401): try again after it is renewed.
    Unauthorized,
    /// 429 `rate_limited`: try again after this long.
    RateLimited(Duration),
    /// An answer that will not change until something else does: Control's
    /// error code (`self_mint_disabled`, `node_name_taken`, ...) or, for
    /// one it did not name, the HTTP status.
    Refused { status: u16, code: String },
}

impl MintError {
    /// Whether trying again later may succeed without anyone acting.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            MintError::Unreachable | MintError::Unauthorized | MintError::RateLimited(_)
        )
    }

    /// The sentence the person sees for a refusal: spawning is unavailable
    /// on this computer, and why, ending with Control's code.
    pub fn unavailable_reason(&self) -> String {
        let why = match self {
            MintError::Refused { code, .. } => match code.as_str() {
                "self_mint_disabled" => {
                    "Agent Relay's Control does not allow this computer to mint its own relay-node \
                     token"
                }
                "node_name_taken" => {
                    "another person already holds the relay-node token for this computer's node \
                     name; ask them or an administrator to revoke it"
                }
                "too_many_tokens" => {
                    "you already hold the most relay-node tokens Agent Relay's Control allows; \
                     sign out on another computer, or revoke one of your tokens"
                }
                "desktop_session_required" => {
                    "Agent Relay's Control mints relay-node tokens only for a desktop sign-in; \
                     sign out and sign in again from this app"
                }
                "not_found" => {
                    "Agent Relay's Control does not offer relay-node tokens for this workspace"
                }
                "invalid_request" => "Agent Relay's Control refused this computer's node name",
                _ => "Agent Relay's Control refused to mint this computer's relay-node token",
            },
            MintError::Unreachable => "Agent Relay's Control could not be reached",
            MintError::Unauthorized => "Agent Relay needs you to sign in again",
            MintError::RateLimited(_) => {
                "Agent Relay's Control is limiting how often relay-node tokens are minted"
            }
        };
        let code = match self {
            MintError::Refused { code, .. } => code.clone(),
            MintError::Unreachable => "unreachable".into(),
            MintError::Unauthorized => "unauthorized".into(),
            MintError::RateLimited(_) => "rate_limited".into(),
        };
        format!("Spawning is unavailable on this computer: {why} ({code}).")
    }
}

impl std::fmt::Display for MintError {
    /// The person's sentence: [`MintError::unavailable_reason`].
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.unavailable_reason())
    }
}

impl std::error::Error for MintError {}

fn self_tokens_url(site: &str, workspace_id: &str) -> Option<String> {
    let site = validated_base_url(site)?;
    super::is_workspace_uuid(workspace_id).then(|| {
        format!(
            "{}/workspaces/{}/relay-node/self-tokens",
            api_base(&site),
            workspace_id.to_ascii_lowercase()
        )
    })
}

/// `Retry-After` in seconds, clamped to one second to an hour. A date, or
/// nothing readable, waits [`DEFAULT_RETRY_AFTER`].
pub fn retry_after(header: Option<&str>) -> Duration {
    header
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|seconds| Duration::from_secs(seconds.clamp(1, 3600)))
        .unwrap_or(DEFAULT_RETRY_AFTER)
}

/// Control's error code: `{"error": {"code"}}`, `{"code"}` or `{"error": "<code>"}`.
fn error_code(body: &Value) -> Option<String> {
    body["error"]["code"]
        .as_str()
        .or(body["code"].as_str())
        .or(body["error"].as_str())
        .map(str::trim)
        .filter(|code| {
            !code.is_empty()
                && code.len() <= 64
                && code.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
        })
        .map(str::to_string)
}

/// Reads a mint's answer.
pub fn decode_mint(status: u16, body: &Value, retry: Option<&str>) -> Result<SelfToken, MintError> {
    match status {
        201 | 200 => {}
        401 => return Err(MintError::Unauthorized),
        429 => return Err(MintError::RateLimited(retry_after(retry))),
        500.. => return Err(MintError::Unreachable),
        _ => {
            return Err(MintError::Refused {
                status,
                code: error_code(body).unwrap_or_else(|| match status {
                    404 => "not_found".into(),
                    _ => format!("http_{status}"),
                }),
            })
        }
    }
    let body = match body.get("data") {
        Some(data) if data.is_object() => data,
        _ => body,
    };
    let text = |key: &str| {
        body[key]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let token = text("accessToken").filter(|token| token.starts_with(NODE_KEY_PREFIX));
    match (text("id"), token) {
        (Some(id), Some(token)) => Ok(SelfToken {
            id,
            node_name: text("nodeName").unwrap_or_default(),
            expires_at: text("expiresAt").or(text("accessTokenExpiresAt")),
            token: Secret::new(token),
        }),
        _ => Err(MintError::Unreachable),
    }
}

/// Mints this computer's relay-node token for `node_name` in `workspace_id`.
pub async fn mint(
    site: &str,
    bearer: &Secret,
    workspace_id: &str,
    node_name: &str,
) -> Result<SelfToken, MintError> {
    let url = self_tokens_url(site, workspace_id).ok_or(MintError::Refused {
        status: 0,
        code: "invalid_request".into(),
    })?;
    let bearer = bearer.expose().to_string();
    let body = json!({ "nodeName": node_name });
    tokio::task::spawn_blocking(move || post_blocking(&url, &bearer, &body))
        .await
        .unwrap_or(Err(MintError::Unreachable))
}

fn post_blocking(url: &str, bearer: &str, body: &Value) -> Result<SelfToken, MintError> {
    if !cloud_http::sendable(url) || !platform_root_environment_is_safe() {
        return Err(MintError::Unreachable);
    }
    let agent = ureq::AgentBuilder::new()
        .tls_config(client_config())
        .redirects(0)
        .timeout(TIMEOUT)
        .build();
    let response = match agent
        .post(url)
        .set("Accept", "application/json")
        .set("Authorization", &format!("Bearer {bearer}"))
        .send_json(body)
    {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(ureq::Error::Transport(_)) => return Err(MintError::Unreachable),
    };
    let status = response.status();
    let retry = response.header("Retry-After").map(str::to_string);
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_BODY)
        .read_to_end(&mut bytes)
        .map_err(|_| MintError::Unreachable)?;
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    decode_mint(status, &body, retry.as_deref())
}

/// Revokes this computer's token `token_id`. Whether Control answered that
/// it is gone: deleted (2xx) or not found (404). A failure is otherwise in
/// the request log only.
pub async fn delete(site: &str, bearer: &Secret, workspace_id: &str, token_id: &str) -> bool {
    let Some(url) = self_tokens_url(site, workspace_id) else {
        return false;
    };
    let id: String = url::form_urlencoded::byte_serialize(token_id.as_bytes()).collect();
    cloud_http::send(
        "DELETE",
        format!("{url}/{id}"),
        Some(bearer.expose().to_string()),
        None,
        TIMEOUT,
    )
    .await
    .is_ok_and(|answer| answer.ok() || answer.status == 404)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mint_error_displays_the_persons_sentence() {
        let error = MintError::Refused {
            status: 403,
            code: "self_mint_disabled".into(),
        };
        assert_eq!(error.to_string(), error.unavailable_reason());
        let error: &dyn std::error::Error = &MintError::Unreachable;
        assert!(error.to_string().ends_with("(unreachable)."));
    }

    #[test]
    fn a_minted_token_is_read_from_the_answer() {
        let answer = json!({"id": "tok_1", "label": "desktop Desk", "purpose": "relay-node",
            "nodeName": "desk-desktop", "scopes": ["relay-node"], "expiresAt": "2027-02-14T00:00:00Z",
            "accessToken": "sh_wk_fixture1234"});
        let token = decode_mint(201, &answer, None).unwrap();
        assert_eq!(token.id, "tok_1");
        assert_eq!(token.node_name, "desk-desktop");
        assert_eq!(token.expires_at.as_deref(), Some("2027-02-14T00:00:00Z"));
        assert_eq!(token.token.expose(), "sh_wk_fixture1234");
        // Anything but a node-scoped key is not taken.
        let wrong = json!({"id": "tok_1", "accessToken": "rk_live_abcdefgh1234"});
        assert_eq!(decode_mint(201, &wrong, None), Err(MintError::Unreachable));
    }

    #[test]
    fn each_refusal_names_why_spawning_is_unavailable() {
        for (status, code, words) in [
            (
                403,
                "self_mint_disabled",
                "does not allow this computer to mint its own relay-node token",
            ),
            (409, "node_name_taken", "another person already holds"),
            (409, "too_many_tokens", "the most relay-node tokens"),
            (
                403,
                "desktop_session_required",
                "only for a desktop sign-in",
            ),
            (404, "not_found", "does not offer relay-node tokens"),
            (400, "invalid_request", "refused this computer's node name"),
        ] {
            for body in [
                json!({"error": {"code": code, "message": "x"}}),
                json!({"error": code}),
                json!({"code": code}),
            ] {
                let error = decode_mint(status, &body, None).unwrap_err();
                assert!(!error.is_transient(), "{code}");
                let reason = error.unavailable_reason();
                assert!(
                    reason.starts_with("Spawning is unavailable on this computer: "),
                    "{reason}"
                );
                assert!(reason.contains(words), "{reason}");
                assert!(reason.ends_with(&format!("({code}).")), "{reason}");
            }
        }
        assert_eq!(
            decode_mint(403, &json!({"error": {"code": "self_mint_disabled"}}), None)
                .unwrap_err()
                .unavailable_reason(),
            "Spawning is unavailable on this computer: Agent Relay's Control does not allow this \
             computer to mint its own relay-node token (self_mint_disabled)."
        );
        // A 404 from a Control without the route names no code.
        assert_eq!(
            decode_mint(404, &Value::Null, None),
            Err(MintError::Refused {
                status: 404,
                code: "not_found".into()
            })
        );
        assert_eq!(
            decode_mint(418, &Value::Null, None),
            Err(MintError::Refused {
                status: 418,
                code: "http_418".into()
            })
        );
    }

    #[test]
    fn transient_answers_are_tried_again() {
        for (status, retry) in [(401, None), (503, None), (500, None), (429, Some("120"))] {
            let error = decode_mint(status, &Value::Null, retry).unwrap_err();
            assert!(error.is_transient(), "{status}");
        }
        assert_eq!(
            decode_mint(429, &Value::Null, Some("120")),
            Err(MintError::RateLimited(Duration::from_secs(120)))
        );
        assert_eq!(
            decode_mint(401, &Value::Null, None),
            Err(MintError::Unauthorized)
        );
        assert_eq!(
            decode_mint(503, &Value::Null, None),
            Err(MintError::Unreachable)
        );
        assert_eq!(
            decode_mint(429, &json!({"error": "rate_limited"}), Some("7")),
            Err(MintError::RateLimited(Duration::from_secs(7)))
        );
        assert_eq!(retry_after(Some("0")), Duration::from_secs(1));
        assert_eq!(retry_after(Some("999999")), Duration::from_secs(3600));
        assert_eq!(
            retry_after(Some("Wed, 21 Oct 2026 07:28:00 GMT")),
            DEFAULT_RETRY_AFTER
        );
        assert_eq!(retry_after(None), DEFAULT_RETRY_AFTER);
    }

    #[test]
    fn the_route_takes_only_a_workspace_uuid_and_a_valid_site() {
        let id = "0F8FAD5B-D9CB-469F-A165-70867728950E";
        assert_eq!(
            self_tokens_url("https://relay.example.test/", id).as_deref(),
            Some(
                "https://relay.example.test/cloud/api/v1/workspaces/\
                 0f8fad5b-d9cb-469f-a165-70867728950e/relay-node/self-tokens"
            )
        );
        assert_eq!(self_tokens_url("https://relay.example.test", "../x"), None);
        assert_eq!(self_tokens_url("ftp://relay.example.test", id), None);
    }
}
