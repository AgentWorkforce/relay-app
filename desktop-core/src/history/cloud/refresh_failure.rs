//! What a failed session refresh means for the request that needed it.
use super::{
    acquire_refresh_lock_with_budget, load_auth, normalized_stage, refresh_auth_with_agent,
    save_auth, AuthRefreshLock, RequestBudget, StoredAuth,
};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, MutexGuard};

/// Whether an authenticated request needs the shared refresh path.
pub(super) fn is_unauthorized(error: &ureq::Error) -> bool {
    matches!(error, ureq::Error::Status(401, _))
}

/// Whether the refresh endpoint refused the refresh token (`map_http_err`'s
/// `HTTP 400/401/403`), as opposed to failing to answer.
pub(super) fn refresh_refused(error: &anyhow::Error) -> bool {
    // Through any context added on the way up.
    error.chain().any(|cause| {
        let message = cause.to_string();
        ["HTTP 400:", "HTTP 401:", "HTTP 403:"]
            .iter()
            .any(|status| message.starts_with(status))
    })
}

/// A refresh that never got an answer on the token: the service failed or the
/// connection dropped. The refresh token may still be good, so this is not a
/// missing sign-in. It reads as the failure it wraps.
#[derive(Debug)]
pub(crate) struct RefreshUnavailable(pub(super) anyhow::Error);

impl RefreshUnavailable {
    pub(crate) fn wrap(error: anyhow::Error) -> anyhow::Error {
        if super::HistoryTlsConfigurationRefused::caused_by(&error) {
            super::HistoryTlsConfigurationRefused.into()
        } else {
            Self(error).into()
        }
    }
}

impl fmt::Display for RefreshUnavailable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, formatter)
    }
}

impl std::error::Error for RefreshUnavailable {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

/// The refresh lock for `auth`'s stage and the pair stored under it (`auth`
/// when none is), or the local failure that kept either from the disk.
pub(super) fn locked_reload(
    auth: &StoredAuth,
    budget: RequestBudget<'_>,
) -> anyhow::Result<(AuthRefreshLock, StoredAuth)> {
    let lock = acquire_refresh_lock_with_budget(&auth.base_url, budget).map_err(local_failure)?;
    let Some(current) = load_auth(Some(&auth.base_url)).map_err(local_failure)? else {
        forget_unsaved(&auth.base_url);
        return Ok((lock, auth.clone()));
    };
    if unsaved().contains_key(&stage(&current.base_url)) {
        // Under the lock, so no other refresh in this process races it. A
        // store that works forgets the unsaved copy.
        let _ = save_auth(&current);
    }
    Ok((lock, current))
}

/// A rotated pair this process could not store, and the refresh token it
/// replaced. Cloud retired that token with the rotation, so until the pair
/// is stored this copy is the stage's session here: loads read it in place of
/// the stored pair it replaced, and every later send tries to store it.
struct Unsaved {
    retired: Option<String>,
    pair: StoredAuth,
}

static UNSAVED: Mutex<BTreeMap<String, Unsaved>> = Mutex::new(BTreeMap::new());

fn unsaved() -> MutexGuard<'static, BTreeMap<String, Unsaved>> {
    UNSAVED.lock().unwrap_or_else(|e| e.into_inner())
}

fn stage(base_url: &str) -> String {
    normalized_stage(base_url).unwrap_or_else(|_| base_url.to_owned())
}

/// Keeps the pair `current` rotated to: on disk, or, on a full or unwritable
/// disk, in this process until a later send stores it. Never drops it, since
/// the refresh token on disk no longer refreshes.
pub(super) fn save_rotated(current: &StoredAuth, refreshed: &StoredAuth) {
    if save_auth(refreshed).is_err() {
        let mut unsaved = unsaved();
        let key = stage(&refreshed.base_url);
        // Rotating an unsaved pair replaces it, not the stored pair it stands
        // for: keep naming the token on disk, which loads still read.
        let retired = match unsaved.get(&key) {
            Some(kept) if kept.pair.refresh_token == current.refresh_token => kept.retired.clone(),
            _ => current.refresh_token.clone(),
        };
        unsaved.insert(
            key,
            Unsaved {
                retired,
                pair: refreshed.clone(),
            },
        );
    }
}

/// `auth` refreshed with `agent`, its rotated pair kept (`save_rotated`).
pub(super) fn refresh_kept(auth: &StoredAuth, agent: &ureq::Agent) -> anyhow::Result<StoredAuth> {
    let refreshed = refresh_auth_with_agent(auth, agent)?;
    save_rotated(auth, &refreshed);
    Ok(refreshed)
}

/// Forgets the unsaved pair of a stage whose session was stored or removed.
pub(super) fn forget_unsaved(base_url: &str) {
    unsaved().remove(&stage(base_url));
}

/// The session `stored` stands for in this process: the unsaved pair that
/// replaced it, if any. A stored pair it did not replace (a new sign-in)
/// supersedes the unsaved one.
pub(super) fn over_unsaved(stored: StoredAuth) -> StoredAuth {
    let mut unsaved = unsaved();
    let key = stage(&stored.base_url);
    match unsaved.get(&key) {
        Some(kept) if kept.retired == stored.refresh_token => kept.pair.clone(),
        Some(kept) if kept.pair.refresh_token != stored.refresh_token => {
            unsaved.remove(&key);
            stored
        }
        _ => stored,
    }
}

/// The session to send first: `auth`, unless this process holds an unsaved
/// rotation for its stage, which is stored again first (under the lock) and
/// sent in place of the pair it retired.
pub(super) fn with_unsaved(auth: &StoredAuth, budget: RequestBudget<'_>) -> StoredAuth {
    if !unsaved().contains_key(&stage(&auth.base_url)) {
        return auth.clone();
    }
    match locked_reload(auth, budget) {
        Ok((_lock, current)) => current,
        Err(_) => over_unsaved(auth.clone()),
    }
}

/// A file the refresh needs (its lock, the stored pair) that could not be
/// read or written, on a full or unwritable disk: not a refused token, so the
/// request is retried once the disk clears instead of asking for sign-in.
fn local_failure(error: anyhow::Error) -> anyhow::Error {
    if error.chain().any(|cause| cause.is::<std::io::Error>()) {
        RefreshUnavailable(error).into()
    } else {
        error
    }
}

#[cfg(test)]
mod tests {
    use super::refresh_refused;
    use anyhow::Context;

    #[test]
    fn a_refused_refresh_is_told_apart_from_a_failed_one() {
        let refused: anyhow::Result<()> =
            Err(anyhow::anyhow!("HTTP 401: expired")).context("refreshing relayhistory session");
        assert!(refresh_refused(&refused.unwrap_err()));
        assert!(refresh_refused(&anyhow::anyhow!("HTTP 400: bad token")));
        assert!(!refresh_refused(&anyhow::anyhow!("HTTP 503: down")));
        assert!(!refresh_refused(&anyhow::anyhow!("connection reset")));
    }
}
