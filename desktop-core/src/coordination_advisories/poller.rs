//! Polling: filling the cache from the read API, and sending acks due.

use super::*;

impl Advisories {
    /// One poll over `sessions`, the live sessions the core can reach at a
    /// turn's end: at most [`SESSIONS_PER_TICK`] of them, then the acks due.
    pub async fn poll(&self, sessions: &[Target], now: Instant) {
        self.poll_selected(sessions, None, now).await;
    }

    pub(super) async fn poll_selected(
        &self,
        sessions: &[Target],
        selected: Option<&[Target]>,
        now: Instant,
    ) {
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
            self.urgent.lock().unwrap_or_else(|p| p.into_inner()).status =
                "Read access unavailable; urgent advisories cannot be fetched.";
            return;
        };
        let due = {
            let mut state = self.state();
            // The account changed while the token was fetched.
            if !state.belong_to(Some(&token.principal)) {
                return;
            }
            if state.paused_until.is_some_and(|until| now < until)
                || state.rate_until.is_some_and(|until| now < until)
            {
                Vec::new()
            } else {
                state.paused_until = None;
                let start = state.cursor % sessions.len().max(1);
                let ready: Vec<Target> = sessions
                    .iter()
                    .cycle()
                    .skip(start)
                    .take(sessions.len())
                    .filter(|target| selected.is_none_or(|due| due.contains(target)))
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
        let mut token_refused = false;
        for target in &due {
            let mut listing = self.api.list(&token, target).await;
            if listing == Listing::Unauthorized && !renewed {
                // One new token, once a tick: never a loop.
                renewed = true;
                token_refused = true;
                self.tokens.refused(&token);
                match self.tokens.token().await {
                    Some(fresh) => {
                        token = fresh;
                        listing = self.api.list(&token, target).await;
                        token_refused = listing == Listing::Unauthorized;
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
                Listing::Throttled => {
                    self.state().rate_until = Some(now + SESSION_BACKOFF);
                    break;
                }
                Listing::Unauthorized | Listing::Off => {
                    if listing == Listing::Unauthorized {
                        token_refused = true;
                        self.tokens.refused(&token);
                    }
                    self.state().paused_until = Some(now + ROUTE_OFF_BACKOFF);
                    break;
                }
            }
        }
        if !token_refused && selected.is_none() {
            self.send_due_acks(&token, now).await;
        }
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
        if self.state().rate_until.is_some_and(|until| now < until) {
            return;
        }
        // Save shown IDs before acknowledging them; a normal restart resumes
        // retries without showing the message again. No credentials are saved.
        self.checkpoint_receipts(&token.principal);
        let Some(due) = self.claim_due_acks(&token.principal, now) else {
            return;
        };
        let mut sent = 0;
        for (target, id) in &due {
            if self.tokens.principal().as_ref() != Some(&token.principal) {
                break;
            }
            let ack = self.api.ack(token, target, id).await;
            self.acked(&token.principal, target, id, ack, now);
            sent += 1;
            if ack == Ack::Unauthorized {
                self.tokens.refused(token);
            }
            if matches!(ack, Ack::Unauthorized | Ack::Throttled) {
                break;
            }
        }
        self.release_unsent_acks(&token.principal, &due[sent..]);
        self.checkpoint_receipts(&token.principal);
    }

    /// Marks and returns the shown, saved, unacknowledged notes whose retry is
    /// due. `None` when the state belongs to another account: its token never
    /// acks this one's notes.
    fn claim_due_acks(&self, principal: &Principal, now: Instant) -> Option<Vec<(Target, String)>> {
        let mut state = self.state();
        if state.principal.as_ref() != Some(principal) {
            return None;
        }
        // A failed new checkpoint must not stall retries for receipts
        // already saved. Newly shown, unsaved notes still wait for disk.
        let saved = receipts::saved(&state, principal);
        let mut due = Vec::new();
        for (target, taken) in state.taken.iter_mut() {
            for (id, taken) in taken.iter_mut() {
                if taken.ack_due(now) && saved.contains(&(target.clone(), id.clone())) {
                    taken.sending = true;
                    due.push((target.clone(), id.clone()));
                }
            }
        }
        Some(due)
    }

    /// A refused token stops the batch. Release only claims that were never
    /// sent, so a later fresh token can retry them.
    fn release_unsent_acks(&self, principal: &Principal, unsent: &[(Target, String)]) {
        let mut state = self.state();
        if state.principal.as_ref() != Some(principal) {
            return;
        }
        for (target, id) in unsent {
            if let Some(taken) = state.taken.get_mut(target).and_then(|t| t.get_mut(id)) {
                taken.sending = false;
            }
        }
    }

    pub(super) fn acked(
        &self,
        principal: &Principal,
        target: &Target,
        id: &str,
        ack: Ack,
        now: Instant,
    ) {
        let mut state = self.state();
        if state.principal.as_ref() != Some(principal) {
            return;
        }
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
            Ack::Throttled => {
                taken.retry_at = Some(now + SESSION_BACKOFF);
                state.rate_until = Some(now + SESSION_BACKOFF);
            }
            // Authentication recovery says nothing about whether the
            // advisory can be acknowledged; it must not exhaust an ended
            // session's bounded transport-error retries.
            Ack::Unauthorized => taken.retry_at = Some(now + ACK_RETRY),
            Ack::Failed => {
                let wait = ACK_RETRY
                    .saturating_mul(1 << taken.failures.min(6))
                    .min(ACK_RETRY_MAX);
                taken.failures += 1;
                taken.retry_at = Some(now + wait);
            }
        }
    }
}
