//! Handing the cache to the core's stop hooks, once.

use super::*;

/// Appended to the line of a note released to the turn end after its
/// provider held it unconfirmed: the provider may have delivered it as well.
pub(super) const MAY_REPEAT: &str = "(This may repeat an earlier delivery of the same advisory.)";

/// How a turn end treats one waiting row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hand {
    /// Kept back: a provider may hold it, or who does is unknown.
    Keep,
    /// Handed out with [`MAY_REPEAT`]: it may have reached the session before.
    Repeat,
    Plain,
}

impl Advisories {
    /// [`TurnEndAdvisories::take`] at `now`.
    pub fn take_at(&self, target: &Target, now: Instant) -> Option<TurnEndAdvisory> {
        if !self.enabled() {
            return None;
        }
        let current = self.tokens.principal();
        // Serialize ownership with an urgent reservation. Both paths hold
        // state before consulting the urgent journal, so neither can take
        // an ID after the other has claimed its delivery.
        let mut state = self.state();
        // Signed out, or another account: what was cached is not theirs.
        let principal = current.filter(|p| state.belong_to(Some(p)))?;
        let waiting = state.waiting.remove(target)?;
        let mut lines = Vec::new();
        let mut handed = Vec::new();
        let mut kept = Vec::new();
        // Every line, the footer and the line breaks fit in MAX_TEXT; what
        // does not fit waits for the next turn.
        let mut size = FOOTER.len();
        let mut overflow = false;
        for advisory in waiting {
            let hand = self.hand(&state, &principal, target, &advisory);
            if hand == Hand::Keep {
                kept.push(advisory);
                continue;
            }
            let line = match hand {
                Hand::Repeat => format!("{} {MAY_REPEAT}", line(&advisory)),
                _ => line(&advisory),
            };
            if size + line.len() + 1 <= MAX_TEXT && !overflow {
                size += line.len() + 1;
                lines.push(line);
                handed.push(advisory);
            } else {
                overflow = true;
                kept.push(advisory);
            }
        }
        if !kept.is_empty() {
            state.waiting.insert(target.clone(), kept);
        }
        if handed.is_empty() {
            return None;
        }
        let taken = state.taken.entry(target.clone()).or_default();
        for advisory in &handed {
            taken.insert(advisory.key.clone(), Taken::of(now, advisory));
        }
        lines.push(FOOTER.to_string());
        Some(TurnEndAdvisory {
            text: lines.join("\n"),
            ids: handed.into_iter().map(|a| a.key).collect(),
        })
    }

    /// How the turn end treats `advisory` for `target`: kept back while a
    /// provider may hold it (its own urgent receipt, or for an instance row
    /// the held legacy receipt of its id) or while unusable storage leaves
    /// that unknown; labelled when it was released to the turn end, when a
    /// legacy receipt or legacy hand-out of its id may have shown it, or when
    /// no local notice reaches the session while storage is unusable.
    fn hand(
        &self,
        state: &State,
        principal: &Principal,
        target: &Target,
        advisory: &Advisory,
    ) -> Hand {
        let recorded = self
            .urgent
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .turn_end(principal, target, advisory);
        // Receipt storage unusable: who holds an urgent note is unknown,
        // so the notes the urgent path would attempt wait here too.
        if recorded == Hand::Keep || self.urgent_unknown(target, advisory) {
            Hand::Keep
        } else if recorded == Hand::Repeat
            || state.shown_as_legacy(target, advisory)
            || self.urgent_unknown_unreached(target, advisory)
        {
            Hand::Repeat
        } else {
            Hand::Plain
        }
    }

    /// Marks `keys` shown to `target`'s session; the acks go out from
    /// [`Advisories::send_shown`] or the next poll.
    pub(super) fn mark_shown(&self, target: &Target, keys: &[String]) -> bool {
        let mut state = self.state();
        let Some(taken) = state.taken.get_mut(target) else {
            return false;
        };
        let mut any = false;
        for key in keys {
            if let Some(taken) = taken.get_mut(key) {
                if !taken.acked {
                    taken.shown = true;
                    any = true;
                }
            }
        }
        any
    }

    /// Acks what was just shown, now.
    pub async fn send_shown(&self, now: Instant) {
        if let Some(token) = self.tokens.token().await {
            self.send_due_acks(&token, now).await;
        }
    }
}
