//! Handing the cache to the core's stop hooks, once.

use super::*;

/// Appended to the line of a note released to the turn end after its
/// provider held it unconfirmed: the provider may have delivered it as well.
pub(super) const MAY_REPEAT: &str = "(This may repeat an earlier delivery of the same advisory.)";

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
        let (provider_owned, released) = current
            .as_ref()
            .map(|p| self.provider_owned(p, target))
            .unwrap_or_default();
        // Signed out, or another account: what was cached is not theirs.
        if current.is_none() || !state.belong_to(current.as_ref()) {
            return None;
        }
        let waiting = state.waiting.remove(target)?;
        let mut lines = Vec::new();
        let mut ids = Vec::new();
        let mut kept = Vec::new();
        // Every line, the footer and the line breaks fit in MAX_TEXT; what
        // does not fit waits for the next turn.
        let mut size = FOOTER.len();
        let mut overflow = false;
        for advisory in waiting {
            // Receipt storage unusable: who holds an urgent note is unknown,
            // so the notes the urgent path would attempt wait here too.
            if provider_owned.contains(&advisory.id) || self.urgent_unknown(target, &advisory) {
                kept.push(advisory);
                continue;
            }
            let line = if released.contains(&advisory.id)
                || self.urgent_unknown_unreached(target, &advisory)
            {
                format!("{} {MAY_REPEAT}", line(&advisory))
            } else {
                line(&advisory)
            };
            if size + line.len() + 1 <= MAX_TEXT && !overflow {
                size += line.len() + 1;
                ids.push(advisory.id.clone());
                lines.push(line);
            } else {
                overflow = true;
                kept.push(advisory);
            }
        }
        if !kept.is_empty() {
            state.waiting.insert(target.clone(), kept);
        }
        if ids.is_empty() {
            return None;
        }
        let taken = state.taken.entry(target.clone()).or_default();
        for id in &ids {
            taken.insert(id.clone(), Taken::new(now));
        }
        lines.push(FOOTER.to_string());
        Some(TurnEndAdvisory {
            text: lines.join("\n"),
            ids,
        })
    }

    /// Marks `ids` shown to `target`'s session; the acks go out from
    /// [`Advisories::send_shown`] or the next poll.
    pub(super) fn mark_shown(&self, target: &Target, ids: &[String]) -> bool {
        let mut state = self.state();
        let Some(taken) = state.taken.get_mut(target) else {
            return false;
        };
        let mut any = false;
        for id in ids {
            if let Some(taken) = taken.get_mut(id) {
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
