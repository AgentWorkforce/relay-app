//! Urgent notes handed to agents: reserve durably, inject once, record the
//! outcome.

use super::*;

impl Advisories {
    /// Hands one urgent note to its agent, at most once. `Break` when the
    /// switch or the account changed, which stops the whole pass.
    pub(super) async fn deliver_urgent_agent(
        &self,
        delivery: &dyn Delivery,
        principal: &Principal,
        target: &Target,
        advisory: &Advisory,
        epoch: u64,
        now: Instant,
    ) -> ControlFlow<()> {
        if advisory.dm_delivered {
            return ControlFlow::Continue(());
        }
        let prior = self
            .urgent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .agent_receipt(principal, target, &advisory.id);
        match prior {
            // Shown (and so acked) only once Delivered is durable; a save that
            // failed earlier is retried here, never answered by an inject.
            Some((Attempt::Delivered, _)) => {
                if self.save_urgent().await && self.urgent_still_current(principal) {
                    self.mark_urgent_shown(principal, target, &advisory.id, now);
                }
                return ControlFlow::Continue(());
            }
            // Handed over (the core delivers it when the agent is free),
            // reserved before a crash, or in doubt: observed, never injected
            // again, whatever the eligibility or reach is now.
            Some((Attempt::Reserved | Attempt::Uncertain | Attempt::Accepted, _)) => {
                return self
                    .observe_held(delivery, principal, target, &advisory.id, epoch, now)
                    .await;
            }
            // Refused, or released to the turn end: never injected again; it
            // reaches the session at its turn's end.
            Some((Attempt::Refused | Attempt::Released, _)) => return ControlFlow::Continue(()),
            Some((Attempt::Retry, at)) if epoch.saturating_sub(at) < URGENT_RETRY_AFTER => {
                return ControlFlow::Continue(());
            }
            _ => {}
        }
        // Mid-turn text needs the server's validated-quality decision, not
        // only the turn-end switch, and a session a local notice can reach.
        if !advisory.notify_eligible || !delivery.reaches(target) {
            return ControlFlow::Continue(());
        }
        let Some(before) = self.reserve_urgent(principal, target, advisory, epoch) else {
            return ControlFlow::Continue(());
        };
        if !self.save_urgent().await {
            self.urgent.lock().unwrap_or_else(|p| p.into_inner()).saved = before;
            return ControlFlow::Continue(());
        }
        if !self.urgent_still_current(principal) {
            // Nothing was injected: undo the reservation, or the note would
            // be held from both paths until the receipt is pruned.
            self.urgent.lock().unwrap_or_else(|p| p.into_inner()).saved = before;
            self.save_urgent().await;
            return ControlFlow::Break(());
        }
        let text = format!("{}\n{FOOTER}", line(advisory));
        let attempt = attempt_of(delivery.inject(target, &advisory.id, &text).await);
        // Keep the observed result in memory even if this save fails; the
        // earlier durable reservation prevents a blind restart retry.
        self.urgent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .set_agent(principal, target, &advisory.id, attempt.clone(), epoch);
        let saved = self.save_urgent().await;
        if saved
            && attempt == Attempt::Delivered
            && self.tokens.principal().as_ref() == Some(principal)
        {
            self.mark_urgent_shown(principal, target, &advisory.id, now);
        }
        ControlFlow::Continue(())
    }

    /// Reserves the note in the journal (not yet saved) and returns the
    /// journal as it was, so a failed save can restore it. `None` when the
    /// note is no longer waiting, a stop hook took it after the snapshot, the
    /// account changed or the journal is blocked.
    fn reserve_urgent(
        &self,
        principal: &Principal,
        target: &Target,
        advisory: &Advisory,
        epoch: u64,
    ) -> Option<Saved> {
        let state = self.state();
        // Claim under the same lock before persisting.
        let waiting = state
            .waiting
            .get(target)
            .is_some_and(|rows| rows.iter().any(|a| a.id == advisory.id));
        let taken = state
            .taken
            .get(target)
            .is_some_and(|rows| rows.contains_key(&advisory.id));
        if state.principal.as_ref() != Some(principal) || !waiting || taken {
            return None;
        }
        let mut journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
        if journal.blocked() {
            return None;
        }
        journal.prune(epoch, &state.cached());
        let before = journal.saved.clone();
        journal.set_agent(principal, target, &advisory.id, Attempt::Reserved, epoch);
        Some(before)
    }
}

/// What the journal records for an injection outcome.
fn attempt_of(outcome: InjectOutcome) -> Attempt {
    match outcome {
        InjectOutcome::Delivered => Attempt::Delivered,
        InjectOutcome::Accepted(_) | InjectOutcome::WaitingForTurn(_) => Attempt::Accepted,
        InjectOutcome::InDoubt(_) => Attempt::Uncertain,
        InjectOutcome::Refused(_) => Attempt::Refused,
        InjectOutcome::Waiting(_) | InjectOutcome::Retry(_) => Attempt::Retry,
    }
}
