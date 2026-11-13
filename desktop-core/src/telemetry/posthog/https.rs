//! Analytics uses the same strict platform roots as sign-in and uploads.
//! There is no separate CA file, bundled-root fallback or redirect path.

use super::{Answer, Transport, TransportFailure, Value, SEND_TIMEOUT};
use agent_relay_node::relay::websocket_tls::{client_config, platform_root_environment_is_safe};
use std::sync::{Arc, Mutex};

type RootLoader = fn() -> Arc<rustls::ClientConfig>;

pub(super) struct Https {
    admitted_roots: bool,
    load_roots: RootLoader,
    pooled: Mutex<Option<(Arc<rustls::ClientConfig>, ureq::Agent)>>,
}

impl Https {
    pub(super) fn new() -> Self {
        Self::with_roots(client_config)
    }

    fn with_roots(load_roots: RootLoader) -> Self {
        Self {
            admitted_roots: platform_root_environment_is_safe(),
            load_roots,
            pooled: Mutex::new(None),
        }
    }

    fn pooled_agent(&self) -> ureq::Agent {
        let roots = (self.load_roots)();
        let mut pooled = self.pooled.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((previous, agent)) = pooled.as_ref() {
            if Arc::ptr_eq(previous, &roots) {
                return agent.clone();
            }
        }
        let next = agent(roots.clone());
        *pooled = Some((roots, next.clone()));
        next
    }
}

fn agent(tls: Arc<rustls::ClientConfig>) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .tls_config(tls)
        .redirects(0)
        .timeout(SEND_TIMEOUT)
        .build()
}

fn post(
    agent: &ureq::Agent,
    url: &str,
    bearer: Option<&str>,
    body: Value,
) -> Result<Answer, String> {
    let mut request = agent.post(url);
    if let Some(bearer) = bearer {
        request = request.set("Authorization", &format!("Bearer {bearer}"));
    }
    let response = match request.send_json(body) {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(ureq::Error::Transport(_)) => return Err("PostHog could not be reached.".into()),
    };
    Ok(Answer {
        status: response.status(),
        retry_after: response.header("Retry-After").map(str::to_string),
    })
}

impl Transport for Https {
    fn post(&self, url: &str, bearer: Option<&str>, body: Value) -> Result<Answer, String> {
        self.checked_post(url, bearer, body)
            .map_err(TransportFailure::reason)
    }

    fn checked_post(
        &self,
        url: &str,
        bearer: Option<&str>,
        body: Value,
    ) -> Result<Answer, TransportFailure> {
        // Check before loading roots, and on every send. A sender constructed
        // under an override cannot become usable merely by clearing it.
        if !self.admitted_roots || !platform_root_environment_is_safe() {
            return Err(TransportFailure::Permanent(
                "Certificate file overrides are unsupported for analytics.".into(),
            ));
        }
        // Keep connections while the shared trust Arc is current. A cache
        // refresh replaces the pool before the next send; no lock spans I/O.
        post(&self.pooled_agent(), url, bearer, body).map_err(TransportFailure::Transient)
    }
}

#[cfg(test)]
mod tests;
