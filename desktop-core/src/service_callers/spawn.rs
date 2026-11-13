//! The core's spawn caller plugin (`docs/service-callers.md` section 6) and
//! the delivery reader that fills the inbox (section 8).

mod delivery;
mod late;

use late::late_line;
pub(crate) use late::{pending_of, Late};

use super::auth::Resolved;
use super::inbox;
use super::registry::{self, Binding, CallerRecord};
use super::{node, peer, scope, Class, ServiceCallers};
use agent_relay_node::relay::contract::spawn_control::SpawnControlInvocation;
use agent_relay_node::relay::daemon::{
    CallContext, ConnectionCaller, ExternalCaller, Permit, SpawnCall, SpawnCallers, SpawnRequest,
};
use agent_relay_node::relay::protocol::RpcError;

/// Invokes a caller may make in [`RATE_WINDOW_MS`].
pub(crate) const RATE: usize = 60;
pub(crate) const RATE_WINDOW_MS: i64 = 60_000;
/// Live agents a caller's spawns may hold at once.
pub(crate) const LIVE_AGENTS: usize = 32;

/// What `admit` hands to `record`.
struct Admitted {
    /// The account, workspace and files the call was admitted under, so a
    /// permit that outlives a revoke or sign-out still knows them.
    binding: Binding,
    caller_id: String,
    generation: u64,
    method: &'static str,
    /// An invoke's reserved result slot.
    reservation: Option<Reservation>,
    /// The worker the call was admitted for: a release's
    /// `expected_agent_id`, once admission found the caller owns it. The
    /// core's release receipt names no agent, so the ledger takes it from
    /// here.
    worker: Option<String>,
}

/// How many admitted permits hold each reserved slot, by registry key,
/// caller id and idempotency key. Duplicate invokes of one key share its
/// slot, so it is released only when the last of them ends unbound. Kept
/// under the files lock; never persisted.
pub(crate) type Claims = std::collections::HashMap<(String, String, String), usize>;

/// Releases a failed inbox write left on disk, by registry key and caller
/// id to idempotency keys. Read and written under the files lock and
/// written at the caller's next inbox load, record or reservation, so the
/// capacity comes back then and not after [`inbox::UNBOUND_MS`]. Never
/// persisted: after a restart the purge reclaims the slot.
pub(crate) type Releases =
    std::collections::HashMap<(String, String), std::collections::BTreeSet<String>>;

/// Takes the releases pending for `caller_id`.
fn take_releases(
    releases: &std::sync::Mutex<Releases>,
    binding: &Binding,
    caller_id: &str,
) -> std::collections::BTreeSet<String> {
    releases
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&(binding.key.clone(), caller_id.to_string()))
        .unwrap_or_default()
}

/// Puts `keys` back as pending for `caller_id`: their write failed.
fn keep_releases(
    releases: &std::sync::Mutex<Releases>,
    binding: &Binding,
    caller_id: &str,
    keys: impl IntoIterator<Item = String>,
) {
    let mut releases = releases
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let entry = releases
        .entry((binding.key.clone(), caller_id.to_string()))
        .or_default();
    entry.extend(keys);
    if entry.is_empty() {
        releases.remove(&(binding.key.clone(), caller_id.to_string()));
    }
}

/// An invoke's reserved result slot, held by its permit. `record` binds or
/// releases it; a permit dropped without `record` (a cancelled handler)
/// releases it at once instead of leaving it for [`inbox::UNBOUND_MS`].
struct Reservation {
    files: std::sync::Arc<std::sync::Mutex<Claims>>,
    releases: std::sync::Arc<std::sync::Mutex<Releases>>,
    host: std::sync::Arc<dyn super::Host>,
    binding: Binding,
    caller_id: String,
    key: String,
    /// Cleared once the permit's claim has ended.
    armed: bool,
}

impl Reservation {
    fn claim(binding: &Binding, caller_id: &str, key: &str) -> (String, String, String) {
        (binding.key.clone(), caller_id.to_string(), key.to_string())
    }

    /// Ends this permit's claim, under the files lock: the slot is bound to
    /// `invocation_id`, or released once no other admitted permit of the
    /// same key still claims it. Nothing is written for a caller a revoke
    /// removed, so its inbox file is never recreated.
    fn settle(&mut self, claims: &mut Claims, invocation_id: Option<&str>) -> Result<(), Class> {
        if !self.armed {
            return Ok(());
        }
        self.armed = false;
        let claim = Reservation::claim(&self.binding, &self.caller_id, &self.key);
        let last = match claims.get_mut(&claim) {
            Some(count) if *count > 1 => {
                *count -= 1;
                false
            }
            _ => {
                claims.remove(&claim);
                true
            }
        };
        if invocation_id.is_none() && !last {
            return Ok(());
        }
        let written = self.write(invocation_id);
        if written.is_err() && invocation_id.is_none() {
            // The slot stays reserved on disk; the caller's next inbox
            // load, record or reservation writes the release.
            keep_releases(
                &self.releases,
                &self.binding,
                &self.caller_id,
                [self.key.clone()],
            );
        }
        written
    }

    /// Binds or releases the slot on disk, with the caller's pending
    /// releases written in the same write.
    fn write(&self, invocation_id: Option<&str>) -> Result<(), Class> {
        let gone = registry::load(&self.binding)?
            .by_id(&self.caller_id)
            .is_none();
        let existing = if gone {
            None
        } else {
            inbox::load_existing(&self.binding, &self.caller_id)?
        };
        let pending = take_releases(&self.releases, &self.binding, &self.caller_id);
        // A revoked caller's slots went with its inbox file.
        let Some(mut file) = existing else {
            return Ok(());
        };
        for key in &pending {
            file.unreserve(key);
        }
        match invocation_id {
            Some(invocation_id) => file.bind(&self.key, invocation_id),
            None => file.unreserve(&self.key),
        }
        let saved = inbox::save(&self.binding, &self.caller_id, &file);
        if saved.is_err() {
            keep_releases(&self.releases, &self.binding, &self.caller_id, pending);
        }
        saved
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let files = self.files.clone();
        let mut claims = files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = self.settle(&mut claims, None);
        // A permit dropped after its caller was revoked or signed out still
        // ends with one terminal line; no worker id is known to release.
        if caller_gone(&self.binding, &self.caller_id) {
            late_line(
                &*self.host,
                &self.binding,
                &self.caller_id,
                "spawn.invoke",
                "dropped",
            );
        }
    }
}

/// Whether `caller_id` is gone from `binding`'s registry: revoked, or its
/// whole tree deleted by a sign-out. Under the files lock.
fn caller_gone(binding: &Binding, caller_id: &str) -> bool {
    matches!(registry::load(binding), Ok(file) if file.by_id(caller_id).is_none())
}

/// Whether `binding` is still the signed-in account's and the core's
/// workspace's: the same registry, person and Relaycast workspace.
fn still_bound(host: &dyn super::Host, binding: &Binding) -> bool {
    Binding::current(host).is_ok_and(|current| {
        current.key == binding.key
            && current.user_id == binding.user_id
            && current.folder == binding.folder
            && binding.is(&current.workspace)
    })
}

fn method_of(request: &SpawnRequest<'_>) -> &'static str {
    match request {
        SpawnRequest::Invoke(_) => "spawn.invoke",
        SpawnRequest::Status(_) => "spawn.status",
        SpawnRequest::Release(_) => "spawn.release",
    }
}

/// Why a spawn call was not admitted, with what the audit line can name.
type Refusal = (Option<Box<Binding>>, Option<String>, Class);

impl ServiceCallers {
    /// The record `caller` is, live, in the current binding's registry.
    fn admitted_record(
        &self,
        binding: &Binding,
        caller: &ExternalCaller,
    ) -> Result<CallerRecord, Class> {
        let file = self.load_current(binding)?;
        let record = file
            .callers
            .into_iter()
            .find(|record| !record.agent_id.is_empty() && record.agent_id == caller.agent_id)
            .ok_or(Class::CallerUnknown)?;
        record.live(self.host.now_ms())?;
        Ok(record)
    }

    fn admit_invoke(
        &self,
        binding: &Binding,
        record: &CallerRecord,
        params: &agent_relay_node::relay::contract::spawn_control::SpawnInvokeParams,
    ) -> Result<(), Class> {
        let node = node::enrolled(&*self.host, &binding.workspace)?;
        if params.node_id != node.id || params.node_name != node.name {
            return Err(Class::NodeOther);
        }
        if !record.harnesses.contains(&params.harness) {
            return Err(Class::HarnessOutside);
        }
        let app_data = binding.folder.parent().unwrap_or(&binding.folder);
        scope::directory(&params.directory, &record.directory_roots, app_data)?;
        let now = self.host.now_ms();
        let live_agents = self
            .host
            .spawned(&binding.workspace)
            .iter()
            .filter(|spawned| {
                spawned.active && spawned.requester_id.as_deref() == Some(record.agent_id.as_str())
            })
            .count();
        if live_agents >= LIVE_AGENTS {
            return Err(Class::LiveAgentCap);
        }
        {
            let mut live = self.live();
            let times = live.invokes.entry(record.caller_id.clone()).or_default();
            while times.front().is_some_and(|at| at + RATE_WINDOW_MS <= now) {
                times.pop_front();
            }
            if times.len() >= RATE {
                return Err(Class::RateLimited);
            }
        }
        let mut file = self.load_inbox(binding, &record.caller_id)?;
        file.purge(now);
        file.reserve(&params.idempotency_key, now)?;
        inbox::save(binding, &record.caller_id, &file)?;
        self.live()
            .invokes
            .entry(record.caller_id.clone())
            .or_default()
            .push_back(now);
        Ok(())
    }

    fn admit_release(
        &self,
        binding: &Binding,
        record: &CallerRecord,
        expected: &str,
    ) -> Result<(), Class> {
        let in_ledger = record
            .invocations
            .iter()
            .any(|entry| entry.worker_agent_id.as_deref() == Some(expected));
        let spawned = self.host.spawned(&binding.workspace);
        let worker = spawned.iter().find(|spawned| spawned.agent_id == expected);
        let spawned_by_it = worker.is_some_and(|spawned| {
            spawned.requester_id.as_deref() == Some(record.agent_id.as_str())
                && record.owns(&spawned.invocation_id)
        });
        if in_ledger || spawned_by_it {
            Ok(())
        } else if worker.is_some() {
            // A live worker someone else asked for.
            Err(Class::RequesterOther)
        } else {
            Err(Class::NotInLedger)
        }
    }

    fn admit_call(
        &self,
        caller: &ExternalCaller,
        call: &SpawnCall<'_>,
    ) -> Result<Admitted, Refusal> {
        peer::caller(&*self.host, call.peer_pid).map_err(|class| (None, None, class))?;
        let binding = Binding::current(&*self.host).map_err(|class| (None, None, class))?;
        if !binding.is(call.workspace) {
            return Err((Some(Box::new(binding)), None, Class::WorkspaceMismatch));
        }
        let generation = self.host.generation();
        let mut claims = self.files();
        let record = match self.admitted_record(&binding, caller) {
            Ok(record) => record,
            Err(class) => return Err((Some(Box::new(binding)), None, class)),
        };
        let caller_id = record.caller_id.clone();
        let fail = |class| {
            (
                Some(Box::new(binding.clone())),
                Some(caller_id.clone()),
                class,
            )
        };
        let (reservation, worker) = match call.request {
            SpawnRequest::Invoke(params) => {
                self.admit_invoke(&binding, &record, params).map_err(fail)?;
                let claim = Reservation::claim(&binding, &caller_id, &params.idempotency_key);
                *claims.entry(claim).or_default() += 1;
                let reservation = Reservation {
                    files: self.files.clone(),
                    releases: self.releases.clone(),
                    host: self.host.clone(),
                    binding: binding.clone(),
                    caller_id: caller_id.clone(),
                    key: params.idempotency_key.clone(),
                    armed: true,
                };
                (Some(reservation), None)
            }
            SpawnRequest::Status(params) => {
                if !record.owns(&params.invocation_id) {
                    return Err(fail(Class::NotInLedger));
                }
                (None, None)
            }
            SpawnRequest::Release(params) => {
                self.admit_release(&binding, &record, &params.expected_agent_id)
                    .map_err(fail)?;
                (None, Some(params.expected_agent_id.clone()))
            }
        };
        Ok(Admitted {
            binding: binding.clone(),
            caller_id,
            generation,
            method: method_of(&call.request),
            reservation,
            worker,
        })
    }
}

impl SpawnCallers for ServiceCallers {
    /// Resolves the calling connection from memory, without file I/O
    /// ([`ServiceCallers::bound_external`]).
    fn connection(&self, context: &CallContext) -> ConnectionCaller {
        const METHOD: &str = "spawn.connection";
        let Some(connection_id) = context.connection_id else {
            return ConnectionCaller::NotExternal;
        };
        let binding_id = match self.peek(connection_id) {
            Resolved::NotExternal => return ConnectionCaller::NotExternal,
            Resolved::Refused(class) => {
                let binding = Binding::current(&*self.host).ok();
                return ConnectionCaller::Refused(self.refused(
                    binding.as_ref(),
                    None,
                    METHOD,
                    class,
                ));
            }
            Resolved::Bound(binding_id) => binding_id,
        };
        match self.bound_external(connection_id, &binding_id) {
            Ok(caller) => ConnectionCaller::Bound(caller),
            Err((binding, caller_id, class)) => ConnectionCaller::Refused(self.refused(
                binding.as_deref(),
                caller_id.as_deref(),
                METHOD,
                class,
            )),
        }
    }

    fn admit(&self, caller: &ExternalCaller, call: &SpawnCall<'_>) -> Result<Permit, RpcError> {
        let method = method_of(&call.request);
        match self.admit_call(caller, call) {
            Ok(admitted) => Ok(Permit::new(admitted)),
            Err((binding, caller_id, class)) => {
                Err(self.refused(binding.as_deref(), caller_id.as_deref(), method, class))
            }
        }
    }

    fn record(
        &self,
        permit: Permit,
        outcome: &Result<SpawnControlInvocation, RpcError>,
    ) -> Result<(), RpcError> {
        let Ok(mut admitted) = permit.downcast::<Admitted>() else {
            return Ok(());
        };
        let binding = admitted.binding.clone();
        let caller_id = admitted.caller_id.clone();
        let caller_id = caller_id.as_str();
        let mut claims = self.files();
        if caller_gone(&binding, caller_id) {
            // Revoked or signed out while the call was in flight: its claim
            // ends without a write, and what it started is cleaned up once.
            if let Some(reservation) = admitted.reservation.as_mut() {
                let _ = reservation.settle(&mut claims, None);
            }
            self.late(&binding, caller_id, admitted.method, outcome, &claims);
            return Ok(());
        }
        // The identity generation moved while the call was in flight. What
        // it started is never left unowned: while the binding it was
        // admitted under is still the account's, its outcome is recorded
        // as usual, so the ledger owns the worker and the receipt stands
        // (the caller binds again to reach it); once the account or
        // workspace is another, the worker is released exactly once as a
        // late one and the caller gets `generation_moved`, not a receipt.
        let moved = self.host.generation() != admitted.generation;
        if moved && !still_bound(&*self.host, &binding) {
            if let Some(reservation) = admitted.reservation.as_mut() {
                let _ = reservation.settle(&mut claims, None);
            }
            self.late(&binding, caller_id, admitted.method, outcome, &claims);
            drop(claims);
            return match outcome {
                Ok(_) if admitted.method == "spawn.invoke" => Err(Class::GenerationMoved.error()),
                _ => Ok(()),
            };
        }
        let stored = self.record_outcome(&binding, &mut admitted, outcome, &mut claims);
        drop(claims);
        let outcome_class = match (outcome, stored) {
            (_, Err(class)) => class.name(),
            (_, Ok(())) if moved => Class::GenerationMoved.name(),
            (Ok(_), Ok(())) => "ok",
            (Err(_), Ok(())) => "failed",
        };
        self.audit(
            &binding,
            Some(caller_id),
            admitted.method,
            outcome_class,
            None,
        );
        // The receipt is gated on the write: a success whose record did not
        // reach the disk answers a retryable `store_failed`, and the same
        // request again replays the invocation and recovers its receipt.
        match (outcome, stored) {
            (Ok(_), Err(_)) => Err(Class::StoreFailed.error()),
            _ => Ok(()),
        }
    }
}

impl ServiceCallers {
    /// The external caller bound on `connection_id` as `binding_id`,
    /// rechecked against its record, the identity generation and the
    /// current scope (`check_bound`), or why it is not usable.
    ///
    /// From memory only, so the core's `connection` never blocks on I/O:
    /// the live binding and the registry as this process last read or wrote
    /// it ([`registry::cached`]). Every write (provision, rotation,
    /// revocation, enable or disable, a decision, an expiry or retirement
    /// advance, a rollback put back) goes through [`registry::save`], which
    /// refreshes that copy before the writer returns, so a retired secret or
    /// an expired, revoked or disabled caller is refused on the next call.
    /// An advance the clock makes due but no write has recorded yet (a
    /// digest past its retirement, an expiry not yet marked) holds no
    /// longer either: the binding goes stale exactly as the write would make
    /// it, and the write itself is left to the next check that holds the
    /// files lock (the periodic sweep at the latest). A rotation still
    /// marked for rollback is refused `rotation_rollback_pending` until a
    /// check settles it, without touching the binding.
    fn bound_external(
        &self,
        connection_id: u64,
        binding_id: &str,
    ) -> Result<ExternalCaller, Refusal> {
        peer::available().map_err(|class| (None, None, class))?;
        let binding = Binding::current(&*self.host).map_err(|class| (None, None, class))?;
        let Some(caller_id) = self.bound_caller(connection_id, binding_id) else {
            let class = self
                .bound_gone(connection_id, binding_id)
                .unwrap_or(Class::BindingStale);
            return Err((Some(Box::new(binding)), None, class));
        };
        let fail = |class| {
            (
                Some(Box::new(binding.clone())),
                Some(caller_id.clone()),
                class,
            )
        };
        let file = registry::cached(&binding).transpose().map_err(fail)?;
        let now = self.host.now_ms();
        let record = file
            .as_deref()
            .and_then(|file| file.by_id(&caller_id).map(|index| &file.callers[index]));
        if record.is_some_and(|record| record.rollback.is_some()) {
            return Err(fail(Class::RotationRollbackPending));
        }
        // What `load_current` would advance, and so stale, on its next run.
        let current = record.filter(|record| {
            record.pending_provision()
                || (record.secrets.iter().all(|digest| digest.live(now))
                    && !(record.expires_at_ms <= now && !record.expiry_advanced))
        });
        {
            let mut live = self.live();
            self.check_bound(
                &mut live,
                connection_id,
                binding_id,
                current,
                Some(&binding),
            )
            .map_err(fail)?;
        }
        current
            .and_then(CallerRecord::external)
            .ok_or_else(|| fail(Class::CallerUnknown))
    }
}
