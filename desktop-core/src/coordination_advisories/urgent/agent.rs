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
        let record = self
            .urgent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .record_for(principal, target, advisory);
        if let Some(record) = record {
            let step = self
                .recorded(delivery, principal, target, advisory, record, (epoch, now))
                .await;
            if let Some(step) = step {
                return step;
            }
        }
        // Mid-turn text needs an urgent instance row, the server's
        // validated-quality decision, not only the turn-end switch, and a
        // session a local notice can reach.
        if !advisory.urgent() || !advisory.notify_eligible || !delivery.reaches(target) {
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
        let attempt = attempt_of(delivery.inject(target, &advisory.key, &text).await);
        // Keep the observed result in memory even if this save fails; the
        // earlier durable reservation prevents a blind restart retry.
        self.urgent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .set_agent(principal, target, &advisory.key, attempt.clone(), epoch);
        let saved = self.save_urgent().await;
        if saved
            && attempt == Attempt::Delivered
            && self.tokens.principal().as_ref() == Some(principal)
        {
            self.mark_urgent_shown(principal, target, advisory, now);
        }
        ControlFlow::Continue(())
    }

    /// A note with an urgent receipt, never injected blindly: `None` only
    /// for its own `Retry` past its wait, which may be handed over again.
    /// `(epoch, now)` is the pass's clock, in epoch seconds and as an instant.
    async fn recorded(
        &self,
        delivery: &dyn Delivery,
        principal: &Principal,
        target: &Target,
        advisory: &Advisory,
        record: journal::Record,
        (epoch, now): (u64, Instant),
    ) -> Option<ControlFlow<()>> {
        let (attempt, at) = match record {
            // An instance row whose id has a legacy receipt is never handed
            // over under its own key while that receipt exists. Held: the
            // legacy key is observed, and its delivery shows a note that may
            // not be this instance, so it marks nothing shown. Settled: the
            // turn end hands the row out once, labelled (journal::turn_end).
            journal::Record::Legacy(attempt) if journal::unsettled(&attempt) => {
                let held = Held {
                    key: &advisory.id,
                    shows: None,
                };
                return Some(
                    self.observe_held(delivery, principal, target, held, epoch, now)
                        .await,
                );
            }
            journal::Record::Legacy(_) => return Some(ControlFlow::Continue(())),
            journal::Record::Own(attempt, at) => (attempt, at),
        };
        match attempt {
            // Shown (and so acked) only once Delivered is durable; a save that
            // failed earlier is retried here, never answered by an inject.
            Attempt::Delivered => {
                if self.save_urgent().await && self.urgent_still_current(principal) {
                    self.mark_urgent_shown(principal, target, advisory, now);
                }
                Some(ControlFlow::Continue(()))
            }
            // Handed over (the core delivers it when the agent is free),
            // reserved before a crash, or in doubt: observed, never injected
            // again, whatever the eligibility or reach is now.
            Attempt::Reserved | Attempt::Uncertain | Attempt::Accepted => {
                let held = Held {
                    key: &advisory.key,
                    shows: Some(advisory),
                };
                Some(
                    self.observe_held(delivery, principal, target, held, epoch, now)
                        .await,
                )
            }
            // Refused, or released to the turn end: never injected again; it
            // reaches the session at its turn's end.
            Attempt::Refused | Attempt::Released => Some(ControlFlow::Continue(())),
            Attempt::Retry if epoch.saturating_sub(at) < URGENT_RETRY_AFTER => {
                Some(ControlFlow::Continue(()))
            }
            Attempt::Retry => None,
        }
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
            .is_some_and(|rows| rows.iter().any(|a| a.key == advisory.key));
        let taken = state
            .taken
            .get(target)
            .is_some_and(|rows| rows.contains_key(&advisory.key));
        if state.principal.as_ref() != Some(principal) || !waiting || taken {
            return None;
        }
        let mut journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
        if journal.blocked() {
            return None;
        }
        journal.prune(epoch, &state.cached());
        let before = journal.saved.clone();
        journal.set_agent(principal, target, &advisory.key, Attempt::Reserved, epoch);
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
