//! Connection-bound challenge authentication
//! (`docs/service-callers-auth.md`): a caller's runtime proves once per
//! bridge connection that it holds the `caller.json` secret, by answering a
//! fresh challenge with an HMAC-SHA256 over a canonical transcript. The
//! binding that results lives only in [`Live::connections`], keyed by the
//! core's server-only connection id; later calls on that connection carry
//! no credential.
//!
//! The secret, its key and every digest stay in this process: no log, audit
//! line, error or result carries them.

mod handshake;
mod lifecycle;
mod transcript;

pub(crate) use transcript::{
    has_legacy_keys, hmac, is_caller_id, key_of, parse_authenticate, Transcript,
};
#[cfg(test)]
pub(crate) use transcript::{is_challenge_id, HMAC_CALLS};

use super::registry::{Binding, CallerRecord};
use super::{Class, Live, ServiceCallers};
use std::sync::MutexGuard;

/// How long a challenge can be answered (section 2): `expires_at_ms` is
/// `issued_at_ms` plus this.
pub(crate) const TTL_MS: i64 = 30_000;
/// Attempts that may end without a binding on one connection before it is
/// locked (section 4).
pub(crate) const MAX_ATTEMPTS: u32 = 3;
/// Outstanding challenges per `caller_id`, across connections.
pub(crate) const PER_CALLER: usize = 8;
/// Outstanding challenges in total.
pub(crate) const TOTAL: usize = 256;
/// The largest `spawn_callers.authenticate` params accepted, in bytes of
/// their compact JSON. A well-formed body is 116 bytes; anything larger is
/// `invalid_input` and consumes the connection's challenge.
pub(crate) const AUTH_PARAMS_MAX_BYTES: usize = 256;
/// The transcript's domain-separation string (section 3).
pub(crate) const DOMAIN: &[u8] = b"agent-relay/service-caller/v1";
/// The transcript version byte, and `CallerChallenge.version`.
pub(crate) const VERSION: u8 = 1;
/// The longest `site_url` a challenge is issued for, in bytes.
const SITE_URL_MAX: usize = 2048;
/// The longest `workspace_id` or `user_id`, in bytes.
const ID_MAX: usize = 256;
/// The key used where a record has fewer than two digests, so every
/// `authenticate` does the same MAC work.
const DUMMY_KEY: [u8; 32] = [0x5a; 32];

/// A challenge held in its connection's slot.
pub(crate) struct Slot {
    pub(crate) challenge_id: String,
    pub(crate) nonce: [u8; 32],
    pub(crate) caller_id: String,
    pub(crate) site_url: String,
    pub(crate) workspace_id: String,
    pub(crate) user_id: String,
    /// The registry key of the scope it was issued in.
    pub(crate) key: String,
    pub(crate) issued_ms: i64,
    pub(crate) expires_ms: i64,
}

impl Slot {
    fn expired(&self, now: i64) -> bool {
        now >= self.expires_ms
    }

    pub(crate) fn transcript(&self) -> Vec<u8> {
        Transcript {
            version: VERSION,
            domain: DOMAIN,
            challenge_id: self.challenge_id.as_bytes(),
            nonce: &self.nonce,
            caller_id: self.caller_id.as_bytes(),
            site_url: self.site_url.as_bytes(),
            workspace_id: self.workspace_id.as_bytes(),
            user_id: self.user_id.as_bytes(),
            issued_at_ms: self.issued_ms as u64,
            expires_at_ms: self.expires_ms as u64,
        }
        .bytes()
    }

    /// A slot of the same shape and length as a real one, for an
    /// `authenticate` that found none.
    fn dummy(binding: Option<&Binding>, now: i64) -> Slot {
        let scope = |pick: fn(&Binding) -> &String| binding.map(pick).cloned().unwrap_or_default();
        Slot {
            challenge_id: format!("ch_{}", "0".repeat(32)),
            nonce: [0; 32],
            caller_id: format!("sc_{}", "0".repeat(16)),
            site_url: scope(|b| &b.site_url),
            workspace_id: scope(|b| &b.workspace_id),
            user_id: scope(|b| &b.user_id),
            key: String::new(),
            issued_ms: now,
            expires_ms: now,
        }
    }
}

/// A connection's binding to one caller (section 4).
pub(crate) struct Bound {
    pub(crate) binding_id: String,
    pub(crate) caller_id: String,
    pub(crate) authority_generation: u64,
    /// `Host::generation()` when it was made.
    pub(crate) identity_generation: u64,
    /// The registry key of the scope it was made in.
    pub(crate) binding_key: String,
    /// When the digest it was proven against retires, if it does.
    pub(crate) retires_at_ms: Option<i64>,
}

pub(crate) enum State {
    Untouched,
    Challenged { slot: Option<Slot>, attempts: u32 },
    Bound(Bound),
    Stale,
    Locked,
    Closed,
}

impl State {
    /// Whether the connection has become external: anything but untouched.
    fn external(&self) -> bool {
        !matches!(self, State::Untouched)
    }
}

/// How many closed connection ids are remembered. The core never reuses
/// an id; past this many, the oldest falls back to `caller_unbound`, which
/// still fails closed.
pub(crate) const CLOSED_REMEMBERED: usize = 4096;

/// The most recent [`CLOSED_REMEMBERED`] connection ids the core reported
/// closed.
#[derive(Default)]
pub(crate) struct ClosedIds {
    order: std::collections::VecDeque<u64>,
    ids: std::collections::HashSet<u64>,
}

impl ClosedIds {
    pub(crate) fn insert(&mut self, connection_id: u64) {
        if !self.ids.insert(connection_id) {
            return;
        }
        self.order.push_back(connection_id);
        if self.order.len() > CLOSED_REMEMBERED {
            if let Some(oldest) = self.order.pop_front() {
                self.ids.remove(&oldest);
            }
        }
    }

    pub(crate) fn contains(&self, connection_id: u64) -> bool {
        self.ids.contains(&connection_id)
    }
}

/// One bridge connection's in-flight count and auth state.
pub(crate) struct ConnectionAuth {
    pub(crate) in_flight: usize,
    pub(crate) state: State,
}

/// A call's resolution at entry (section 2, tri-state resolution),
/// captured for the whole call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Resolved {
    /// No connection id, or a connection never challenged.
    NotExternal,
    /// A bound connection, by its binding id.
    Bound(String),
    /// Known but unbound, locked, stale or torn down.
    Refused(Class),
}

/// Who a binding-required call acts as.
pub(crate) enum Who {
    /// The per-request name and secret of phase 1, on a connection that is
    /// not external.
    Legacy {
        peer_pid: Option<i32>,
        connection_id: Option<u64>,
    },
    /// The connection's binding, captured at entry.
    Bound {
        connection_id: u64,
        binding_id: String,
    },
}
/// How a method is admitted (section 2, existing methods).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Management,
    BindingRequired,
    Auth,
}

pub(crate) fn kind(method: &str) -> Option<Kind> {
    use crate::contract::method as m;
    Some(match method {
        m::SpawnCallersList::NAME
        | m::SpawnCallersProvision::NAME
        | m::SpawnCallersRotate::NAME
        | m::SpawnCallersSetEnabled::NAME
        | m::SpawnCallersRevoke::NAME
        | m::SpawnCallersDecide::NAME => Kind::Management,
        m::SpawnCallersIdentity::NAME
        | m::SpawnCallersInboxOpen::NAME
        | m::SpawnCallersInboxNext::NAME
        | m::SpawnCallersInboxAck::NAME
        | m::SpawnCallersInboxFail::NAME
        | m::SpawnCallersInboxClose::NAME
        | m::SpawnCallersDisconnect::NAME => Kind::BindingRequired,
        m::SpawnCallersChallenge::NAME | m::SpawnCallersAuthenticate::NAME => Kind::Auth,
        _ => return None,
    })
}

fn resolve(state: &State) -> Resolved {
    match state {
        State::Untouched => Resolved::NotExternal,
        State::Challenged { .. } => Resolved::Refused(Class::CallerUnbound),
        State::Locked => Resolved::Refused(Class::AuthLocked),
        State::Bound(bound) => Resolved::Bound(bound.binding_id.clone()),
        State::Stale => Resolved::Refused(Class::BindingStale),
        State::Closed => Resolved::Refused(Class::ConnectionClosed),
    }
}

/// A call in flight on its connection: counted from entry until dropped,
/// with its resolution captured once.
pub(crate) struct Entry<'a> {
    callers: &'a ServiceCallers,
    connection_id: Option<u64>,
    pub(crate) resolved: Resolved,
}

impl Drop for Entry<'_> {
    fn drop(&mut self) {
        let Some(connection_id) = self.connection_id else {
            return;
        };
        let mut live = self.callers.live();
        let remove = match live.connections.get_mut(&connection_id) {
            Some(entry) => {
                entry.in_flight = entry.in_flight.saturating_sub(1);
                entry.in_flight == 0 && matches!(entry.state, State::Untouched | State::Closed)
            }
            None => false,
        };
        if remove {
            live.connections.remove(&connection_id);
        }
    }
}

/// One attempt ended on `connection_id` without a binding: the third locks
/// it. A closed connection stays closed.
fn count_attempt(live: &mut Live, connection_id: u64) {
    let Some(entry) = live.connections.get_mut(&connection_id) else {
        return;
    };
    if let State::Challenged { attempts, .. } = &mut entry.state {
        *attempts += 1;
        if *attempts >= MAX_ATTEMPTS {
            entry.state = State::Locked;
        }
    }
}

/// Ends the iterators opened on `connections` and releases their leases.
fn close_iterators(live: &mut Live, connections: &[u64]) -> bool {
    let Live {
        iterators, leases, ..
    } = live;
    let before = iterators.len();
    iterators.retain(|_, iterator| {
        !iterator
            .connection_id
            .is_some_and(|id| connections.contains(&id))
    });
    leases.retain(|_, lease| iterators.contains_key(&lease.iterator_id));
    iterators.len() != before
}

impl ServiceCallers {
    /// Counts a call in on its connection and resolves it.
    pub(crate) fn enter(&self, connection_id: Option<u64>) -> Entry<'_> {
        let resolved = match connection_id {
            None => Resolved::NotExternal,
            Some(id) => {
                let mut live = self.live();
                if live.closed.contains(id) {
                    // Counted nowhere: there is no entry to keep.
                    return Entry {
                        callers: self,
                        connection_id: None,
                        resolved: Resolved::Refused(Class::ConnectionClosed),
                    };
                }
                let entry = live.connections.entry(id).or_insert(ConnectionAuth {
                    in_flight: 0,
                    state: State::Untouched,
                });
                entry.in_flight += 1;
                resolve(&entry.state)
            }
        };
        Entry {
            callers: self,
            connection_id,
            resolved,
        }
    }

    /// How `connection_id` resolves now, without counting a call in on it
    /// (the core's `SpawnCallers::connection`): one never challenged is
    /// not external, one the core reported closed is `connection_closed`.
    pub(crate) fn peek(&self, connection_id: u64) -> Resolved {
        let live = self.live();
        if live.closed.contains(connection_id) {
            return Resolved::Refused(Class::ConnectionClosed);
        }
        live.connections
            .get(&connection_id)
            .map_or(Resolved::NotExternal, |entry| resolve(&entry.state))
    }

    /// Clears every expired challenge, counting each as an attempt (the
    /// sweep of section 4).
    pub(crate) fn expire_challenges(&self, now: i64) {
        expire_all(&mut self.live(), now);
    }

    /// The binding `binding_id` of `connection_id`, rechecked against the
    /// record, the core's identity generation and the current scope, under
    /// the live lock (`check_bound`, section 4). A binding that no longer
    /// holds is made stale here, its iterators closed and its leases
    /// released.
    pub(crate) fn check_bound(
        &self,
        live: &mut Live,
        connection_id: u64,
        binding_id: &str,
        record: Option<&CallerRecord>,
        binding: Option<&Binding>,
    ) -> Result<(), Class> {
        let now = self.host.now_ms();
        let generation = self.host.generation();
        let Some(entry) = live.connections.get_mut(&connection_id) else {
            return Err(Class::ConnectionClosed);
        };
        let holds = match &entry.state {
            State::Closed => return Err(Class::ConnectionClosed),
            State::Bound(bound) if bound.binding_id == binding_id => {
                record.is_some_and(|record| {
                    record.caller_id == bound.caller_id
                        && record.authority_generation == bound.authority_generation
                        && record.live(now).is_ok()
                }) && bound.retires_at_ms.is_none_or(|retires| retires > now)
                    && bound.identity_generation == generation
                    && binding.is_some_and(|binding| binding.key == bound.binding_key)
            }
            _ => return Err(Class::BindingStale),
        };
        if holds {
            return Ok(());
        }
        entry.state = State::Stale;
        if close_iterators(live, &[connection_id]) {
            self.changed.notify_waiters();
        }
        Err(Class::BindingStale)
    }

    /// The live lock for a gated side effect of `who`, once its authority
    /// still holds (section 4, recheck before every side effect). Called
    /// with the files lock held.
    pub(crate) fn gate(&self, who: &Who, binding: &Binding) -> Result<MutexGuard<'_, Live>, Class> {
        match who {
            Who::Legacy { connection_id, .. } => {
                let live = self.live();
                // A connection the core reported closed gains no state,
                // even one counted in before the close.
                if connection_id.is_some_and(|id| live.closed.contains(id)) {
                    return Err(Class::ConnectionClosed);
                }
                let external = connection_id
                    .and_then(|id| live.connections.get(&id))
                    .is_some_and(|entry| entry.state.external());
                if external {
                    return Err(Class::CallerUnbound);
                }
                Ok(live)
            }
            Who::Bound {
                connection_id,
                binding_id,
            } => {
                let caller_id = self.bound_caller(*connection_id, binding_id);
                let record = caller_id.and_then(|caller_id| {
                    let file = self.load_current(binding).ok()?;
                    file.callers.get(file.by_id(&caller_id)?).cloned()
                });
                let current = Binding::current(&*self.host).ok();
                let key_ok = current.as_ref().is_some_and(|c| c.key == binding.key);
                let mut live = self.live();
                self.check_bound(
                    &mut live,
                    *connection_id,
                    binding_id,
                    record.as_ref(),
                    key_ok.then_some(binding),
                )?;
                Ok(live)
            }
        }
    }

    /// The live lock for a management side effect, while the call's
    /// connection is still not external (section 2, NotExternal-only
    /// methods): one challenged, bound, stale or locked since the call
    /// entered is `external_connection`, one the core reported closed
    /// `connection_closed`. Called with the files lock held; hold it across
    /// the write it gates.
    pub(crate) fn management_gate(
        &self,
        connection_id: Option<u64>,
    ) -> Result<MutexGuard<'_, Live>, Class> {
        let live = self.live();
        let Some(id) = connection_id else {
            return Ok(live);
        };
        if live.closed.contains(id) {
            return Err(Class::ConnectionClosed);
        }
        match live.connections.get(&id).map(|entry| &entry.state) {
            None | Some(State::Untouched) => Ok(live),
            Some(State::Closed) => Err(Class::ConnectionClosed),
            Some(_) => Err(Class::ExternalConnection),
        }
    }

    /// The caller id of `connection_id`'s binding while it is `binding_id`.
    pub(crate) fn bound_caller(&self, connection_id: u64, binding_id: &str) -> Option<String> {
        match &self.live().connections.get(&connection_id)?.state {
            State::Bound(bound) if bound.binding_id == binding_id => Some(bound.caller_id.clone()),
            _ => None,
        }
    }

    /// Why a bound call can no longer proceed, or `None` while its binding
    /// is still the connection's: `connection_closed` or `binding_stale`.
    pub(crate) fn bound_gone(&self, connection_id: u64, binding_id: &str) -> Option<Class> {
        match self
            .live()
            .connections
            .get(&connection_id)
            .map(|e| &e.state)
        {
            None | Some(State::Closed) => Some(Class::ConnectionClosed),
            Some(State::Bound(bound)) if bound.binding_id == binding_id => None,
            Some(_) => Some(Class::BindingStale),
        }
    }

    /// `disconnect` on a bound connection: its binding goes stale.
    pub(crate) fn unbind(&self, live: &mut Live, connection_id: u64) {
        if let Some(entry) = live.connections.get_mut(&connection_id) {
            entry.state = State::Stale;
        }
        if close_iterators(live, &[connection_id]) {
            self.changed.notify_waiters();
        }
    }
}

fn expire_all(live: &mut Live, now: i64) {
    for entry in live.connections.values_mut() {
        if let State::Challenged { slot, attempts } = &mut entry.state {
            if slot.as_ref().is_some_and(|slot| slot.expired(now)) {
                *slot = None;
                *attempts += 1;
                if *attempts >= MAX_ATTEMPTS {
                    entry.state = State::Locked;
                }
            }
        }
    }
}

#[cfg(test)]
impl ServiceCallers {
    /// The state of `connection_id`'s entry, by name, or `None` without one.
    pub(crate) fn connection_state(&self, connection_id: u64) -> Option<&'static str> {
        Some(match &self.live().connections.get(&connection_id)?.state {
            State::Untouched => "untouched",
            State::Challenged { .. } => "challenged",
            State::Bound(_) => "bound",
            State::Stale => "stale",
            State::Locked => "locked",
            State::Closed => "closed",
        })
    }

    /// How many challenges are outstanding, across every connection.
    pub(crate) fn outstanding_challenges(&self) -> usize {
        self.live()
            .connections
            .values()
            .filter(|e| matches!(e.state, State::Challenged { slot: Some(_), .. }))
            .count()
    }
}
