//! Polling: filling the cache from the read API, and sending acks due.

use super::*;

impl Advisories {
    /// One poll over `sessions`, the live sessions the core can reach at a
    /// turn's end: at most [`SESSIONS_PER_TICK`] of them, then the acks due.
    pub async fn poll(&self, sessions: &[Target], now: Instant) {
        if !self.enabled() {
            return;
        }
        let principal = self.tokens.principal();
        {
            let mut state = self.state();
            state.belong_to(principal.as_ref());
            state.prune(&sessions.iter().collect(), now);
        }
        if principal.is_none() {
            return;
        }
        let Some(token) = self.tokens.token().await else {
            return;
        };
        let due = {
            let mut state = self.state();
            // The account changed while the token was fetched.
            if !state.belong_to(Some(&token.principal)) {
                return;
            }
            if state.paused_until.is_some_and(|until| now < until) {
                Vec::new()
            } else {
                state.paused_until = None;
                let start = state.cursor % sessions.len().max(1);
                let ready: Vec<Target> = sessions
                    .iter()
                    .cycle()
                    .skip(start)
                    .take(sessions.len())
                    .filter(|target| {
                        state
                            .sessions
                            .get(*target)
                            .and_then(|b| b.until)
                            .is_none_or(|until| until <= now)
                    })
                    .take(SESSIONS_PER_TICK)
                    .cloned()
                    .collect();
                state.cursor = start + SESSIONS_PER_TICK.min(sessions.len());
                ready
            }
        };
        let mut token = token;
        let mut renewed = false;
        for target in &due {
            let mut listing = self.api.list(&token, target).await;
            if listing == Listing::Unauthorized && !renewed {
                // One new token, once a tick: never a loop.
                renewed = true;
                self.tokens.refused(&token);
                match self.tokens.token().await {
                    Some(fresh) => {
                        token = fresh;
                        listing = self.api.list(&token, target).await;
                    }
                    None => listing = Listing::Off,
                }
            }
            match listing {
                Listing::Advisories(advisories) => {
                    self.listed(&token.principal, target, advisories, now)
                }
                Listing::Failed => {
                    let mut state = self.state();
                    let backoff = state.sessions.entry(target.clone()).or_default();
                    let wait = SESSION_BACKOFF
                        .saturating_mul(1 << backoff.failures.min(6))
                        .min(SESSION_BACKOFF_MAX);
                    backoff.failures += 1;
                    backoff.until = Some(now + wait);
                }
                Listing::Unauthorized | Listing::Off => {
                    if listing == Listing::Unauthorized {
                        self.tokens.refused(&token);
                    }
                    self.state().paused_until = Some(now + ROUTE_OFF_BACKOFF);
                    break;
                }
            }
        }
        self.send_due_acks(&token, now).await;
    }

    /// Records what the pipeline lists for `target` now, for `principal`:
    /// a listing made for an account no longer signed in, or after the
    /// setting was turned off, is dropped.
    pub(super) fn listed(
        &self,
        principal: &Principal,
        target: &Target,
        advisories: Vec<Advisory>,
        now: Instant,
    ) {
        let current = self.tokens.principal();
        let mut state = self.state();
        if !self.enabled()
            || current.as_ref() != Some(principal)
            || state.principal.as_ref() != Some(principal)
        {
            return;
        }
        state.sessions.remove(target);
        let listed: HashSet<String> = advisories.iter().map(|a| a.id.clone()).collect();
        // Handed out and no longer listed: acked or expired upstream.
        if let Some(taken) = state.taken.get_mut(target) {
            taken.retain(|id, taken| listed.contains(id) || (taken.shown && !taken.acked));
        }
        let mut waiting = Vec::new();
        for advisory in advisories {
            let taken = state
                .taken
                .get_mut(target)
                .and_then(|t| t.get_mut(&advisory.id));
            match taken {
                // Shown, or acked: never again, whatever the ack's retries.
                // Taken and never confirmed shown: again once 30 minutes
                // passed and it is still listed.
                Some(taken) if taken.acked || taken.sending || taken.shown => continue,
                Some(taken) if now.duration_since(taken.at) < REDELIVER_AFTER => continue,
                Some(_) => {
                    state.taken.get_mut(target).map(|t| t.remove(&advisory.id));
                    waiting.push(advisory);
                }
                None => waiting.push(advisory),
            }
        }
        if waiting.is_empty() {
            state.waiting.remove(target);
        } else {
            state.waiting.insert(target.clone(), waiting);
        }
    }

    /// Acks every shown advisory whose retry is due.
    pub(super) async fn send_due_acks(&self, token: &ReadToken, now: Instant) {
        let due: Vec<(Target, String)> = {
            let mut state = self.state();
            // Another account's token never acks this one's notes.
            if state.principal.as_ref() != Some(&token.principal) {
                return;
            }
            let mut due = Vec::new();
            for (target, taken) in state.taken.iter_mut() {
                for (id, taken) in taken.iter_mut() {
                    if taken.shown
                        && !taken.acked
                        && !taken.sending
                        && taken.retry_at.is_none_or(|at| at <= now)
                    {
                        taken.sending = true;
                        due.push((target.clone(), id.clone()));
                    }
                }
            }
            due
        };
        for (target, id) in due {
            let ack = self.api.ack(token, &target, &id).await;
            if ack == Ack::Unauthorized {
                self.tokens.refused(token);
            }
            self.acked(&target, &id, ack, now);
        }
    }

    pub(super) fn acked(&self, target: &Target, id: &str, ack: Ack, now: Instant) {
        let mut state = self.state();
        let Some(taken) = state.taken.get_mut(target).and_then(|t| t.get_mut(id)) else {
            return;
        };
        taken.sending = false;
        match ack {
            Ack::Done => {
                taken.acked = true;
                taken.retry_at = None;
            }
            Ack::Forbidden => taken.retry_at = Some(now + ACK_FORBIDDEN_RETRY),
            Ack::Unauthorized | Ack::Failed => {
                let wait = ACK_RETRY
                    .saturating_mul(1 << taken.failures.min(6))
                    .min(ACK_RETRY_MAX);
                taken.failures += 1;
                taken.retry_at = Some(now + wait);
            }
        }
    }
}
