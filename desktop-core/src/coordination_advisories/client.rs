//! The read API: listing a session's advisories and acking one.

use super::*;

// Upstream.

/// A read-API origin and the bearer it takes.
#[derive(Clone, Debug)]
pub struct ReadToken {
    pub origin: String,
    pub bearer: Secret,
    pub workspace_id: String,
}

/// Where the poller gets its read-API token; swapped in tests.
pub trait TokenSource: Send + Sync + 'static {
    /// A usable token, or `None` (signed out, an edition with no client
    /// read token, or a failed exchange, which waits before the next).
    fn token(&self) -> BoxFuture<'_, Option<ReadToken>>;
    /// The read API refused `token` (401): it is dropped, and the next
    /// [`TokenSource::token`] asks for a new one.
    fn refused(&self, token: &ReadToken);
}

/// What one listing gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Listing {
    Advisories(Vec<Advisory>),
    /// 401: the token was refused.
    Unauthorized,
    /// 404, 501 or 403: the route is not there, switched off, or not for
    /// this token. Every session waits.
    Off,
    /// The network, a bad body, or any other status. This session waits.
    Failed,
}

/// What one ack gave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ack {
    /// 2xx, or 404: not (or no longer) this session's, which is done too.
    Done,
    /// 401.
    Unauthorized,
    /// 403: the token may not ack; retried at most hourly.
    Forbidden,
    /// Anything else.
    Failed,
}

/// The read API; faked in tests.
pub trait AdvisoryApi: Send + Sync + 'static {
    fn list<'a>(&'a self, token: &'a ReadToken, target: &'a Target) -> BoxFuture<'a, Listing>;
    fn ack<'a>(
        &'a self,
        token: &'a ReadToken,
        target: &'a Target,
        id: &'a str,
    ) -> BoxFuture<'a, Ack>;
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
                Ok(answer) if answer.status == 200 => match parse(&answer.body) {
                    Some(advisories) => Listing::Advisories(advisories),
                    None => {
                        debug(format_args!("the listing was not {CONTRACT}"));
                        Listing::Failed
                    }
                },
                Ok(answer) => {
                    debug(format_args!("the listing answered {}", answer.status));
                    match answer.status {
                        401 => Listing::Unauthorized,
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
    ) -> BoxFuture<'a, Ack> {
        Box::pin(async move {
            let url = format!(
                "{}/v1/coordination/advisories/{}/ack",
                token.origin.trim_end_matches('/'),
                segment(id)
            );
            let body = json!({"source": source(target.kind), "sessionId": target.id});
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
                Ok(answer) if answer.ok() || answer.status == 404 => Ack::Done,
                Ok(answer) => {
                    debug(format_args!("an ack answered {}", answer.status));
                    match answer.status {
                        401 => Ack::Unauthorized,
                        403 => Ack::Forbidden,
                        _ => Ack::Failed,
                    }
                }
                Err(_) => Ack::Failed,
            }
        })
    }
}
