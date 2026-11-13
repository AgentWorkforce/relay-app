//! What session history tells the core about sessions its discovery cannot
//! see (`agent_relay_node::relay::session_sources`): the Devin Cloud and
//! Claude Code on the web sessions the install catalogs list, delivery into
//! Devin Cloud, and Hermes's homes as relayhistory names them.

use agent_relay_node::relay::inject::InjectOutcome;
use agent_relay_node::relay::local::LocalConfig;
use agent_relay_node::relay::session_sources::{
    HermesRoots, RemoteSessions, RemoteSources, SessionSources,
};
use agent_relay_node::relay::sessions::DiscoveredSession;
use agent_relay_node::relay::store::{SessionKind, Target};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

/// The sources a desktop build serves with.
pub fn session_sources(config: &agent_relay_node::relay::daemon::Config) -> SessionSources {
    SessionSources {
        remote: RemoteSources(vec![Arc::new(Catalogs {
            root: probe_root(config),
        })]),
        hermes: Some(Arc::new(ProviderRoots)),
    }
}

/// Where the install catalogs live. Under the real home, so Dev's private
/// provider profiles never move it.
pub fn probe_root(config: &agent_relay_node::relay::daemon::Config) -> PathBuf {
    config.home.join(".agentworkforce/probe")
}

/// The remote sessions the install catalogs under `root` list.
pub struct Catalogs {
    root: PathBuf,
}

impl RemoteSessions for Catalogs {
    fn sessions(&self, config: &LocalConfig, kind: SessionKind) -> Vec<DiscoveredSession> {
        match kind {
            SessionKind::Devin => super::devin_cloud::discover(&config.devin, &self.root),
            SessionKind::ClaudeCloud => super::claude_web::discover(&config.claude, &self.root),
            _ => Vec::new(),
        }
    }

    /// Devin reviews: sessions an automation started, with nobody in them.
    fn hidden(&self, _config: &LocalConfig) -> HashSet<Target> {
        super::devin_cloud::automation_session_ids(&self.root)
            .into_iter()
            .map(|id| Target::new(SessionKind::Devin, id))
            .collect()
    }

    fn may_join(&self, session: &DiscoveredSession) -> Option<bool> {
        (session.target.kind == SessionKind::Devin
            && session.host.as_deref() == Some(agent_relay_node::relay::devin_cloud::CLOUD_HOST))
        .then(|| super::devin_cloud::recent_enough_to_join(session))
    }

    fn send(&self, config: &LocalConfig, target: &Target, text: &str) -> Option<InjectOutcome> {
        if target.kind != SessionKind::Devin {
            return None;
        }
        super::devin_cloud::send_cloud(&config.devin, Some(&self.root), &target.id, text)
    }
}

/// Hermes's homes as relayhistory's importer resolves them, so a catalog
/// session id names the same home the importer read it from.
pub struct ProviderRoots;

impl HermesRoots for ProviderRoots {
    fn roots(&self) -> Option<(Vec<PathBuf>, Option<PathBuf>)> {
        // Explicit HERMES_HOME roots have hash identities even when they
        // happen to name ~/.hermes; only a native default root gets the
        // portable default/profile namespaces.
        let home = PathBuf::from(std::env::var_os("HOME")?);
        let roots = serde_json::to_value(ai_hist::ProviderRoots::from_env(home)).ok()?;
        let paths: Vec<PathBuf> =
            serde_json::from_value(roots["provider_roots"]["hermes"].clone()).ok()?;
        let default: Option<PathBuf> =
            serde_json::from_value(roots["provider_default_roots"]["hermes"].clone()).ok();
        Some((paths, default))
    }
}
