//! Cloud's async handoff (`claude_attestation_async_handoff`). This app
//! login declares to Cloud that it hands Claude proofs off in the
//! background (`POST …/attestations {"async_handoff": true}`). After that,
//! Cloud waits briefly for a proof that arrives after its call, so the hook
//! need not wait for the handoff, and a slow Cloud no longer turns into a
//! denied tool call. Cloud keeps a declaration for 2 hours. It is renewed
//! every half hour, and the background handoff runs only while one is
//! recent. While Cloud may still wait on this login's calls, every call
//! the core will not sign gets a marker, so Cloud stops waiting for it.

use super::{DesktopIdentitySync, Identities};
use agent_relay_node::relay::cloud_session::{segment, CloudAccount};
use serde_json::json;
use std::time::{Duration, Instant};

/// How often a declaration is renewed.
const RENEW: Duration = Duration::from_secs(30 * 60);
/// Proofs go out in the background only while the last declaration is this
/// recent: well inside the 2 hours Cloud keeps it.
const BACKGROUND_FOR: Duration = Duration::from_secs(90 * 60);
/// Cloud may wait on this login's calls for up to 2 hours after a
/// declaration. Markers keep going a little longer than that, to allow for
/// clock differences.
const MARKERS_FOR: Duration = Duration::from_secs(135 * 60);

pub(super) struct Declared {
    account: CloudAccount,
    generation: u64,
    at: Instant,
    /// How much older a test made this declaration. An `Instant` cannot be
    /// moved back past the clock's origin (on Windows, the boot, so a fresh
    /// host would panic), so tests add to its age instead.
    #[cfg(test)]
    aged: Duration,
}

impl Declared {
    fn age(&self) -> Duration {
        let age = self.at.elapsed();
        #[cfg(test)]
        let age = age.saturating_add(self.aged);
        age
    }
}

impl DesktopIdentitySync {
    fn declared_within(&self, account: &CloudAccount, generation: u64, within: Duration) -> bool {
        self.async_declared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|declared| {
                declared.account == *account
                    && declared.generation == generation
                    && declared.age() < within
            })
    }

    fn record_declared(&self, account: CloudAccount, generation: u64) {
        *self
            .async_declared
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(Declared {
            account,
            generation,
            at: Instant::now(),
            #[cfg(test)]
            aged: Duration::ZERO,
        });
    }

    #[cfg(test)]
    fn age_declaration(&self, by: Duration) {
        if let Some(declared) = self
            .async_declared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
        {
            declared.aged = declared.aged.saturating_add(by);
        }
    }
}

/// Cloud's handoff route for the account workspace.
pub(crate) fn handoff_path(account: &CloudAccount) -> String {
    format!(
        "/workspaces/{}/{}",
        segment(&account.workspace_id),
        crate::claude_attestation::HANDOFF_PATH
    )
}

impl Identities {
    /// Proofs for this account generation go to Cloud in the background:
    /// Cloud advertises the async handoff and took this login's declaration
    /// recently.
    pub fn claude_async_handoff(&self) -> bool {
        let cloud = &self.plugins.cloud;
        cloud.account().is_some_and(|account| {
            let generation = cloud.identity_sync_generation();
            self.state
                .claude_async_handoff_supported(&account, generation)
                && self
                    .state
                    .declared_within(&account, generation, BACKGROUND_FOR)
        })
    }

    /// Cloud may wait on this login's calls, so a call that gets no proof
    /// needs a marker.
    pub fn claude_markers_expected(&self) -> bool {
        let cloud = &self.plugins.cloud;
        cloud.account().is_some_and(|account| {
            self.state
                .declared_within(&account, cloud.identity_sync_generation(), MARKERS_FOR)
        })
    }

    /// Makes the last declaration `by` older.
    #[cfg(test)]
    pub(crate) fn age_async_declaration(&self, by: Duration) {
        self.state.age_declaration(by);
    }

    /// Declares, or renews, this login's async handoff when Cloud
    /// advertises it. Runs on each maintenance pass and sends nothing
    /// while the last declaration is recent.
    pub async fn maintain_async_handoff(&self) {
        let cloud = &self.plugins.cloud;
        let Some(account) = cloud.account() else {
            return;
        };
        let generation = cloud.identity_sync_generation();
        if !self
            .state
            .claude_async_handoff_supported(&account, generation)
            || self.state.declared_within(&account, generation, RENEW)
        {
            return;
        }
        let declared = cloud
            .request(
                "POST",
                handoff_path(&account),
                Some(json!({ "async_handoff": true })),
            )
            .await;
        if declared.is_err() {
            agent_relay_node::relay::diagnostics::write(format_args!(
                "Claude async handoff declaration failed"
            ));
            return;
        }
        if cloud.account().as_ref() == Some(&account)
            && cloud.identity_sync_generation() == generation
        {
            self.state.record_declared(account, generation);
        }
    }
}
