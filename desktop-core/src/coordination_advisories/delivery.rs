//! Handing the cache to the core's stop hooks, once.

use super::*;

impl Advisories {
    /// [`TurnEndAdvisories::take`] at `now`.
    pub fn take_at(&self, target: &Target, now: Instant) -> Option<TurnEndAdvisory> {
        if !self.enabled() {
            return None;
        }
        let current = self.tokens.principal();
        let mut state = self.state();
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
        for advisory in waiting {
            let line = line(&advisory);
            if size + line.len() + 1 <= MAX_TEXT && kept.is_empty() {
                size += line.len() + 1;
                ids.push(advisory.id.clone());
                lines.push(line);
            } else {
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
