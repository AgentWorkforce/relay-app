//! The relay-node token a gate-only install mints for itself after sign-in
//! (`cloud_api::self_tokens`), kept in the OS keychain
//! (`crate::relay_node_token`) and revoked on sign-out.
//!
//! The mint runs off supervision, never inside sign-in: a sign-in onto a
//! gate-only workspace wakes supervision, which mints when the keychain has
//! no live token for the workspace. Control being unreachable, refusing the
//! session or limiting the rate is tried again with a backoff (a 429 first
//! waits its `Retry-After` in place, a few times); a refusal is reported
//! once, as spawning unavailable with its reason, until the next sign-in.
//! A keychain that fails is reported once and tried again with the same
//! backoff: it may be locked, or its daemon restarting.
//!
//! Sign-out waits for a mint in flight (`Mint::wait_for_sign_out`), so the
//! token it mints is kept and revoked with the rest. Lock order:
//! `relay_node_mint` before `refreshing`. The mint takes `refreshing`
//! (renewing the session) while it holds `relay_node_mint`, and sign-out
//! takes them in the same order; nothing holding `refreshing` waits for
//! `relay_node_mint` (sign-out's own `forget_relay_node_token` is handed the
//! guard it already holds).
//!
//! The token loaded or minted, or the reason there is none, is what the
//! node's `relay_node_credential` hook answers (`relay_node_credential`).
//! When Control refuses the token on the node's bridge, it is forgotten
//! here and another minted once.

use super::relay_node_credential::{
    bare_reason, gate_only_workspace, node_credential, REFUSED_AGAIN, UNAVAILABLE_PREFIX,
};
use super::*;
use crate::cloud_api::self_tokens::{self, MintError};
use crate::relay_node_token::{self as keychain, Kept};

/// Mints a 429 waits out in place before supervision takes over.
const MINT_ATTEMPTS: u32 = 3;
/// The longest `Retry-After` waited out in place.
const LONGEST_WAIT_IN_PLACE: Duration = Duration::from_secs(60);

/// The mint's backoff, and the sign-out epoch and workspace it last
/// reported a refusal for, or found a token for.
#[derive(Default)]
pub(super) struct MintProgress {
    retry: Retry,
    refused: Option<(u64, String)>,
    kept: Option<(u64, String)>,
    /// The keychain failed last time and the person was told.
    keychain_failing: bool,
}

/// One mint at a time, with its progress, and sign-out's signal that cuts
/// a `Retry-After` the mint waits out in place short.
#[derive(Default)]
pub(super) struct Mint {
    progress: tokio::sync::Mutex<MintProgress>,
    sign_outs: AtomicU64,
    signing_out: tokio::sync::Notify,
}

impl Mint {
    /// For sign-out: cuts a wait in place short, then waits for the mint
    /// in flight to finish, and holds the mint off until the guard drops.
    pub(super) async fn wait_for_sign_out(&self) -> tokio::sync::MutexGuard<'_, MintProgress> {
        self.sign_outs.fetch_add(1, Ordering::SeqCst);
        self.signing_out.notify_waiters();
        self.progress.lock().await
    }

    /// Waits `wait` unless a sign-out begins (or began since `sign_outs`
    /// was read): whether it waited the whole time.
    async fn wait_in_place(&self, wait: Duration, sign_outs: u64) -> bool {
        let notified = self.signing_out.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.sign_outs.load(Ordering::SeqCst) != sign_outs {
            return false;
        }
        tokio::select! {
            _ = tokio::time::sleep(wait) => true,
            _ = notified => false,
        }
    }
}

/// How long a transient failure waits before the next mint: 10 s after the
/// first, doubling to 320 s, or Control's `Retry-After` when that is longer.
pub(super) fn backoff(failures: u32, error: &MintError) -> Duration {
    let backoff = Duration::from_secs(5 * (1 << failures.clamp(1, 6)));
    match error {
        MintError::RateLimited(wait) => backoff.max(*wait),
        _ => backoff,
    }
}

impl Account {
    /// On a gate-only workspace, wakes supervision so it mints this
    /// computer's relay-node token now, outside the sign-in.
    pub(super) fn wake_for_relay_node_token(&self, core: &Core) {
        if gate_only_workspace(core).is_some() {
            self.wake.notify_one();
        }
    }

    /// Starts the mint in the background unless one is running.
    pub(super) fn schedule_relay_node_token(self: &Arc<Self>) {
        let account = self.clone();
        tokio::spawn(async move { account.ensure_relay_node_token().await });
    }

    /// Whether a mint is running now. Tests wait for it to settle.
    #[cfg(any(test, feature = "test-site"))]
    pub fn relay_node_mint_running(&self) -> bool {
        self.relay_node_mint.progress.try_lock().is_err()
    }

    /// Lets the next mint run now, its backoff cut short. For tests, which
    /// would otherwise wait out ten seconds or more.
    #[cfg(any(test, feature = "test-site"))]
    pub async fn relay_node_mint_retry_now(&self) {
        self.relay_node_mint.progress.lock().await.retry.not_before = None;
    }

    /// The site the account's token is minted on, kept under and revoked
    /// at: the account's, or with none, the one this build signs in to.
    pub(super) fn relay_node_site(&self) -> String {
        self.account()
            .map(|account| account.site_url)
            .unwrap_or_else(|| self.site_url.clone())
    }

    /// Mints this computer's relay-node token when the account's workspace
    /// is gate-only and the keychain has none for it. Never fails: what
    /// went wrong is reported or tried again later.
    pub async fn ensure_relay_node_token(&self) {
        let Ok(mut progress) = self.relay_node_mint.progress.try_lock() else {
            return;
        };
        let Some(core) = self.core() else { return };
        let Some(workspace) = gate_only_workspace(&core) else {
            // Not gate-only (any more): the node has no credential of ours,
            // and a workspace that is gate-only again is looked at afresh.
            progress.kept = None;
            progress.refused = None;
            return self.drop_relay_node_credential(Some(&core), false);
        };
        // The account moved to another workspace without signing out: the
        // one it is in now is looked at afresh.
        if self.drop_other_relay_node_credential(&core, &workspace) {
            progress.kept = None;
            progress.refused = None;
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        let settled = Some((epoch, workspace.clone()));
        if progress.refused == settled {
            return;
        }
        if let Some((refused, again)) = self.take_relay_node_refusal(epoch) {
            // Control refused the token on the node's bridge: it is no use
            // to anyone, so it goes, and another is minted once.
            self.discard_relay_node_token(&refused).await;
            self.drop_relay_node_credential(Some(&core), false);
            progress.kept = None;
            if again {
                return self.refuse_relay_node_token(
                    &core,
                    &mut progress,
                    epoch,
                    &workspace,
                    REFUSED_AGAIN,
                );
            }
        }
        if progress.kept == settled {
            return;
        }
        let now = tokio::time::Instant::now();
        if progress.retry.not_before.is_some_and(|at| now < at) {
            return;
        }
        let site = self.relay_node_site();
        let entry = keychain::entry(&site, &workspace);
        match self.keychain(move |k| keychain::load(k, &entry)).await {
            Ok(Some(kept)) if !kept.expired(agent_relay_node::relay::state::now_ms()) => {
                progress.kept = settled;
                progress.keychain_failing = false;
                let credential = node_credential(&site, &kept);
                return self.hold_relay_node_credential(&core, epoch, &workspace, credential);
            }
            Ok(_) => progress.keychain_failing = false,
            Err(error) => {
                return self.keychain_failed(&core, &mut progress, epoch, &workspace, &error)
            }
        }
        let node_name = agent_relay_node::relay::spawn::node_name(&core.machine_prefix);
        let Some((site, bearer, minted)) = self
            .mint_relay_node_token(epoch, &workspace, &node_name)
            .await
        else {
            return;
        };
        // Signed out while it was minted: nobody keeps it, and nothing is
        // reported for a sign-in that is over.
        let signed_out = || self.epoch.load(Ordering::SeqCst) != epoch;
        let token = match minted {
            Ok(token) if signed_out() => {
                self_tokens::delete(&site, &bearer, &workspace, &token.id).await;
                return;
            }
            Ok(token) => token,
            Err(_) if signed_out() => return,
            Err(error) if error.is_transient() => {
                return back_off(&mut progress.retry, &error);
            }
            Err(error) => {
                let reason = error.unavailable_reason();
                return self.refuse_relay_node_token(
                    &core,
                    &mut progress,
                    epoch,
                    &workspace,
                    &reason,
                );
            }
        };
        let kept = Kept {
            node_name: if token.node_name.is_empty() {
                node_name
            } else {
                token.node_name
            },
            id: token.id,
            expires_at: token.expires_at,
            token: token.token,
        };
        let (entry, value) = (keychain::entry(&site, &workspace), kept.clone());
        if let Err(error) = self
            .keychain(move |k| keychain::store(k, &entry, &value))
            .await
        {
            self_tokens::delete(&site, &bearer, &workspace, &kept.id).await;
            if signed_out() {
                return;
            }
            return self.keychain_failed(&core, &mut progress, epoch, &workspace, &error);
        }
        // Signed out while it was kept: sign-out found nothing to take, so
        // it goes now, before another sign-in could load it.
        if signed_out() {
            self.unkeep_relay_node_token(&workspace, &kept.id).await;
            self_tokens::delete(&site, &bearer, &workspace, &kept.id).await;
            return;
        }
        progress.retry = Retry::default();
        progress.kept = settled;
        progress.keychain_failing = false;
        let credential = node_credential(&site, &kept);
        self.hold_relay_node_credential(&core, epoch, &workspace, credential);
    }

    /// Mints `node_name`'s token in `workspace`: a short `Retry-After` is
    /// waited out in place, a few times, unless a sign-out begins, and a
    /// session Control refuses is renewed once. The site and bearer it went
    /// out with, and Control's answer; `None` when signed out meanwhile.
    async fn mint_relay_node_token(
        &self,
        epoch: u64,
        workspace: &str,
        node_name: &str,
    ) -> Option<(String, Secret, Result<self_tokens::SelfToken, MintError>)> {
        let sign_outs = self.relay_node_mint.sign_outs.load(Ordering::SeqCst);
        let (mut site, mut bearer) = self.pinned_credential(None).await?;
        let (mut attempt, mut renewed) = (1, false);
        loop {
            match self_tokens::mint(&site, &bearer, workspace, node_name).await {
                Err(MintError::RateLimited(wait))
                    if attempt < MINT_ATTEMPTS && wait <= LONGEST_WAIT_IN_PLACE =>
                {
                    if !self.relay_node_mint.wait_in_place(wait, sign_outs).await {
                        return Some((site, bearer, Err(MintError::RateLimited(wait))));
                    }
                    attempt += 1;
                }
                Err(MintError::Unauthorized) if !renewed => {
                    renewed = true;
                    match self.pinned_credential(Some(bearer.clone())).await {
                        Some(credential) => (site, bearer) = credential,
                        None => return Some((site, bearer, Err(MintError::Unauthorized))),
                    }
                }
                other => return Some((site, bearer, other)),
            }
            if self.epoch.load(Ordering::SeqCst) != epoch {
                return None;
            }
        }
    }

    /// Takes the token `id` out of `workspace`'s keychain entry, if that
    /// entry still holds it.
    pub(super) async fn unkeep_relay_node_token(&self, workspace: &str, id: &str) {
        let entry = keychain::entry(&self.relay_node_site(), workspace);
        let id = id.to_string();
        let _ = self
            .keychain(move |k| match keychain::load(k, &entry)? {
                Some(kept) if kept.id == id => k.delete(&entry),
                _ => Ok(()),
            })
            .await;
    }

    /// The keychain could not be read or written: spawning is unavailable
    /// for now, said once while it keeps failing, and tried again after
    /// the backoff rather than given up on until the next sign-in.
    fn keychain_failed(
        &self,
        core: &Core,
        progress: &mut MintProgress,
        epoch: u64,
        workspace: &str,
        error: &str,
    ) {
        back_off(&mut progress.retry, &MintError::Unreachable);
        let reason = format!(
            "{UNAVAILABLE_PREFIX}its relay-node token cannot be kept in the keychain. {error}"
        );
        if !std::mem::replace(&mut progress.keychain_failing, true) {
            core.hub.activity(Level::Warning, reason.clone());
        }
        self.hold_relay_node_credential(core, epoch, workspace, Err(bare_reason(&reason)));
    }

    /// Control will not give this computer a token: said once, and not
    /// asked again until the next sign-in.
    fn refuse_relay_node_token(
        &self,
        core: &Core,
        progress: &mut MintProgress,
        epoch: u64,
        workspace: &str,
        reason: &str,
    ) {
        progress.refused = Some((epoch, workspace.to_string()));
        core.hub.activity(Level::Warning, reason.to_string());
        self.hold_relay_node_credential(core, epoch, workspace, Err(bare_reason(reason)));
    }

    /// Looks up this sign-in's relay-node tokens for sign-out, with the
    /// mint's `progress` sign-out holds: each one's workspace and token
    /// id, which its `DELETE` takes. The keychain entry stays until Control
    /// answers that the token is gone (`unkeep_relay_node_token`).
    /// Only a gate-only workspace, or one whose token this sign-in held, is
    /// looked for: other installs never wait on the keychain.
    pub(super) async fn forget_relay_node_token(
        &self,
        progress: &mut MintProgress,
    ) -> Vec<(String, String)> {
        *progress = MintProgress::default();
        let core = self.core();
        let mut workspaces = self.take_kept_relay_node_workspaces();
        self.drop_relay_node_credential(core.as_deref(), true);
        workspaces.extend(core.as_deref().and_then(gate_only_workspace));
        let site = self.relay_node_site();
        let mut forgotten = Vec::new();
        for workspace in workspaces {
            let entry = keychain::entry(&site, &workspace);
            // A keychain that fails or does not answer holds sign-out up
            // no longer: that workspace's token is left as it is.
            if let Ok(Some(kept)) = self.keychain(move |k| keychain::load(k, &entry)).await {
                forgotten.push((workspace, kept.id));
            }
        }
        forgotten
    }

    /// Takes `workspace`'s relay-node token out of the keychain and deletes
    /// it at Control, best effort.
    async fn discard_relay_node_token(&self, workspace: &str) {
        let entry = keychain::entry(&self.relay_node_site(), workspace);
        let kept = self
            .keychain(move |k| {
                let kept = keychain::load(k, &entry);
                let _ = k.delete(&entry);
                kept
            })
            .await;
        if let Ok(Some(kept)) = kept {
            if let Some((site, bearer)) = self.pinned_credential(None).await {
                self_tokens::delete(&site, &bearer, workspace, &kept.id).await;
            }
        }
    }

    async fn keychain<T: Send + 'static>(
        &self,
        work: impl FnOnce(&dyn keychain::Keychain) -> Result<T, String> + Send + 'static,
    ) -> Result<T, String> {
        let keychain = self.relay_node_keychain.clone();
        tokio::task::spawn_blocking(move || work(&*keychain))
            .await
            .unwrap_or_else(|_| Err("The keychain did not answer.".into()))
    }
}

/// Counts a transient failure and sets when the next mint may run.
fn back_off(retry: &mut Retry, error: &MintError) {
    retry.failures = retry.failures.saturating_add(1);
    retry.not_before = Some(tokio::time::Instant::now() + backoff(retry.failures, error));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transient_failure_backs_off_from_ten_seconds_doubling_to_320() {
        let seconds = |failures| backoff(failures, &MintError::Unreachable).as_secs();
        assert_eq!(
            (1..=8).map(seconds).collect::<Vec<_>>(),
            [10, 20, 40, 80, 160, 320, 320, 320]
        );
        assert_eq!(backoff(1, &MintError::Unauthorized).as_secs(), 10);
        // A longer Retry-After than the backoff is waited out in full.
        let limited = MintError::RateLimited(Duration::from_secs(120));
        assert_eq!(backoff(1, &limited).as_secs(), 120);
        assert_eq!(backoff(6, &limited).as_secs(), 320);
    }
}
