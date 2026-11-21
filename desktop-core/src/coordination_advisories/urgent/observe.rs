//! Notes already handed over: read the provider's own record, never submit
//! them again. Only an observed delivery settles one.

use super::*;

/// What the core reports for a note it was handed, without submitting
/// anything. Mirrors the core's `NoticeObservation`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::coordination_advisories) enum Observation {
    /// The provider's own record shows the note.
    Delivered,
    /// Still queued, behind a turn, being recorded, or the queue is unreadable.
    Pending,
    InDoubt,
    /// This provider has no observer; the note stays held.
    Unsupported,
    /// The core is stopping, or the registration or workspace changed.
    Fenced,
}

/// At most one observation per note in this long (seconds); memory only.
const OBSERVE_EVERY: u64 = 60;

impl Advisories {
    /// A held note (accepted, reserved before a crash, or in doubt): observe
    /// it, never inject it again. Only an observed `Delivered` changes the
    /// receipt: journal Delivered, saved, then shown, which gives exactly one
    /// ack. `Break` when the account or the switch changed meanwhile.
    pub(super) async fn observe_held(
        &self,
        delivery: &dyn Delivery,
        principal: &Principal,
        target: &Target,
        id: &str,
        epoch: u64,
        now: Instant,
    ) -> ControlFlow<()> {
        if !self.observe_due(principal, target, id, epoch) {
            return ControlFlow::Continue(());
        }
        let seen = delivery.observe(target, id).await;
        if !self.urgent_still_current(principal) {
            return ControlFlow::Break(());
        }
        if seen != Observation::Delivered {
            return ControlFlow::Continue(());
        }
        let prior = {
            let mut journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
            let prior = journal.agent_receipt(principal, target, id);
            journal.set_agent(principal, target, id, Attempt::Delivered, epoch);
            prior
        };
        if !self.save_urgent().await {
            // Not durable: put the held receipt back, so nothing is shown or
            // acked on a record a restart would not have.
            if let Some((attempt, at)) = prior {
                self.urgent
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .set_agent(principal, target, id, attempt, at);
            }
            return ControlFlow::Continue(());
        }
        if self.urgent_still_current(principal) {
            self.mark_urgent_shown(principal, target, id, now);
        }
        ControlFlow::Continue(())
    }

    /// Whether this note may be observed now, recording the attempt.
    fn observe_due(&self, principal: &Principal, target: &Target, id: &str, epoch: u64) -> bool {
        let key = (receipts::Scope::from(principal), target.clone(), id.to_string());
        let mut journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
        if journal
            .observed
            .get(&key)
            .is_some_and(|at| epoch.saturating_sub(*at) < OBSERVE_EVERY)
        {
            return false;
        }
        journal.observed.insert(key, epoch);
        true
    }
}
