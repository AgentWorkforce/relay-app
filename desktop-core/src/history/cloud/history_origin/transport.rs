//! History's address rule and strict platform TLS share one transport boundary.

use super::connectable;
use agent_relay_node::relay::websocket_tls::{client_config, platform_root_environment_is_safe};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::{Arc, Mutex, Once};

pub(super) fn agent() -> ureq::AgentBuilder {
    platform_agent(client_config)
}

type RootLoader = fn() -> Arc<rustls::ClientConfig>;

/// Whether a connection to `host` on `port` may reach a non-public
/// address: only a direct-upload storage pool ([`HistoryAgentPool::storage`])
/// lists any.
pub(crate) type StorageListed = fn(&str, u16) -> bool;

/// No storage exemption: every History and Cloud agent.
fn no_storage(_: &str, _: u16) -> bool {
    false
}

/// Keep connections only while the shared platform trust snapshot is current.
pub(crate) struct HistoryAgentPool {
    admitted: bool,
    load: RootLoader,
    configure: fn(ureq::AgentBuilder) -> ureq::AgentBuilder,
    listed: StorageListed,
    pooled: Mutex<Option<(Arc<rustls::ClientConfig>, ureq::Agent)>>,
}

impl HistoryAgentPool {
    pub(crate) fn new(configure: fn(ureq::AgentBuilder) -> ureq::AgentBuilder) -> Self {
        Self::with_loader(client_config, configure)
    }

    /// A pool for direct-upload storage PUTs only, never for a request that
    /// carries a History session or Cloud's assertion: its resolver also
    /// admits a non-public address for a `host:port` that `listed` names
    /// (the site build's storage origins). Everything else is as [`Self::new`]:
    /// the same roots, the same refusal of certificate overrides and the
    /// address rule for every other host and port.
    pub(crate) fn storage(
        configure: fn(ureq::AgentBuilder) -> ureq::AgentBuilder,
        listed: StorageListed,
    ) -> Self {
        Self {
            listed,
            ..Self::new(configure)
        }
    }

    fn with_loader(
        load: RootLoader,
        configure: fn(ureq::AgentBuilder) -> ureq::AgentBuilder,
    ) -> Self {
        Self {
            admitted: roots_are_safe(),
            load,
            configure,
            listed: no_storage,
            pooled: Mutex::new(None),
        }
    }

    pub(crate) fn get(&self) -> ureq::Agent {
        // Do not load roots or reuse a connection under a certificate override.
        // A pool constructed under one remains refused after it is cleared.
        if !self.admitted || !roots_are_safe() {
            return (self.configure)(with_selected_roots(None, self.listed)).build();
        }
        let roots = (self.load)();
        let mut pooled = self.pooled.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((previous, agent)) = pooled.as_ref() {
            if Arc::ptr_eq(previous, &roots) {
                return agent.clone();
            }
        }
        let agent = (self.configure)(with_selected_roots(Some(roots.clone()), self.listed)).build();
        *pooled = Some((roots, agent.clone()));
        agent
    }
}

fn platform_agent(load: RootLoader) -> ureq::AgentBuilder {
    #[cfg(test)]
    LOADER.with(|slot| slot.set(Some(load)));
    with_roots(load)
}

#[cfg(test)]
thread_local! {
    static LOADER: std::cell::Cell<Option<RootLoader>> = const { std::cell::Cell::new(None) };
}

/// A configuration refusal, distinct from an unavailable network or DNS.
#[derive(Debug)]
pub(crate) struct HistoryTlsConfigurationRefused;
impl std::fmt::Display for HistoryTlsConfigurationRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "History refuses SSL_CERT_FILE or SSL_CERT_DIR; use the operating system trust store",
        )
    }
}
impl std::error::Error for HistoryTlsConfigurationRefused {}
impl HistoryTlsConfigurationRefused {
    pub(crate) fn caused_by(error: &anyhow::Error) -> bool {
        error.chain().any(|cause| {
            cause.is::<Self>()
                || cause.downcast_ref::<ureq::Error>().is_some_and(|error| {
                    matches!(error, ureq::Error::Transport(t) if certificate_override_refused(t))
                })
        })
    }
}

pub(super) fn require_safe_roots() -> Result<(), HistoryTlsConfigurationRefused> {
    roots_are_safe()
        .then_some(())
        .ok_or(HistoryTlsConfigurationRefused)
}

pub(crate) fn certificate_override_refused(error: &ureq::Transport) -> bool {
    std::error::Error::source(error)
        .and_then(|e| e.downcast_ref::<std::io::Error>())
        .and_then(std::io::Error::get_ref)
        .is_some_and(|e| e.is::<HistoryTlsConfigurationRefused>())
}

fn roots_are_safe() -> bool {
    if platform_root_environment_is_safe() {
        return true;
    }
    static WARNED: Once = Once::new();
    WARNED.call_once(|| {
        // Never print the environment values: they can contain private paths.
        agent_relay_node::relay::diagnostics::write(format_args!(
            "relay: History refuses certificate-file overrides; unset SSL_CERT_FILE and SSL_CERT_DIR and install the CA in the operating system trust store"
        ));
    });
    false
}

fn with_roots(load: impl FnOnce() -> Arc<rustls::ClientConfig>) -> ureq::AgentBuilder {
    // An agent constructed under an override stays refused if it is cleared.
    // Do not call the native root loader at all under a file override.
    with_selected_roots(roots_are_safe().then(load), no_storage)
}

fn with_selected_roots(
    roots: Option<Arc<rustls::ClientConfig>>,
    listed: StorageListed,
) -> ureq::AgentBuilder {
    let admitted = roots.is_some();
    let builder = match roots {
        Some(roots) => ureq::AgentBuilder::new().tls_config(roots),
        None => ureq::AgentBuilder::new(),
    };
    builder.resolver(move |netloc: &str| {
        // Also catch overrides introduced after construction, before DNS.
        if !admitted || !roots_are_safe() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                HistoryTlsConfigurationRefused,
            ));
        }
        admit(netloc, netloc.to_socket_addrs()?.collect(), listed)
    })
}

/// The addresses ureq resolved for `netloc` (always `host:port`), when the
/// connection may use them: a `host` and `port` that `listed` names exactly
/// may be private; anything else meets [`connectable`].
fn admit(
    netloc: &str,
    addresses: Vec<SocketAddr>,
    listed: StorageListed,
) -> std::io::Result<Vec<SocketAddr>> {
    let (host, port) = netloc
        .rsplit_once(':')
        .map_or((netloc, None), |(host, port)| (host, port.parse().ok()));
    if port.is_some_and(|port| listed(host, port)) {
        return Ok(addresses);
    }
    connectable(host, addresses)
}

#[cfg(test)]
mod pool_tests;
#[cfg(test)]
mod storage_tests;
#[cfg(test)]
mod tests;
