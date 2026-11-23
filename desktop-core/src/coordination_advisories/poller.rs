//! Polling: filling the cache from the read API, and sending acks due.

use super::*;

/// Acknowledgements sent per pass, at most (each waits for the shared request pacing).
pub(super) const MAX_ACKS_PER_PASS: usize = 16;

/// Shown on the status line, until the next urgent pass, when a v2 ack found
/// no match.
pub(super) const NO_MATCH_STATUS: &str =
    "An advisory had changed or expired before it was acknowledged; it was not delivered again.";

/// One ack claimed: the session, the cache key, and what the ack names
/// (the advisory id, and an instance row's incarnation).
pub(super) struct Due {
    target: Target,
    key: String,
    id: String,
    incarnation: Option<String>,
}

impl Advisories {
    /// One poll over `sessions`, the live sessions the core can reach at a
    /// turn's end: at most [`SESSIONS_PER_TICK`] of them, then the acks due.
    pub async fn poll(&self, sessions: &[Target], now: Instant) {
        let _ = self.poll_selected(sessions, None, now).await;
    }

    pub(super) async fn poll_selected(
        &self,
        sessions: &[Target],
        selected: Option<&[Target]>,
        now: Instant,
    ) -> Vec<Target> {
        let mut answered = Vec::new();
        if !self.enabled() {
            return answered;
        }
        let principal = self.tokens.principal();
        {
            let mut state = self.state();
            state.belong_to(principal.as_ref());
            state.prune(&sessions.iter().collect(), now);
        }
        if principal.is_none() {
            return answered;
        }
        let Some(token) = self.tokens.token().await else {
            self.urgent.lock().unwrap_or_else(|p| p.into_inner()).status =
                "Read access unavailable; urgent advisories cannot be fetched.";
            return answered;
        };
        // Read access works again: drop a stale fetch or setting status.
        self.reset_urgent_status_if(|status| status.starts_with("Read access unavailable"));
        let (due, backed_off) = {
            let mut state = self.state();
            // The account changed while the token was fetched.
            if !state.belong_to(Some(&token.principal)) {
                return answered;
            }
            if state.paused_until.is_some_and(|until| now < until)
                || state.rate_until.is_some_and(|until| now < until)
            {
                (Vec::new(), Vec::new())
            } else {
                state.paused_until = None;
                // Selected but in their own backoff: as good as checked, so
                // they do not keep a slot from healthy sessions every tick.
                let backed_off: Vec<Target> = selected
                    .unwrap_or_default()
                    .iter()
                    .filter(|target| {
                        state
                            .sessions
                            .get(*target)
                            .and_then(|b| b.until)
                            .is_some_and(|until| until > now)
                    })
                    .cloned()
                    .collect();
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
                (ready, backed_off)
            }
        };
        answered.extend(backed_off);
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
                Listing::Advisories(advisories, contract) => {
                    self.listed(&token.principal, target, advisories, contract, now);
                    answered.push(target.clone());
                }
                Listing::Failed => {
                    answered.push(target.clone());
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
        answered
    }

    /// Records what the pipeline lists for `target` now, for `principal`:
    /// a listing made for an account no longer signed in, or after the
    /// setting was turned off, is dropped.
    pub(super) fn listed(
        &self,
        principal: &Principal,
        target: &Target,
        advisories: Vec<Advisory>,
        contract: ListingContract,
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
        // The most recent listing's contract, for this account only: the
        // status says when the workspace lists no urgent-capable rows.
        state.listing_contract = Some(contract);
        state.sessions.remove(target);
        let scope = receipts::Scope::from(principal);
        let advisories: Vec<Advisory> = advisories
            .into_iter()
            .map(|advisory| advisory.keyed(&scope, target))
            .collect();
        let listed: HashSet<String> = advisories.iter().map(|a| a.key.clone()).collect();
        let instances: HashSet<&str> = advisories
            .iter()
            .filter(|a| a.incarnation.is_some())
            .map(|a| a.id.as_str())
            .collect();
        // Handed out and no longer listed: acked or expired upstream. A
        // no-match stays settled for as long as its session lives, and a
        // legacy hand-out while an instance of its id is listed, which it
        // labels (State::shown_as_legacy).
        if let Some(taken) = state.taken.get_mut(target) {
            taken.retain(|key, taken| {
                listed.contains(key)
                    || (taken.shown && !taken.acked)
                    || taken.no_match
                    || (taken.instance.is_none() && instances.contains(key.as_str()))
            });
        }
        let mut waiting = Vec::new();
        for advisory in advisories {
            let taken = state
                .taken
                .get_mut(target)
                .and_then(|t| t.get_mut(&advisory.key));
            match taken {
                // Shown, or acked: never again, whatever the ack's retries.
                // Taken and never confirmed shown: again once 30 minutes
                // passed and it is still listed.
                Some(taken) if taken.acked || taken.sending || taken.shown => continue,
                Some(taken) if now.duration_since(taken.at) < REDELIVER_AFTER => continue,
                Some(_) => {
                    state.taken.get_mut(target).map(|t| t.remove(&advisory.key));
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
        for due in &due {
            if self.tokens.principal().as_ref() != Some(&token.principal) {
                break;
            }
            let ack = self
                .api
                .ack(token, &due.target, &due.id, due.incarnation.as_deref())
                .await;
            self.acked(&token.principal, &due.target, &due.key, ack, now);
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
    fn claim_due_acks(&self, principal: &Principal, now: Instant) -> Option<Vec<Due>> {
        let mut state = self.state();
        if state.principal.as_ref() != Some(principal) {
            return None;
        }
        // A failed new checkpoint must not stall retries for receipts
        // already saved. Newly shown, unsaved notes still wait for disk.
        let saved = receipts::saved(&state, principal);
        let mut due = Vec::new();
        // Bounded, so a backlog of paced acks never stalls the watch loop;
        // the rest go on the next pass.
        'claim: for (target, taken) in state.taken.iter_mut() {
            for (key, taken) in taken.iter_mut() {
                if due.len() == MAX_ACKS_PER_PASS {
                    break 'claim;
                }
                if taken.ack_due(now) && saved.contains(&(target.clone(), key.clone())) {
                    taken.sending = true;
                    // An instance row is acked with its own incarnation, a
                    // legacy one by its key, its id: never one for the other.
                    let (id, incarnation) = match &taken.instance {
                        Some(i) => (i.id.clone(), Some(i.incarnation.clone())),
                        None => (key.clone(), None),
                    };
                    due.push(Due {
                        target: target.clone(),
                        key: key.clone(),
                        id,
                        incarnation,
                    });
                }
            }
        }
        Some(due)
    }

    /// A refused token stops the batch. Release only claims that were never
    /// sent, so a later fresh token can retry them.
    fn release_unsent_acks(&self, principal: &Principal, unsent: &[Due]) {
        let mut state = self.state();
        if state.principal.as_ref() != Some(principal) {
            return;
        }
        for due in unsent {
            if let Some(taken) = state
                .taken
                .get_mut(&due.target)
                .and_then(|t| t.get_mut(&due.key))
            {
                taken.sending = false;
            }
        }
    }

    pub(super) fn acked(
        &self,
        principal: &Principal,
        target: &Target,
        key: &str,
        ack: Ack,
        now: Instant,
    ) {
        let mut state = self.state();
        if state.principal.as_ref() != Some(principal) {
            return;
        }
        let Some(taken) = state.taken.get_mut(target).and_then(|t| t.get_mut(key)) else {
            return;
        };
        taken.sending = false;
        match ack {
            Ack::Done => {
                taken.acked = true;
                taken.retry_at = None;
            }
            Ack::NoMatch => {
                taken.acked = true;
                taken.no_match = true;
                taken.retry_at = None;
                drop(state);
                let mut journal = self.urgent.lock().unwrap_or_else(|p| p.into_inner());
                if !journal.blocked() {
                    journal.status = NO_MATCH_STATUS;
                }
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
