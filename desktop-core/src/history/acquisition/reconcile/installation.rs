//! This installation's registration and the sign-in it waits for.
use super::*;

impl Pass<'_> {
    pub(super) fn installation(&mut self) -> Result<(String, Secret)> {
        if let Some(installation) = &self.installation {
            return Ok(installation.clone());
        }
        let installation =
            ensure_installation(self.issuer, self.store, self.account, self.origin_id)?;
        self.installation = Some(installation.clone());
        Ok(installation)
    }
}

/// The issuer answered a call under the current sign-in with anything but
/// `not_authorized`: acquisition no longer waits for a sign-in.
pub(super) fn authorized(state: &mut super::super::store::State, account: &Account) {
    state
        .reauthorization
        .retain(|marker| marker.account != *account);
}

pub(super) fn not_authorized(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<IssuerError>(),
        Some(IssuerError::NotAuthorized)
    )
}

/// The issuer could not be reached (network, 5xx, an uncured 401).
pub(super) fn transport(error: &anyhow::Error) -> bool {
    matches!(
        error.downcast_ref::<IssuerError>(),
        Some(IssuerError::Transport(_))
    )
}

/// Record that the issuer refused `sign_in` for `account`, once: a marker
/// already naming it is left as it is.
pub(super) fn sign_in_required(
    store: &Store,
    account: &Account,
    sign_in: &str,
    done: &mut Reconciled,
) -> Result<()> {
    done.sign_in_required = true;
    let marked = store.update(|state| {
        if state
            .reauthorization(account)
            .is_some_and(|marker| marker.sign_in == sign_in)
        {
            return Ok(false);
        }
        state
            .reauthorization
            .retain(|marker| marker.account != *account);
        state.reauthorization.push(Reauthorization {
            account: account.clone(),
            sign_in: sign_in.to_owned(),
        });
        Ok(true)
    })?;
    done.newly_required |= marked;
    Ok(())
}

/// This installation's id and proof for `account`, registering it first if
/// needed. The proof and the request uuid are generated and saved before
/// the first registration request, so a lost response is replayed with the
/// same pair and returns the same installation. A proof is never replaced.
pub fn ensure_installation(
    issuer: &dyn AcquisitionIssuer,
    store: &Store,
    account: &Account,
    origin_id: &str,
) -> Result<(String, Secret)> {
    let installation = store.update(|state| {
        if let Some(saved) = state.installation(account) {
            return Ok(saved.clone());
        }
        let fresh = Installation {
            account: account.clone(),
            request_uuid: new_uuid()?,
            proof: new_proof()?,
            installation_id: None,
            origin_id: origin_id.to_owned(),
            issuer_key_id: None,
        };
        state.installations.push(fresh.clone());
        Ok(fresh)
    })?;
    if let Some(id) = installation.installation_id {
        return Ok((id, installation.proof));
    }
    let registered = issuer
        .register(
            &installation.proof,
            &InstallationRequest {
                request_uuid: installation.request_uuid.clone(),
                origin_id: installation.origin_id.clone(),
                label: None,
            },
        )
        .map_err(anyhow::Error::new)?;
    store.update(|state| {
        if let Some(saved) = state
            .installations
            .iter_mut()
            .find(|saved| saved.account == *account)
        {
            saved.installation_id = Some(registered.installation_id.clone());
            saved.issuer_key_id = registered.issuer_key_id.clone();
        }
        authorized(state, account);
        Ok(())
    })?;
    Ok((registered.installation_id, installation.proof))
}
