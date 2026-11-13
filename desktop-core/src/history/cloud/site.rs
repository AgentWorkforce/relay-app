//! The sites and origins Cloud and RelayHistory are trusted at: the site
//! this build is made for, the Cloud API root under it, and the History
//! origins a session may name.

use anyhow::{Context, Result};
use url::Url;

/// The Cloud API of the site this build is made for.
pub(super) fn default_cloud_api_url() -> String {
    format!(
        "{}/cloud",
        agent_relay_node::relay::links::built_in_site_url()
    )
}

/// The documented development opt-in, shared with `validate_cloud_exchange_base_url`.
pub(super) fn untrusted_cloud_base_url_allowed() -> bool {
    std::env::var("RELAYHISTORY_ALLOW_UNTRUSTED_CLOUD_BASE_URL")
        .ok()
        .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

pub(super) fn is_loopback_origin(url: &Url) -> bool {
    matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
}

/// The Agent Relay Cloud site this process will talk to, as a normalized origin.
///
/// The site this build is made for (`AGENT_RELAY_SITE_URL` at build time,
/// `links::built_in_site_url`) is trusted as it is, and so is production,
/// `https://agentrelay.com`, in a production build; a build made for its own
/// site ([`crate::site_build`]) does not trust production. Loopback over http
/// is the documented local-development site. Any other https origin requires
/// the same explicit opt-in that lets a Cloud bearer reach a non-default
/// RelayHistory.
pub fn site_origin(site: &str) -> Result<String> {
    site_origin_with(agent_relay_node::relay::links::built_in_site_url(), site)
}

/// [`site_origin`] for a build made for `built_in`.
pub fn site_origin_with(built_in: &str, site: &str) -> Result<String> {
    let url = Url::parse(site.trim())?;
    let local = is_loopback_origin(&url);
    let production_trusted = !crate::site_build::is_site_build_for(built_in);
    let built_in = Url::parse(built_in).ok().map(|built_in| built_in.origin());
    anyhow::ensure!(
        (local && url.scheme() == "http")
            || (!local
                && url.scheme() == "https"
                && ((production_trusted
                    && url.host_str() == Some("agentrelay.com")
                    && url.port().is_none())
                    || built_in.is_some_and(|built_in| built_in == url.origin())
                    || untrusted_cloud_base_url_allowed())),
        "untrusted site"
    );
    anyhow::ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "site must be an origin"
    );
    Ok(url.origin().ascii_serialization())
}

/// Returns the normalized origin. Callers store and compare that, never the
/// raw value: a trailing slash survives the checks below and would then be
/// concatenated into request paths as `//v1/...`.
pub fn history_origin(raw: &str, site: &str) -> Result<String> {
    super::history_origin::history_origin_for(raw, site)
}

pub(super) const PRODUCTION_HISTORY_HOST: &str = "history.agentrelay.com";
/// The RelayHistory hosts a session Cloud pushed to a hosted sandbox may name:
/// production, and the `dev` stage that relayhistory-cloud deploys as
/// `dev.history.agentrelay.com` (`stageTarget` in its
/// `scripts/deploy-cloudflare.mjs`). Compiled in: a pushed session cannot
/// widen it, and no environment switch does either.
pub(super) const PUSHED_HISTORY_HOSTS: &[&str] =
    &[PRODUCTION_HISTORY_HOST, "dev.history.agentrelay.com"];

/// [`history_origin`] for a session pushed with no Cloud site beside it
/// (`history flush`): one of [`PUSHED_HISTORY_HOSTS`] over https on its
/// default port, or loopback http, the same local development stage
/// `history_origin` accepts beside a loopback site. A build made for its own
/// site trusts no production History implicitly, so there only loopback
/// http is taken.
pub fn pushed_history_origin(raw: &str) -> Result<String> {
    pushed_history_origin_with(agent_relay_node::relay::links::built_in_site_url(), raw)
}

/// [`pushed_history_origin`] for a build made for `built_in`.
pub fn pushed_history_origin_with(built_in: &str, raw: &str) -> Result<String> {
    let url = Url::parse(raw.trim())?;
    anyhow::ensure!(
        (is_loopback_origin(&url) && url.scheme() == "http")
            || (!crate::site_build::is_site_build_for(built_in)
                && url.scheme() == "https"
                && url
                    .host_str()
                    .is_some_and(|host| PUSHED_HISTORY_HOSTS.contains(&host))
                && url.port().is_none()),
        "untrusted history origin"
    );
    history_url_origin(url)
}

/// A trusted history URL's origin, refusing anything beyond one.
pub(super) fn history_url_origin(url: Url) -> Result<String> {
    anyhow::ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "invalid history URL"
    );
    Ok(url.origin().ascii_serialization())
}

/// The Cloud API root: explicit selection, else `CLOUD_API_URL`, else the
/// site this build is made for. Always `{trusted site origin}/cloud`,
/// without a trailing slash.
pub fn cloud_api_url(explicit: Option<&str>) -> Result<String> {
    let selected = explicit
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(String::from)
        .or_else(|| {
            std::env::var("CLOUD_API_URL")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(default_cloud_api_url);
    Ok(format!("{}/cloud", cloud_site(&selected)?))
}

/// The site origin backing a Cloud API URL.
pub fn cloud_site(api_url: &str) -> Result<String> {
    let origin = api_url
        .trim()
        .trim_end_matches('/')
        .strip_suffix("/cloud")
        .context("Agent Relay Cloud API URL must be an origin followed by `/cloud`")?;
    site_origin(origin)
}
