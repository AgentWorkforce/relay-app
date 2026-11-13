//! Calls whose caller ended while they were in flight
//! (`docs/service-callers.md` section 6): the one terminal line each gets,
//! and the exact release of a worker an invoke started, kept across a
//! restart until it is done.

use super::Claims;
use crate::service_callers::inbox;
use crate::service_callers::registry::{self, Binding};
use crate::service_callers::{releases, Class, ServiceCallers};
use agent_relay_node::relay::contract::spawn_control::SpawnControlInvocation;
use agent_relay_node::relay::protocol::RpcError;

/// The one terminal line for a call whose caller ended while it was in
/// flight, under the files lock: in the audit while the binding's registry
/// still exists (a revoke keeps it), and in the host log once a sign-out
/// deleted the tree, so the audit never recreates it.
pub(super) fn late_line(
    host: &dyn crate::service_callers::Host,
    binding: &Binding,
    caller_id: &str,
    method: &str,
    outcome: &str,
) {
    if std::fs::symlink_metadata(binding.registry()).is_ok() {
        if !crate::service_callers::audit::write(
            binding,
            host.now_ms(),
            Some(caller_id),
            method,
            outcome,
            None,
        ) {
            host.log(&format!("{method}: the audit could not be written"));
        }
    } else {
        host.log(&format!("{method}: {outcome} after its caller ended"));
    }
}

/// How long a late worker without a known agent id is looked for, and how
/// long a finished cleanup is remembered so a duplicate permit of the same
/// invocation never releases it again.
const LATE_LOOKUP_MS: i64 = inbox::UNBOUND_MS;
const LATE_REMEMBER_MS: i64 = 24 * 60 * 60 * 1000;
/// Release attempts before a late worker is reported as not released.
const LATE_ATTEMPTS: u32 = 5;

/// A worker an invoke started after its caller was revoked or signed out.
/// No caller entry names it any more, so it is released exactly once from
/// here (`docs/service-callers.md` section 3, in-flight calls).
#[derive(Clone)]
pub(crate) struct LateWorker {
    binding: Binding,
    caller_id: String,
    name: Option<String>,
    agent_id: Option<String>,
    since_ms: i64,
    attempts: u32,
    /// A release of it is in flight.
    busy: bool,
}

/// Late workers waiting for their one release, and the invocations already
/// cleaned up. Until its terminal line, a worker of a revoked caller is
/// also kept in the registry the revoke left
/// ([`registry::RegistryFile::late_workers`]); after a sign-out the tree
/// stays absent and the worker is kept in the releases file beside it
/// ([`super::releases`]). Either way a restart still releases it.
#[derive(Default)]
pub(crate) struct Late {
    pending: std::collections::HashMap<String, LateWorker>,
    done: std::collections::HashMap<String, i64>,
    /// Registry keys whose kept late workers this process has taken up.
    adopted: std::collections::HashSet<String>,
    /// Whether this process has taken up the releases file.
    adopted_releases: bool,
}

impl LateWorker {
    fn release(&self, invocation_id: &str) -> releases::Release {
        releases::Release::new(
            &self.binding,
            invocation_id,
            &self.caller_id,
            self.name.clone(),
            self.agent_id.clone(),
            self.since_ms,
        )
    }
}

/// The late workers of `key` this process still holds, as releases to
/// keep across a sign-out.
pub(crate) fn pending_of(late: &Late, key: &str) -> Vec<releases::Release> {
    late.pending
        .iter()
        .filter(|(_, worker)| worker.binding.key == key)
        .map(|(id, worker)| worker.release(id))
        .collect()
}

/// Keeps `worker` of `invocation_id` in `binding`'s registry, when the
/// registry is still there (a revoke keeps it), and otherwise in the
/// releases file beside the tree (a sign-out deleted it, and nothing
/// recreates it). Under the files lock.
fn keep_late(binding: &Binding, invocation_id: &str, worker: &LateWorker) {
    if std::fs::symlink_metadata(binding.registry()).is_err() {
        if let Some(path) = releases::path_of(binding) {
            releases::keep(&path, [worker.release(invocation_id)]);
        }
        return;
    }
    let Ok(mut file) = registry::load(binding) else {
        return;
    };
    if file
        .late_workers
        .iter()
        .any(|late| late.invocation_id == invocation_id)
    {
        return;
    }
    file.late_workers.push(registry::LateEntry {
        invocation_id: invocation_id.to_string(),
        caller_id: worker.caller_id.clone(),
        name: worker.name.clone(),
        agent_id: worker.agent_id.clone(),
        since_ms: worker.since_ms,
    });
    let _ = registry::save(binding, &file);
}

/// Drops `invocation_id` from the late workers kept in `binding`'s
/// registry and in the releases file: its terminal line is written. Under
/// the files lock.
fn forget_late(binding: &Binding, invocation_id: &str) {
    if let Some(path) = releases::path_of(binding) {
        releases::forget(&path, invocation_id);
    }
    if std::fs::symlink_metadata(binding.registry()).is_err() {
        return;
    }
    let Ok(mut file) = registry::load(binding) else {
        return;
    };
    let kept = file.late_workers.len();
    file.late_workers
        .retain(|late| late.invocation_id != invocation_id);
    if file.late_workers.len() != kept {
        let _ = registry::save(binding, &file);
    }
}

impl ServiceCallers {
    /// The end of a call whose caller is gone, under the files lock
    /// (`_claims`). A worker an invoke started is queued for one exact
    /// release by [`ServiceCallers::release_late`], which writes its one
    /// terminal line; anything else gets its terminal line now.
    pub(super) fn late(
        &self,
        binding: &Binding,
        caller_id: &str,
        method: &'static str,
        outcome: &Result<SpawnControlInvocation, RpcError>,
        _claims: &Claims,
    ) {
        let outcome = match outcome {
            Ok(invocation) if method == "spawn.invoke" && !invocation.released => {
                let mut live = self.live();
                let known = live.late.pending.contains_key(&invocation.id)
                    || live.late.done.contains_key(&invocation.id);
                if !known {
                    let worker = LateWorker {
                        binding: binding.clone(),
                        caller_id: caller_id.to_string(),
                        name: invocation.name.clone(),
                        agent_id: invocation.agent_id.clone(),
                        since_ms: self.host.now_ms(),
                        attempts: 0,
                        busy: false,
                    };
                    live.late
                        .pending
                        .insert(invocation.id.clone(), worker.clone());
                    drop(live);
                    keep_late(binding, &invocation.id, &worker);
                }
                return;
            }
            Ok(_) => Class::CallerUnknown.name(),
            Err(_) => "failed",
        };
        late_line(&*self.host, binding, caller_id, method, outcome);
    }

    /// Releases each late worker exactly once, with the workspace key the
    /// call was admitted under, and writes its one terminal line. A worker
    /// whose agent id the invoke did not report is looked up by its exact
    /// invocation id among the core's spawns for [`LATE_LOOKUP_MS`]; a
    /// failed release is retried on later ticks up to [`LATE_ATTEMPTS`].
    pub(crate) async fn release_late(&self) {
        self.adopt_late();
        let now = self.host.now_ms();
        let taken: Vec<(String, LateWorker)> = {
            let mut live = self.live();
            live.late.done.retain(|_, at| *at + LATE_REMEMBER_MS > now);
            live.late
                .pending
                .iter_mut()
                .filter(|(_, worker)| !worker.busy)
                .map(|(id, worker)| {
                    worker.busy = true;
                    (id.clone(), worker.clone())
                })
                .collect()
        };
        for (invocation_id, mut worker) in taken {
            if worker.agent_id.is_none() || worker.name.is_none() {
                if let Some(spawned) = self
                    .host
                    .spawned(&worker.binding.workspace)
                    .into_iter()
                    .find(|spawned| spawned.invocation_id == invocation_id)
                {
                    worker.agent_id = Some(spawned.agent_id);
                    worker.name = Some(spawned.name);
                }
            }
            let terminal = match (worker.name.as_deref(), worker.agent_id.as_deref()) {
                (Some(name), Some(agent_id)) => {
                    let key = format!("service-caller-late-{invocation_id}");
                    match self
                        .host
                        .release_worker(&worker.binding.workspace, name, agent_id, &key)
                        .await
                    {
                        Ok(()) => Some("late_worker_released"),
                        Err(_) => {
                            worker.attempts += 1;
                            (worker.attempts >= LATE_ATTEMPTS)
                                .then_some("late_worker_release_failed")
                        }
                    }
                }
                _ => (now >= worker.since_ms + LATE_LOOKUP_MS).then_some("late_worker_unknown"),
            };
            let _files = self.files();
            let mut live = self.live();
            match terminal {
                Some(outcome) => {
                    live.late.pending.remove(&invocation_id);
                    live.late.done.insert(invocation_id.clone(), now);
                    drop(live);
                    forget_late(&worker.binding, &invocation_id);
                    late_line(
                        &*self.host,
                        &worker.binding,
                        &worker.caller_id,
                        "spawn.invoke",
                        outcome,
                    );
                }
                None => {
                    worker.busy = false;
                    live.late.pending.insert(invocation_id, worker);
                }
            }
        }
    }

    /// Takes up, once per process, the releases a sign-out left pending
    /// before a restart, each with the workspace key it was admitted
    /// under, signed in or not.
    fn adopt_releases(&self) {
        if self.live().late.adopted_releases {
            return;
        }
        let Some(app_data) = self.host.app_data_dir() else {
            return;
        };
        let _files = self.files();
        let kept = releases::load(&app_data.join(releases::FILE));
        let mut live = self.live();
        live.late.adopted_releases = true;
        for release in kept {
            if live.late.pending.contains_key(&release.invocation_id)
                || live.late.done.contains_key(&release.invocation_id)
            {
                continue;
            }
            let binding = release.binding(&app_data);
            live.late.pending.insert(
                release.invocation_id,
                LateWorker {
                    binding,
                    caller_id: release.caller_id,
                    name: release.name,
                    agent_id: release.agent_id,
                    since_ms: release.since_ms,
                    attempts: 0,
                    busy: false,
                },
            );
        }
    }

    /// Takes up the late workers kept in the current binding's registry,
    /// once per process: those a revoke left pending before a restart.
    /// Each is released with the current binding's workspace key, which is
    /// the key of the workspace the registry belongs to.
    fn adopt_late(&self) {
        self.adopt_releases();
        let Ok(binding) = Binding::current(&*self.host) else {
            return;
        };
        if self.live().late.adopted.contains(&binding.key) {
            return;
        }
        let _files = self.files();
        let kept = if std::fs::symlink_metadata(binding.registry()).is_ok() {
            match registry::load(&binding) {
                Ok(file) => file.late_workers,
                Err(_) => return,
            }
        } else {
            Vec::new()
        };
        let mut live = self.live();
        live.late.adopted.insert(binding.key.clone());
        for entry in kept {
            if live.late.pending.contains_key(&entry.invocation_id)
                || live.late.done.contains_key(&entry.invocation_id)
            {
                continue;
            }
            live.late.pending.insert(
                entry.invocation_id,
                LateWorker {
                    binding: binding.clone(),
                    caller_id: entry.caller_id,
                    name: entry.name,
                    agent_id: entry.agent_id,
                    since_ms: entry.since_ms,
                    attempts: 0,
                    busy: false,
                },
            );
        }
    }
}
