//! Backoff for a failed capability read before an account generation's first
//! sync. Supervision asks for a sync every minute, so a read that keeps
//! failing is retried after a short wait that doubles up to the refusal retry
//! interval instead of being sent again on every pass; a new sign-in or
//! changed account reads at once.

use super::failure::identity_sync_unsupported;
use super::refusal::RETRY_BASE;
use super::{DesktopIdentitySync, Identities};
use agent_relay_node::relay::cloud_session::{CloudAccount, CloudError};
use std::time::{Duration, Instant};

/// The wait after a generation's first failed read; each later failure
/// doubles it, up to `RETRY_BASE`.
const FIRST_RETRY: Duration = Duration::from_secs(30);

pub(super) struct FailedRead {
    account: CloudAccount,
    generation: u64,
    pub(super) delay: Duration,
    pub(super) retry_at: Instant,
    /// Whether syncs wait for the capability (`holds_sync_back`).
    holds: bool,
}

impl DesktopIdentitySync {
    /// While a failed read for this account generation backs off, whether
    /// it holds syncs back; `None` when the capability may be read now.
    pub(super) fn wait_for_capability_retry(
        &self,
        account: &CloudAccount,
        generation: u64,
    ) -> Option<bool> {
        let mut failed = self
            .capability_read_failed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match failed.as_ref() {
            Some(read) if read.account == *account && read.generation == generation => {
                (read.retry_at > Instant::now()).then_some(read.holds)
            }
            Some(_) => {
                *failed = None;
                None
            }
            None => None,
        }
    }

    pub(super) fn remember_failed_capability_read(
        &self,
        account: CloudAccount,
        generation: u64,
        holds: bool,
    ) {
        let mut failed = self
            .capability_read_failed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let delay = match failed.as_ref() {
            Some(read) if read.account == account && read.generation == generation => {
                (read.delay * 2).min(RETRY_BASE)
            }
            _ => FIRST_RETRY,
        };
        *failed = Some(FailedRead {
            account,
            generation,
            delay,
            retry_at: Instant::now() + delay,
            holds,
        });
    }

    pub(super) fn forget_failed_capability_read(&self) {
        *self
            .capability_read_failed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

impl Identities {
    /// Cloud replaces the installation's attestation key with each sync's,
    /// so a body built before the capability is known (the first sync after
    /// launch) would delete the key until the next pass: the capability is
    /// read first. Whether the sync may go on: not when the sign-in changed
    /// during the read, nor while a failed read holds it back.
    pub(super) async fn read_capability_before_sync(
        &self,
        account: &CloudAccount,
        generation: u64,
    ) -> bool {
        if self.state.capability_known(account, generation) {
            return true;
        }
        let held = match self.state.wait_for_capability_retry(account, generation) {
            Some(holds) => holds,
            None => match self.read_capability_locked(account, generation).await {
                Ok(()) => false,
                Err(error) => {
                    let holds = self.holds_sync_back(account, &error);
                    self.state
                        .remember_failed_capability_read(account.clone(), generation, holds);
                    holds
                }
            },
        };
        if held {
            // The identities are not synced this pass.
            self.note_identity_sync(Some(false));
            return false;
        }
        self.plugins.cloud.account().as_ref() == Some(account)
            && self.plugins.cloud.identity_sync_generation() == generation
    }

    /// Whether a failed read keeps syncs back until a read succeeds. A sync
    /// without the capability carries no attestation key, which deletes the
    /// one Cloud holds and fails every chat proof until a later sync: so an
    /// installation with a key (one with the account plugin) waits. Without
    /// one there is no key to lose, and a site without the identity route
    /// answers the sync with the same 404, which the sync reports; those
    /// syncs go on, since identities must not wait on the read.
    fn holds_sync_back(&self, account: &CloudAccount, error: &CloudError) -> bool {
        self.account.is_some() && !identity_sync_unsupported(&account.site_url, error)
    }
}
