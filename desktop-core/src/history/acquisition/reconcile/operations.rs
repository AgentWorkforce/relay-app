//! Claims and rebinds: the operations a pass sends and what their answers
//! make of a held session.
use super::*;

impl Pass<'_> {
    /// Save a new operation for the session, replacing a refused one, and
    /// send it.
    pub(super) fn begin(&mut self, mut operation: Operation) -> Result<Outcome> {
        operation.operation_uuid = new_uuid()?;
        let account = self.account.clone();
        let saved = operation.clone();
        self.store.update(move |state| {
            state.end_operation(&account, &saved.source, &saved.session_id);
            state.operations.push(saved);
            Ok(())
        })?;
        self.send(operation)
    }

    pub(super) fn begin_claim(&mut self, source: &str, session_id: &str) -> Result<Outcome> {
        if !self
            .claimed
            .insert((source.to_owned(), session_id.to_owned()))
        {
            return Ok(Outcome::Hold);
        }
        let (installation_id, _) = self.installation()?;
        self.begin(Operation {
            account: self.account.clone(),
            kind: CLAIM.to_owned(),
            source: source.to_owned(),
            session_id: session_id.to_owned(),
            operation_uuid: String::new(),
            installation_id,
            origin_id: self.origin_id.to_owned(),
            grant_id: None,
            grant: None,
            owner_epoch: None,
            previous_origin_id: None,
            refused: None,
            not_before_ms: 0,
        })
    }

    pub(super) fn begin_rebind(&mut self, grant: &Grant) -> Result<Outcome> {
        let (installation_id, _) = self.installation()?;
        self.begin(Operation {
            account: self.account.clone(),
            kind: REBIND.to_owned(),
            source: grant.source.clone(),
            session_id: grant.session_id.clone(),
            operation_uuid: String::new(),
            installation_id,
            origin_id: self.origin_id.to_owned(),
            grant_id: Some(grant.grant_id.clone()),
            grant: Some(grant.grant.clone()),
            owner_epoch: Some(grant.owner_epoch),
            previous_origin_id: Some(grant.origin_id.clone()),
            refused: None,
            not_before_ms: 0,
        })
    }

    /// Send a stored operation: the request is built from the record alone.
    pub(super) fn send(&mut self, operation: Operation) -> Result<Outcome> {
        let (_, proof) = self.installation()?;
        match operation.kind.as_str() {
            CLAIM => {
                let request = claim_request(&operation);
                match retrying(|| self.issuer.claim(&proof, &request)) {
                    Ok(answer) => self.claimed_grant(&operation, answer),
                    Err(error) => self.failed(&operation, error),
                }
            }
            REBIND => {
                let request = rebind_request(&operation)?;
                let answer = retrying(|| self.issuer.rebind(&proof, &request));
                drop(request);
                match answer {
                    Ok(answer) => self.rebound(&operation, &answer),
                    Err(error) => self.failed(&operation, error),
                }
            }
            _ => anyhow::bail!("unknown acquisition operation"),
        }
    }

    /// A claim answered `installed` or `replay`. The grant is kept bound to
    /// the origin the claim named; a replay from before a reset is then
    /// rebound by a separate operation. Released only once status shows it
    /// active.
    pub(super) fn claimed_grant(
        &mut self,
        operation: &Operation,
        answer: ClaimResponse,
    ) -> Result<Outcome> {
        let grant = Grant {
            account: self.account.clone(),
            source: operation.source.clone(),
            session_id: operation.session_id.clone(),
            grant_id: answer.grant_id.clone(),
            owner_epoch: answer.owner_epoch,
            grant: Secret::new(answer.grant.clone()),
            expires_at: answer.expires_at.clone(),
            origin_id: operation.origin_id.clone(),
        };
        drop(answer.grant);
        let outcome_known = [contract::OUTCOME_INSTALLED, contract::OUTCOME_REPLAY]
            .contains(&answer.outcome.as_str());
        if !outcome_known || !usable(&grant) {
            return self.refuse(operation, contract::INVALID_REQUEST, REFUSED_PAUSE_MS);
        }
        let (account, kept) = (self.account.clone(), grant.clone());
        self.store.update(move |state| {
            state.end_operation(&account, &kept.source, &kept.session_id);
            state.end_watch(&account, &kept.source, &kept.session_id);
            state.put_grant(kept);
            authorized(state, &account);
            Ok(())
        })?;
        if grant.origin_id != self.origin_id {
            return self.begin_rebind(&grant);
        }
        self.verify(&grant)
    }

    /// A rebind answered. Spec 4.7: `rebound` or `replay`, and the epoch is
    /// kept, so an answer with another epoch changes nothing and ends the
    /// operation as refused.
    pub(super) fn rebound(
        &mut self,
        operation: &Operation,
        answer: &RebindResponse,
    ) -> Result<Outcome> {
        let outcome_known = [contract::OUTCOME_REBOUND, contract::OUTCOME_REPLAY]
            .contains(&answer.outcome.as_str());
        if !outcome_known || Some(answer.owner_epoch) != operation.owner_epoch {
            return self.refuse(operation, REBIND_MISMATCH, REFUSED_PAUSE_MS);
        }
        let (account, kept) = (self.account.clone(), operation.clone());
        let grant = self.store.update(move |state| {
            state.end_operation(&account, &kept.source, &kept.session_id);
            authorized(state, &account);
            let Some(mut grant) = state
                .grant(&account, &kept.source, &kept.session_id)
                .cloned()
                .filter(|grant| Some(&grant.grant_id) == kept.grant_id.as_ref())
            else {
                return Ok(None);
            };
            grant.origin_id = kept.origin_id.clone();
            state.put_grant(grant.clone());
            Ok(Some(grant))
        })?;
        match grant {
            Some(grant) if grant.origin_id == self.origin_id => self.verify(&grant),
            // Replaced meanwhile, or bound again before another reset: the
            // next pass plans from what is stored.
            _ => Ok(Outcome::Hold),
        }
    }

    /// Settle an operation that failed. A busy refusal or a transport
    /// failure keeps the operation exactly as it is for the next pass. A
    /// terminal refusal ends it: an owner wait polls status from then on,
    /// anything else paces the next operation. `403 not_owner` is an owner
    /// wait, relabeled [`contract::NOT_OWNER`]; `403 not_installer` is paced
    /// and quarantines a rebind's grant (see [`Self::not_installer`]); a
    /// rebind refused `owner_unknown` drops its grant and claims afresh
    /// (see [`Self::owner_unknown`]); any other 403 but `not_authorized`
    /// and `installation_not_proven` is a terminal refusal of this session
    /// alone. An unproven installation stops the pass, and so does a
    /// sign-in without `rth:acquire` (on any route, a rebind's included):
    /// neither is a refusal of the operation, which is kept.
    pub(super) fn failed(&mut self, operation: &Operation, error: IssuerError) -> Result<Outcome> {
        match &error {
            // Kept as stored for the next pass: a 503 (a claim's install
            // without a positive epoch yet included) replays the same
            // immutable operation.
            IssuerError::Transport(_) => Ok(Outcome::Hold),
            IssuerError::Refused { reason, retryable } if busy(reason, *retryable) => {
                Ok(Outcome::Hold)
            }
            IssuerError::NotAuthorized | IssuerError::NotProven | IssuerError::ProofMismatch => {
                Err(error.into())
            }
            // Terminal for the rebind's grant: see [`Self::owner_unknown`].
            // A claim refused so takes the terminal path below.
            IssuerError::Refused { reason, .. }
                if reason == contract::OWNER_UNKNOWN_REFUSAL && operation.kind == REBIND =>
            {
                self.owner_unknown(operation)
            }
            IssuerError::Refused { reason, .. }
                if contract::OWNER_WAIT_CODES.contains(&reason.as_str()) =>
            {
                let reason = reason.clone();
                self.refuse(operation, &reason, 0)?;
                let observed = self.observe(&operation.source, &operation.session_id).ok();
                self.watch(&operation.source, &operation.session_id, &reason, observed)?;
                Ok(Outcome::Hold)
            }
            // Another installation owns the session: scoped to it, and no
            // sign-in cures it. It waits on its owner's status.
            IssuerError::NotOwner => {
                self.refuse(operation, contract::NOT_OWNER, 0)?;
                let observed = self.observe(&operation.source, &operation.session_id).ok();
                self.watch(
                    &operation.source,
                    &operation.session_id,
                    contract::NOT_OWNER,
                    observed,
                )?;
                Ok(Outcome::Relabel(contract::NOT_OWNER))
            }
            IssuerError::NotInstaller => self.not_installer(operation),
            // Any other 403: fail closed for this session alone. It stays
            // held, nothing asks for sign-in, and the next operation waits.
            IssuerError::Forbidden { .. } => {
                self.refuse(operation, &error.to_string(), REFUSED_PAUSE_MS)
            }
            IssuerError::Refused { reason, .. } => {
                let reason = if reason.is_empty() {
                    contract::ACQUISITION_REFUSED.to_owned()
                } else {
                    reason.clone()
                };
                self.refuse(operation, &reason, REFUSED_PAUSE_MS)
            }
            other => self.refuse(operation, &other.to_string(), REFUSED_PAUSE_MS),
        }
    }

    /// `403 not_installer`. On a route it means the grant named was not
    /// issued to this installation, but the service also answers it from
    /// its SQL installer-role guard (42501), which refuses a valid grant
    /// while the database role is misconfigured. No sign-in cures either,
    /// and it is not an owner wait. The operation is refused and paced like
    /// any terminal refusal. A claim presented no grant, so that is all. A
    /// rebind's grant is quarantined, never dropped on this code alone: the
    /// scoped status decides at once (see [`Self::quarantined`]), and while
    /// it does not disown the grant, the grant is kept and not presented
    /// again until the pause is over and a fresh status shows it still this
    /// installation's active grant.
    pub(super) fn not_installer(&mut self, operation: &Operation) -> Result<Outcome> {
        self.refuse(operation, contract::NOT_INSTALLER, REFUSED_PAUSE_MS)?;
        if operation.kind != REBIND {
            return Ok(Outcome::Hold);
        }
        let grant = self
            .store
            .load()?
            .grant(self.account, &operation.source, &operation.session_id)
            .filter(|grant| Some(&grant.grant_id) == operation.grant_id.as_ref())
            .cloned();
        match grant {
            Some(grant) => self.quarantined(&grant, None, false),
            // Replaced meanwhile: the next pass plans from what is stored.
            None => Ok(Outcome::Hold),
        }
    }

    /// A rebind refused `409 owner_unknown`: the session can hold no live
    /// grant and waiting never resolves it, so the operation is refused and
    /// the grant it presented (that grant id only) is dropped, its watch
    /// ended and the session claimed afresh at once. A claim refused so is
    /// terminal (see [`contract::OWNER_UNKNOWN_REFUSAL`]), so this never
    /// loops.
    pub(super) fn owner_unknown(&mut self, operation: &Operation) -> Result<Outcome> {
        self.refuse(operation, contract::OWNER_UNKNOWN_REFUSAL, 0)?;
        let grant = self
            .store
            .load()?
            .grant(self.account, &operation.source, &operation.session_id)
            .filter(|grant| Some(&grant.grant_id) == operation.grant_id.as_ref())
            .cloned();
        match grant {
            Some(grant) => self.disowned(&grant),
            // Replaced meanwhile: the next pass plans from what is stored.
            None => Ok(Outcome::Hold),
        }
    }

    /// Mark the operation refused: it is never sent again, and the next one
    /// waits `pause_ms`.
    pub(super) fn refuse(
        &self,
        operation: &Operation,
        reason: &str,
        pause_ms: i64,
    ) -> Result<Outcome> {
        let not_before_ms = self.now_ms + pause_ms;
        self.store.update(|state| {
            if let Some(kept) = state
                .operations
                .iter_mut()
                .find(|kept| kept.operation_uuid == operation.operation_uuid)
            {
                kept.refused = Some(reason.to_owned());
                kept.not_before_ms = not_before_ms;
            }
            authorized(state, self.account);
            Ok(())
        })?;
        Ok(Outcome::Hold)
    }
}

/// The refusal recorded for a rebind answer that did not keep the epoch or
/// named no known outcome.
pub(super) const REBIND_MISMATCH: &str = "rebind_mismatch";

/// The issuer's "the session is busy, try the same operation again".
pub(super) const SESSION_BUSY: &str = "session_busy";

/// The claim a stored operation sends, from its record alone.
pub(super) fn claim_request(operation: &Operation) -> ClaimRequest {
    ClaimRequest {
        operation_uuid: operation.operation_uuid.clone(),
        installation_id: operation.installation_id.clone(),
        source: operation.source.clone(),
        session_id: operation.session_id.clone(),
        origin_id: operation.origin_id.clone(),
    }
}

/// The rebind a stored operation sends, from its record alone.
pub(super) fn rebind_request(operation: &Operation) -> Result<RebindRequest> {
    let (Some(grant_id), Some(grant)) = (&operation.grant_id, &operation.grant) else {
        anyhow::bail!("incomplete rebind operation");
    };
    Ok(RebindRequest {
        operation_uuid: operation.operation_uuid.clone(),
        installation_id: operation.installation_id.clone(),
        source: operation.source.clone(),
        session_id: operation.session_id.clone(),
        grant_id: grant_id.clone(),
        origin_id: operation.origin_id.clone(),
        grant: grant.expose().to_owned(),
    })
}

/// A refusal that only says "not now" (`session_busy`, or another code the
/// issuer marks retryable): the same operation goes again. A terminal code
/// or an owner wait is never that, even when marked retryable; an owner
/// wait polls status instead.
pub(super) fn busy(reason: &str, retryable: bool) -> bool {
    reason == SESSION_BUSY
        || retryable
            && !contract::TERMINAL_CODES.contains(&reason)
            && !contract::OWNER_WAIT_CODES.contains(&reason)
}

/// Busy refusals are retried at once with the same request, up to
/// [`ATTEMPTS`] in a pass.
pub(super) fn retrying<T>(
    mut call: impl FnMut() -> Result<T, IssuerError>,
) -> Result<T, IssuerError> {
    let mut attempt = 1;
    loop {
        match call() {
            Err(IssuerError::Refused { reason, retryable })
                if attempt < ATTEMPTS && busy(&reason, retryable) =>
            {
                attempt += 1
            }
            other => return other,
        }
    }
}
