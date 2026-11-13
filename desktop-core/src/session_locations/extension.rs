//! `sessions.location` on the core's bridge, and the observations that let it
//! name a session's last window after its process has gone.

use super::{
    observe, planner, report, unavailable, SessionLocationReport, SessionOpenPlan,
    SessionOpenRequest, SessionsLocation, SessionsOpenPlan,
};
use agent_relay_node::relay::contract::agents::SessionTitleIdentity;
use agent_relay_node::relay::contract::{answer, Availability, Method, MethodSpec};
use agent_relay_node::relay::daemon::{CallContext, Core, MethodExtension};
use agent_relay_node::relay::local::LocalConfig;
use agent_relay_node::relay::local_methods::discovered_sessions;
use agent_relay_node::relay::protocol::{ErrorKind, RpcError, Topic};
use agent_relay_node::relay::session_titles::validate_identity;
use futures_util::future::BoxFuture;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

/// How often sessions are observed when the session list holds still: a
/// session can move to another terminal without the list changing.
const OBSERVE_EVERY: Duration = Duration::from_secs(5);

/// The bridge method and what it reads: the agents' configuration and the
/// folder its database lives in. Without them it reports locations
/// unavailable and observes nothing.
pub struct SessionLocations {
    config: Option<Arc<(LocalConfig, PathBuf)>>,
}

impl SessionLocations {
    pub fn new(config: Option<(LocalConfig, PathBuf)>) -> SessionLocations {
        SessionLocations {
            config: config.map(Arc::new),
        }
    }

    async fn locate(
        &self,
        core: &Core,
        params: SessionTitleIdentity,
    ) -> Result<SessionLocationReport, RpcError> {
        validate_identity(&params.source, &params.session_id)?;
        let Some(config) = self.config.clone() else {
            return Ok(unavailable());
        };
        let table = core.plugins.processes.clone();
        tokio::task::spawn_blocking(move || {
            report(
                &config.0,
                &config.1,
                &*table,
                &params.source,
                &params.session_id,
            )
        })
        .await
        .map_err(|_| RpcError::new(ErrorKind::Internal, "Could not locate session."))?
        .map_err(|error| RpcError::new(ErrorKind::Internal, error))
    }
    async fn open_plan(
        &self,
        core: &Core,
        params: SessionOpenRequest,
    ) -> Result<SessionOpenPlan, RpcError> {
        validate_identity(&params.source, &params.session_id)?;
        let report = self
            .locate(
                core,
                SessionTitleIdentity {
                    source: params.source.clone(),
                    session_id: params.session_id.clone(),
                },
            )
            .await?;
        Ok(planner::resolve(
            &params,
            &report,
            planner::Platform::current(),
        ))
    }
}

impl MethodExtension for SessionLocations {
    fn specs(&self) -> Vec<MethodSpec> {
        vec![
            MethodSpec::of::<SessionsLocation>(Availability::Always),
            MethodSpec::of::<SessionsOpenPlan>(Availability::Always),
        ]
    }

    fn call<'a>(
        &'a self,
        core: &'a Arc<Core>,
        // The bridge admits only this user's processes; no method here varies by caller.
        _context: &'a CallContext,
        method: &'a str,
        raw: Value,
    ) -> BoxFuture<'a, Option<Result<Value, RpcError>>> {
        Box::pin(async move {
            match method {
                SessionsLocation::NAME => Some(
                    answer::<SessionsLocation, _>(raw, |params| self.locate(core, params)).await,
                ),
                SessionsOpenPlan::NAME => Some(
                    answer::<SessionsOpenPlan, _>(raw, |params| self.open_plan(core, params)).await,
                ),
                _ => None,
            }
        })
    }
}

/// Observes the discovered sessions on each session-list change and on a
/// slow timer, for the daemon's life.
pub async fn watch(core: Arc<Core>, locations: Arc<SessionLocations>) {
    let Some(config) = locations.config.clone() else {
        return;
    };
    let mut events = core.hub.subscribe();
    let mut ticks = tokio::time::interval(OBSERVE_EVERY);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = ticks.tick() => {}
            event = events.recv() => match event {
                Ok(event) => {
                    if event.topic != Topic::Invalidated || !["sessions", "agents", "roster"].iter().any(|what| event.data["what"] == *what) {
                        continue;
                    }
                }
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => return,
            },
        }
        // One pass over the list as it stands covers every change so far.
        while events.try_recv().is_ok() {}
        let sessions = discovered_sessions(&core);
        let (config, table) = (config.clone(), core.plugins.processes.clone());
        let _ =
            tokio::task::spawn_blocking(move || observe(&config.0, &config.1, &*table, &sessions))
                .await;
    }
}
