//! The delivery reader that fills the inbox (`docs/service-callers.md`
//! section 8): each admitted invoke's outcome, the core's delivery poll,
//! and the writes that keep results and releases.

use super::{keep_releases, take_releases, Admitted, Claims};
use crate::contract::service_callers::{InboxFailure, InboxFailureCode};
use crate::service_callers::inbox::{self, Item};
use crate::service_callers::registry::{self, Binding};
use crate::service_callers::{Class, ServiceCallers};
use agent_relay_node::relay::contract::spawn_control::SpawnControlInvocation;
use agent_relay_node::relay::protocol::RpcError;
use agent_relay_node::relay::workspace::Secret;

impl ServiceCallers {
    pub(super) fn record_outcome(
        &self,
        binding: &Binding,
        admitted: &mut Admitted,
        outcome: &Result<SpawnControlInvocation, RpcError>,
        claims: &mut Claims,
    ) -> Result<(), Class> {
        let caller_id = admitted.caller_id.as_str();
        if let Some(reservation) = admitted.reservation.as_mut() {
            let Ok(invocation) = outcome else {
                return reservation.settle(claims, None);
            };
            // The ledger names the invocation, and the slot it is still to
            // be bound to, before the claim ends: should the bind not reach
            // the disk, the caller's next inbox load writes it, after a
            // restart as well, so the worker's report is still kept.
            let ledgered = self.load_current(binding).and_then(|mut file| {
                let index = file.by_id(caller_id).ok_or(Class::CallerUnknown)?;
                let caller = &mut file.callers[index];
                let unsettled = inbox::unsettled(binding, caller_id);
                let dropped =
                    caller.remember(&invocation.id, invocation.agent_id.as_deref(), |id| {
                        unsettled.contains(id)
                    });
                caller.unbound_slot(&invocation.id, &reservation.key);
                registry::save(binding, &file)?;
                inbox::drop_invocations(binding, caller_id, &dropped);
                Ok((file, index))
            });
            let bound = reservation.settle(claims, Some(&invocation.id));
            let (mut file, index) = ledgered?;
            bound?;
            file.callers[index].slots_bound([invocation.id.as_str()]);
            // A failed write keeps the pending bind, which the next inbox
            // load finds already on disk and clears.
            let _ = registry::save(binding, &file);
            return Ok(());
        }
        let Ok(invocation) = outcome else {
            return Ok(());
        };
        let mut file = self.load_current(binding)?;
        let index = file.by_id(caller_id).ok_or(Class::CallerUnknown)?;
        // A release's receipt names no agent: the worker admission found
        // this caller owns is the one the ledger keeps, whatever the
        // receipt says.
        let worker = admitted
            .worker
            .as_deref()
            .or(invocation.agent_id.as_deref());
        let remembers = match admitted.method {
            "spawn.invoke" => true,
            // A release answers with an invocation of its own, for a worker
            // admission already found this caller owns: the ledger names it
            // so the caller can poll that release with `spawn.status`.
            "spawn.release" => true,
            "spawn.status" => file.callers[index].owns(&invocation.id),
            _ => false,
        };
        if !remembers {
            return Ok(());
        }
        let unsettled = inbox::unsettled(binding, caller_id);
        let dropped =
            file.callers[index].remember(&invocation.id, worker, |id| unsettled.contains(id));
        registry::save(binding, &file)?;
        inbox::drop_invocations(binding, caller_id, &dropped);
        Ok(())
    }

    /// Reads every caller's deliveries once, keeps its own workers' reports
    /// and acknowledges what it kept or dropped.
    pub(crate) async fn poll_once(&self) {
        let Ok(binding) = Binding::current(&*self.host) else {
            return;
        };
        let callers: Vec<(String, String, Secret)> = {
            let _files = self.files();
            match registry::load(&binding) {
                Ok(file) => file
                    .callers
                    .iter()
                    .filter_map(|caller| {
                        Some((
                            caller.caller_id.clone(),
                            caller.agent_id.clone(),
                            caller.token.clone()?,
                        ))
                    })
                    .collect(),
                Err(_) => return,
            }
        };
        for (caller_id, agent_id, token) in callers {
            let base_url = binding.workspace.base_url.clone();
            let incoming = match self.host.deliveries(&base_url, &token).await {
                Ok(incoming) => incoming,
                Err(_) => {
                    self.host.log("inbox: deliveries could not be read");
                    continue;
                }
            };
            if incoming.is_empty() {
                continue;
            }
            let (acks, kept) = self.keep(&binding, &caller_id, &agent_id, incoming);
            for delivery_id in acks {
                if self
                    .host
                    .ack(&base_url, &token, &delivery_id)
                    .await
                    .is_err()
                {
                    self.host.log("inbox: a delivery could not be acknowledged");
                }
            }
            if kept > 0 {
                self.changed.notify_waiters();
            }
        }
    }

    /// Files `incoming` into the caller's inbox; returns the deliveries to
    /// acknowledge and how many items were kept.
    fn keep(
        &self,
        binding: &Binding,
        caller_id: &str,
        agent_id: &str,
        incoming: Vec<crate::service_callers::host::Incoming>,
    ) -> (Vec<String>, u64) {
        let _files = self.files();
        let Ok(file) = registry::load(binding) else {
            return (Vec::new(), 0);
        };
        let Some(record) = file.by_id(caller_id).map(|index| &file.callers[index]) else {
            return (Vec::new(), 0);
        };
        if record.agent_id != agent_id {
            return (Vec::new(), 0);
        }
        let Ok(mut inbox) = self.load_inbox(binding, caller_id) else {
            return (Vec::new(), 0);
        };
        let now = self.host.now_ms();
        let spawned = self.host.spawned(&binding.workspace);
        let mut acks = Vec::new();
        let mut new_items: Vec<(String, String)> = Vec::new();
        let (mut dropped, mut later) = (0u64, 0u64);
        for message in incoming {
            if inbox.has_message(&message.message_id) {
                acks.push(message.delivery_id);
                continue;
            }
            let worker = message.sender_id.as_deref().and_then(|sender| {
                spawned.iter().find(|spawned| {
                    spawned.agent_id == sender
                        && spawned.requester_id.as_deref() == Some(agent_id)
                        && record.owns(&spawned.invocation_id)
                })
            });
            let Some(worker) = worker else {
                // An invoke still on its way may own this sender: wait.
                if inbox.has_unbound() {
                    continue;
                }
                dropped += 1;
                acks.push(message.delivery_id);
                continue;
            };
            let Some(slot) = inbox.slot_for(&worker.invocation_id) else {
                dropped += 1;
                acks.push(message.delivery_id);
                continue;
            };
            if slot.item.is_some() {
                slot.later_reports += 1;
                later += 1;
                acks.push(message.delivery_id);
                continue;
            }
            let Some(item_id) =
                crate::service_callers::random_hex(16).map(|hex| format!("item_{hex}"))
            else {
                continue;
            };
            let (text, failure) = inbox::terminal(message.text);
            slot.item = Some(Item {
                item_id,
                message_id: message.message_id,
                sender_id: worker.agent_id.clone(),
                sender_name: worker.name.clone(),
                text,
                failure,
                sent_at_ms: message.created_at_ms.unwrap_or(now),
            });
            new_items.push((message.delivery_id, worker.invocation_id.clone()));
        }
        let changed = !new_items.is_empty() || later > 0;
        let mut kept = new_items.len() as u64;
        if changed && inbox::save(binding, caller_id, &inbox).is_err() {
            // The reserved slot still takes a small typed failure.
            for (_, invocation_id) in &new_items {
                if let Some(item) = inbox
                    .slot_for(invocation_id)
                    .and_then(|slot| slot.item.as_mut())
                {
                    let bytes = item.text.as_ref().map_or(0, |text| text.len() as u64);
                    item.text = None;
                    item.failure.get_or_insert(InboxFailure {
                        code: InboxFailureCode::ResultWriteFailed,
                        bytes,
                        limit: inbox::SLOT_BYTES,
                    });
                }
            }
            if inbox::save(binding, caller_id, &inbox).is_err() {
                self.audit(
                    binding,
                    Some(caller_id),
                    "inbox.deliver",
                    Class::StoreFailed.name(),
                    None,
                );
                // Nothing kept: leave every new item with Relaycast.
                new_items.clear();
                kept = 0;
            }
        }
        acks.extend(new_items.into_iter().map(|(delivery_id, _)| delivery_id));
        if kept > 0 {
            self.audit(
                binding,
                Some(caller_id),
                "inbox.deliver",
                "kept",
                Some(kept),
            );
        }
        if dropped > 0 {
            self.audit(
                binding,
                Some(caller_id),
                "inbox.deliver",
                "dropped",
                Some(dropped),
            );
        }
        if later > 0 {
            self.audit(
                binding,
                Some(caller_id),
                "inbox.deliver",
                "later_report",
                Some(later),
            );
        }
        (acks, kept)
    }

    /// `caller_id`'s inbox, with the releases a failed write left pending
    /// and the binds the ledger still names
    /// ([`registry::LedgerEntry::slot_key`]) written first; until that write
    /// succeeds the load fails and they stay pending. Under the files lock,
    /// for a caller in the registry.
    pub(crate) fn load_inbox(
        &self,
        binding: &Binding,
        caller_id: &str,
    ) -> Result<inbox::InboxFile, Class> {
        let mut file = inbox::load(binding, caller_id)?;
        let pending = take_releases(&self.releases, binding, caller_id);
        let unbound = registry::load(binding).ok().and_then(|registry| {
            let index = registry.by_id(caller_id)?;
            let slots = registry.callers[index].unbound_slots();
            (!slots.is_empty()).then_some((registry, index, slots))
        });
        if pending.is_empty() && unbound.is_none() {
            return Ok(file);
        }
        for key in &pending {
            file.unreserve(key);
        }
        if let Some((_, _, slots)) = &unbound {
            let now = self.host.now_ms();
            for (invocation_id, key) in slots {
                file.bind_late(key, invocation_id, now);
            }
        }
        if let Err(class) = inbox::save(binding, caller_id, &file) {
            keep_releases(&self.releases, binding, caller_id, pending);
            return Err(class);
        }
        if let Some((mut registry, index, slots)) = unbound {
            registry.callers[index].slots_bound(slots.iter().map(|(id, _)| id.as_str()));
            // A failed write keeps them pending; binding them again is a
            // no-op.
            let _ = registry::save(binding, &registry);
        }
        Ok(file)
    }
}
