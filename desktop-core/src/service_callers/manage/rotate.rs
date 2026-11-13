//! `spawn_callers.rotate` (`docs/service-callers.md` section 4): a new
//! secret, the old one kept until the overlap ends; a probe-initiated
//! rotation waits for the person.

use super::{new_secret, requester, shown, DEFAULT_DAYS, DEFAULT_OVERLAP, MAX_OVERLAP};
use crate::contract::service_callers::{CallerRotateParams, CallerRotated};
use crate::service_callers::peer::Initiator;
use crate::service_callers::registry::{
    self, Binding, CallerRecord, PendingChange, RegistryFile, SecretDigest, DAY_MS,
    PENDING_ROTATION,
};
use crate::service_callers::{handoff, Class, ServiceCallers};
use agent_relay_node::relay::protocol::RpcError;
use std::path::PathBuf;

impl ServiceCallers {
    pub(crate) async fn rotate(
        &self,
        peer_pid: Option<i32>,
        connection_id: Option<u64>,
        params: &CallerRotateParams,
    ) -> Result<CallerRotated, RpcError> {
        const METHOD: &str = "spawn_callers.rotate";
        let (initiator, binding) = self.manager(peer_pid, &params.caller_secret, METHOD)?;
        let refuse = |class| self.refused(Some(&binding), Some(&params.caller_id), METHOD, class);
        let overlap = params.overlap_seconds.unwrap_or(DEFAULT_OVERLAP);
        if overlap > MAX_OVERLAP {
            return Err(refuse(Class::InvalidInput));
        }
        if initiator == Initiator::Probe && params.handoff_path.is_some() {
            return Err(refuse(Class::AppAttestationUnavailable));
        }
        let handoff_path = params
            .handoff_path
            .as_deref()
            .map(|given| handoff::path(&binding, &params.caller_id, Some(given)))
            .transpose()
            .map_err(refuse)?;
        let _files = self.files();
        let mut file = self.registry(&binding, METHOD)?;
        let index = file
            .by_id(&params.caller_id)
            .ok_or_else(|| refuse(Class::NotFound))?;
        let now = self.host.now_ms();
        let caller = &mut file.callers[index];
        if caller.pending_provision() {
            return Err(refuse(Class::InvalidInput));
        }
        caller.requested_by = requester(initiator);
        caller.requested_at_ms = now;
        if initiator == Initiator::Probe {
            caller.pending = Some(PendingChange {
                kind: PENDING_ROTATION.into(),
                expires_in_days: None,
                overlap_seconds: Some(overlap),
            });
            let summary = caller.summary(now);
            let _live = self.management_gate(connection_id).map_err(refuse)?;
            registry::save(&binding, &file).map_err(refuse)?;
            self.audit(
                &binding,
                Some(&params.caller_id),
                METHOD,
                "pending_rotation",
                None,
            );
            return Ok(CallerRotated {
                caller: summary,
                handoff_path: None,
                old_secret_retires_at: None,
            });
        }
        let (caller, retires) = self
            .apply_rotation(
                &binding,
                &mut file,
                index,
                overlap,
                handoff_path,
                connection_id,
            )
            .map_err(refuse)?;
        self.audit(&binding, Some(&params.caller_id), METHOD, "rotated", None);
        Ok(CallerRotated {
            handoff_path: shown(&caller.handoff_path),
            caller: caller.summary(self.host.now_ms()),
            old_secret_retires_at: retires.map(crate::service_callers::rfc3339),
        })
    }

    /// A new secret, the newest old one kept until the overlap ends, and
    /// `caller.json` replaced. Called with the files lock held.
    pub(super) fn apply_rotation(
        &self,
        binding: &Binding,
        file: &mut RegistryFile,
        index: usize,
        overlap: u32,
        handoff_path: Option<PathBuf>,
        connection_id: Option<u64>,
    ) -> Result<(CallerRecord, Option<i64>), Class> {
        let now = self.host.now_ms();
        let secret = new_secret().ok_or(Class::StoreFailed)?;
        let before = file.callers[index].clone();
        let caller = &mut file.callers[index];
        let retires = now + i64::from(overlap) * 1000;
        let old = caller
            .secrets
            .iter()
            .filter(|kept| kept.live(now))
            .max_by_key(|kept| kept.created_at_ms)
            .cloned()
            .filter(|_| overlap > 0)
            .map(|mut kept| {
                kept.retires_at_ms = Some(retires);
                kept
            });
        caller.secrets = old.into_iter().collect();
        caller.secrets.push(SecretDigest {
            sha256: registry::digest(&secret),
            created_at_ms: now,
            retires_at_ms: None,
        });
        caller.rotated_at_ms = Some(now);
        caller.expires_at_ms = now + i64::from(DEFAULT_DAYS) * DAY_MS;
        caller.expiry_advanced = false;
        caller.pending = None;
        // A rotation is an authority change: every binding of the caller
        // goes stale (`docs/service-callers-auth.md` section 4).
        caller.authority_generation += 1;
        if let Some(path) = handoff_path {
            caller.handoff_path = Some(path);
        } else if caller.handoff_path.is_none() {
            caller.handoff_path = Some(binding.default_handoff(&caller.caller_id));
        }
        let caller = caller.clone();
        let kept_old = overlap > 0 && caller.secrets.len() > 1;
        // The registry first, carrying the record as it was: a failed save
        // leaves the old secret and the old caller.json as they were. A
        // failed write then puts the old record back, so the registry never
        // names a secret no file holds. Should that write fail too, or the
        // process end before either, the record kept with the rotated one
        // is put back by the next check, after a restart as well
        // (`ServiceCallers::load_current`).
        file.callers[index].rollback = Some(Box::new(before.clone()));
        let live = match self.management_gate(connection_id) {
            Ok(live) => live,
            Err(class) => {
                file.callers[index] = before;
                return Err(class);
            }
        };
        if let Err(class) = registry::save(binding, file) {
            file.callers[index] = before;
            return Err(class);
        }
        if let Err(class) = handoff::write(
            binding,
            &caller,
            &secret,
            &self.host.socket(),
            &self.host.version(),
        ) {
            file.callers[index] = before;
            if registry::save(binding, file).is_err() {
                // The registry still names a secret no file holds, with the
                // old record beside it. Every check puts the old record
                // back before it reads authority and refuses until that
                // write succeeds.
                return Err(Class::RotationRollbackPending);
            }
            return Err(class);
        }
        // Committed: caller.json holds the new secret. A failed write of
        // the cleared mark leaves it for the next check, which finds the
        // new secret in caller.json and keeps the rotation.
        file.callers[index].rollback = None;
        let _ = registry::save(binding, file);
        drop(live);
        if caller.handoff_path != before.handoff_path {
            handoff::remove_kept(&before);
        }
        self.stale_caller(&caller.caller_id);
        Ok((caller, kept_old.then_some(retires)))
    }
}
