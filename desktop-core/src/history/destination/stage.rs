//! A receiver's stage and the session it keeps between sends.

use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

/// A receiver's stage session, kept between sends. It is read from the auth
/// store when there is none, when its access token is within a minute of its
/// stored expiry (or has no stored expiry), and after any failed send, so a
/// 401, a rotation or a revocation is followed by a fresh read; otherwise the
/// copy the last successful send used is reused. A 401 normally goes through
/// the shared locked reload-and-refresh path, which also picks up a rotation
/// another process made. A command-scoped fixed session never does.
#[derive(Default)]
pub(super) struct SessionCache {
    current: Mutex<Option<(Option<String>, cloud::StoredAuth)>>,
    /// A command-scoped session that must never be read from or written to
    /// the shared auth store. Hosted teardown flushes use this so a forced
    /// process exit cannot strand their bearer on disk.
    fixed: Option<(Option<String>, cloud::StoredAuth)>,
    /// Bumped whenever the host replaces the session: what was obtained with
    /// an earlier one (a send in flight, credentials handed out to it) is
    /// never kept past it.
    epoch: AtomicU64,
}
/// Never prints the session: it holds bearer and refresh tokens.
impl std::fmt::Debug for SessionCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SessionCache")
    }
}
impl SessionCache {
    pub(super) fn fixed(base_url: String, auth: cloud::StoredAuth) -> Self {
        Self {
            current: Mutex::new(None),
            fixed: Some((Some(base_url), auth)),
            epoch: AtomicU64::new(0),
        }
    }

    fn get(&self, base_url: Option<&str>) -> Result<cloud::StoredAuth> {
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((stage, auth)) = current.as_ref() {
            if stage.as_deref() == base_url && cloud::token_has_valid_lifetime(auth) {
                return Ok(auth.clone());
            }
        }
        *current = None;
        if let Some((stage, auth)) = self.fixed.as_ref() {
            if stage.as_deref() == base_url {
                *current = Some((stage.clone(), auth.clone()));
                return Ok(auth.clone());
            }
        }
        let auth = cloud::load_selected_auth(base_url)
            .map_err(|_| fail(DeliveryFailure::AuthenticationRequired))?
            .ok_or_else(|| fail(DeliveryFailure::AuthenticationRequired))?;
        *current = Some((base_url.map(str::to_owned), auth.clone()));
        Ok(auth)
    }
    #[cfg(test)]
    pub(super) fn keep(&self, base_url: Option<&str>, auth: cloud::StoredAuth) {
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((base_url.map(str::to_owned), auth));
    }
    pub(super) fn forget(&self) {
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    /// The session generation now: changed by [`Self::replace`] only.
    pub(super) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }
    /// The host stored another session: forget the kept one and start a
    /// new generation, so nothing obtained with the old one is kept.
    pub(super) fn replace(&self) {
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        self.epoch.fetch_add(1, Ordering::SeqCst);
        *current = None;
    }
    /// Keep `auth` only while the generation is still `epoch`: a send that
    /// began before the session was replaced must not bring the old one
    /// back.
    pub(super) fn keep_from(&self, epoch: u64, base_url: Option<&str>, auth: cloud::StoredAuth) {
        let mut current = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if self.epoch() == epoch {
            *current = Some((base_url.map(str::to_owned), auth));
        }
    }
}

/// A receiver's stage: the base URL that selects its stored session, and the
/// cache that keeps it between sends.
#[derive(Clone, Copy)]
pub(super) struct Stage<'a> {
    pub(super) cache: &'a SessionCache,
    pub(super) base_url: Option<&'a str>,
    /// Where the limits a receipt publishes are kept.
    pub(super) published: &'a ReceiverState,
}
impl Stage<'_> {
    pub(super) fn session(&self) -> Result<cloud::StoredAuth> {
        self.cache.get(self.base_url)
    }
    /// Keep `auth` if the session was not replaced since `epoch`.
    pub(super) fn keep_from(&self, epoch: u64, auth: cloud::StoredAuth) {
        self.cache.keep_from(epoch, self.base_url, auth)
    }
    pub(super) fn epoch(&self) -> u64 {
        self.cache.epoch()
    }
    pub(super) fn forget(&self) {
        self.cache.forget()
    }
    pub(super) fn fixed(&self) -> bool {
        self.cache
            .fixed
            .as_ref()
            .is_some_and(|(stage, _)| stage.as_deref() == self.base_url)
    }
}
