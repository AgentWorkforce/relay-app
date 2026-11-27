//! Build the wire snapshot and keep the provenance used by Claude proofs.
use super::{claude, native, owner_matches, verified_as_account_workspace, Identities};
use agent_relay_node::relay::{
    cloud_session::CloudAccount, state::Verification, store::Target, workspace::Secret,
};
use serde_json::{json, Value};

#[derive(PartialEq)]
pub(super) struct Payload {
    pub(super) body: Value,
    pub(super) claude_senders: Vec<claude::Sender>,
    pub(super) relay_workspace: RelayWorkspace,
}

#[derive(PartialEq)]
pub(super) struct RelayWorkspace {
    base_url: String,
    id: Option<String>,
    key: Secret,
}

impl Identities {
    /// The complete identity set this desktop syncs for the account, or
    /// `None` when there is nothing it may send now.
    pub(super) fn sync_payload(
        &self,
        account: &CloudAccount,
        generation: u64,
        target: Option<&Target>,
    ) -> Option<Payload> {
        // Agent tokens go only with a session confirmed as this person's.
        let (support, attestation_key) = self.native_support(account, generation);
        let active_workspace = self.workspace();
        let active_status = self.status().workspace;
        self.store.read(|store| {
            let source_id = store.desktop_sync_source_id.clone()?;
            let cloud = store.cloud.as_ref()?;
            if cloud.account_workspace_id.as_deref() != Some(account.workspace_id.as_str()) {
                return None;
            }
            // Relaycast can issue multiple keys for one workspace. Accept an
            // active local key only after Relaycast verified its workspace
            // id against the account binding; key equality remains the safe
            // path before verification completes.
            let active_is_account_workspace = verified_as_account_workspace(
                active_workspace.as_ref(),
                &active_status,
                cloud.workspace_id.as_ref(),
            );
            let unresolved_active_key = active_workspace.as_ref().is_some_and(|workspace| {
                workspace.base_url == cloud.base_url
                    && workspace.key != cloud.key
                    && matches!(
                        active_status.verification,
                        Verification::Pending | Verification::Unreachable
                    )
                    && store.registrations.iter().any(|registration| {
                        store.tokens.contains_key(&registration.name)
                            && registration.base_url == workspace.base_url
                            && registration
                                .workspace_key
                                .as_ref()
                                .is_none_or(|key| key == &workspace.key)
                    })
            });
            // Do not briefly delete a valid alternate-key set while its
            // workspace lookup is still pending or retryable. A rejected
            // key is resolved (and therefore safely excluded).
            if unresolved_active_key {
                return None;
            }
            // An absent registration's token stays on this computer: Cloud
            // acts as the identities it holds, and an ended session's agent
            // must not look present anywhere until the session runs again.
            let mut claude_senders = Vec::new();
            let identities: Vec<Value> = store
                .registrations
                .iter()
                .filter(|registration| registration.missing_since.is_none())
                .filter(|registration| target.is_none_or(|target| registration.target == *target))
                // A token on this computer is not permission to transfer the
                // agent when a different person signs in: an agent recorded
                // for someone else (or another workspace) stays here. Legacy
                // and spawned registrations have no local owner record; they
                // sync as before, and Cloud checks each agent's owner itself.
                .filter(|registration| {
                    store
                        .agent_owners
                        .get(&registration.agent_id)
                        .is_none_or(|owner| owner_matches(owner, account))
                })
                // Native spawns are Cloud-created worker identities, not
                // desktop-session identities. Their Relaycast metadata and
                // ownership come from the audited spawn invocation, so the
                // desktop must not try to re-admit them through this route.
                .filter(|registration| registration.spawn.is_none())
                .filter_map(|registration| {
                    let token = store.tokens.get(&registration.name)?;
                    let key = registration.workspace_key.as_ref().or_else(|| {
                        active_workspace
                            .as_ref()
                            .filter(|workspace| workspace.base_url == registration.base_url)
                            .map(|workspace| &workspace.key)
                    });
                    let key_matches = key == Some(&cloud.key)
                        || (active_is_account_workspace
                            && key == active_workspace.as_ref().map(|workspace| &workspace.key));
                    (registration.base_url == cloud.base_url && key_matches).then(|| {
                        let rows =
                            native::identities(registration, token, &self.machine_prefix, support);
                        if let Some(sender) = claude::Sender::from_rows(
                            registration,
                            &self.machine_prefix,
                            support,
                            &rows,
                        ) {
                            claude_senders.push(sender);
                        }
                        rows
                    })
                })
                .flatten()
                .collect();
            let mut body = json!({"source_id": source_id, "identities": identities});
            if support.claude_active_alias {
                body["claude_attestation_version"] =
                    json!(crate::claude_attestation::BOUND_PROOF_VERSION);
            }
            Some(Payload {
                body: native::with_attestation_key(body, attestation_key.as_ref()),
                claude_senders,
                relay_workspace: RelayWorkspace {
                    base_url: cloud.base_url.clone(),
                    id: cloud.workspace_id.clone(),
                    key: cloud.key.clone(),
                },
            })
        })
    }
}
