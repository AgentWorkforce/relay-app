//! Provisioning, rotation and the person's decisions
//! (`docs/service-callers.md` section 4). Only the signed-in person's own
//! clients reach these, outside every session and spawned process tree; a
//! probe-initiated provision or rotation waits for the person's decision.

mod provision;
mod rotate;

use super::peer::{self, Initiator};
use super::registry::{self, Binding, RegistryFile, PENDING_PROVISION};
use super::{handoff, inbox, Class, ServiceCallers};
use crate::contract::service_callers::{
    CallerChanged, CallerDecideParams, CallerDecided, CallerEnabledParams, CallerIdParams,
    CallerList, CallerListParams,
};
use agent_relay_node::relay::cloud_session::CloudAccount;
use agent_relay_node::relay::protocol::RpcError;
use base64::Engine;
use std::path::PathBuf;

pub(crate) const DEFAULT_DAYS: u32 = 90;
pub(crate) const MAX_DAYS: u32 = 365;
pub(crate) const DEFAULT_OVERLAP: u32 = 600;
pub(crate) const MAX_OVERLAP: u32 = 86_400;

/// A new secret: `rsc_` and 32 random bytes in base64url.
pub(crate) fn new_secret() -> Option<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).ok()?;
    Some(format!(
        "rsc_{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    ))
}

/// The core's `segment` rule: 1 to 200 of `A-Z a-z 0-9 _ -`.
pub(crate) fn segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn requester(initiator: Initiator) -> String {
    match initiator {
        Initiator::App => "app".into(),
        Initiator::Probe => "probe".into(),
    }
}

/// The path a client is shown.
fn shown(path: &Option<PathBuf>) -> Option<String> {
    path.as_ref().map(|path| path.display().to_string())
}

/// A Relaycast failure, by kind only: its message may echo what was sent.
fn relaycast(error: &RpcError, class: &'static str) -> RpcError {
    RpcError::new(error.data.kind, class)
}

impl ServiceCallers {
    fn manager(
        &self,
        peer_pid: Option<i32>,
        secret: &Option<String>,
        method: &str,
    ) -> Result<(Initiator, Binding), RpcError> {
        let initiator = peer::management(&*self.host, peer_pid, secret.is_some())
            .map_err(|class| self.refused(None, None, method, class))?;
        let binding = Binding::current(&*self.host)
            .map_err(|class| self.refused(None, None, method, class))?;
        Ok((initiator, binding))
    }

    /// Refuses an app-only change from a peer the operating system has not
    /// attested as this app. Reported, never hidden.
    fn app_only(
        &self,
        initiator: Initiator,
        binding: &Binding,
        caller_id: &str,
        method: &str,
    ) -> Result<(), RpcError> {
        if initiator == Initiator::App {
            return Ok(());
        }
        Err(self.refused(
            Some(binding),
            Some(caller_id),
            method,
            Class::AppAttestationUnavailable,
        ))
    }

    fn registry(&self, binding: &Binding, method: &str) -> Result<RegistryFile, RpcError> {
        self.load_current(binding)
            .map_err(|class| self.refused(Some(binding), None, method, class))
    }

    pub(crate) fn list(
        &self,
        peer_pid: Option<i32>,
        connection_id: Option<u64>,
        params: &CallerListParams,
    ) -> Result<CallerList, RpcError> {
        const METHOD: &str = "spawn_callers.list";
        let (_, binding) = self.manager(peer_pid, &params.caller_secret, METHOD)?;
        let _files = self.files();
        let file = self.registry(&binding, METHOD)?;
        drop(
            self.management_gate(connection_id)
                .map_err(|class| self.refused(Some(&binding), None, METHOD, class))?,
        );
        let now = self.host.now_ms();
        Ok(CallerList {
            callers: file.callers.iter().map(|c| c.summary(now)).collect(),
        })
    }

    pub(crate) fn set_enabled(
        &self,
        peer_pid: Option<i32>,
        connection_id: Option<u64>,
        params: &CallerEnabledParams,
    ) -> Result<CallerChanged, RpcError> {
        const METHOD: &str = "spawn_callers.set_enabled";
        let (initiator, binding) = self.manager(peer_pid, &params.caller_secret, METHOD)?;
        self.app_only(initiator, &binding, &params.caller_id, METHOD)?;
        let refuse = |class| self.refused(Some(&binding), Some(&params.caller_id), METHOD, class);
        let _files = self.files();
        let mut file = self.registry(&binding, METHOD)?;
        let index = file
            .by_id(&params.caller_id)
            .ok_or_else(|| refuse(Class::NotFound))?;
        if file.callers[index].pending_provision() {
            return Err(refuse(Class::InvalidInput));
        }
        file.callers[index].enabled = params.enabled;
        file.callers[index].authority_generation += 1;
        let live = self.management_gate(connection_id).map_err(refuse)?;
        if let Err(class) = registry::save(&binding, &file) {
            return Err(refuse(class));
        }
        drop(live);
        self.stale_caller(&params.caller_id);
        let outcome = if params.enabled {
            "enabled"
        } else {
            "disabled"
        };
        self.audit(&binding, Some(&params.caller_id), METHOD, outcome, None);
        Ok(CallerChanged {
            caller: file.callers[index].summary(self.host.now_ms()),
        })
    }

    pub(crate) async fn revoke(
        &self,
        peer_pid: Option<i32>,
        connection_id: Option<u64>,
        params: &CallerIdParams,
    ) -> Result<(), RpcError> {
        const METHOD: &str = "spawn_callers.revoke";
        let (initiator, binding) = self.manager(peer_pid, &params.caller_secret, METHOD)?;
        self.app_only(initiator, &binding, &params.caller_id, METHOD)?;
        let refuse = |class| self.refused(Some(&binding), Some(&params.caller_id), METHOD, class);
        let removed = {
            let _files = self.files();
            let mut file = self.registry(&binding, METHOD)?;
            let index = file
                .by_id(&params.caller_id)
                .ok_or_else(|| refuse(Class::NotFound))?;
            let removed = file.callers.remove(index);
            let live = self.management_gate(connection_id).map_err(refuse)?;
            registry::save(&binding, &file).map_err(refuse)?;
            drop(live);
            handoff::remove_kept(&removed);
            inbox::remove(&binding, &removed.caller_id);
            self.stale_caller(&removed.caller_id);
            self.drop_live(&removed.caller_id);
            removed
        };
        self.audit(&binding, Some(&params.caller_id), METHOD, "revoked", None);
        self.changed.notify_waiters();
        if removed.token.is_some() {
            let _ = self
                .host
                .delete_agent(&binding.workspace, &removed.name)
                .await;
        }
        Ok(())
    }

    /// The person's decision on a pending provision or rotation, from
    /// Settings. Only a peer attested as this app decides. An approval picks
    /// the protected place for `caller.json` itself and returns it to the
    /// app; a rejected provision leaves no record, file or registration.
    pub(crate) async fn decide(
        &self,
        peer_pid: Option<i32>,
        connection_id: Option<u64>,
        params: &CallerDecideParams,
    ) -> Result<CallerDecided, RpcError> {
        const METHOD: &str = "spawn_callers.decide";
        let (initiator, binding) = self.manager(peer_pid, &params.caller_secret, METHOD)?;
        self.app_only(initiator, &binding, &params.caller_id, METHOD)?;
        let refuse = |class| self.refused(Some(&binding), Some(&params.caller_id), METHOD, class);
        let now = self.host.now_ms();
        {
            let _files = self.files();
            let mut file = self.registry(&binding, METHOD)?;
            let index = file
                .by_id(&params.caller_id)
                .ok_or_else(|| refuse(Class::NotFound))?;
            let Some(pending) = file.callers[index].pending.clone() else {
                return Err(refuse(Class::NotPending));
            };
            if pending.kind == PENDING_PROVISION {
                if !params.approved {
                    let removed = file.callers.remove(index);
                    let _live = self.management_gate(connection_id).map_err(refuse)?;
                    registry::save(&binding, &file).map_err(refuse)?;
                    self.audit(&binding, Some(&params.caller_id), METHOD, "rejected", None);
                    return Ok(CallerDecided {
                        caller: removed.summary(now),
                        handoff_path: None,
                    });
                }
            } else {
                if !params.approved {
                    file.callers[index].pending = None;
                    let _live = self.management_gate(connection_id).map_err(refuse)?;
                    registry::save(&binding, &file).map_err(refuse)?;
                    self.audit(&binding, Some(&params.caller_id), METHOD, "rejected", None);
                    return Ok(CallerDecided {
                        caller: file.callers[index].summary(now),
                        handoff_path: None,
                    });
                }
                // One commit: the registry names the new secret and the
                // pending change clears together, or neither happens and the
                // old secret stays the live one.
                let overlap = pending.overlap_seconds.unwrap_or(DEFAULT_OVERLAP);
                let (caller, _) = self
                    .apply_rotation(&binding, &mut file, index, overlap, None, connection_id)
                    .map_err(refuse)?;
                self.audit(&binding, Some(&params.caller_id), METHOD, "approved", None);
                return Ok(CallerDecided {
                    handoff_path: shown(&caller.handoff_path),
                    caller: caller.summary(self.host.now_ms()),
                });
            }
        }
        let caller = self
            .complete_provision(&binding, &params.caller_id, METHOD, connection_id)
            .await?;
        self.audit(&binding, Some(&params.caller_id), METHOD, "approved", None);
        Ok(CallerDecided {
            handoff_path: shown(&caller.handoff_path),
            caller: caller.summary(self.host.now_ms()),
        })
    }

    /// Sign-out: every registry, inbox, audit and handoff file goes, live
    /// state is cleared, and each caller's Relaycast agent is deleted with
    /// the workspace key, best effort. Late workers still waiting for their
    /// release move to the releases file beside the tree first
    /// ([`super::releases`]), so a cold start still releases each once.
    pub async fn forget(&self, account: &CloudAccount) {
        let Some(app_data) = self.host.app_data_dir() else {
            return;
        };
        let folder = app_data.join(registry::FOLDER);
        let key = registry::key(&account.site_url, &account.workspace_id);
        let workspace = self.host.workspace();
        let names: Vec<String> = {
            let _files = self.files();
            let file = registry::read_private(&folder.join(format!("{key}.json")))
                .ok()
                .flatten()
                .and_then(|bytes| serde_json::from_slice::<RegistryFile>(&bytes).ok());
            let mut pending = super::spawn::pending_of(&self.live().late, &key);
            if let (Some(file), Some(workspace)) = (&file, &workspace) {
                if workspace.workspace_id.as_deref() == Some(file.relaycast_workspace_id.as_str()) {
                    let binding = registry::Binding {
                        key: key.clone(),
                        site_url: file.site_url.clone(),
                        workspace_id: file.workspace_id.clone(),
                        user_id: file.user_id.clone(),
                        workspace: workspace.clone(),
                        relaycast_workspace_id: file.relaycast_workspace_id.clone(),
                        folder: folder.clone(),
                    };
                    pending.extend(file.late_workers.iter().map(|late| {
                        super::releases::Release::new(
                            &binding,
                            &late.invocation_id,
                            &late.caller_id,
                            late.name.clone(),
                            late.agent_id.clone(),
                            late.since_ms,
                        )
                    }));
                }
            }
            if !pending.is_empty() {
                super::releases::keep(&app_data.join(super::releases::FILE), pending);
            }
            let names = file
                .map(|file| {
                    for caller in &file.callers {
                        handoff::remove_kept(caller);
                    }
                    file.callers
                        .into_iter()
                        .filter(|caller| caller.token.is_some())
                        .map(|caller| caller.name)
                        .collect()
                })
                .unwrap_or_default();
            let _ = std::fs::remove_dir_all(&folder);
            registry::forget_cached(&folder);
            {
                // Bindings go stale but connections stay known, so none of
                // them resolves untouched again (`docs/service-callers-auth.md`
                // section 4).
                let mut live = self.live();
                self.stale_all(&mut live);
                live.iterators.clear();
                live.leases.clear();
                live.invokes.clear();
            }
            names
        };
        self.changed.notify_waiters();
        if let Some(workspace) = workspace {
            for name in names {
                let _ = self.host.delete_agent(&workspace, &name).await;
            }
        }
    }
}
