//! The storage hosts a site build's direct uploads may PUT to on a private
//! address: an organization's own store (MinIO, say) on its private DNS,
//! which the History address rule would refuse. `build.rs` compiles them in
//! from `AGENT_RELAY_HISTORY_UPLOAD_STORAGE_ORIGINS` (`storage_origins`),
//! site builds only; a hosted build has none and puts as before.
//!
//! The exemption is the storage PUT's alone: a PUT whose URL names a listed
//! origin exactly (https, host and port) goes through the storage agents
//! here, whose resolver lets that host and port reach a private address.
//! Nothing that carries a History session or Cloud's assertion uses them,
//! and a listed origin is never a trusted History destination. TLS
//! verification and the refusal of redirects are as for every History agent.
use super::super::MAX_IN_FLIGHT;
use crate::history::cloud;
use std::{collections::BTreeSet, sync::LazyLock};
use url::Url;

/// A listed origin: its host, lowercased, and its effective port.
type Origin = (String, u16);

/// The origins this build lists, from what `build.rs` checked.
static LISTED: LazyLock<BTreeSet<Origin>> =
    LazyLock::new(|| origins_from_json(env!("DESKTOP_HISTORY_UPLOAD_STORAGE_ORIGINS_JSON")));

/// The canonical JSON array `build.rs` hands on, read; nothing for anything
/// else, though the build never hands on anything else.
fn origins_from_json(json: &str) -> BTreeSet<Origin> {
    serde_json::from_str::<Vec<String>>(json)
        .unwrap_or_default()
        .iter()
        .filter_map(|origin| https_origin(origin))
        .collect()
}

/// The host and effective port of an https URL on a DNS name; `None` for
/// any other scheme, an IP literal or a URL that does not parse.
fn https_origin(url: &str) -> Option<Origin> {
    let url = Url::parse(url).ok()?;
    let Some(url::Host::Domain(host)) = url.host() else {
        return None;
    };
    if url.scheme() != "https" {
        return None;
    }
    Some((host.to_ascii_lowercase(), url.port_or_known_default()?))
}

/// Whether `listed` names `host` (any case) on `port`.
fn listed_in(listed: &BTreeSet<Origin>, host: &str, port: u16) -> bool {
    listed.contains(&(host.to_ascii_lowercase(), port))
}

/// Whether a PUT to `url` goes to a listed origin: https, the host in any
/// case and the effective port, all exactly.
fn url_listed_in(listed: &BTreeSet<Origin>, url: &str) -> bool {
    https_origin(url).is_some_and(|(host, port)| listed_in(listed, &host, port))
}

/// [`url_listed_in`] this build's list.
pub(super) fn url_listed(url: &str) -> bool {
    url_listed_in(&LISTED, url)
}

/// The storage agents' resolver exemption: this build's list.
fn listed(host: &str, port: u16) -> bool {
    listed_in(&LISTED, host, port)
}

/// The storage counterpart of `post::http_agent`, pooled the same way, for
/// a PUT to a listed origin ([`url_listed`]) only.
pub(super) fn storage_agent() -> ureq::Agent {
    static AGENT: LazyLock<cloud::HistoryAgentPool> = LazyLock::new(|| {
        cloud::HistoryAgentPool::storage(
            |builder| {
                builder
                    .redirects(0)
                    .max_idle_connections_per_host(MAX_IN_FLIGHT)
            },
            listed,
        )
    });
    AGENT.get()
}

/// The storage counterpart of `post::fresh_agent`: no reused connection, for
/// the one resend a PUT makes after a connection closed under it.
pub(super) fn fresh_storage_agent() -> ureq::Agent {
    static AGENT: LazyLock<cloud::HistoryAgentPool> = LazyLock::new(|| {
        cloud::HistoryAgentPool::storage(
            |builder| builder.redirects(0).max_idle_connections(0),
            listed,
        )
    });
    AGENT.get()
}

#[cfg(test)]
mod tests;
