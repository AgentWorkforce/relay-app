//! `spawn_callers.provision` (`docs/service-callers.md` section 4): a
//! caller is recorded pending, its publisher registered, its secret made
//! and `caller.json` written; a probe-initiated one waits for the person.

use super::{new_secret, relaycast, requester, segment, shown, DEFAULT_DAYS, MAX_DAYS};
use crate::contract::service_callers::{CallerProvisionParams, CallerProvisioned};
use crate::service_callers::peer::Initiator;
use crate::service_callers::registry::{
    self, Binding, CallerRecord, PendingChange, RegistryFile, SecretDigest, DAY_MS,
    PENDING_PROVISION,
};
use crate::service_callers::{handoff, scope, Class, ServiceCallers};
use agent_relay_node::relay::protocol::RpcError;
use std::path::PathBuf;

impl ServiceCallers {
    pub(crate) async fn provision(
        &self,
        peer_pid: Option<i32>,
        connection_id: Option<u64>,
        params: &CallerProvisionParams,
    ) -> Result<CallerProvisioned, RpcError> {
        const METHOD: &str = "spawn_callers.provision";
        let (initiator, binding) = self.manager(peer_pid, &params.caller_secret, METHOD)?;
        let refuse = |class| self.refused(Some(&binding), None, METHOD, class);
        let days = params.expires_in_days.unwrap_or(DEFAULT_DAYS);
        if !segment(&params.name) || !(1..=MAX_DAYS).contains(&days) {
            return Err(refuse(Class::InvalidInput));
        }
        let harnesses = scope::harnesses(&params.harnesses).map_err(refuse)?;
        let app_data = binding
            .folder
            .parent()
            .map(PathBuf::from)
            .unwrap_or_default();
        let roots = scope::roots(
            &params.directory_roots,
            self.host.home().as_deref(),
            &app_data,
        )
        .map_err(refuse)?;
        let caller_id = crate::service_callers::random_hex(8)
            .map(|hex| format!("sc_{hex}"))
            .ok_or_else(|| refuse(Class::StoreFailed))?;
        // A pending request never takes, records or returns a place for
        // the secret: Desktop picks it when the person approves.
        let handoff_path = match initiator {
            Initiator::App => Some(
                handoff::path(&binding, &caller_id, params.handoff_path.as_deref())
                    .map_err(refuse)?,
            ),
            // Choosing the place is the app's immediate provision, which
            // needs an attested app peer.
            Initiator::Probe if params.handoff_path.is_some() => {
                return Err(refuse(Class::AppAttestationUnavailable));
            }
            Initiator::Probe => None,
        };
        let now = self.host.now_ms();
        let record = CallerRecord {
            caller_id: caller_id.clone(),
            name: params.name.clone(),
            agent_id: String::new(),
            token: None,
            secrets: Vec::new(),
            harnesses,
            directory_roots: roots,
            owns_only: true,
            enabled: false,
            created_at_ms: now,
            rotated_at_ms: None,
            expires_at_ms: now + i64::from(days) * DAY_MS,
            handoff_path,
            requested_by: requester(initiator),
            requested_at_ms: now,
            pending: Some(PendingChange {
                kind: PENDING_PROVISION.into(),
                expires_in_days: Some(days),
                overlap_seconds: None,
            }),
            invocations: Vec::new(),
            authority_generation: 1,
            expiry_advanced: false,
            rollback: None,
        };
        {
            let _files = self.files();
            let mut file = self.registry(&binding, METHOD)?;
            self.name_free(&file, &params.name).map_err(refuse)?;
            file.callers.push(record.clone());
            let _live = self.management_gate(connection_id).map_err(refuse)?;
            registry::save(&binding, &file).map_err(refuse)?;
        }
        if initiator == Initiator::Probe {
            self.audit(
                &binding,
                Some(&caller_id),
                METHOD,
                "pending_provision",
                None,
            );
            return Ok(CallerProvisioned {
                caller: record.summary(now),
                handoff_path: None,
            });
        }
        let caller = self
            .complete_provision(&binding, &caller_id, METHOD, connection_id)
            .await?;
        Ok(CallerProvisioned {
            handoff_path: shown(&caller.handoff_path),
            caller: caller.summary(self.host.now_ms()),
        })
    }

    fn name_free(&self, file: &RegistryFile, name: &str) -> Result<(), Class> {
        if file.callers.iter().any(|caller| caller.name == name)
            || self
                .host
                .registration_names()
                .iter()
                .any(|taken| taken == name)
        {
            return Err(Class::NameTaken);
        }
        Ok(())
    }

    /// Provision steps 3 to 5 for a recorded pending provision: register
    /// the publisher, make the secret, keep its digest and write
    /// `caller.json`. On any failure the caller stays pending and nothing
    /// half-made is kept.
    pub(super) async fn complete_provision(
        &self,
        binding: &Binding,
        caller_id: &str,
        method: &str,
        connection_id: Option<u64>,
    ) -> Result<CallerRecord, RpcError> {
        let refuse = |class| self.refused(Some(binding), Some(caller_id), method, class);
        let name = {
            let _files = self.files();
            let file = self.registry(binding, method)?;
            let index = file
                .by_id(caller_id)
                .ok_or_else(|| refuse(Class::NotFound))?;
            drop(self.management_gate(connection_id).map_err(refuse)?);
            file.callers[index].name.clone()
        };
        let registered = self
            .host
            .register_agent(&binding.workspace, &name)
            .await
            .map_err(|error| {
                self.audit(
                    binding,
                    Some(caller_id),
                    method,
                    "publisher_registration_failed",
                    None,
                );
                relaycast(&error, "publisher_registration_failed")
            })?;
        let written = (|| {
            let _files = self.files();
            let mut file = self.load_current(binding)?;
            let index = file.by_id(caller_id).ok_or(Class::NotFound)?;
            if !file.callers[index].pending_provision() {
                return Err(Class::NotPending);
            }
            let secret = new_secret().ok_or(Class::StoreFailed)?;
            let now = self.host.now_ms();
            let caller = &mut file.callers[index];
            let days = caller
                .pending
                .as_ref()
                .and_then(|pending| pending.expires_in_days)
                .unwrap_or(DEFAULT_DAYS);
            caller.agent_id = registered.id.clone();
            caller.token = Some(registered.token.clone());
            caller.secrets = vec![SecretDigest {
                sha256: registry::digest(&secret),
                created_at_ms: now,
                retires_at_ms: None,
            }];
            caller.enabled = true;
            caller.pending = None;
            caller.created_at_ms = now;
            caller.expires_at_ms = now + i64::from(days) * DAY_MS;
            caller.expiry_advanced = false;
            if caller.handoff_path.is_none() {
                caller.handoff_path = Some(binding.default_handoff(caller_id));
            }
            let caller = caller.clone();
            let _live = self.management_gate(connection_id)?;
            handoff::write(
                binding,
                &caller,
                &secret,
                &self.host.socket(),
                &self.host.version(),
            )?;
            if let Err(class) = registry::save(binding, &file) {
                handoff::remove_kept(&caller);
                return Err(class);
            }
            Ok(caller)
        })();
        match written {
            Ok(caller) => {
                self.audit(binding, Some(caller_id), method, "provisioned", None);
                Ok(caller)
            }
            Err(class) => {
                let _ = self
                    .host
                    .delete_agent(&binding.workspace, &registered.name)
                    .await;
                Err(refuse(class))
            }
        }
    }
}
