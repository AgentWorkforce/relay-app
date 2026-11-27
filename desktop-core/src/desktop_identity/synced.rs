//! The identity snapshot Cloud last accepted. Supervision asks for a sync
//! every minute, and Cloud verifies every identity against Relaycast on each
//! one, so an unchanged snapshot is sent again only after `REFRESH`; a new
//! sign-in, changed account, or changed identity set is sent at once.

use super::{claude, payload::Payload, DesktopIdentitySync};
use agent_relay_node::relay::cloud_session::CloudAccount;
use serde_json::Value;
use std::collections::BTreeSet;
use std::time::Duration;
use tokio::time::Instant;

const REFRESH: Duration = Duration::from_secs(30 * 60);
const RETRY_SKIPPED: Duration = Duration::from_secs(60);

pub(super) struct SyncedSnapshot {
    account: CloudAccount,
    generation: u64,
    payload: Payload,
    accepted_agents: BTreeSet<String>,
    at: Instant,
    refresh_needed: bool,
}

impl DesktopIdentitySync {
    /// The exact current native identity must be in the accepted snapshot.
    /// A failed rename sync cannot reuse the previous sender's credential.
    pub(super) fn identity_synced(
        &self,
        account: &CloudAccount,
        generation: u64,
        source: &str,
        identity: &Value,
    ) -> bool {
        self.synced
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|snapshot| {
                snapshot.account == *account
                    && snapshot.generation == generation
                    && snapshot.payload.body["source_id"] == source
                    && snapshot.payload.body["identities"]
                        .as_array()
                        .is_some_and(|identities| identities.contains(identity))
            })
    }

    pub(super) fn claude_sender_synced(
        &self,
        account: &CloudAccount,
        generation: u64,
        current: &Payload,
        sender: &claude::Sender,
    ) -> bool {
        self.synced
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|snapshot| {
                snapshot.account == *account
                    && snapshot.generation == generation
                    && snapshot.fresh()
                    && snapshot.payload.relay_workspace == current.relay_workspace
                    && snapshot.payload.body["source_id"] == current.body["source_id"]
                    && snapshot.payload.body["attestation_key"] == current.body["attestation_key"]
                    && snapshot.payload.claude_senders.contains(sender)
                    && snapshot.accepted_agents.contains(sender.agent_id())
            })
    }

    /// Whether Cloud already holds exactly this snapshot, accepted less than
    /// `REFRESH` ago.
    pub(super) fn already_synced(
        &self,
        account: &CloudAccount,
        generation: u64,
        payload: &Payload,
    ) -> bool {
        self.synced
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|snapshot| {
                snapshot.account == *account
                    && snapshot.generation == generation
                    && snapshot.payload == *payload
                    && snapshot.fresh()
                    && (snapshot.at.elapsed() < RETRY_SKIPPED
                        || snapshot
                            .payload
                            .claude_senders
                            .iter()
                            .all(|sender| snapshot.accepted_agents.contains(sender.agent_id())))
            })
    }

    pub(super) fn remember_synced(
        &self,
        account: CloudAccount,
        generation: u64,
        payload: Payload,
        response: &Value,
    ) {
        let accepted_agents = accepted_agents(&payload.body, response).unwrap_or_default();
        *self
            .synced
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(SyncedSnapshot {
            account,
            generation,
            payload,
            accepted_agents,
            at: Instant::now(),
            refresh_needed: false,
        });
    }

    /// A failed refresh must be retried under the normal backoff, but it
    /// does not prove that Cloud removed an unchanged, accepted sender.
    pub(super) fn invalidate_sync_freshness(&self) {
        if let Some(snapshot) = self
            .synced
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            snapshot.refresh_needed = true;
        }
    }

    pub(super) fn forget_synced(&self) {
        *self
            .synced
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

impl SyncedSnapshot {
    fn fresh(&self) -> bool {
        !self.refresh_needed && self.at.elapsed() < REFRESH
    }
}

/// Cloud's PUT can succeed while skipping some or all agents. A complete,
/// consistent receipt is required to treat any particular Claude row as accepted.
fn accepted_agents(body: &Value, response: &Value) -> Option<BTreeSet<String>> {
    if body
        .get("claude_attestation_version")
        .is_some_and(|version| response.get("claude_attestation_version") != Some(version))
    {
        return None;
    }
    let rows = body["identities"].as_array()?;
    let mut accepted = BTreeSet::new();
    for row in rows {
        if !accepted.insert(row["agent_id"].as_str()?.to_owned()) {
            return None;
        }
    }
    if let Some(skipped) = response.get("skipped") {
        for row in skipped.as_array()? {
            if !accepted.remove(row["agent_id"].as_str()?) {
                return None;
            }
        }
    }
    (response["synced"].as_u64()? == accepted.len() as u64).then_some(accepted)
}
