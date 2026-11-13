//! Status reads: verifying a grant, and the owner waits polled on the
//! hold cadence.
use super::*;

impl Pass<'_> {
    /// Released only if the status shows the stored grant active with its
    /// epoch; `changed` or not, that is the authority. `released` is free
    /// to claim; anything else waits on the hold cadence.
    pub(super) fn verify(&mut self, grant: &Grant) -> Result<Outcome> {
        let observed = match self.observe(&grant.source, &grant.session_id) {
            Ok(observed) => observed,
            Err(error) => return self.unanswered(error),
        };
        if own_active(&observed, grant) {
            self.end_watch(&grant.source, &grant.session_id)?;
            return Ok(Outcome::Release);
        }
        if observed.state == contract::OWNER_RELEASED {
            let paused = self
                .store
                .load()?
                .operation(self.account, &grant.source, &grant.session_id)
                .is_some_and(|operation| operation.not_before_ms > self.now_ms);
            if !paused
                && !self
                    .claimed
                    .contains(&(grant.source.clone(), grant.session_id.clone()))
            {
                return self.begin_claim(&grant.source, &grant.session_id);
            }
        }
        let reason = format!("owner_{}", observed.state);
        self.watch(&grant.source, &grant.session_id, &reason, Some(observed))?;
        Ok(Outcome::Hold)
    }

    /// A due owner wait: release on the session's own active grant, plan a
    /// new operation on a change of the status, otherwise wait again.
    pub(super) fn poll(
        &mut self,
        watch: Watch,
        grant: Option<&Grant>,
        paused: bool,
    ) -> Result<Outcome> {
        let (source, session_id) = (watch.source.as_str(), watch.session_id.as_str());
        let observed = match self.observe(source, session_id) {
            Ok(observed) => observed,
            Err(error) => {
                self.watch(source, session_id, &watch.reason, watch.observed.clone())?;
                return self.unanswered(error);
            }
        };
        if grant.is_some_and(|grant| own_active(&observed, grant)) {
            self.end_watch(source, session_id)?;
            return Ok(Outcome::Release);
        }
        match &watch.observed {
            Some(before) if *before != observed && !paused => {
                self.end_watch(source, session_id)?;
                self.begin_claim(source, session_id)
            }
            Some(_) => {
                self.watch(source, session_id, &watch.reason, watch.observed.clone())?;
                Ok(Outcome::Hold)
            }
            None => {
                self.watch(source, session_id, &watch.reason, Some(observed))?;
                Ok(Outcome::Hold)
            }
        }
    }

    /// A due owner wait of a usable grant bound to an older origin (left by
    /// a reset). [`Self::poll`] would release on the session's own active
    /// grant, but this grant is not bound to the current origin yet, so it
    /// is never released here:
    ///
    /// - the status shows the stored grant active with its id and epoch:
    ///   the wait ends and the grant is rebound (a new operation); the
    ///   rebind's answer is verified by status before any release;
    /// - the status is unchanged (or the first one is read now): the watch
    ///   is armed again with the same reason, and nothing else is sent;
    /// - the status changed to anything else: as in [`Self::poll`], the
    ///   wait ends and the session is claimed, since a rebind of a grant the
    ///   service no longer shows active would be refused again. When the
    ///   status disowns the stored grant (see [`disowns`]: `released`, or
    ///   another grant id or epoch), that grant is dropped first, so a
    ///   refused claim never leads back to rebinding it; a grant the status
    ///   does not name otherwise (`unknown`, no id) is kept until a claim
    ///   replaces it.
    ///
    /// The caller only comes here when no operation is pending or paused.
    pub(super) fn poll_stale(&mut self, watch: Watch, grant: &Grant) -> Result<Outcome> {
        let (source, session_id) = (watch.source.as_str(), watch.session_id.as_str());
        let observed = match self.observe(source, session_id) {
            Ok(observed) => observed,
            Err(error) => {
                self.watch(source, session_id, &watch.reason, watch.observed.clone())?;
                return self.unanswered(error);
            }
        };
        if own_active(&observed, grant) {
            self.end_watch(source, session_id)?;
            return self.begin_rebind(grant);
        }
        match &watch.observed {
            Some(before) if *before != observed => {
                let drop_grant = disowns(&observed, grant);
                let (account, dead) = (self.account.clone(), grant.clone());
                self.store.update(move |state| {
                    state.end_watch(&account, &dead.source, &dead.session_id);
                    if drop_grant {
                        state.grants.retain(|kept| {
                            !(kept.account == account
                                && kept.source == dead.source
                                && kept.session_id == dead.session_id
                                && kept.grant_id == dead.grant_id)
                        });
                    }
                    Ok(())
                })?;
                self.begin_claim(source, session_id)
            }
            Some(_) => {
                self.watch(source, session_id, &watch.reason, watch.observed.clone())?;
                Ok(Outcome::Hold)
            }
            None => {
                self.watch(source, session_id, &watch.reason, Some(observed))?;
                Ok(Outcome::Hold)
            }
        }
    }

    /// An expired grant: renewal is not supported, so the session stays
    /// held and only its status is refreshed, on the hold cadence. No claim
    /// is made and no new installation either.
    pub(super) fn renewal_unsupported(&mut self, source: &str, session_id: &str) -> Result<()> {
        let watch = self
            .store
            .load()?
            .watch(self.account, source, session_id)
            .cloned();
        if watch
            .as_ref()
            .is_some_and(|watch| watch.not_before_ms > self.now_ms)
        {
            return Ok(());
        }
        let observed = match self.observe(source, session_id) {
            Ok(observed) => Some(observed),
            Err(error) => {
                self.unanswered(error)?;
                watch.and_then(|watch| watch.observed)
            }
        };
        self.watch(
            source,
            session_id,
            hold_reason::RENEWAL_UNSUPPORTED,
            observed,
        )
    }

    /// The session's owner, from the scoped status. A session the status
    /// leaves out is `unknown`.
    pub(super) fn observe(&mut self, source: &str, session_id: &str) -> Result<Observed> {
        Ok(self.observe_scoped(source, session_id)?.0)
    }

    /// [`Self::observe`], and whether the status says the owner is this
    /// installation (`mine`). A session the status leaves out is
    /// `Some(false)`: `unknown`, with no owner epoch, so never another
    /// owner. A row that leaves `mine` out is `None`: unknown, which no
    /// destructive decision may act on.
    pub(super) fn observe_scoped(
        &mut self,
        source: &str,
        session_id: &str,
    ) -> Result<(Observed, Option<bool>)> {
        let (installation_id, proof) = self.installation()?;
        let request = contract::status_request(&installation_id, source, &[session_id.to_owned()])
            .ok_or(IssuerError::InvalidRequest)?;
        let answer = retrying(|| self.issuer.status(&proof, &request))?;
        let account = self.account.clone();
        self.store.update(move |state| {
            authorized(state, &account);
            Ok(())
        })?;
        Ok(answer
            .sessions
            .into_iter()
            .find(|status| status.session_id == session_id)
            .map_or_else(
                || {
                    let unknown = Observed {
                        state: contract::OWNER_UNKNOWN.to_owned(),
                        owner_epoch: None,
                        grant_id: None,
                    };
                    (unknown, Some(false))
                },
                |status| {
                    let observed = Observed {
                        state: status.state,
                        owner_epoch: status.owner_epoch,
                        grant_id: status.grant_id,
                    };
                    (observed, status.mine)
                },
            ))
    }

    /// A status that gave no answer: the session stays held and is asked
    /// about again. A refused sign-in or an unproven installation stops as
    /// everywhere else; any other 403 (`not_owner` included) holds only
    /// this session.
    pub(super) fn unanswered(&self, error: anyhow::Error) -> Result<Outcome> {
        match error.downcast_ref::<IssuerError>() {
            Some(
                IssuerError::NotAuthorized | IssuerError::NotProven | IssuerError::ProofMismatch,
            )
            | None => Err(error),
            Some(_) => Ok(Outcome::Hold),
        }
    }

    pub(super) fn watch(
        &self,
        source: &str,
        session_id: &str,
        reason: &str,
        observed: Option<Observed>,
    ) -> Result<()> {
        let watch = Watch {
            account: self.account.clone(),
            source: source.to_owned(),
            session_id: session_id.to_owned(),
            reason: reason.to_owned(),
            observed,
            not_before_ms: self.now_ms + WATCH_INTERVAL_MS,
        };
        self.store.update(move |state| {
            state.put_watch(watch);
            Ok(())
        })
    }

    pub(super) fn end_watch(&self, source: &str, session_id: &str) -> Result<()> {
        self.store.update(|state| {
            state.end_watch(self.account, source, session_id);
            Ok(())
        })
    }

    /// Drop `grant` (that grant id only, never presented again), end the
    /// session's watch and claim the session afresh.
    pub(super) fn disowned(&mut self, grant: &Grant) -> Result<Outcome> {
        let (account, dead) = (self.account.clone(), grant.clone());
        self.store.update(move |state| {
            state.end_watch(&account, &dead.source, &dead.session_id);
            state.grants.retain(|kept| {
                !(kept.account == account
                    && kept.source == dead.source
                    && kept.session_id == dead.session_id
                    && kept.grant_id == dead.grant_id)
            });
            Ok(())
        })?;
        self.begin_claim(&grant.source, &grant.session_id)
    }

    /// A grant quarantined by `403 not_installer`, judged by the scoped
    /// status alone (`before` is the status the quarantine last read):
    ///
    /// - the status disowns it (see [`disowns`], or an owner that is not
    ///   this installation): only then is the grant (that grant id only)
    ///   dropped, the quarantine ended and the session claimed afresh;
    /// - the status shows it this installation's active grant with its id
    ///   and epoch: the grant is valid and the refusal came from the
    ///   service's role guard. With `rebind`, after the pause, the
    ///   quarantine ends and the grant is rebound (a new operation);
    ///   otherwise the quarantine waits for the paced pass;
    /// - anything else (`unknown`, `draining`, no answer, a row that leaves
    ///   `mine` out): the grant is kept and the quarantine is read again on
    ///   the hold cadence. A row without `mine` is held like no answer: it
    ///   is never read as another owner, nor as this installation's.
    pub(super) fn quarantined(
        &mut self,
        grant: &Grant,
        before: Option<Observed>,
        rebind: bool,
    ) -> Result<Outcome> {
        let (source, session_id) = (grant.source.as_str(), grant.session_id.as_str());
        let (observed, mine) = match self.observe_scoped(source, session_id) {
            Ok(answer) => answer,
            Err(error) => {
                self.watch(source, session_id, contract::NOT_INSTALLER, before)?;
                return self.unanswered(error);
            }
        };
        let Some(mine) = mine else {
            self.watch(source, session_id, contract::NOT_INSTALLER, before)?;
            return Ok(Outcome::Hold);
        };
        if own_active(&observed, grant) && mine {
            if rebind {
                self.end_watch(source, session_id)?;
                return self.begin_rebind(grant);
            }
        } else if disowns(&observed, grant) || (!mine && observed.owner_epoch.is_some()) {
            return self.disowned(grant);
        }
        self.watch(source, session_id, contract::NOT_INSTALLER, Some(observed))?;
        Ok(Outcome::Hold)
    }
}

pub(super) fn own_active(observed: &Observed, grant: &Grant) -> bool {
    observed.state == contract::OWNER_ACTIVE
        && observed.grant_id.as_deref() == Some(grant.grant_id.as_str())
        && observed.owner_epoch == Some(grant.owner_epoch)
}

/// The status positively shows `grant` is not the session's grant any
/// more: the session is released, or the status names another grant id or
/// another epoch. A status that leaves them out (`unknown`, an owner whose
/// grant id is not named to this installation, with the same epoch) does
/// not disown it.
pub(super) fn disowns(observed: &Observed, grant: &Grant) -> bool {
    observed.state == contract::OWNER_RELEASED
        || observed
            .grant_id
            .as_deref()
            .is_some_and(|id| id != grant.grant_id)
        || observed
            .owner_epoch
            .is_some_and(|epoch| epoch != grant.owner_epoch)
}
