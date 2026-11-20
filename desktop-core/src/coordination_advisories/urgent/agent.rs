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
        // Mid-turn text needs the server's validated-quality decision, not
        // only the turn-end switch, and a session a local notice can reach.
        if advisory.dm_delivered || !advisory.notify_eligible || !delivery.reaches(target) {
            return ControlFlow::Continue(());
        }
        let prior = self
            .urgent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .agent_receipt(principal, target, &advisory.id);
        match prior {
            Some((Attempt::Delivered, _)) => {
                self.mark_urgent_shown(principal, target, &advisory.id, now);
                return ControlFlow::Continue(());
            }
            // Handed off (the core delivers it when the agent is free), in
            // doubt, or refused: never injected again. A refused note still
            // reaches the session at its turn's end.
            Some((Attempt::Reserved | Attempt::Uncertain | Attempt::Accepted | Attempt::Refused, _)) => {
                return ControlFlow::Continue(());
            }
            Some((Attempt::Retry, at)) if epoch.saturating_sub(at) < URGENT_RETRY_AFTER => {
                return ControlFlow::Continue(());
            }
            _ => {}
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
        self.save_urgent().await;
        if attempt == Attempt::Delivered && self.tokens.principal().as_ref() == Some(principal) {
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
        if journal.blocked {
            return None;
        }
        journal.prune(epoch);
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
