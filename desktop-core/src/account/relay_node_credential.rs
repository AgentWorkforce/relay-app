//! Handing a gate-only install's relay-node token to the spawn node
//! (`CloudAccess::relay_node_credential`), and acting when Control refuses
//! it.
//!
//! The node reads the credential synchronously whenever the workspace is
//! rediscovered, so the hook answers from memory alone: the token the mint
//! loaded or minted (`relay_node_token`), or the reason it recorded for
//! spawning being unavailable. Every change to that answer rediscovers the
//! workspace so the node follows it.
//!
//! When Control refuses the credential on its node bridge, the node stops
//! and reports `credential_refused` in its spawn status. The watcher here
//! sees that and wakes the mint, which forgets the token (keychain and
//! Control) and mints another once. A second refusal in a row, before the
//! node was live again, reports spawning unavailable until the next
//! sign-in.

use super::*;
use crate::relay_node_token::Kept;
use agent_relay_node::relay::cloud_session::RelayNodeCredential;
use agent_relay_node::relay::protocol::{Event, Topic};
use tokio::sync::broadcast::{error::RecvError, Receiver};

/// What the core puts before the desktop's own reason
/// (`Err(Some(reason))`) when it shows that spawning is unavailable.
pub(super) const UNAVAILABLE_PREFIX: &str = "Spawning is unavailable on this computer: ";

/// The reason a second refusal in a row records.
pub(super) const REFUSED_AGAIN: &str = "Spawning is unavailable on this computer: Agent Relay's \
     Control refused this computer's new relay-node token too; sign out and sign in again to try \
     once more (credential_refused).";

/// What the hook answers, and Control's refusals of the credential.
#[derive(Default)]
pub(super) struct Held {
    answer: Option<Answer>,
    /// Control refused the credential and the mint has not acted on it.
    refused: bool,
    /// Refusals since the node was last live.
    refusals: u32,
    /// Workspaces whose token this sign-in held: sign-out takes each out
    /// of the keychain, the account's workspace having maybe moved since.
    kept: std::collections::BTreeSet<String>,
}

/// The hook's answer for one sign-out epoch and workspace: the credential,
/// or the reason without [`UNAVAILABLE_PREFIX`].
struct Answer {
    epoch: u64,
    workspace: String,
    credential: Result<RelayNodeCredential, String>,
}

/// The credential `kept` makes for the node: Control's bare origin is the
/// site the token was minted on.
pub(super) fn node_credential(site: &str, kept: &Kept) -> Result<RelayNodeCredential, String> {
    let control_url = validated_base_url(site).ok_or_else(|| {
        "this computer's Agent Relay site is not an https origin, so its relay-node token \
         cannot reach Agent Relay's Control"
            .to_string()
    })?;
    Ok(RelayNodeCredential {
        control_url,
        node_name: kept.node_name.clone(),
        token: kept.token.clone(),
    })
}

/// The account's workspace when it is gate-only: the one whose token the
/// node is handed. Read from the store in memory, as the core reads it.
pub(super) fn gate_only_workspace(core: &Core) -> Option<String> {
    core.store.read(|s| {
        s.cloud
            .as_ref()
            .filter(|cloud| cloud.gate_url.is_some())
            .and_then(|cloud| cloud.account_workspace_id.clone())
    })
}

/// `reason` as the core wants it: without the prefix it adds.
pub(super) fn bare_reason(reason: &str) -> String {
    reason
        .strip_prefix(UNAVAILABLE_PREFIX)
        .unwrap_or(reason)
        .trim()
        .to_string()
}

impl Account {
    /// Watches the node's spawn status for Control refusing the token.
    pub(super) fn watch_relay_node_refusals(self: &Arc<Self>, core: &Core) {
        tokio::spawn(watch_refusals(Arc::downgrade(self), core.hub.subscribe()));
    }

    fn held(&self) -> std::sync::MutexGuard<'_, Held> {
        self.relay_node_held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The hook's answer, from memory: no keychain or network. One held
    /// for a workspace the account has left since is no answer, so the
    /// node never takes one workspace's token for another.
    pub(super) fn held_relay_node_credential(&self) -> Result<RelayNodeCredential, Option<String>> {
        let epoch = self.epoch.load(Ordering::SeqCst);
        let workspace = self.core().and_then(|core| gate_only_workspace(&core));
        match &self.held().answer {
            Some(answer)
                if answer.epoch == epoch
                    && workspace.as_deref() == Some(answer.workspace.as_str()) =>
            {
                answer.credential.clone().map_err(Some)
            }
            _ => Err(None),
        }
    }

    /// Answers the hook with `credential` for `workspace` from now on, and
    /// has the node read it again when that changes the answer.
    pub(super) fn hold_relay_node_credential(
        &self,
        core: &Core,
        epoch: u64,
        workspace: &str,
        credential: Result<RelayNodeCredential, String>,
    ) {
        let changed = {
            let mut held = self.held();
            let same = held.answer.as_ref().is_some_and(|answer| {
                answer.epoch == epoch
                    && answer.workspace == workspace
                    && answer.credential == credential
            });
            if credential.is_ok() {
                held.kept.insert(workspace.to_string());
            }
            held.answer = Some(Answer {
                epoch,
                workspace: workspace.to_string(),
                credential,
            });
            !same
        };
        if changed {
            core.rediscover_workspace();
        }
    }

    /// Stops answering with a credential or reason; the node reads the
    /// hook again when there was one. `all` also forgets refusals.
    pub(super) fn drop_relay_node_credential(&self, core: Option<&Core>, all: bool) {
        let dropped = {
            let mut held = self.held();
            let dropped = held.answer.take().is_some();
            if all {
                *held = Held::default();
            }
            dropped
        };
        if let (true, Some(core)) = (dropped, core) {
            core.rediscover_workspace();
        }
    }

    /// Stops answering for another workspace than `workspace`, the
    /// account having moved without a sign-out; a refusal of that one's
    /// token is no longer acted on. Its token stays kept for a move back.
    /// Whether it dropped one.
    pub(super) fn drop_other_relay_node_credential(&self, core: &Core, workspace: &str) -> bool {
        let dropped = {
            let mut held = self.held();
            let other = held
                .answer
                .as_ref()
                .is_some_and(|answer| answer.workspace != workspace);
            if other {
                held.answer = None;
                held.refused = false;
                held.refusals = 0;
            }
            other
        };
        if dropped {
            core.rediscover_workspace();
        }
        dropped
    }

    /// The workspaces whose token this sign-in held, taken for sign-out.
    pub(super) fn take_kept_relay_node_workspaces(&self) -> std::collections::BTreeSet<String> {
        std::mem::take(&mut self.held().kept)
    }

    /// The workspace whose held credential Control refused, taken once,
    /// and whether it was refused before since the node was last live. A
    /// refusal while no credential is held for `epoch` is dropped.
    pub(super) fn take_relay_node_refusal(&self, epoch: u64) -> Option<(String, bool)> {
        let mut held = self.held();
        if !std::mem::take(&mut held.refused) {
            return None;
        }
        let answer = held.answer.as_ref()?;
        if answer.epoch != epoch || answer.credential.is_err() {
            return None;
        }
        Some((answer.workspace.clone(), held.refusals > 1))
    }

    /// The spawn node's status changed: a new refusal is noted and the
    /// mint woken; a live node clears the count of refusals in a row.
    fn note_spawn_status(self: &Arc<Self>, spawn: &Value, was_refused: bool) -> bool {
        let refused = spawn["credential_refused"] == true;
        if spawn["state"] == "live" {
            self.held().refusals = 0;
        }
        if refused && !was_refused {
            {
                let mut held = self.held();
                held.refused = true;
                held.refusals = held.refusals.saturating_add(1);
            }
            self.schedule_relay_node_token();
        }
        refused
    }
}

/// Follows the core's spawn status for the daemon's life, telling
/// `account` each time Control refuses the node's relay-node credential.
pub(super) async fn watch_refusals(account: Weak<Account>, mut events: Receiver<Event>) {
    let mut refused = false;
    loop {
        let spawn = match events.recv().await {
            Ok(event) if event.topic == Topic::Status => event.data["spawn"].clone(),
            Ok(_) => continue,
            Err(RecvError::Lagged(_)) => {
                let Some(core) = account.upgrade().and_then(|account| account.core()) else {
                    return;
                };
                serde_json::to_value(core.hub.snapshot().spawn).unwrap_or(Value::Null)
            }
            Err(RecvError::Closed) => return,
        };
        let Some(account) = account.upgrade() else {
            return;
        };
        refused = account.note_spawn_status(&spawn, refused);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept() -> Kept {
        Kept {
            id: "tok_1".into(),
            node_name: "desk-desktop".into(),
            expires_at: None,
            token: Secret::new("sh_wk_fixture1234"),
        }
    }

    #[test]
    fn the_control_origin_is_the_sites_bare_origin() {
        let credential = node_credential("https://relay.example.test/", &kept()).unwrap();
        assert_eq!(credential.control_url, "https://relay.example.test");
        assert_eq!(credential.node_name, "desk-desktop");
        assert_eq!(credential.token.expose(), "sh_wk_fixture1234");
        for site in [
            "https://relay.example.test/cloud",
            "http://relay.example.test",
            "",
        ] {
            let reason = node_credential(site, &kept()).unwrap_err();
            assert!(!reason.contains("sh_wk_"), "{reason}");
        }
    }

    #[test]
    fn a_reason_loses_the_prefix_the_core_adds() {
        assert_eq!(
            bare_reason(
                "Spawning is unavailable on this computer: Agent Relay's Control does not allow \
                 this computer to mint its own relay-node token (self_mint_disabled)."
            ),
            "Agent Relay's Control does not allow this computer to mint its own relay-node \
             token (self_mint_disabled)."
        );
        assert_eq!(bare_reason("no keychain."), "no keychain.");
        assert!(!bare_reason(REFUSED_AGAIN).starts_with("Spawning"));
    }
}
