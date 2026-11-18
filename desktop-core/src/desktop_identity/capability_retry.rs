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

/// The least wait after a generation's first failed read; each later
/// failure doubles it, up to `RETRY_BASE`. The read is only sent by a sync
/// pass, so it is retried on the first pass after the wait: supervision runs
/// one every minute, and an identity change runs one sooner.
const FIRST_RETRY: Duration = Duration::from_secs(30);

/// How long failed reads of one account generation may hold syncs back,
/// counted from the first of them. A time, not a count of reads: how often
/// the read is retried depends on what runs sync passes.
const HOLD_LIMIT: Duration = RETRY_BASE;

/// The failed reads of one account generation: retried no sooner than
/// `retry_at` (see `FIRST_RETRY`), and holding syncs back until `hold_ends`.
pub(super) struct FailedRead {
    account: CloudAccount,
    generation: u64,
    pub(super) delay: Duration,
    pub(super) retry_at: Instant,
    /// `HOLD_LIMIT` after the first failed read, kept across retries.
    pub(super) hold_ends: Instant,
    /// Whether the last failure holds syncs back (`holds_sync_back`).
    holds: bool,
    /// Whether a failure that held syncs back has been reported.
    reported: bool,
}

impl FailedRead {
    fn holds_now(&self) -> bool {
        self.holds && Instant::now() < self.hold_ends
    }
}

/// Whether a failed read may answer if sent again soon: Cloud unreachable,
/// a timeout, rate limit or server error. Any other answer would come again,
/// so holding a sync for it would hold it until the limit for nothing.
fn read_failure_is_transient(error: &CloudError) -> bool {
    match error {
        CloudError::Unreachable => true,
        CloudError::Status { status, .. } => matches!(status, 408 | 429 | 500..=599),
        CloudError::SignedOut | CloudError::Rejected | CloudError::GateOnly { .. } => false,
    }
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
                (read.retry_at > Instant::now()).then(|| read.holds_now())
            }
            Some(_) => {
                *failed = None;
                None
            }
            None => None,
        }
    }

    /// Remembers a failed read. Returns whether it is the first of this
    /// account generation's failed reads to hold syncs back, which is
    /// reported, and whether it holds them back.
    pub(super) fn remember_failed_capability_read(
        &self,
        account: CloudAccount,
        generation: u64,
        holds: bool,
    ) -> (bool, bool) {
        let mut failed = self
            .capability_read_failed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = Instant::now();
        let (delay, hold_ends, reported) = match failed.as_ref() {
            Some(read) if read.account == account && read.generation == generation => (
                (read.delay * 2).min(RETRY_BASE),
                read.hold_ends,
                read.reported,
            ),
            _ => (FIRST_RETRY, now + HOLD_LIMIT, false),
        };
        let mut read = FailedRead {
            account,
            generation,
            delay,
            retry_at: now + delay,
            hold_ends,
            holds,
            reported,
        };
        let held = read.holds_now();
        read.reported = reported || held;
        *failed = Some(read);
        (held && !reported, held)
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
                Err(error) => self.capability_read_failed(account, generation, &error),
            },
        };
        if held {
            // The identities are not synced this pass.
            self.note_identity_sync(Some(false));
            return false;
        }
        self.signed_in_as(account, generation)
    }

    /// Records a failed read and returns whether it holds the sync back.
    /// A read that failed after the sign-in changed says nothing about the
    /// current one: it is neither recorded nor reported. The first failure
    /// that holds syncs back is reported like a failed sync; later ones of
    /// the same account generation are not.
    fn capability_read_failed(
        &self,
        account: &CloudAccount,
        generation: u64,
        error: &CloudError,
    ) -> bool {
        if !self.signed_in_as(account, generation) {
            return false;
        }
        let holds = self.holds_sync_back(account, error);
        let (first_held, held) =
            self.state
                .remember_failed_capability_read(account.clone(), generation, holds);
        if first_held {
            self.report_sync_failure(error);
        }
        held
    }

    fn signed_in_as(&self, account: &CloudAccount, generation: u64) -> bool {
        self.plugins.cloud.account().as_ref() == Some(account)
            && self.plugins.cloud.identity_sync_generation() == generation
    }

    /// Whether a failed read keeps syncs back, for up to `HOLD_LIMIT`. A
    /// sync without the capability carries no attestation key, which deletes
    /// the one Cloud holds and fails every chat proof until a later sync: so
    /// an installation with a key (one with the account plugin) waits while
    /// the read may answer soon (`read_failure_is_transient`). Without a key
    /// there is nothing to lose, and a site without the identity route
    /// answers the sync with the same 404, which the sync reports; those
    /// syncs go on, as do syncs after any other failure, which report their
    /// own.
    fn holds_sync_back(&self, account: &CloudAccount, error: &CloudError) -> bool {
        self.account.is_some()
            && read_failure_is_transient(error)
            && !identity_sync_unsupported(&account.site_url, error)
    }
}
