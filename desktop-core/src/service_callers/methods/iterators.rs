//! The result inbox a caller's runtime reads (`docs/service-callers.md`
//! section 8): iterators opened on a connection, `next` with its wait,
//! and the settlement of each delivered item.

use super::{NEXT_MS, RECHECK};
use crate::contract::service_callers::{
    InboxIteratorParams, InboxNext, InboxNextParams, InboxOpenParams, InboxOpened,
    InboxSettleParams, InboxSettled,
};
use crate::service_callers::auth::Who;
use crate::service_callers::inbox::{self, Lease, ACK};
use crate::service_callers::registry::{self, Binding};
use crate::service_callers::{Class, ServiceCallers};
use agent_relay_node::relay::contract::Done;
use agent_relay_node::relay::protocol::RpcError;
use std::time::Duration;

impl ServiceCallers {
    /// Opens an iterator bound to `connection_id`, the core's id of the
    /// calling bridge connection, when the core knows it.
    pub(crate) fn open(
        &self,
        who: &Who,
        params: &InboxOpenParams,
    ) -> Result<InboxOpened, RpcError> {
        const METHOD: &str = "spawn_callers.inbox.open";
        let (binding, record) =
            self.caller(who, &params.caller_name, &params.caller_secret, METHOD)?;
        let refuse = |class| self.refused(Some(&binding), Some(&record.caller_id), METHOD, class);
        if !record.owns(&params.invocation_id) {
            return Err(refuse(Class::NotInLedger));
        }
        let iterator_id = crate::service_callers::random_hex(16)
            .map(|hex| format!("it_{hex}"))
            .ok_or_else(|| refuse(Class::StoreFailed))?;
        let now = self.host.now_ms();
        let (connection_id, _) = Self::provenance(who);
        {
            let _files = self.files();
            let mut live = self.gate(who, &binding).map_err(refuse)?;
            live.iterators.insert(
                iterator_id.clone(),
                inbox::Iterator {
                    caller_id: record.caller_id.clone(),
                    invocation_id: params.invocation_id.clone(),
                    last_used_ms: now,
                    waiting: 0,
                    connection_id,
                },
            );
        }
        self.audit(&binding, Some(&record.caller_id), METHOD, "ok", None);
        Ok(InboxOpened { iterator_id })
    }

    /// Ends iterators left idle past the lease with no connection whose
    /// close would end them, and their leases.
    pub(super) fn sweep(&self, now: i64) {
        let mut live = self.live();
        live.iterators
            .retain(|_, iterator| !iterator.abandoned(now));
        let crate::service_callers::Live {
            iterators, leases, ..
        } = &mut *live;
        leases.retain(|_, lease| iterators.contains_key(&lease.iterator_id));
    }

    /// The iterator's invocation, when it is open and this caller's.
    fn iterator(&self, caller_id: &str, iterator_id: &str) -> Result<String, Class> {
        self.live()
            .iterators
            .get(iterator_id)
            .filter(|iterator| iterator.caller_id == caller_id)
            .map(|iterator| iterator.invocation_id.clone())
            .ok_or(Class::NotFound)
    }

    pub(crate) async fn next(
        &self,
        who: &Who,
        params: &InboxNextParams,
    ) -> Result<InboxNext, RpcError> {
        const METHOD: &str = "spawn_callers.inbox.next";
        let (binding, record) =
            self.caller(who, &params.caller_name, &params.caller_secret, METHOD)?;
        let caller_id = record.caller_id.as_str();
        let refuse = |class| self.refused(Some(&binding), Some(caller_id), METHOD, class);
        if params.timeout_ms > NEXT_MS {
            return Err(refuse(Class::InvalidInput));
        }
        self.sweep(self.host.now_ms());
        self.iterator(caller_id, &params.iterator_id)
            .map_err(refuse)?;
        self.waiting(&params.iterator_id, 1);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(params.timeout_ms);
        let found = loop {
            let woken = self.changed.notified();
            tokio::pin!(woken);
            woken.as_mut().enable();
            match self.lease_next(who, &binding, caller_id, &params.iterator_id) {
                Ok(Some(delivery)) => break Ok(Some(delivery)),
                Ok(None) => {}
                // Closed while waiting: nothing consumed. A bound call whose
                // connection closed, or whose binding went stale, says so.
                Err(Class::NotFound) => match who {
                    Who::Bound {
                        connection_id,
                        binding_id,
                    } => match self.bound_gone(*connection_id, binding_id) {
                        Some(class) => break Err(class),
                        None => break Ok(None),
                    },
                    Who::Legacy { .. } => break Ok(None),
                },
                Err(class) => break Err(class),
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break Ok(None);
            }
            let wait = (deadline - now).min(RECHECK);
            tokio::select! {
                _ = &mut woken => {}
                _ = tokio::time::sleep(wait) => {}
            }
        };
        self.waiting(&params.iterator_id, -1);
        let delivery = found.map_err(refuse)?;
        self.audit(
            &binding,
            Some(caller_id),
            METHOD,
            if delivery.is_some() {
                "delivered"
            } else {
                "empty"
            },
            None,
        );
        Ok(InboxNext { delivery })
    }

    fn waiting(&self, iterator_id: &str, change: isize) {
        let now = self.host.now_ms();
        if let Some(iterator) = self.live().iterators.get_mut(iterator_id) {
            iterator.waiting = iterator.waiting.saturating_add_signed(change);
            iterator.last_used_ms = now;
        }
    }

    /// The iterator's invocation's unsettled item, leased to it, unless
    /// another iterator holds an unexpired lease on it.
    fn lease_next(
        &self,
        who: &Who,
        binding: &Binding,
        caller_id: &str,
        iterator_id: &str,
    ) -> Result<Option<crate::contract::service_callers::InboxDelivery>, Class> {
        let _files = self.files();
        let invocation_id = self.iterator(caller_id, iterator_id)?;
        let mut file = self.load_inbox(binding, caller_id)?;
        let Some(slot) = file.slot_for(&invocation_id) else {
            return Ok(None);
        };
        if slot.settled.is_some() {
            return Ok(None);
        }
        let Some(delivery) = slot.delivery() else {
            return Ok(None);
        };
        let now = self.host.now_ms();
        let mut live = self.gate(who, binding)?;
        let crate::service_callers::Live {
            iterators, leases, ..
        } = &mut *live;
        if !iterators.contains_key(iterator_id) {
            return Err(Class::NotFound);
        }
        if leases
            .get(&delivery.item_id)
            .is_some_and(|lease| lease.until_ms > now && iterators.contains_key(&lease.iterator_id))
        {
            return Ok(None);
        }
        leases.insert(
            delivery.item_id.clone(),
            Lease {
                iterator_id: iterator_id.to_string(),
                until_ms: now + inbox::LEASE_MS,
            },
        );
        Ok(Some(delivery))
    }

    /// `ack` or `fail`, keyed by caller and item id. An identical repeat is
    /// `Done` while its tombstone lives; anything else on a settled item is
    /// `settlement_conflict`.
    pub(crate) fn settle(
        &self,
        who: &Who,
        params: &InboxSettleParams,
        kind: &str,
    ) -> Result<InboxSettled, RpcError> {
        let method = if kind == ACK {
            "spawn_callers.inbox.ack"
        } else {
            "spawn_callers.inbox.fail"
        };
        let (binding, record) =
            self.caller(who, &params.caller_name, &params.caller_secret, method)?;
        let caller_id = record.caller_id.as_str();
        let refuse = |class| self.refused(Some(&binding), Some(caller_id), method, class);
        let outcome = {
            let _files = self.files();
            self.settle_item(who, &binding, caller_id, &params.item_id, kind, params)
        };
        match outcome {
            Ok((receipt, first)) => {
                if first {
                    self.live().leases.remove(&params.item_id);
                    self.changed.notify_waiters();
                }
                self.audit(
                    &binding,
                    Some(caller_id),
                    method,
                    if first { "settled" } else { "repeat" },
                    None,
                );
                Ok(receipt)
            }
            Err(class) => Err(refuse(class)),
        }
    }

    /// The item's receipt, and whether this call settled it (`false` for
    /// an identical repeat, which returns the original receipt). The
    /// tombstone is durable before the first receipt is returned.
    fn settle_item(
        &self,
        who: &Who,
        binding: &Binding,
        caller_id: &str,
        item_id: &str,
        kind: &str,
        params: &InboxSettleParams,
    ) -> Result<(InboxSettled, bool), Class> {
        let now = self.host.now_ms();
        let mut file = self.load_inbox(binding, caller_id)?;
        if file.purge(now) {
            inbox::save(binding, caller_id, &file)?;
        }
        let Some(slot) = file.by_item(item_id) else {
            return Err(self.owner_of(binding, caller_id, item_id, now));
        };
        // A settled item answers from its tombstone alone: the identical
        // settlement gets the original receipt, any other one (the
        // opposite action, or other metadata) is a conflict.
        if let Some(settled) = &slot.settled {
            if settled.kind != kind || settled.metadata != params.metadata.relayflows_result {
                return Err(Class::SettlementConflict);
            }
            let mut receipt = settled.receipt(item_id);
            if receipt.message_id.is_empty() {
                if let Some(item) = &slot.item {
                    receipt.message_id = item.message_id.clone();
                }
            }
            return Ok((receipt, false));
        }
        let metadata = inbox::metadata_for(kind, &params.metadata)?;
        let item = slot.item.as_mut().ok_or(Class::NotFound)?;
        if item.failure.is_some() && kind == ACK {
            return Err(Class::SettlementInvalid);
        }
        let message_id = item.message_id.clone();
        // A settled item keeps its ids and metadata, never its text.
        item.text = None;
        let settled = inbox::Settled {
            kind: kind.to_string(),
            metadata,
            at_ms: now,
            message_id,
        };
        let receipt = settled.receipt(item_id);
        slot.settled = Some(settled);
        // The authority is rechecked under the live lock, which is held
        // across the durable write (`docs/service-callers-auth.md`
        // section 4).
        let live = self.gate(who, binding)?;
        inbox::save(binding, caller_id, &file)?;
        drop(live);
        Ok((receipt, true))
    }

    /// `item_not_owned` when another caller of this registry holds the
    /// item, else `not_found`.
    fn owner_of(&self, binding: &Binding, caller_id: &str, item_id: &str, now: i64) -> Class {
        let Ok(file) = registry::load(binding) else {
            return Class::NotFound;
        };
        let elsewhere = file
            .callers
            .iter()
            .filter(|other| other.caller_id != caller_id)
            .any(|other| {
                inbox::load(binding, &other.caller_id).is_ok_and(|mut inbox| {
                    inbox.purge(now);
                    inbox.by_item(item_id).is_some()
                })
            });
        if elsewhere {
            Class::ItemNotOwned
        } else {
            Class::NotFound
        }
    }

    pub(crate) fn close(&self, who: &Who, params: &InboxIteratorParams) -> Result<Done, RpcError> {
        const METHOD: &str = "spawn_callers.inbox.close";
        let (binding, record) =
            self.caller(who, &params.caller_name, &params.caller_secret, METHOD)?;
        let caller_id = record.caller_id.as_str();
        {
            let _files = self.files();
            let mut live = self
                .gate(who, &binding)
                .map_err(|class| self.refused(Some(&binding), Some(caller_id), METHOD, class))?;
            let owned = live
                .iterators
                .get(&params.iterator_id)
                .is_some_and(|iterator| iterator.caller_id == caller_id);
            if !owned {
                drop(live);
                return Err(self.refused(Some(&binding), Some(caller_id), METHOD, Class::NotFound));
            }
            live.iterators.remove(&params.iterator_id);
            live.leases
                .retain(|_, lease| lease.iterator_id != params.iterator_id);
        }
        self.changed.notify_waiters();
        self.audit(&binding, Some(caller_id), METHOD, "ok", None);
        Ok(Done {})
    }
}
