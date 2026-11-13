//! Cloud calls of the account: the browser handoff exchange, revoke, `whoami`,
//! the workspace resolve, the workspace chooser and the former session file.

use agent_relay_node::relay::cloud_http;
use agent_relay_node::relay::cloud_session::{
    api, api_base, status_error, CloudError, REQUEST_TIMEOUT,
};
use agent_relay_node::relay::store::{CloudSession, CloudWorkspace};
use agent_relay_node::relay::workspace::{
    is_workspace_key, validated_base_url, Secret, DEFAULT_BASE_URL,
};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn string(value: &Value, key: &str) -> Option<String> {
    value[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Trades the browser sign-in's short-lived handoff token for a renewable
/// session. The handoff arrives on a loopback redirect, so Cloud keeps the
/// refresh token out of it; `code_verifier` proves this core opened that
/// sign-in (it matches the challenge on the login URL). Single use.
pub async fn exchange_handoff(
    handoff: &CloudSession,
    code_verifier: &Secret,
) -> Result<CloudSession, CloudError> {
    let site = validated_base_url(&handoff.site_url).ok_or(CloudError::Rejected)?;
    let answer = post_from_site(
        &site,
        format!("{}/auth/desktop/session", api_base(&site)),
        Some(handoff.access_token.expose().to_string()),
        json!({ "codeVerifier": code_verifier.expose() }),
        Duration::from_secs(10),
    )
    .await
    .map_err(|_| CloudError::Unreachable)?;
    if !answer.ok() {
        return Err(status_error(&answer));
    }
    let body = answer.inner();
    let (Some(access), Some(refresh)) = (string(body, "accessToken"), string(body, "refreshToken"))
    else {
        return Err(CloudError::Status {
            status: answer.status,
            message: Some("Agent Relay did not return a renewable session.".into()),
        });
    };
    Ok(CloudSession {
        access_token: Secret::new(access),
        access_token_expires_at: string(body, "accessTokenExpiresAt"),
        refresh_token: Some(Secret::new(refresh)),
        refresh_token_expires_at: string(body, "refreshTokenExpiresAt"),
        site_url: site,
    })
}

/// A JSON POST to `site` that names the site's public origin in `Origin`, as
/// the site asks of a native client. A site that does not check it ignores it.
pub(crate) async fn post_from_site(
    site: &str,
    url: String,
    bearer: Option<String>,
    body: Value,
    timeout: Duration,
) -> Result<cloud_http::Answer, cloud_http::Unreachable> {
    let origin = url::Url::parse(site)
        .map_err(|_| cloud_http::Unreachable)?
        .origin()
        .ascii_serialization();
    tokio::task::spawn_blocking(move || {
        let headers = [("Origin", origin.as_str())];
        cloud_http::send_blocking_with_headers(
            "POST",
            &url,
            bearer.as_deref(),
            &headers,
            Some(&body),
            timeout,
        )
        .map_err(|_| cloud_http::Unreachable)
    })
    .await
    .unwrap_or(Err(cloud_http::Unreachable))
}

/// Revokes a refresh token so it cannot be used again. Best effort.
pub async fn revoke(session: &CloudSession) {
    let (Some(refresh), Some(site)) = (
        session.refresh_token.as_ref(),
        validated_base_url(&session.site_url),
    ) else {
        return;
    };
    let _ = cloud_http::send(
        "POST",
        format!("{}/auth/token/revoke", api_base(&site)),
        None,
        Some(json!({ "token": refresh.expose() })),
        Duration::from_secs(10),
    )
    .await;
}

/// Whether `id` is an Agent Relay workspace id (a UUID), the only form the
/// resolve route takes.
pub fn is_workspace_uuid(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|g| g.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Why the account's workspace could not be resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// Not an Agent Relay workspace id.
    InvalidWorkspace,
    /// The workspace shares no Relaycast key with this account.
    NoKey,
    /// A build made for its own site: the site's resolve named no valid
    /// `relaycastUrl`, and such a build never falls back to the public
    /// Relaycast.
    NoRelaycastUrl,
    Cloud(CloudError),
}

impl std::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolveError::InvalidWorkspace => f.write_str("That is not an Agent Relay workspace."),
            ResolveError::NoKey => f.write_str(
                "Your Agent Relay workspace did not share an Agent Relay key with this account. Ask a workspace owner, or paste a key.",
            ),
            ResolveError::NoRelaycastUrl => f.write_str(
                "Your Agent Relay site did not name a valid Relaycast address for this workspace. This app is built for its own site and does not fall back to the public Agent Relay.",
            ),
            ResolveError::Cloud(error) => error.fmt(f),
        }
    }
}

/// The Relaycast workspace behind the account's Agent Relay workspace:
/// `GET /workspaces/{id}/resolve`. Only a workspace key and a validated
/// origin are accepted.
pub async fn resolve_workspace(
    site_url: &str,
    bearer: &Secret,
    workspace_id: &str,
) -> Result<CloudWorkspace, ResolveError> {
    if !is_workspace_uuid(workspace_id) {
        return Err(ResolveError::InvalidWorkspace);
    }
    let id = workspace_id.to_ascii_lowercase();
    let value = api(
        site_url,
        bearer,
        "GET",
        &format!("/workspaces/{id}/resolve"),
        None,
    )
    .await
    .map_err(ResolveError::Cloud)?;
    decode_resolution(&value, &id)
}

mod recovery;
pub use recovery::resolve_workspace_for_recovery;

/// Reads a resolve answer, bare or in a `data` envelope. A production build
/// takes the public Relaycast when the answer names no valid `relaycastUrl`;
/// a build made for its own site refuses it as
/// [`ResolveError::NoRelaycastUrl`].
pub fn decode_resolution(
    value: &Value,
    workspace_id: &str,
) -> Result<CloudWorkspace, ResolveError> {
    decode_resolution_with(
        agent_relay_node::relay::links::built_in_site_url(),
        value,
        workspace_id,
    )
}

/// [`decode_resolution`] for a build made for `built_in`.
pub fn decode_resolution_with(
    built_in: &str,
    value: &Value,
    workspace_id: &str,
) -> Result<CloudWorkspace, ResolveError> {
    let value = match value.get("data") {
        Some(data) if data.is_object() => data,
        _ => value,
    };
    let key = value["relaycastApiKey"]
        .as_str()
        .map(str::trim)
        .filter(|key| is_workspace_key(key))
        .ok_or(ResolveError::NoKey)?;
    let base_url = match value["urls"]["relaycastUrl"]
        .as_str()
        .and_then(validated_base_url)
    {
        Some(base_url) => base_url,
        None if crate::site_build::is_site_build_for(built_in) => {
            return Err(ResolveError::NoRelaycastUrl)
        }
        None => DEFAULT_BASE_URL.to_string(),
    };
    Ok(CloudWorkspace {
        key: Secret::new(key),
        base_url,
        workspace_id: string(value, "relaycastWorkspaceId")
            .or(string(value, "workspaceId"))
            .or(Some(workspace_id.to_string())),
        account_workspace_id: Some(workspace_id.to_string()),
        name: string(value, "name"),
        relayfile_workspace_id: string(value, "relayfileWorkspaceId"),
    })
}

/// Linux's former `cloud-session.json`, in the format `cloud_auth.py` wrote:
/// `{"apiUrl", "accessToken", "accessTokenExpiresAt", "refreshToken",
/// "refreshTokenExpiresAt"}`. Read once, to import it into the store.
pub struct SessionFile {
    pub path: PathBuf,
}

impl SessionFile {
    pub fn new(app_data_dir: &Path) -> SessionFile {
        SessionFile {
            path: app_data_dir.join("cloud-session.json"),
        }
    }

    pub fn load(&self) -> Option<CloudSession> {
        let value: Value = serde_json::from_slice(&fs::read(&self.path).ok()?).ok()?;
        let api = value["apiUrl"].as_str()?.trim().trim_end_matches('/');
        let site = validated_base_url(api.strip_suffix("/cloud")?)?;
        Some(CloudSession {
            access_token: Secret::new(string(&value, "accessToken")?),
            access_token_expires_at: string(&value, "accessTokenExpiresAt"),
            refresh_token: string(&value, "refreshToken").map(Secret::new),
            refresh_token_expires_at: string(&value, "refreshTokenExpiresAt"),
            site_url: site,
        })
    }

    pub fn remove(&self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Membership choices for the native welcome screen, before any setup runs.
pub async fn desktop_workspaces(
    site: &str,
    bearer: &Secret,
) -> Result<Vec<agent_relay_node::relay::state::WorkspaceChoice>, CloudError> {
    let value = api(site, bearer, "GET", "/auth/desktop/workspaces", None).await?;
    serde_json::from_value(value["workspaces"].clone()).map_err(|_| CloudError::Unreachable)
}

pub async fn select_desktop_workspace(
    site: &str,
    bearer: &Secret,
    workspace: &str,
) -> Result<CloudSession, CloudError> {
    let value = api(
        site,
        bearer,
        "POST",
        "/auth/desktop/workspaces",
        Some(json!({"workspaceId": workspace})),
    )
    .await?;
    Ok(CloudSession {
        access_token: Secret::new(string(&value, "accessToken").ok_or(CloudError::Unreachable)?),
        access_token_expires_at: string(&value, "accessTokenExpiresAt"),
        refresh_token: Some(Secret::new(
            string(&value, "refreshToken").ok_or(CloudError::Unreachable)?,
        )),
        refresh_token_expires_at: string(&value, "refreshTokenExpiresAt"),
        site_url: site.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resolution_takes_only_a_workspace_key_and_a_valid_origin() {
        let id = "0f8fad5b-d9cb-469f-a165-70867728950e";
        let resolved = decode_resolution(
            &json!({"data": {"relaycastApiKey": "rk_live_abcdefgh1234", "name": "Acme",
                "relaycastWorkspaceId": "rw_1", "relayfileWorkspaceId": "rf_1",
                "urls": {"relaycastUrl": "https://cast.agentrelay.com/"}}}),
            id,
        )
        .unwrap();
        assert_eq!(resolved.key.expose(), "rk_live_abcdefgh1234");
        assert_eq!(resolved.base_url, "https://cast.agentrelay.com");
        assert_eq!(resolved.workspace_id.as_deref(), Some("rw_1"));
        assert_eq!(resolved.account_workspace_id.as_deref(), Some(id));
        assert_eq!(resolved.relayfile_workspace_id.as_deref(), Some("rf_1"));
        assert_eq!(resolved.name.as_deref(), Some("Acme"));

        let untrusted = decode_resolution_with(
            agent_relay_node::relay::links::DEFAULT_SITE_URL,
            &json!({"relaycastApiKey": "rk_live_abcdefgh1234", "urls": {"relaycastUrl": "http://evil.example"}}),
            id,
        )
        .unwrap();
        assert_eq!(untrusted.base_url, DEFAULT_BASE_URL);
        assert_eq!(untrusted.workspace_id.as_deref(), Some(id));
        assert_eq!(
            decode_resolution(&json!({"relaycastApiKey": "at_live_agenttoken"}), id),
            Err(ResolveError::NoKey)
        );
        assert_eq!(decode_resolution(&json!({}), id), Err(ResolveError::NoKey));
    }

    const SITE: &str = "https://relay.example.test";

    #[test]
    fn a_production_build_takes_the_public_relaycast_when_the_resolve_names_none() {
        let production = agent_relay_node::relay::links::DEFAULT_SITE_URL;
        let id = "0f8fad5b-d9cb-469f-a165-70867728950e";
        for answer in [
            json!({"relaycastApiKey": "rk_live_abcdefgh1234"}),
            json!({"relaycastApiKey": "rk_live_abcdefgh1234", "urls": {"relaycastUrl": "http://evil.example"}}),
        ] {
            let found = decode_resolution_with(production, &answer, id).unwrap();
            assert_eq!(found.base_url, DEFAULT_BASE_URL);
        }
    }

    #[test]
    fn a_site_build_refuses_a_resolve_that_names_no_valid_relaycast() {
        let id = "0f8fad5b-d9cb-469f-a165-70867728950e";
        for answer in [
            json!({"relaycastApiKey": "rk_live_abcdefgh1234"}),
            json!({"relaycastApiKey": "rk_live_abcdefgh1234", "urls": {"relaycastUrl": ""}}),
            json!({"relaycastApiKey": "rk_live_abcdefgh1234", "urls": {"relaycastUrl": "http://evil.example"}}),
        ] {
            assert_eq!(
                decode_resolution_with(SITE, &answer, id),
                Err(ResolveError::NoRelaycastUrl)
            );
        }
        assert!(ResolveError::NoRelaycastUrl
            .to_string()
            .contains("did not name a valid Relaycast address"));
        // The site's own Relaycast is taken as it names it; a missing key is
        // still reported first.
        let found = decode_resolution_with(
            SITE,
            &json!({"relaycastApiKey": "rk_live_abcdefgh1234",
                    "urls": {"relaycastUrl": "https://cast.relay.example.test/"}}),
            id,
        )
        .unwrap();
        assert_eq!(found.base_url, "https://cast.relay.example.test");
        assert_eq!(
            decode_resolution_with(SITE, &json!({}), id),
            Err(ResolveError::NoKey)
        );
    }

    #[test]
    fn only_uuids_are_workspace_ids() {
        assert!(is_workspace_uuid("0f8fad5b-d9cb-469f-a165-70867728950e"));
        assert!(is_workspace_uuid("0F8FAD5B-D9CB-469F-A165-70867728950E"));
        assert!(!is_workspace_uuid("acme"));
        assert!(!is_workspace_uuid("../../members"));
        assert!(!is_workspace_uuid("0f8fad5b-d9cb-469f-a165-70867728950"));
    }

    /// Every build takes the key a self-hosted Relaycast issues.
    #[test]
    fn a_resolved_armadillo_key_is_a_workspace_key_in_every_build() {
        for built_in in [agent_relay_node::relay::links::DEFAULT_SITE_URL, SITE] {
            let found = decode_resolution_with(
                built_in,
                &json!({"relaycastApiKey": "ar_ws_0123456789abcdef",
                        "urls": {"relaycastUrl": "https://cast.relay.example.test"}}),
                "workspace",
            )
            .unwrap();
            assert_eq!(found.key.expose(), "ar_ws_0123456789abcdef");
        }
    }

    #[test]
    fn a_resolved_armadillo_key_keeps_the_origin_cloud_names() {
        let found = decode_resolution(
            &json!({"relaycastApiKey": "ar_ws_0123456789abcdef",
                    "urls": {"relaycastUrl": "https://armadillo.agentrelay.com"}}),
            "workspace",
        )
        .unwrap();
        assert_eq!(found.base_url, "https://armadillo.agentrelay.com");
    }

    #[test]
    fn the_former_linux_session_file_is_read_in_cloud_auth_s_format() {
        let dir = tempfile::tempdir().unwrap();
        let file = SessionFile::new(dir.path());
        assert!(file.load().is_none());
        fs::write(
            &file.path,
            json!({"apiUrl": "https://agentrelay.com/cloud/", "accessToken": "access",
                   "accessTokenExpiresAt": "2026-09-30T11:00:00Z", "refreshToken": "refresh"})
            .to_string(),
        )
        .unwrap();
        let kept = file.load().unwrap();
        assert_eq!(kept.site_url, "https://agentrelay.com");
        assert_eq!(kept.refresh_token.unwrap().expose(), "refresh");
        file.remove();
        assert!(!file.path.exists());
    }

    /// Serves one JSON answer on loopback and hands back the request head it
    /// read. The body is read too before answering: closing a socket with
    /// unread bytes resets the connection, which macOS reports to the client
    /// before the answer.
    fn one_answer(body: Value) -> (String, std::thread::JoinHandle<String>) {
        use crate::history::cloud::tests::{read_http_request, write_http_response};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let site = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let (line, headers, _) = read_http_request(&mut stream);
            write_http_response(&mut stream, "200 OK", &body.to_string());
            let origin = headers.get("origin").cloned().unwrap_or_default();
            format!("{}\norigin: {origin}", line.trim_end()).to_lowercase()
        });
        (site, server)
    }

    /// The handoff exchange is a JSON POST to the site, so it names the
    /// site's origin; the answer is read as before.
    #[tokio::test]
    async fn the_handoff_exchange_names_the_site_origin() {
        let (site, server) = one_answer(
            json!({"accessToken": "synthetic-access", "refreshToken": "synthetic-refresh"}),
        );
        let handoff = CloudSession {
            access_token: Secret::new("synthetic-handoff"),
            access_token_expires_at: None,
            refresh_token: None,
            refresh_token_expires_at: None,
            site_url: site.clone(),
        };
        let session = exchange_handoff(&handoff, &Secret::new("synthetic-verifier"))
            .await
            .unwrap();
        let request = server.join().unwrap();
        assert!(request.starts_with("post /cloud/api/v1/auth/desktop/session "));
        assert!(request.ends_with(&format!("\norigin: {site}")), "{request}");
        assert_eq!(session.access_token.expose(), "synthetic-access");
        assert_eq!(session.refresh_token.unwrap().expose(), "synthetic-refresh");
    }
}
