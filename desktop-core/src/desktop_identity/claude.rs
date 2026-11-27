//! Claude's proof uses the same native row the sync exported.
use super::{native, payload::Payload, Identities};
use agent_relay_node::relay::{
    cloud_session::CloudAccount,
    store::{Registration, Target},
};
use serde_json::Value;
use std::time::Instant;

#[derive(PartialEq)]
pub(crate) struct Sender {
    row: Value,
    registration: (String, String),
    alias_registered_at: Option<String>,
    pub(crate) link_address: String,
    pub(crate) display_name: String,
    pub(crate) binds_credential: bool,
}

impl Sender {
    pub(super) fn from_rows(
        registration: &Registration,
        prefix: &str,
        support: native::Support,
        rows: &[Value],
    ) -> Option<Self> {
        let row = rows.last()?;
        (row["native_session"]["kind"] == "claude").then(|| Self {
            row: row.clone(),
            registration: (
                registration.agent_id.clone(),
                registration.registered_at.clone(),
            ),
            alias_registered_at: registration
                .alias
                .as_ref()
                .map(|alias| alias.registered_at.clone()),
            link_address: registration.link_address(prefix),
            display_name: registration.display_name().to_owned(),
            binds_credential: support.claude_active_alias,
        })
    }

    pub(crate) fn agent_id(&self) -> &str {
        self.row["agent_id"].as_str().unwrap_or_default()
    }

    pub(crate) fn address(&self) -> &str {
        self.row["address"].as_str().unwrap_or_default()
    }

    pub(crate) fn token(&self) -> &str {
        self.row["token"].as_str().unwrap_or_default()
    }
}

/// The current registration, source, and credential captured before any
/// refresh. A change while the hook waits must make this call retry.
pub(crate) struct ClaudeIdentity(Payload);

impl ClaudeIdentity {
    pub(crate) fn source(&self) -> &str {
        self.0.body["source_id"].as_str().unwrap_or_default()
    }

    pub(crate) fn key_matches(&self, key: &Value) -> bool {
        self.0.body["attestation_key"] == *key
    }

    pub(crate) fn sender(&self) -> &Sender {
        &self.0.claude_senders[0]
    }
}

impl Identities {
    pub(crate) fn current_claude_identity(
        &self,
        account: &CloudAccount,
        generation: u64,
        target: &Target,
    ) -> Option<ClaudeIdentity> {
        let cloud = &self.plugins.cloud;
        if !cloud.may_sync_identities() {
            return None;
        }
        let payload = self.sync_payload(account, generation, Some(target))?;
        (payload.claude_senders.len() == 1
            && cloud.account().as_ref() == Some(account)
            && cloud.identity_sync_generation() == generation)
            .then_some(ClaudeIdentity(payload))
    }

    pub(crate) fn claude_identity_accepted(
        &self,
        account: &CloudAccount,
        generation: u64,
        target: &Target,
        expected: &ClaudeIdentity,
    ) -> bool {
        self.current_claude_identity(account, generation, target)
            .is_some_and(|current| {
                current.0 == expected.0
                    && self.state.claude_sender_synced(
                        account,
                        generation,
                        &current.0,
                        &current.0.claude_senders[0],
                    )
            })
    }

    pub(crate) async fn sync_claude_identity(
        &self,
        account: &CloudAccount,
        generation: u64,
        target: &Target,
        expected: &ClaudeIdentity,
        deadline: Instant,
    ) -> bool {
        if self.claude_identity_accepted(account, generation, target, expected) {
            return true;
        }
        Instant::now() < deadline
            && self.sync_before(deadline).await.is_some()
            && self.claude_identity_accepted(account, generation, target, expected)
    }
}
