//! Held notes the provider never confirms are released to the turn-end
//! hand-out, once, labelled as possibly a repeat; and a blocked journal is
//! loaded again on each pass.

use super::*;

/// How long a provider owns a held note it never confirms (seconds), well
/// under the server's 7-day retention: past it the note goes to the turn end.
pub(in crate::coordination_advisories) const ACCEPTED_OWNED_FOR: u64 = 24 * HOUR;
/// A provider with no observer never answers Delivered: its held note goes
/// to the turn end this long (seconds) after its receipt was recorded.
pub(super) const UNOBSERVED_GRACE: u64 = 30 * 60;

/// Whether a note held since `at` (epoch seconds) is released, given what was
/// just observed. Only `Unsupported` gets the short grace: `Fenced` also
/// covers a core that is stopping, which may still deliver after restart. A
/// clock that went back counts as no time held.
pub(super) fn release_due(at: u64, seen: &Observation, epoch: u64) -> bool {
    let held_for = epoch.saturating_sub(at);
    match seen {
        Observation::Delivered => false,
        Observation::Unsupported => held_for >= UNOBSERVED_GRACE,
        Observation::Pending | Observation::InDoubt | Observation::Fenced => {
            held_for >= ACCEPTED_OWNED_FOR
        }
    }
}

impl Advisories {
    /// Records the held note as released, saved before the journal in memory
    /// changes, so the turn end never hands out a release a restart would not
    /// have. The urgent path never injects a released note again; the turn
    /// end hands it out once and it is acked like any turn-end row. Runs
    /// under the urgent pass's lock, the journal's only writer.
    pub(super) async fn release_held(
        &self,
        principal: &Principal,
        target: &Target,
        id: &str,
        epoch: u64,
    ) {
        let mut snapshot = {
            let journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
            if journal.blocked() {
                return;
            }
            journal.clone()
        };
        snapshot.set_agent(principal, target, id, Attempt::Released, epoch);
        let directory = self.app_data_dir.clone();
        let saved = tokio::task::spawn_blocking(move || snapshot.save(&directory))
            .await
            .unwrap_or(false);
        let mut journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
        if !saved {
            journal.status = SAVE_FAILED_STATUS;
            return;
        }
        journal.set_agent(principal, target, id, Attempt::Released, epoch);
    }

    /// While unusable storage blocks urgent delivery, loads the file again:
    /// a fault that cleared, or a file the person moved aside, unblocks
    /// without a restart; one still unusable keeps blocking, with its fault
    /// named again. A blocked journal holds no receipts, so nothing is lost.
    pub(super) async fn retry_blocked_journal(&self) {
        if !self
            .urgent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .blocked()
        {
            return;
        }
        let directory = self.app_data_dir.clone();
        let Ok(loaded) = tokio::task::spawn_blocking(move || Journal::load(&directory)).await
        else {
            return;
        };
        let mut journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
        if loaded.blocked() {
            journal.fault = loaded.fault;
        } else {
            *journal = loaded;
        }
    }

    /// While blocked, notes which candidate sessions no local notice reaches,
    /// the same test the urgent path makes before it reserves a note: the
    /// turn end keeps back urgent notes only for the others.
    pub(super) fn record_reach(&self, delivery: &dyn Delivery, candidates: &[(Target, Advisory)]) {
        let blocked = self
            .urgent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .blocked();
        // Asked with no lock held: the core's store has locks of its own.
        let unreached = if blocked {
            candidates
                .iter()
                .map(|(target, _)| target)
                .filter(|target| !delivery.reaches(target))
                .cloned()
                .collect()
        } else {
            HashSet::new()
        };
        self.urgent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .unreached = unreached;
    }

    /// Whether the turn end keeps back `advisory` for `target` because
    /// unusable storage leaves unknown who holds it: an urgent note the urgent
    /// path would attempt, for a session a local notice may reach.
    pub(in crate::coordination_advisories) fn urgent_unknown(
        &self,
        target: &Target,
        advisory: &Advisory,
    ) -> bool {
        let journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
        blocked_urgent(&journal, advisory) && !journal.unreached.contains(target)
    }

    /// Whether the turn end hands out `advisory` for `target` only because no
    /// local notice reaches the session now, while unusable storage may record
    /// that a provider still holds it: such a line carries the may-repeat label.
    pub(in crate::coordination_advisories) fn urgent_unknown_unreached(
        &self,
        target: &Target,
        advisory: &Advisory,
    ) -> bool {
        let journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
        blocked_urgent(&journal, advisory) && journal.unreached.contains(target)
    }
}

/// A note the urgent path would attempt, or one a build before instances
/// would have (an unreadable journal may hold its legacy receipt), while the
/// journal is blocked.
fn blocked_urgent(journal: &Journal, advisory: &Advisory) -> bool {
    journal.blocked()
        && advisory.priority_one()
        && advisory.notify_eligible
        && !advisory.dm_delivered
}
