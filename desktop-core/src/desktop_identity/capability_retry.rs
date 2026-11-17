//! Backoff for a failed capability read before an account generation's first
//! sync. The sync still goes without it, and supervision asks for one every
//! minute, so a read that keeps failing waits like a refused snapshot instead
//! of being sent again on every pass; a new sign-in or changed account reads
//! at once.

use super::refusal::RETRY_BASE;
use super::{DesktopIdentitySync, Identities};
use agent_relay_node::relay::cloud_session::CloudAccount;
use std::time::Instant;

pub(super) struct FailedRead {
    account: CloudAccount,
    generation: u64,
    pub(super) retry_at: Instant,
}

impl DesktopIdentitySync {
    /// Whether a failed read for this account generation still backs off.
    pub(super) fn wait_for_capability_retry(
        &self,
        account: &CloudAccount,
        generation: u64,
    ) -> bool {
        let mut failed = self
            .capability_read_failed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match failed.as_ref() {
            Some(read) if read.account == *account && read.generation == generation => {
                read.retry_at > Instant::now()
            }
            Some(_) => {
                *failed = None;
                false
            }
            None => false,
        }
    }

    pub(super) fn remember_failed_capability_read(&self, account: CloudAccount, generation: u64) {
        *self
            .capability_read_failed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(FailedRead {
            account,
            generation,
            retry_at: Instant::now() + RETRY_BASE,
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
    /// read first. A failed read still lets the sync go, since identities
    /// must not wait on it, and backs off like a refusal instead of being
    /// read again on every pass. Whether the sync may go on: not when the
    /// sign-in changed during the read.
    pub(super) async fn read_capability_before_sync(
        &self,
        account: &CloudAccount,
        generation: u64,
    ) -> bool {
        if self.state.capability_known(account, generation)
            || self.state.wait_for_capability_retry(account, generation)
        {
            return true;
        }
        if !self.read_capability_locked(account, generation).await {
            self.state
                .remember_failed_capability_read(account.clone(), generation);
        }
        self.plugins.cloud.account().as_ref() == Some(account)
            && self.plugins.cloud.identity_sync_generation() == generation
    }
}
