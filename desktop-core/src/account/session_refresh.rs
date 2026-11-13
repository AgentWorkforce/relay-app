//! Renewing the kept Cloud session, and saying when it is gone.
//!
//! Cloud rotates the refresh token on every use and refuses a retired one.
//! A refresh therefore runs under the store's session refresh lock (shared
//! with every other process keeping this store and with the desktop
//! identity clears), starts from what the store holds on disk, and replaces
//! every copy of the old pair. Before this, a refresh made from a stale copy
//! was refused and the newer session another holder had just saved was
//! forgotten with it.
//!
//! When no usable session is left (Cloud refused the kept one),
//! `status.account` says `needs_sign_in` instead of carrying on as plainly
//! signed in.

use super::{Account, Kept, FRESH_MARGIN};
use agent_relay_node::relay::cloud_session::{self, access_is_fresh, can_refresh, CloudAccess, CloudError};
use agent_relay_node::relay::store::session_refresh::SessionRefreshLock;
use agent_relay_node::relay::store::CloudSession;
use agent_relay_node::relay::workspace::Secret;
use serde_json::Value;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

impl Account {
    /// The refresh locks, in-process then cross-process, with the store's
    /// view of the session brought up to date with the file. `None` when the
    /// cross-process lock cannot be had (or the core is stopping): nothing is
    /// refreshed then, since another process may be rotating the same pair.
    async fn lock_refresh(&self) -> Option<(tokio::sync::MutexGuard<'_, ()>, SessionRefreshLock)> {
        let one = self.refreshing.lock().await;
        let file = self.core()?.store.lock_session_refresh().await?;
        // A store this process cannot read back could not keep a new pair.
        self.core()?.store.reload().ok()?;
        Some((one, file))
    }

    /// Replaces the kept session `expected` (and every copy of its pair)
    /// with `next`. A refresh completing after cancellation must not
    /// overwrite a different account selected in the meantime, or restore
    /// credentials after sign-out: `false` when `expected` is no longer kept.
    pub(super) fn replace_session(
        &self,
        expected: &CloudSession,
        next: Option<&CloudSession>,
    ) -> bool {
        self.core().is_some_and(|core| {
            core.store
                .update_latest(|store| store.rotate_session(expected, next))
                .unwrap_or(false)
        })
    }

    /// The kept session for `site`, refreshed and saved when it has less
    /// than `margin` left. Touches no backoff. A refresh Cloud refuses is
    /// remembered (the person must sign in again); one Cloud could not
    /// answer is `Unavailable`, for a later try.
    pub(super) async fn kept_session(&self, site: &str, margin: Duration) -> Kept {
        let Some(_locks) = self.lock_refresh().await else {
            return Kept::Unavailable(CloudError::Unreachable);
        };
        let Some(session) = self.load_session().filter(|s| s.site_url == site) else {
            return Kept::Absent;
        };
        let now = agent_relay_node::relay::state::now_ms();
        if access_is_fresh(&session, margin, now) {
            return Kept::Usable(session);
        }
        if !can_refresh(&session, now) {
            return Kept::Unrenewable;
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        let refreshed = match cloud_session::refresh(&session).await {
            Ok(refreshed) => refreshed,
            Err(CloudError::Rejected) => {
                // The person is asked to sign in again.
                self.note_refused(&session);
                return Kept::Unrenewable;
            }
            Err(error) => return Kept::Unavailable(error),
        };
        // A sign-out while Cloud answered keeps nothing from it.
        if self.epoch.load(Ordering::SeqCst) != epoch
            || !self.replace_session(&session, Some(&refreshed))
        {
            return Kept::Unrenewable;
        }
        Kept::Usable(refreshed)
    }

    /// Replaces the kept session for `site` whose access token Cloud just
    /// refused: with one another call (or process) already renewed, else by
    /// refreshing it. Runs under the refresh locks and saves nothing after a
    /// sign-out. A session kept for another site meanwhile is never handed
    /// out for `site`.
    pub(super) async fn renew(
        &self,
        site: &str,
        refused: &Secret,
    ) -> Result<CloudSession, CloudError> {
        self.renew_session(site, refused, true).await
    }

    // Background checks preserve the refused app login so they cannot silently
    // fall through to another person's independent CLI credentials.
    pub(super) async fn renew_for_health(
        &self,
        site: &str,
        refused: &Secret,
    ) -> Result<CloudSession, CloudError> {
        self.renew_session(site, refused, false).await
    }

    async fn renew_session(
        &self,
        site: &str,
        refused: &Secret,
        forget_rejected: bool,
    ) -> Result<CloudSession, CloudError> {
        let _locks = self.lock_refresh().await.ok_or(CloudError::Unreachable)?;
        let epoch = self.epoch.load(Ordering::SeqCst);
        let session = self.load_session().ok_or(CloudError::Rejected)?;
        if session.site_url != site {
            return Err(CloudError::SignedOut);
        }
        if &session.access_token != refused {
            return Ok(session);
        }
        if !can_refresh(&session, agent_relay_node::relay::state::now_ms()) {
            if forget_rejected {
                self.replace_session(&session, None);
            }
            return Err(CloudError::Rejected);
        }
        match cloud_session::refresh(&session).await {
            Ok(renewed)
                if self.epoch.load(Ordering::SeqCst) == epoch
                    && self.replace_session(&session, Some(&renewed)) =>
            {
                Ok(renewed)
            }
            Ok(_) => Err(CloudError::SignedOut),
            Err(CloudError::Rejected) => {
                match !forget_rejected || self.replace_session(&session, None) {
                    true => Err(CloudError::Rejected),
                    false => Err(CloudError::SignedOut),
                }
            }
            Err(error) => Err(error),
        }
    }

    /// A bearer for `site` without asking the person, or why there is none:
    /// `Rejected` when no session is left, another error when Cloud could not
    /// renew the kept one just now. `true` when it is the kept session.
    pub(super) async fn request_bearer(&self, site: &str) -> Result<(Secret, bool), CloudError> {
        if let Some(token) = &self.env_token {
            return Ok((token.clone(), false));
        }
        match self.kept_session(site, FRESH_MARGIN).await {
            Kept::Usable(session) => Ok((session.access_token, true)),
            Kept::Unavailable(error) => Err(error),
            Kept::Unrenewable | Kept::Absent => Err(CloudError::Rejected),
        }
    }

    /// Whether the person must sign in again for Cloud calls: attached, yet
    /// with no session this computer may use. Reads only what is on hand.
    fn needs_sign_in_now(&self) -> bool {
        if self.env_token.is_some() {
            return false;
        }
        let Some(account) = CloudAccess::account(self) else {
            return false;
        };
        let site = account.site_url;
        let now = agent_relay_node::relay::state::now_ms();
        match self.load_session().filter(|s| s.site_url == site) {
            Some(kept) => {
                let stale = !access_is_fresh(&kept, FRESH_MARGIN, now);
                let refused = kept.refresh_token.is_some()
                    && *lock(&self.refused_refresh) == kept.refresh_token;
                refused || (stale && !can_refresh(&kept, now))
            }
            None => true,
        }
    }

    /// Whether `account.sign_in` while connected must ask the person: no
    /// session for `site` this computer may use, including one Cloud refused
    /// even renewed, whose access token still looks fresh.
    pub(super) async fn must_sign_in_again(&self, site: &str) -> bool {
        self.needs_sign_in_now() || self.bearer(site).await.is_none()
    }

    /// Remembers that Cloud refused `session` even renewed (or refused to
    /// renew it): the person must sign in again until another is kept.
    pub(super) fn note_refused(&self, session: &CloudSession) {
        *lock(&self.refused_refresh) = session.refresh_token.clone();
    }

    /// Brings `status.account.needs_sign_in` up to date. Takes no attempt
    /// lock: a sign-in keeps its session while holding it.
    pub(super) fn sync_needs_sign_in(&self) {
        let needs = self.needs_sign_in_now();
        if self.status().needs_sign_in != needs {
            self.update(|account| account.needs_sign_in = needs);
        }
    }

    /// After a Cloud call: a refusal may mean the session is gone, and an
    /// answer that the session works again.
    pub(super) fn note_cloud_answer(&self, answer: &Result<Value, CloudError>) {
        if answer.is_ok() {
            *lock(&self.refused_refresh) = None;
        }
        self.note_credential(answer);
    }

    /// After looking for a credential: none may mean the session is gone,
    /// and one that a session is back.
    pub(super) fn note_credential<T>(&self, found: &Result<T, CloudError>) {
        match found {
            Err(CloudError::Rejected) => self.sync_needs_sign_in(),
            Ok(_) if self.status().needs_sign_in => self.sync_needs_sign_in(),
            _ => {}
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
