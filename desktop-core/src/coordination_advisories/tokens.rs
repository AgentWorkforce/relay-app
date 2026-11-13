//! The read token, from the account's own Cloud sign-in.

use super::*;

/// Whether `host` is one of hosted Cloud's own RelayHistory stages, whose
/// tokens the hosted read API does not take.
pub(super) fn hosted_history_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "history.agentrelay.com" || host.ends_with(".history.agentrelay.com")
}

/// Whether `site` is hosted Cloud, which has no client-facing read token
/// yet: the site a production build signs in to. A self-hosted control
/// plane (relay-control) is any other, as `desktop_identity` tells them.
pub(super) fn hosted_site(site: &str) -> bool {
    site.trim_end_matches('/')
        == agent_relay_node::relay::links::DEFAULT_SITE_URL.trim_end_matches('/')
}

#[derive(Default)]
pub(super) struct TokenCache {
    pub(super) token: Option<(ReadToken, Instant)>,
    /// No exchange before this.
    pub(super) next_exchange: Option<Instant>,
    /// The hosted gap was logged.
    pub(super) told_hosted: bool,
}

/// The read token from the workspace's RelayHistory session exchange,
/// `POST {site}/cloud/api/v1/workspaces/:id/relayhistory/session` with
/// `{"mode":"read"}`, made with the Desktop's own Cloud bearer (the account's
/// [`CloudAccess`]). It answers `{baseUrl, accessToken, accessTokenExpiresAt,
/// workspaceId, scopes:["rth:read"]}`: `baseUrl` is the read API's origin.
///
/// Only a self-hosted control plane gives one today. Against hosted Cloud
/// this answers `None` without asking: hosted Cloud's read API does not take
/// the tokens that exchange issues, and nothing else issues one to a client.
pub struct HistoryReadTokens {
    pub(super) cloud: Weak<dyn CloudAccess>,
    /// A development build's origin ([`ORIGIN_VARIABLE`]).
    pub(super) origin_override: Option<String>,
    pub(super) cache: Mutex<TokenCache>,
}

impl HistoryReadTokens {
    pub fn new(cloud: Weak<dyn CloudAccess>, origin_override: Option<String>) -> Self {
        Self {
            cloud,
            origin_override: origin_override
                .map(|origin| origin.trim().trim_end_matches('/').to_string())
                .filter(|origin| agent_relay_node::relay::cloud_http::sendable(origin)),
            cache: Mutex::new(TokenCache::default()),
        }
    }

    pub(super) fn cache(&self) -> std::sync::MutexGuard<'_, TokenCache> {
        self.cache.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The token in an exchange's answer, or `None` when it is not one this
    /// edition's read API takes.
    pub(super) fn read_token(
        &self,
        answer: &Value,
        site: &str,
        workspace: &str,
    ) -> Option<(ReadToken, Instant)> {
        if answer["exchange"] == "client" {
            return None;
        }
        let access = answer["accessToken"].as_str().filter(|t| !t.is_empty())?;
        if answer["workspaceId"].as_str() != Some(workspace) {
            return None;
        }
        if let Some(scopes) = answer["scopes"].as_array() {
            if !scopes.is_empty() && !scopes.iter().any(|s| s == "rth:read") {
                return None;
            }
        }
        let origin = match &self.origin_override {
            Some(origin) => origin.clone(),
            None => {
                let origin = crate::history::cloud::workspace_history_origin(
                    answer["baseUrl"].as_str()?,
                    site,
                )
                .ok()?;
                let host = url::Url::parse(&origin).ok()?.host_str()?.to_string();
                if hosted_history_host(&host) {
                    return None;
                }
                origin
            }
        };
        let now = Instant::now();
        let lifetime = answer["accessTokenExpiresAt"]
            .as_str()
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
            .map(|at| at.with_timezone(&chrono::Utc) - chrono::Utc::now())
            .and_then(|left| left.to_std().ok())
            .map_or(Duration::from_secs(10 * 60), |left| {
                left.saturating_sub(TOKEN_MARGIN).min(TOKEN_MAX_AGE)
            });
        if lifetime.is_zero() {
            return None;
        }
        Some((
            ReadToken {
                origin,
                bearer: Secret::new(access),
                workspace_id: workspace.to_string(),
            },
            now + lifetime,
        ))
    }
}

impl TokenSource for HistoryReadTokens {
    fn token(&self) -> BoxFuture<'_, Option<ReadToken>> {
        Box::pin(async move {
            let cloud = self.cloud.upgrade()?;
            let account = cloud.account()?;
            let now = Instant::now();
            {
                let mut cache = self.cache();
                if let Some((token, until)) = &cache.token {
                    if token.workspace_id == account.workspace_id && now < *until {
                        return Some(token.clone());
                    }
                    cache.token = None;
                }
                if cache.next_exchange.is_some_and(|at| now < at) {
                    return None;
                }
                if self.origin_override.is_none() && hosted_site(&account.site_url) {
                    if !cache.told_hosted {
                        cache.told_hosted = true;
                        debug(format_args!(
                            "hosted Cloud gives clients no read-API token; nothing is fetched"
                        ));
                    }
                    cache.next_exchange = Some(now + ROUTE_OFF_BACKOFF);
                    return None;
                }
                // Claimed before the request, so two callers never both ask.
                cache.next_exchange = Some(now + TOKEN_MIN_INTERVAL);
            }
            let path = format!(
                "/workspaces/{}/relayhistory/session",
                segment(&account.workspace_id)
            );
            let answer = cloud
                .request("POST", path, Some(json!({"mode": "read", "label": LABEL})))
                .await;
            let issued = match &answer {
                Ok(answer) => self.read_token(answer, &account.site_url, &account.workspace_id),
                Err(CloudError::Status { status, message }) => {
                    debug(format_args!(
                        "the read-token exchange answered {status}{}",
                        message
                            .as_deref()
                            .filter(|m| *m == "handoff_not_exchanged")
                            .map_or(String::new(), |m| format!(" ({m})"))
                    ));
                    None
                }
                Err(_) => {
                    debug(format_args!("the read-token exchange failed"));
                    None
                }
            };
            let mut cache = self.cache();
            match issued {
                Some((token, until)) => {
                    cache.token = Some((token.clone(), until));
                    Some(token)
                }
                None => {
                    if answer.is_ok() {
                        debug(format_args!("the read-token exchange gave no usable token"));
                    }
                    cache.next_exchange = Some(Instant::now() + TOKEN_RETRY);
                    None
                }
            }
        })
    }

    fn refused(&self, token: &ReadToken) {
        let mut cache = self.cache();
        if cache
            .token
            .as_ref()
            .is_some_and(|(held, _)| held.bearer.expose() == token.bearer.expose())
        {
            cache.token = None;
        }
    }
}
