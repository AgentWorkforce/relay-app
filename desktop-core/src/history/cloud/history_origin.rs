//! The one widening of the RelayHistory trust rule: the workspace's own
//! History origin, as Cloud names it, and the address check every
//! authenticated request to such a History passes first.

use super::{
    history_origin, history_url_origin, is_loopback_origin, normalized_stage,
    require_secure_transport, untrusted_cloud_base_url_allowed, PRODUCTION_HISTORY_HOST,
    PUSHED_HISTORY_HOSTS,
};
use anyhow::Result;
use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::Mutex;
use url::Url;

/// [`history_origin`]: beside a loopback
/// site, loopback http; beside any other, `https://history.agentrelay.com`
/// alone, and only in a production build. The RelayHistory a self-hosted
/// site's control plane hands out is trusted only as the origin Cloud named
/// ([`workspace_history_origin`], recorded as `cloud_history_origin`), never
/// because a site is not production; a build made for its own site
/// ([`crate::site_build`]) trusts no production History implicitly, so
/// there that is the only History. Never an address, a port or plain http
/// off loopback.
pub fn history_origin_for(raw: &str, site: &str) -> Result<String> {
    history_origin_for_with(
        agent_relay_node::relay::links::built_in_site_url(),
        raw,
        site,
    )
}

/// [`history_origin_for`] for a build made for `built_in`.
pub fn history_origin_for_with(built_in: &str, raw: &str, site: &str) -> Result<String> {
    let url = Url::parse(raw.trim())?;
    let site_url = Url::parse(site.trim())?;
    let local = is_loopback_origin(&site_url);
    let named_host = url.scheme() == "https"
        && url.port().is_none()
        && url
            .host_str()
            .is_some_and(|host| host.contains('.') && !host.ends_with('.'))
        && !is_loopback_origin(&url)
        && !matches!(
            url.host(),
            Some(url::Host::Ipv4(_)) | Some(url::Host::Ipv6(_))
        );
    anyhow::ensure!(
        (local && is_loopback_origin(&url) && url.scheme() == "http")
            || (!local
                && named_host
                && !crate::site_build::is_site_build_for(built_in)
                && url.host_str() == Some(PRODUCTION_HISTORY_HOST)),
        "untrusted history origin"
    );
    history_url_origin(url)
}

/// [`history_origin`], widened by the one origin Cloud names for this
/// workspace. A workspace whose owner registered its own RelayHistory (a
/// customer-hosted data plane) gets that origin back in the authenticated,
/// redirect-free workspace-session answer from a trusted Cloud site; this is
/// the only place a non-default origin is accepted, and only an https origin
/// on a DNS name — never cleartext, loopback, an IP literal, a path or
/// credentials. Nothing local (environment, flag, file) can name one.
pub fn workspace_history_origin(raw: &str, site: &str) -> Result<String> {
    if let Ok(origin) = history_origin(raw, site) {
        return Ok(origin);
    }
    let url = Url::parse(raw.trim())?;
    let host = match url.host() {
        Some(url::Host::Domain(host)) => host.to_ascii_lowercase(),
        _ => anyhow::bail!("untrusted history origin"),
    };
    // A trailing dot (`localhost.`, `intranet.`) names the same host as an
    // absolute DNS name and would slip past the checks below; no public
    // History needs it, so it is refused rather than stripped, as Cloud does.
    anyhow::ensure!(
        !is_loopback_origin(&Url::parse(site.trim())?)
            && url.scheme() == "https"
            && !host.ends_with('.')
            && host.contains('.')
            && host != "localhost"
            && !INTERNAL_HOST_SUFFIXES
                .iter()
                .any(|suffix| host.ends_with(suffix)),
        "untrusted history origin"
    );
    history_url_origin(url)
}

/// The hosts of the RelayHistory origins Cloud named in this process
/// ([`note_cloud_named_origin`]).
static CLOUD_NAMED_HOSTS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Notes `origin` as the RelayHistory the site this build signs in to named
/// for the workspace (its answer, or `cloud_history_origin` as setup
/// recorded it), for [`deployment_host`].
pub(crate) fn note_cloud_named_origin(origin: &str) {
    if let Some(host) = Url::parse(origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
    {
        CLOUD_NAMED_HOSTS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(host);
    }
}

/// Whether `host` belongs to the deployment this build was made for, which
/// may sit on private addresses (split-horizon DNS behind a VPN): the
/// built-in site itself, compiled in, and beside a self-hosted site the
/// RelayHistory that site named. Production's own workspaces still meet the
/// address rule for an origin they registered.
fn deployment_host(host: &str) -> bool {
    let named = CLOUD_NAMED_HOSTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    deployment_host_with(
        agent_relay_node::relay::links::built_in_site_url(),
        &named,
        host,
    )
}

/// [`deployment_host`] for a build made for `built_in`.
fn deployment_host_with(built_in: &str, named: &BTreeSet<String>, host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let built_in_host = Url::parse(built_in)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase));
    built_in_host.as_deref() == Some(host.as_str())
        || (crate::site_build::is_site_build_for(built_in) && named.contains(&host))
}

/// Name suffixes that never belong to a public History. The same list Cloud
/// refuses when a workspace registers its own origin.
const INTERNAL_HOST_SUFFIXES: &[&str] =
    &[".localhost", ".local", ".internal", ".lan", ".home.arpa"];

/// [`require_secure_transport`], and for a workspace's own History on a DNS
/// name, that the name resolves only to public addresses. Origin checks
/// refuse IP literals, but a custom name can still resolve to loopback, a
/// private network, link-local (cloud metadata) or a unique-local address;
/// the probe must not send a History session or Cloud's assertion there.
/// The compiled default stages, loopback development over http and the
/// deployment this build was made for ([`deployment_host`]) are not
/// resolved, nor is anything under the development flag. Every
/// authenticated History request calls this immediately before it is sent.
/// A workspace History's name did not resolve. Unlike a refused address
/// this can clear by itself, so delivery retries it instead of blocking.
#[derive(Debug)]
pub(crate) struct HistoryUnresolved;
impl std::fmt::Display for HistoryUnresolved {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("could not resolve the workspace RelayHistory")
    }
}
impl std::error::Error for HistoryUnresolved {}

pub(crate) fn require_history_destination(base_url: &str) -> Result<()> {
    // A local certificate override must not hide a refused transport or
    // address: those jobs stay blocked even after the override is cleared.
    // A name that does not resolve is not a refusal, so the override is
    // reported first; otherwise it would wait for DNS to recover.
    if let Err(error) = require_history_address(base_url) {
        if error.is::<HistoryUnresolved>() {
            transport::require_safe_roots()?;
        }
        return Err(error);
    }
    transport::require_safe_roots()?;
    Ok(())
}

fn require_history_address(base_url: &str) -> Result<()> {
    require_secure_transport(base_url)?;
    if untrusted_cloud_base_url_allowed() {
        return Ok(());
    }
    let url = Url::parse(&normalized_stage(base_url)?)?;
    let Some(url::Host::Domain(host)) = url.host() else {
        return Ok(());
    };
    if url.scheme() != "https" || PUSHED_HISTORY_HOSTS.contains(&host) || deployment_host(host) {
        return Ok(());
    }
    let port = url.port_or_known_default().unwrap_or(443);
    let addresses: Vec<IpAddr> = ToSocketAddrs::to_socket_addrs(&(host, port))
        .map_err(|_| HistoryUnresolved)?
        .map(|address| address.ip())
        .collect();
    if addresses.is_empty() {
        return Err(HistoryUnresolved.into());
    }
    anyhow::ensure!(
        addresses.into_iter().all(is_public_address),
        "refusing to send a RelayHistory session to `{host}`: it resolves to a loopback, private or link-local address"
    );
    Ok(())
}

/// An agent for every request that carries a History session or Cloud's
/// assertion. Its resolver applies the address rule to the very addresses
/// the connection uses, so a name cannot answer [`require_history_destination`]
/// with a public address and the connection with a private one; TLS still
/// verifies the name itself, using the core's shared operating-system roots.
/// Certificate-file overrides refuse construction and new connections; an
/// existing pooled connection keeps its previously verified trust configuration.
pub fn history_agent() -> ureq::AgentBuilder {
    transport::agent()
}

mod transport;
pub(crate) use transport::{
    certificate_override_refused, HistoryAgentPool, HistoryTlsConfigurationRefused,
};

/// `addresses` for `host`, unless `host` is a name the address rule covers
/// and any of them is not public. IP literals were checked as origins, and
/// `localhost`, the compiled default stages, this build's deployment and the
/// development flag are exempt, as in [`require_history_destination`].
fn connectable(host: &str, addresses: Vec<SocketAddr>) -> std::io::Result<Vec<SocketAddr>> {
    let exempt = untrusted_cloud_base_url_allowed()
        || host.starts_with('[')
        || host.parse::<IpAddr>().is_ok()
        || host.eq_ignore_ascii_case("localhost")
        || PUSHED_HISTORY_HOSTS.contains(&host)
        || deployment_host(host);
    if exempt
        || addresses
            .iter()
            .all(|address| is_public_address(address.ip()))
    {
        return Ok(addresses);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "refusing a RelayHistory connection to a non-public address",
    ))
}

/// Is `address` reachable only across the public internet? Loopback,
/// private (RFC 1918), shared (CGNAT), link-local, unique-local, multicast,
/// unspecified, broadcast, documentation and reserved ranges are not, nor is
/// an IPv4-mapped IPv6 address of any of them.
fn is_public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => is_public_v6(ip),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    let special = ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_broadcast()
        || ip.is_documentation();
    // This network, shared CGNAT, IETF protocol assignments, benchmarking and
    // the reserved class E space.
    let reserved = a == 0
        || (a == 100 && (64..128).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 198 && (18..20).contains(&b))
        || a >= 240;
    !(special || reserved)
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_public_v4(mapped);
    }
    let segments = ip.segments();
    let special = ip.is_unspecified() || ip.is_loopback() || ip.is_multicast();
    // Unique-local fc00::/7, link-local fe80::/10, site-local fec0::/10,
    // documentation 2001:db8::/32, IPv4-compatible ::/96 and NAT64 64:ff9b::/96.
    let reserved = (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] & 0xffc0) == 0xfec0
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
        || segments[..6] == [0, 0, 0, 0, 0, 0]
        || segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0];
    !(special || reserved)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Beside a site off loopback a production build trusts production's
    /// History; a build made for its own site trusts none implicitly, only
    /// the History its site's Cloud names.
    #[test]
    fn only_a_production_build_trusts_production_s_history_beside_a_site() {
        let production = agent_relay_node::relay::links::DEFAULT_SITE_URL;
        let built_in = "https://relay.example.test";
        for site in [production, built_in] {
            assert_eq!(
                history_origin_for_with(production, "https://history.agentrelay.com/", site)
                    .unwrap(),
                "https://history.agentrelay.com"
            );
            let refused = history_origin_for_with(built_in, "https://history.agentrelay.com", site)
                .unwrap_err();
            assert_eq!(refused.to_string(), "untrusted history origin");
        }
        // Loopback development is the same in both.
        for build in [production, built_in] {
            assert_eq!(
                history_origin_for_with(build, "http://127.0.0.1:3102", "http://127.0.0.1:3100")
                    .unwrap(),
                "http://127.0.0.1:3102"
            );
        }
    }

    #[test]
    fn only_public_addresses_may_receive_a_workspace_history_session() {
        let _guard = crate::history::cloud::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var_os("RELAYHISTORY_ALLOW_UNTRUSTED_CLOUD_BASE_URL");
        std::env::remove_var("RELAYHISTORY_ALLOW_UNTRUSTED_CLOUD_BASE_URL");
        for private in [
            "127.0.0.1",
            "10.0.0.5",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "::1",
            "::",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "ff02::1",
            "::ffff:10.0.0.5",
            "::ffff:127.0.0.1",
        ] {
            assert!(
                !is_public_address(private.parse::<IpAddr>().unwrap()),
                "{private}"
            );
        }
        for public in ["93.184.216.34", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(
                is_public_address(public.parse::<IpAddr>().unwrap()),
                "{public}"
            );
        }
        // Every authenticated History request checks it: a name resolving to
        // loopback is refused, while the compiled default stage and loopback
        // development over http are not resolved at all.
        assert!(require_history_destination("https://localhost:8443").is_err());
        assert!(require_history_destination("https://history.agentrelay.com").is_ok());
        assert!(require_history_destination("http://127.0.0.1:8787").is_ok());
        // A refused address is not a lookup failure; a name that does not
        // resolve (`.invalid` never does) is, so delivery can retry it.
        assert!(!require_history_destination("https://localhost:8443")
            .unwrap_err()
            .is::<HistoryUnresolved>());
        assert!(
            require_history_destination("https://history.relayhistory.invalid")
                .unwrap_err()
                .is::<HistoryUnresolved>()
        );
        // The connection itself is held to the same rule, so a name that
        // answered the check with a public address cannot connect privately.
        let at = |ip: &str| vec![SocketAddr::new(ip.parse().unwrap(), 443)];
        assert!(connectable("history.hookdeck.example", at("10.0.0.5")).is_err());
        assert!(connectable("history.hookdeck.example", at("::1")).is_err());
        assert!(connectable("history.hookdeck.example", at("1.1.1.1")).is_ok());
        assert!(connectable("localhost", at("127.0.0.1")).is_ok());
        assert!(connectable("127.0.0.1", at("127.0.0.1")).is_ok());
        assert!(connectable("history.agentrelay.com", at("10.0.0.5")).is_ok());
        // The site this build was made for may sit on a private address.
        assert!(connectable("agentrelay.com", at("10.0.0.5")).is_ok());
        if let Some(value) = previous {
            std::env::set_var("RELAYHISTORY_ALLOW_UNTRUSTED_CLOUD_BASE_URL", value);
        }
    }

    /// A build made for a self-hosted site reaches it, and the RelayHistory it
    /// named, on a private address (split-horizon DNS); production's
    /// workspace-registered origins are still held to the address rule.
    #[test]
    fn the_deployment_a_build_was_made_for_may_be_private() {
        let named: BTreeSet<String> = ["history.corp.example".to_string()].into();
        let corp = "https://relay.corp.example";
        assert!(deployment_host_with(corp, &named, "relay.corp.example"));
        assert!(deployment_host_with(corp, &named, "Relay.Corp.Example"));
        assert!(deployment_host_with(corp, &named, "history.corp.example"));
        assert!(!deployment_host_with(corp, &named, "other.corp.example"));
        assert!(!deployment_host_with(
            corp,
            &BTreeSet::new(),
            "history.corp.example"
        ));
        let production = agent_relay_node::relay::links::DEFAULT_SITE_URL;
        assert!(deployment_host_with(production, &named, "agentrelay.com"));
        assert!(!deployment_host_with(
            production,
            &named,
            "history.corp.example"
        ));

        // This test build is made for production: a noted Cloud-named
        // origin is not exempt, the built-in site is.
        let _guard = crate::history::cloud::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var_os("RELAYHISTORY_ALLOW_UNTRUSTED_CLOUD_BASE_URL");
        std::env::remove_var("RELAYHISTORY_ALLOW_UNTRUSTED_CLOUD_BASE_URL");
        note_cloud_named_origin("https://history.registered.example");
        let at = |ip: &str| vec![SocketAddr::new(ip.parse().unwrap(), 443)];
        assert!(connectable("history.registered.example", at("10.0.0.5")).is_err());
        assert!(connectable("agentrelay.com", at("10.0.0.5")).is_ok());
        if let Some(value) = previous {
            std::env::set_var("RELAYHISTORY_ALLOW_UNTRUSTED_CLOUD_BASE_URL", value);
        }
    }

    #[test]
    fn override_order_child() {
        if std::env::var_os("RELAY_HISTORY_OVERRIDE_ORDER_CHILD").is_none() {
            return;
        }
        // A name that does not resolve (`.invalid` never does) reports the
        // override, so the job takes the local wait rather than DNS backoff.
        let error =
            require_history_destination("https://history.relayhistory.invalid").unwrap_err();
        assert!(error.is::<HistoryTlsConfigurationRefused>());
        assert!(!error.is::<HistoryUnresolved>());
        // A refused address or transport still wins over the override.
        for refused in ["https://localhost:8443", "http://192.168.0.1"] {
            let error = require_history_destination(refused).unwrap_err();
            assert!(!error.is::<HistoryTlsConfigurationRefused>(), "{refused}");
            assert!(!error.is::<HistoryUnresolved>(), "{refused}");
        }
    }

    #[test]
    fn a_certificate_override_is_reported_before_an_unresolved_name() {
        for key in ["SSL_CERT_FILE", "SSL_CERT_DIR"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "history::cloud::history_origin::tests::override_order_child",
                ])
                .env_remove("SSL_CERT_FILE")
                .env_remove("SSL_CERT_DIR")
                .env_remove("RELAYHISTORY_ALLOW_UNTRUSTED_CLOUD_BASE_URL")
                .env("RELAY_HISTORY_OVERRIDE_ORDER_CHILD", "1")
                .env(key, "never-read-test-ca")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{key}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
