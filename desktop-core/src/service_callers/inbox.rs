//! The result inbox (`docs/service-callers.md` section 8): one durable file
//! per caller with one reserved terminal slot per invocation, leased to
//! per-invocation iterators and kept until `ack` or `fail`.

use super::registry::{self, Binding, DAY_MS};
use super::Class;
use crate::contract::service_callers::{
    InboxDelivery, InboxFailure, InboxFailureCode, InboxSender, InboxSettleAction,
    InboxSettleMetadata, InboxSettleState, InboxSettled,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The text budget of one invocation's terminal slot.
pub(crate) const SLOT_BYTES: u64 = 512 * 1024;
/// Per caller: every unsettled slot reserves [`SLOT_BYTES`] of this.
pub(crate) const CALLER_BYTES: u64 = 16 * 1024 * 1024;
/// Unsettled reservations a caller may hold at once.
pub(crate) const ACTIVE: usize = (CALLER_BYTES / SLOT_BYTES) as usize;
/// Records per caller, settled ones included.
pub(crate) const RECORDS: usize = 1000;
/// How long a delivered, unsettled item stays leased to its iterator.
pub(crate) const LEASE_MS: i64 = 60_000;
/// How long a settled item's tombstone answers a repeated settlement.
pub(crate) const TOMBSTONE_MS: i64 = 7 * DAY_MS;
/// A reservation whose invoke never reported back is let go after this.
pub(crate) const UNBOUND_MS: i64 = 10 * 60 * 1000;

pub(crate) const ACK: &str = "ack";
pub(crate) const FAIL: &str = "fail";
pub(crate) const JOURNALED: &str = "journaled";
pub(crate) const REJECTED: &str = "rejected";

#[derive(Clone, Serialize, Deserialize, Default)]
pub(crate) struct InboxFile {
    pub(crate) slots: Vec<Slot>,
}

/// One invocation's reserved terminal slot.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Slot {
    /// The invoke's idempotency key, so a retried invoke reuses the slot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) invocation_id: Option<String>,
    pub(crate) reserved_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) item: Option<Item>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) settled: Option<Settled>,
    #[serde(default)]
    pub(crate) later_reports: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Item {
    pub(crate) item_id: String,
    pub(crate) message_id: String,
    pub(crate) sender_id: String,
    pub(crate) sender_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) failure: Option<InboxFailure>,
    pub(crate) sent_at_ms: i64,
}

/// A settlement: kept, without text, for [`TOMBSTONE_MS`].
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Settled {
    pub(crate) kind: String,
    pub(crate) metadata: String,
    pub(crate) at_ms: i64,
    /// The delivery's `message_id`, so a repeat's receipt is rebuilt from
    /// the tombstone alone. Empty in a tombstone written before receipts,
    /// whose item still carries it.
    #[serde(default)]
    pub(crate) message_id: String,
}

impl Settled {
    /// The receipt every identical settlement of the item returns.
    pub(crate) fn receipt(&self, item_id: &str) -> InboxSettled {
        let (action, state) = if self.kind == ACK {
            (InboxSettleAction::Ack, InboxSettleState::Acked)
        } else {
            (InboxSettleAction::Fail, InboxSettleState::Failed)
        };
        InboxSettled {
            item_id: item_id.to_string(),
            message_id: self.message_id.clone(),
            action,
            state,
            settled_at_ms: self.at_ms,
        }
    }
}

/// An open iterator: one caller, one invocation. It ends when the caller
/// closes it or, when the core identified the bridge connection that
/// opened it, when that connection closes. One opened without a connection
/// id ends once it has had no `next` for [`LEASE_MS`]: the crash fallback,
/// since no close of its connection is ever reported.
pub(crate) struct Iterator {
    pub(crate) caller_id: String,
    pub(crate) invocation_id: String,
    pub(crate) last_used_ms: i64,
    /// `next` calls waiting on it now.
    pub(crate) waiting: usize,
    /// The core's id of the bridge connection that opened it
    /// (`CallContext.connection_id`). Server-only: never on the wire.
    pub(crate) connection_id: Option<u64>,
}

impl Iterator {
    pub(crate) fn abandoned(&self, now: i64) -> bool {
        self.connection_id.is_none() && self.waiting == 0 && self.last_used_ms + LEASE_MS <= now
    }
}

/// A delivered, unsettled item, held by one iterator until it ends (a
/// close, or the close of its bridge connection, releases it at once) or
/// until [`LEASE_MS`] passes.
pub(crate) struct Lease {
    pub(crate) iterator_id: String,
    pub(crate) until_ms: i64,
}

fn path(binding: &Binding, caller_id: &str) -> PathBuf {
    binding.results().join(format!("{caller_id}.json"))
}

pub(crate) fn load(binding: &Binding, caller_id: &str) -> Result<InboxFile, Class> {
    Ok(load_existing(binding, caller_id)?.unwrap_or_default())
}

/// `caller_id`'s inbox file, or `None` when it has none.
pub(crate) fn load_existing(
    binding: &Binding,
    caller_id: &str,
) -> Result<Option<InboxFile>, Class> {
    registry::read_private(&path(binding, caller_id))?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| Class::RegistryUnreadable))
        .transpose()
}

pub(crate) fn save(binding: &Binding, caller_id: &str, file: &InboxFile) -> Result<(), Class> {
    #[cfg(test)]
    if FAIL_SAVE.with(|fail| fail.get()) {
        return Err(Class::StoreFailed);
    }
    registry::write(&path(binding, caller_id), file)
}

/// The invocations whose inbox slot `caller_id` has yet to settle. An inbox
/// that cannot be read names none, and the ledger evicts oldest first.
pub(crate) fn unsettled(binding: &Binding, caller_id: &str) -> std::collections::HashSet<String> {
    load(binding, caller_id)
        .map(|file| {
            file.slots
                .into_iter()
                .filter(|slot| slot.settled.is_none())
                .filter_map(|slot| slot.invocation_id)
                .collect()
        })
        .unwrap_or_default()
}

/// Drops the inbox slots of invocations the ledger no longer names
/// ([`registry::CallerRecord::remember`]): nothing can open or settle them
/// any more, so they would hold an active slot for good. A write that fails
/// keeps them, as before.
pub(crate) fn drop_invocations(binding: &Binding, caller_id: &str, dropped: &[String]) {
    if dropped.is_empty() {
        return;
    }
    let Ok(Some(mut file)) = load_existing(binding, caller_id) else {
        return;
    };
    let kept = file.slots.len();
    file.slots.retain(|slot| {
        slot.invocation_id
            .as_ref()
            .is_none_or(|id| !dropped.contains(id))
    });
    if file.slots.len() != kept {
        let _ = save(binding, caller_id, &file);
    }
}

#[cfg(test)]
thread_local! {
    /// A failpoint: every inbox write on this thread fails, as a crash
    /// before the commit would.
    pub(crate) static FAIL_SAVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn remove(binding: &Binding, caller_id: &str) {
    let _ = std::fs::remove_file(path(binding, caller_id));
}

impl InboxFile {
    /// Drops tombstones past seven days and reservations whose invoke
    /// never reported back. An unsettled item is never dropped.
    pub(crate) fn purge(&mut self, now: i64) -> bool {
        let before = self.slots.len();
        self.slots.retain(
            |slot| match (&slot.settled, &slot.item, &slot.invocation_id) {
                (Some(settled), _, _) => settled.at_ms + TOMBSTONE_MS > now,
                (None, None, None) => slot.reserved_at_ms + UNBOUND_MS > now,
                _ => true,
            },
        );
        self.slots.len() != before
    }

    fn unsettled(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.settled.is_none())
            .count()
    }

    /// Reserves a full slot for an invoke keyed by `key`, or reuses the one
    /// a retry of the same key already holds.
    pub(crate) fn reserve(&mut self, key: &str, now: i64) -> Result<(), Class> {
        if self
            .slots
            .iter()
            .any(|slot| slot.key.as_deref() == Some(key))
        {
            return Ok(());
        }
        if self.slots.len() >= RECORDS || self.unsettled() >= ACTIVE {
            return Err(Class::ReservationFull);
        }
        self.slots.push(Slot {
            key: Some(key.to_string()),
            invocation_id: None,
            reserved_at_ms: now,
            item: None,
            settled: None,
            later_reports: 0,
        });
        Ok(())
    }

    /// Ties the reservation to the invocation the invoke returned.
    pub(crate) fn bind(&mut self, key: &str, invocation_id: &str) {
        if self
            .slots
            .iter()
            .any(|slot| slot.invocation_id.as_deref() == Some(invocation_id))
        {
            self.slots
                .retain(|slot| slot.key.as_deref() != Some(key) || slot.invocation_id.is_some());
            return;
        }
        if let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| slot.key.as_deref() == Some(key))
        {
            slot.invocation_id = Some(invocation_id.to_string());
        }
    }

    /// Ties `key`'s reservation to `invocation_id` as [`Self::bind`] does,
    /// putting the slot back when it is gone: the invoke's bind is written
    /// late, after a failed write or a restart, and a release or purge may
    /// have let the unbound slot go meanwhile.
    pub(crate) fn bind_late(&mut self, key: &str, invocation_id: &str, now: i64) {
        self.bind(key, invocation_id);
        if self.slot_for(invocation_id).is_none() {
            self.slots.push(Slot {
                key: Some(key.to_string()),
                invocation_id: Some(invocation_id.to_string()),
                reserved_at_ms: now,
                item: None,
                settled: None,
                later_reports: 0,
            });
        }
    }

    /// Lets go of a reservation an invoke that failed made.
    pub(crate) fn unreserve(&mut self, key: &str) {
        self.slots
            .retain(|slot| slot.key.as_deref() != Some(key) || slot.invocation_id.is_some());
    }

    pub(crate) fn has_unbound(&self) -> bool {
        self.slots
            .iter()
            .any(|slot| slot.invocation_id.is_none() && slot.settled.is_none())
    }

    pub(crate) fn slot_for(&mut self, invocation_id: &str) -> Option<&mut Slot> {
        self.slots
            .iter_mut()
            .find(|slot| slot.invocation_id.as_deref() == Some(invocation_id))
    }

    pub(crate) fn has_message(&self, message_id: &str) -> bool {
        self.slots.iter().any(|slot| {
            slot.item
                .as_ref()
                .is_some_and(|item| item.message_id == message_id)
        })
    }

    pub(crate) fn by_item(&mut self, item_id: &str) -> Option<&mut Slot> {
        self.slots.iter_mut().find(|slot| {
            slot.item
                .as_ref()
                .is_some_and(|item| item.item_id == item_id)
        })
    }
}

impl Slot {
    pub(crate) fn delivery(&self) -> Option<InboxDelivery> {
        let item = self.item.as_ref()?;
        Some(InboxDelivery {
            item_id: item.item_id.clone(),
            message_id: item.message_id.clone(),
            sender: InboxSender {
                id: item.sender_id.clone(),
                name: item.sender_name.clone(),
            },
            text: if item.failure.is_some() {
                None
            } else {
                item.text.clone()
            },
            failure: item.failure.clone(),
            invocation_id: self.invocation_id.clone()?,
            sent_at: super::rfc3339(item.sent_at_ms),
        })
    }
}

/// The text of a report, or a typed terminal failure when it is over the
/// slot.
pub(crate) fn terminal(text: String) -> (Option<String>, Option<InboxFailure>) {
    let bytes = text.len() as u64;
    if bytes > SLOT_BYTES {
        (
            None,
            Some(InboxFailure {
                code: InboxFailureCode::ResultTooLarge,
                bytes,
                limit: SLOT_BYTES,
            }),
        )
    } else {
        (Some(text), None)
    }
}

/// Checks a settlement's metadata against the one fixed value of `kind`.
pub(crate) fn metadata_for(kind: &str, metadata: &InboxSettleMetadata) -> Result<String, Class> {
    let expected = if kind == ACK { JOURNALED } else { REJECTED };
    if metadata.relayflows_result == expected {
        Ok(metadata.relayflows_result.clone())
    } else {
        Err(Class::SettlementInvalid)
    }
}
