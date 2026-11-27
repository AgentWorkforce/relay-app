//! Native Copilot proofs resolve the synced SID, independent of MCP linking.
use super::{native, Identities};
use agent_relay_node::relay::store::{Registration, SessionKind, Target};
use std::time::Instant;

impl Identities {
    /// Refresh a changed alias before issuing its proof. An accepted old
    /// snapshot is insufficient if the current sender or its token changed.
    pub(crate) async fn sync_copilot_identity(
        &self,
        target: &Target,
        deadline: Instant,
    ) -> Option<(Registration, String)> {
        if Instant::now() >= deadline {
            return None;
        }
        if let Some(identity) = self.accepted_copilot_identity(target) {
            return Some(identity);
        }
        self.sync_before(deadline).await?;
        self.accepted_copilot_identity(target)
    }

    /// Read-only sender provenance, also checked again at dispatch. A
    /// refresh failure alone does not revoke an unchanged accepted row.
    pub(crate) fn accepted_copilot_identity(
        &self,
        target: &Target,
    ) -> Option<(Registration, String)> {
        let account = self.plugins.cloud.account()?;
        let generation = self.plugins.cloud.identity_sync_generation();
        if target.kind != SessionKind::Copilot || !self.copilot_attestation_verified() {
            return None;
        }
        let (support, _) = self.native_support(&account, generation);
        let (registration, source, identity) = self.store.read(|store| {
            let registration = store
                .registration_for(target)
                .filter(|r| r.missing_since.is_none())?
                .clone();
            let token = store.tokens.get(&registration.name)?;
            let identity =
                native::sending_identity(&registration, token, &self.machine_prefix, support);
            Some((
                registration,
                store.desktop_sync_source_id.clone()?,
                identity,
            ))
        })?;
        self.state
            .identity_synced(&account, generation, &source, &identity)
            .then_some((registration, source))
    }
}

#[cfg(test)]
mod tests;
