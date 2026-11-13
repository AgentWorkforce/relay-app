//! Local service callers (`docs/service-callers.md`): a runtime outside any
//! session, such as Flows, starts agents on this computer's own spawn node
//! with a provisioned name and secret, and receives their results through a
//! durable inbox. This plugin owns the callers' records, scope, ledger,
//! inbox and audit; the core owns nothing here.
//!
//! Every check fails closed. No method returns a caller's Relaycast token,
//! and no log, audit line or error carries a secret, hash, token or text.

mod audit;
mod auth;
pub mod command;
mod handoff;
pub mod host;
mod inbox;
mod manage;
mod methods;
mod node;
mod peer;
mod registry;
mod releases;
mod scope;
mod spawn;
#[cfg(test)]
mod tests;

pub use host::{Attestation, CoreHost, Host};

use agent_relay_node::relay::protocol::{ErrorKind, RefusalReason, RpcError};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

/// A refused or failed call, by static class. The class goes to the audit;
/// the error a client sees carries a static message and nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    CallerUnknown,
    SecretMismatch,
    CallerDisabled,
    CallerExpired,
    SecretRetired,
    AccountMismatch,
    WorkspaceMismatch,
    GenerationMoved,
    NotInLedger,
    RequesterOther,
    ItemNotOwned,
    HarnessOutside,
    DirectoryOutside,
    PeerUnreadable,
    PeerUidMismatch,
    PeerInSessionTree,
    SecretOnManagement,
    PeerCheckUnavailable,
    AppAttestationUnavailable,
    NotSignedIn,
    RegistryUnreadable,
    StoreFailed,
    NodeUnavailable,
    NodeOther,
    RateLimited,
    LiveAgentCap,
    ReservationFull,
    SettlementConflict,
    SettlementInvalid,
    NotFound,
    IteratorOpen,
    NameTaken,
    NotPending,
    InvalidInput,
    AuthFailed,
    CallerUnbound,
    BindingStale,
    ConnectionClosed,
    AlreadyBound,
    ChallengeOutstanding,
    ChallengeCapacity,
    AuthLocked,
    BindingUnavailable,
    LegacyCredentialRefused,
    ExternalConnection,
    /// A rotation's `caller.json` write failed and so did the registry
    /// write that undoes it; the record is put back at the next check.
    RotationRollbackPending,
    /// Audit only: answered as `auth_failed`.
    ChallengeExpired,
    /// Audit only: answered as `auth_failed`.
    ChallengeConsumed,
}

/// Every class with its wire and audit name.
const NAMES: [(Class, &str); 48] = [
    (Class::CallerUnknown, "caller_unknown"),
    (Class::SecretMismatch, "secret_mismatch"),
    (Class::CallerDisabled, "caller_disabled"),
    (Class::CallerExpired, "caller_expired"),
    (Class::SecretRetired, "secret_retired"),
    (Class::AccountMismatch, "account_mismatch"),
    (Class::WorkspaceMismatch, "workspace_mismatch"),
    (Class::GenerationMoved, "generation_moved"),
    (Class::NotInLedger, "not_in_ledger"),
    (Class::RequesterOther, "requester_other"),
    (Class::ItemNotOwned, "item_not_owned"),
    (Class::HarnessOutside, "harness_outside"),
    (Class::DirectoryOutside, "directory_outside"),
    (Class::PeerUnreadable, "peer_unreadable"),
    (Class::PeerUidMismatch, "peer_uid_mismatch"),
    (Class::PeerInSessionTree, "peer_in_session_tree"),
    (Class::SecretOnManagement, "secret_on_management"),
    (Class::PeerCheckUnavailable, "peer_check_unavailable"),
    (
        Class::AppAttestationUnavailable,
        "app_attestation_unavailable",
    ),
    (Class::NotSignedIn, "not_signed_in"),
    (Class::RegistryUnreadable, "registry_unreadable"),
    (Class::StoreFailed, "store_failed"),
    (Class::NodeUnavailable, "node_unavailable"),
    (Class::NodeOther, "node_other"),
    (Class::RateLimited, "rate_limited"),
    (Class::LiveAgentCap, "live_agent_cap"),
    (Class::ReservationFull, "reservation_full"),
    (Class::SettlementConflict, "settlement_conflict"),
    (Class::SettlementInvalid, "settlement_invalid"),
    (Class::NotFound, "not_found"),
    (Class::IteratorOpen, "iterator_open"),
    (Class::NameTaken, "name_taken"),
    (Class::NotPending, "not_pending"),
    (Class::InvalidInput, "invalid_input"),
    (Class::AuthFailed, "auth_failed"),
    (Class::CallerUnbound, "caller_unbound"),
    (Class::BindingStale, "binding_stale"),
    (Class::ConnectionClosed, "connection_closed"),
    (Class::AlreadyBound, "already_bound"),
    (Class::ChallengeOutstanding, "challenge_outstanding"),
    (Class::ChallengeCapacity, "challenge_capacity"),
    (Class::AuthLocked, "auth_locked"),
    (Class::BindingUnavailable, "binding_unavailable"),
    (Class::LegacyCredentialRefused, "legacy_credential_refused"),
    (Class::ExternalConnection, "external_connection"),
    (Class::RotationRollbackPending, "rotation_rollback_pending"),
    (Class::ChallengeExpired, "challenge_expired"),
    (Class::ChallengeConsumed, "challenge_consumed"),
];

impl Class {
    pub(crate) fn name(self) -> &'static str {
        NAMES
            .iter()
            .find(|(class, _)| *class == self)
            .map_or("store_failed", |(_, name)| name)
    }

    /// The class string a client sees. An unknown name and a wrong secret
    /// answer alike (section 6); the audit still records which it was.
    /// Why `authenticate` failed is never on the wire
    /// (`docs/service-callers-auth.md` section 5).
    fn wire_name(self) -> &'static str {
        match self {
            Class::SecretMismatch => Class::CallerUnknown.name(),
            Class::ChallengeExpired | Class::ChallengeConsumed => Class::AuthFailed.name(),
            _ => self.name(),
        }
    }

    /// The typed refusal reason section 6 maps the class to, if any.
    fn reason(self) -> Option<RefusalReason> {
        use RefusalReason as R;
        Some(match self {
            Class::CallerUnknown
            | Class::SecretMismatch
            | Class::RegistryUnreadable
            | Class::AuthFailed
            | Class::ChallengeExpired
            | Class::ChallengeConsumed
            | Class::CallerUnbound => R::ServiceUnknown,
            Class::CallerDisabled => R::ServiceDisabled,
            Class::CallerExpired | Class::SecretRetired | Class::BindingStale => R::ServiceExpired,
            Class::AccountMismatch | Class::WorkspaceMismatch | Class::GenerationMoved => {
                R::WorkspaceMismatch
            }
            Class::NotInLedger | Class::RequesterOther | Class::ItemNotOwned => R::NotOwner,
            Class::HarnessOutside | Class::DirectoryOutside => R::OutsideScope,
            Class::PeerUnreadable
            | Class::PeerUidMismatch
            | Class::PeerInSessionTree
            | Class::SecretOnManagement
            | Class::PeerCheckUnavailable
            | Class::AppAttestationUnavailable
            | Class::BindingUnavailable
            | Class::ExternalConnection => R::ManagementPeer,
            _ => return None,
        })
    }

    /// The error kind of a class without a refusal reason.
    fn kind(self) -> ErrorKind {
        match self {
            Class::SettlementConflict
            | Class::IteratorOpen
            | Class::NameTaken
            | Class::NotPending
            | Class::AlreadyBound => ErrorKind::Conflict,
            Class::NotFound => ErrorKind::NotFound,
            Class::NotSignedIn => ErrorKind::NotSignedIn,
            Class::StoreFailed | Class::RotationRollbackPending => ErrorKind::StoreFailed,
            Class::RateLimited | Class::AuthLocked => ErrorKind::RateLimited,
            Class::LiveAgentCap
            | Class::ReservationFull
            | Class::ChallengeOutstanding
            | Class::ChallengeCapacity => ErrorKind::Busy,
            Class::SettlementInvalid | Class::InvalidInput => ErrorKind::InvalidParams,
            // NodeOther, NodeUnavailable, ConnectionClosed and
            // LegacyCredentialRefused.
            _ => ErrorKind::Refused,
        }
    }

    /// The error a client sees: the core's `RpcError` with a kind, a typed
    /// reason where section 6 maps one, and the class string as its whole
    /// message (`docs/service-callers.md` section 8, error wire).
    pub(crate) fn error(self) -> RpcError {
        let message = self.wire_name();
        let mut error = match self.reason() {
            Some(reason) => RpcError::refused_because(reason, message),
            None => RpcError::new(self.kind(), message),
        };
        if self == Class::AuthLocked {
            // Locked for the rest of the connection's life: a retry on it
            // can never succeed.
            error.data.retryable = false;
        }
        error
    }
}

/// Process-lifetime state: open iterators, their leases, and invoke rates.
#[derive(Default)]
pub(crate) struct Live {
    pub(crate) iterators: HashMap<String, inbox::Iterator>,
    /// Item id to its lease.
    pub(crate) leases: HashMap<String, inbox::Lease>,
    /// Caller id to the times of its recent invokes.
    pub(crate) invokes: HashMap<String, VecDeque<i64>>,
    /// The core's bridge connection id to its challenge and binding state
    /// (`docs/service-callers-auth.md` section 4). Server-only, never
    /// persisted.
    pub(crate) connections: HashMap<u64, auth::ConnectionAuth>,
    /// Connection ids the core has reported closed, so a late call on one
    /// fails closed as `connection_closed` once its entry is gone.
    pub(crate) closed: auth::ClosedIds,
    /// Workers started for callers that ended while their invoke was in
    /// flight, each released once.
    pub(crate) late: spawn::Late,
}

/// The service callers of this core's signed-in account.
pub struct ServiceCallers {
    host: Arc<dyn Host>,
    /// One read-modify-write of the registry and inboxes at a time, and
    /// the admitted invokes' claims on their reserved slots.
    files: Arc<Mutex<spawn::Claims>>,
    live: Mutex<Live>,
    /// Reservation releases a failed write left on disk
    /// ([`spawn::Releases`]); read and written under the files lock.
    releases: Arc<Mutex<spawn::Releases>>,
    /// Woken when an item arrives, is settled, or an iterator closes.
    changed: tokio::sync::Notify,
}

impl ServiceCallers {
    pub(crate) fn new(host: Arc<dyn Host>) -> ServiceCallers {
        ServiceCallers {
            host,
            files: Arc::new(Mutex::new(spawn::Claims::default())),
            live: Mutex::new(Live::default()),
            releases: Arc::new(Mutex::new(spawn::Releases::new())),
            changed: tokio::sync::Notify::new(),
        }
    }

    fn files(&self) -> MutexGuard<'_, spawn::Claims> {
        self.files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn live(&self) -> MutexGuard<'_, Live> {
        self.live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// One audit line; a line that cannot be written is logged by method.
    fn audit(
        &self,
        binding: &registry::Binding,
        caller_id: Option<&str>,
        method: &str,
        outcome: &str,
        count: Option<u64>,
    ) {
        let now = self.host.now_ms();
        if !audit::write(binding, now, caller_id, method, outcome, count) {
            self.host
                .log(&format!("{method}: the audit could not be written"));
        }
    }

    /// Audits a refusal by class and returns its static error.
    fn refused(
        &self,
        binding: Option<&registry::Binding>,
        caller_id: Option<&str>,
        method: &str,
        class: Class,
    ) -> RpcError {
        match binding {
            Some(binding) => self.audit(binding, caller_id, method, class.name(), None),
            None => self.host.log(&format!("{method}: {}", class.name())),
        }
        class.error()
    }

    /// Forgets a caller's iterators, leases and invoke times.
    fn drop_live(&self, caller_id: &str) {
        let mut live = self.live();
        live.iterators
            .retain(|_, iterator| iterator.caller_id != caller_id);
        let Live {
            iterators, leases, ..
        } = &mut *live;
        leases.retain(|_, lease| iterators.contains_key(&lease.iterator_id));
        live.invokes.remove(caller_id);
    }

    /// Reads each caller's deliveries every few seconds for the daemon's life.
    pub async fn watch(self: Arc<Self>) {
        let mut ticks = tokio::time::interval(std::time::Duration::from_secs(2));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            self.expire_challenges(self.host.now_ms());
            self.sweep_lifecycle();
            self.release_late().await;
            self.poll_once().await;
        }
    }
}

/// Random lowercase hex of `bytes` bytes from the system RNG.
pub(crate) fn random_hex(bytes: usize) -> Option<String> {
    let mut buffer = vec![0u8; bytes];
    getrandom::getrandom(&mut buffer).ok()?;
    Some(buffer.iter().map(|b| format!("{b:02x}")).collect())
}

/// `ms` as an RFC 3339 time in UTC.
pub(crate) fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}
