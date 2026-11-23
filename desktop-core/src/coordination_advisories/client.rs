//! The read API: listing a session's advisories and acking one.

use super::*;

// Upstream.

/// Whom a read token and the advisories it lists belong to: the site,
/// workspace and person signed in, and the account's sign-in generation
/// ([`CloudAccess::identity_sync_generation`]), which a new sign-in changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Principal {
    pub site: String,
    pub workspace_id: String,
    pub user_id: Option<String>,
    pub generation: u64,
}

impl Principal {
    /// Who is signed in to `cloud` now, or `None` when nobody is.
    pub fn of(cloud: &dyn CloudAccess) -> Option<Self> {
        let account = cloud.account()?;
        Some(Self {
            site: account.site_url.trim_end_matches('/').to_string(),
            workspace_id: account.workspace_id,
            user_id: account.user_id,
            generation: cloud.identity_sync_generation(),
        })
    }
}

/// A read-API origin and the bearer it takes, issued to `principal`.
#[derive(Clone, Debug)]
pub struct ReadToken {
    pub origin: String,
    pub bearer: Secret,
    pub workspace_id: String,
    pub principal: Principal,
}

/// Where the poller gets its read-API token; swapped in tests.
pub trait TokenSource: Send + Sync + 'static {
    /// A usable token, or `None` (signed out, an edition with no client
    /// read token, or a failed exchange, which waits before the next).
    fn token(&self) -> BoxFuture<'_, Option<ReadToken>>;
    /// Who is signed in now, or `None`; cached advisories and tokens belong
    /// to one principal and are dropped when it changes.
    fn principal(&self) -> Option<Principal>;
    /// The read API refused `token` (401): it is dropped, and the next
    /// [`TokenSource::token`] asks for a new one at once, once per token
    /// that was not itself such a replacement.
    fn refused(&self, token: &ReadToken);
}

/// What one listing gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing {
    /// The rows, and the contract they were listed under.
    Advisories(Vec<Advisory>, ListingContract),
    /// 401: the token was refused.
    Unauthorized,
    /// 404, 501 or 403: the route is not there, switched off, or not for
    /// this token. Every session waits.
    Off,
    /// 429: pause this device's advisory traffic before retrying.
    Throttled,
    /// The network, a bad body, or any other status. This session waits.
    Failed,
}

/// What one ack gave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ack {
    /// 2xx; for a legacy row's v1 ack also 404: not (or no longer) this
    /// session's, which is done too.
    Done,
    /// 404 to an instance row's v2 ack: no such instance (a stale
    /// incarnation, or gone). Final: settled for that instance, never retried
    /// or shown again, and never counted as delivered.
    NoMatch,
    /// 401.
    Unauthorized,
    /// 403: the token may not ack; retried at most hourly.
    Forbidden,
    Throttled,
    /// Anything else.
    Failed,
}

/// The read API; faked in tests.
pub trait AdvisoryApi: Send + Sync + 'static {
    fn list<'a>(&'a self, token: &'a ReadToken, target: &'a Target) -> BoxFuture<'a, Listing>;
    /// Acks advisory `id`: with `incarnation` the v2 ack of that instance
    /// only, without it the v1 ack of a legacy row. An instance row is never
    /// acked without its incarnation.
    fn ack<'a>(
        &'a self,
        token: &'a ReadToken,
        target: &'a Target,
        id: &'a str,
        incarnation: Option<&'a str>,
    ) -> BoxFuture<'a, Ack>;
}

/// Shared by listing and acknowledgement traffic for one Desktop. Queued
/// calls recheck their principal after waiting, before any network I/O.
pub(super) struct PacedApi {
    inner: Arc<dyn AdvisoryApi>,
    tokens: Arc<dyn TokenSource>,
    next: tokio::sync::Mutex<tokio::time::Instant>,
}

impl PacedApi {
    pub(super) fn new(inner: Arc<dyn AdvisoryApi>, tokens: Arc<dyn TokenSource>) -> Self {
        Self {
            inner,
            tokens,
            next: tokio::sync::Mutex::new(tokio::time::Instant::now()),
        }
    }
    async fn admit(&self, token: &ReadToken) -> bool {
        let at = {
            let mut next = self.next.lock().await;
            let at = (*next).max(tokio::time::Instant::now());
            *next = at + Duration::from_millis(500);
            at
        };
        tokio::time::sleep_until(at).await;
        self.tokens.principal().as_ref() == Some(&token.principal)
    }
}

impl AdvisoryApi for PacedApi {
    fn list<'a>(&'a self, token: &'a ReadToken, target: &'a Target) -> BoxFuture<'a, Listing> {
        Box::pin(async move {
            if self.admit(token).await {
                self.inner.list(token, target).await
            } else {
                Listing::Off
            }
        })
    }
    fn ack<'a>(
        &'a self,
        token: &'a ReadToken,
        target: &'a Target,
        id: &'a str,
        incarnation: Option<&'a str>,
    ) -> BoxFuture<'a, Ack> {
        Box::pin(async move {
            if self.admit(token).await {
                self.inner.ack(token, target, id, incarnation).await
            } else {
                Ack::Unauthorized
            }
        })
    }
}

/// The pipeline's harness name for `kind`: the history source the core
/// names it by (`claude`, `codex`, `copilot`, `devin`, `cursor`).
pub fn source(kind: SessionKind) -> &'static str {
    kind.source()
}

/// The read API over HTTPS: redirect-free and bounded (`cloud_http`).
pub struct HttpApi;

impl HttpApi {
    pub(super) fn list_url(token: &ReadToken, target: &Target) -> Option<String> {
        let mut url = url::Url::parse(&format!(
            "{}/v1/coordination/advisories",
            token.origin.trim_end_matches('/')
        ))
        .ok()?;
        url.query_pairs_mut()
            .append_pair("workspace", &token.workspace_id)
            .append_pair("source", source(target.kind))
            .append_pair("sessionId", &target.id)
            .append_pair("status", "pending");
        Some(url.into())
    }

    pub(super) fn ack_url(token: &ReadToken, id: &str) -> Option<String> {
        let mut url = url::Url::parse(&format!(
            "{}/v1/coordination/advisories/{}/ack",
            token.origin.trim_end_matches('/'),
            segment(id)
        ))
        .ok()?;
        url.query_pairs_mut()
            .append_pair("workspace", &token.workspace_id);
        Some(url.into())
    }
}

impl AdvisoryApi for HttpApi {
    fn list<'a>(&'a self, token: &'a ReadToken, target: &'a Target) -> BoxFuture<'a, Listing> {
        Box::pin(async move {
            let Some(url) = Self::list_url(token, target) else {
                return Listing::Failed;
            };
            let bearer = token.bearer.expose().to_string();
            match agent_relay_node::relay::cloud_http::send(
                "GET",
                url,
                Some(bearer),
                None,
                REQUEST_TIMEOUT,
            )
            .await
            {
                Ok(answer) if answer.status == 200 => match parse_listing(&answer.body) {
                    Some((advisories, contract)) => Listing::Advisories(advisories, contract),
                    None => {
                        debug(format_args!("the listing was not one of a known contract"));
                        Listing::Failed
                    }
                },
                Ok(answer) => {
                    debug(format_args!("the listing answered {}", answer.status));
                    match answer.status {
                        401 => Listing::Unauthorized,
                        429 => Listing::Throttled,
                        403 | 404 | 501 => Listing::Off,
                        _ => Listing::Failed,
                    }
                }
                Err(_) => {
                    debug(format_args!("the listing got no answer"));
                    Listing::Failed
                }
            }
        })
    }

    fn ack<'a>(
        &'a self,
        token: &'a ReadToken,
        target: &'a Target,
        id: &'a str,
        incarnation: Option<&'a str>,
    ) -> BoxFuture<'a, Ack> {
        Box::pin(async move {
            let Some(url) = Self::ack_url(token, id) else {
                return Ack::Failed;
            };
            let mut body = json!({"source": source(target.kind), "sessionId": target.id});
            if let Some(incarnation) = incarnation {
                body["incarnation"] = json!(incarnation);
            }
            let bearer = token.bearer.expose().to_string();
            match agent_relay_node::relay::cloud_http::send(
                "POST",
                url,
                Some(bearer),
                Some(body),
                REQUEST_TIMEOUT,
            )
            .await
            {
                Ok(answer) if answer.ok() => Ack::Done,
                Ok(answer) => {
                    debug(format_args!("an ack answered {}", answer.status));
                    match answer.status {
                        404 if incarnation.is_some() => Ack::NoMatch,
                        404 => Ack::Done,
                        401 => Ack::Unauthorized,
                        429 => Ack::Throttled,
                        403 => Ack::Forbidden,
                        _ => Ack::Failed,
                    }
                }
                Err(_) => Ack::Failed,
            }
        })
    }
}
