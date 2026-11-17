//! Native Copilot proofs resolve the synced SID, independent of MCP linking.
use super::{native, Identities};
use agent_relay_node::relay::{
    store::{Registration, SessionKind, Target},
    workspace::Secret,
};
use serde_json::{json, Value};

pub(super) fn identities(
    registration: &Registration,
    own_token: &Secret,
    prefix: &str,
    support: native::Support,
) -> Vec<Value> {
    let current = sending_identity(registration, own_token, prefix, support);
    if registration.target.kind == SessionKind::Copilot
        && support.copilot
        && registration.alias.is_some()
    {
        // Keep the original row for explicit legacy MCP links, but only the
        // current alias owns native authority. Never duplicate the native SID.
        let original = sending_identity(
            registration,
            own_token,
            prefix,
            native::Support {
                copilot: false,
                ..support
            },
        );
        vec![original, current]
    } else {
        vec![current]
    }
}

pub(super) fn sending_identity(
    registration: &Registration,
    own_token: &Secret,
    prefix: &str,
    support: native::Support,
) -> Value {
    // Exactly one row owns the native SID. Exporting both the original and
    // its alias would make Cloud correctly refuse an ambiguous native chat.
    let current = (registration.target.kind == SessionKind::Copilot && support.copilot)
        .then(|| registration.alias_credential())
        .flatten();
    let (sender, token) = current
        .as_ref()
        .map(|(sender, token)| (sender, token))
        .unwrap_or((registration, own_token));
    let mut identity = json!({"name":sender.name,
        "address":sender.direct_address(prefix), "agent_id":sender.agent_id, "token":token});
    native::attach(&mut identity, &registration.target, support);
    identity
}

impl Identities {
    /// Refresh a changed alias before issuing its proof. An accepted old
    /// snapshot is insufficient if the current sender or its token changed.
    pub(crate) async fn sync_copilot_identity(
        &self,
        target: &Target,
    ) -> Option<(Registration, String)> {
        self.sync_desktop_identities().await;
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
            let identity = sending_identity(&registration, token, &self.machine_prefix, support);
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
