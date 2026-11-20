//! Desktop's side of moving an organization to its own control plane.
//!
//! The move is done by the server: for a moved organization, a refresh
//! of the kept session (`session_refresh`, the same refresh path as ever)
//! answers with a pair the control plane issued. Desktop stores that pair
//! like any other rotation, with or without this module, so a Desktop that
//! predates it keeps working.
//!
//! With `AGENT_RELAY_DESKTOP_SESSION_BRIDGE` on (it is off by default),
//! Desktop also checks the move took the binding with it. Whether this
//! computer is on the control plane is read from the kept session itself
//! (its access token has the control plane's prefix), never from memory,
//! so it holds after a restart and when another process sharing the store
//! did the refresh:
//!
//! - supervision reads the account workspace's resolve once with the
//!   control plane bearer: on the first tick that finds such a session
//!   (after a start, a crossing, or a sign-in), and again after it was
//!   replaced by a Cloud session or none. The Relaycast workspace is meant
//!   to stay where it was, under the same key and with no gate, so the
//!   answer must match the stored binding. A mismatch is reported, never
//!   adopted: the binding is left as it is. A check Cloud could not answer
//!   is tried [`CHECK_ATTEMPTS`] times in all, then reported;
//! - refused-key recovery needs nothing more: it already refuses a resolve
//!   that moves the Relaycast origin or names a gate
//!   (`cloud_api::resolve_workspace_for_recovery`), on the control plane
//!   as on Cloud;
//! - the rotation that crosses over is announced once.
//!
//! The person's identity (user and workspace) is already compared with the
//! kept account by the auth health check (`health`), so it is not repeated
//! here.
//!
//! Off, nothing here runs: rotations are not inspected, no resolve is
//! read and recovery adopts what it always adopted.

use super::Account;
use agent_relay_node::relay::cloud_session::CloudError;
use agent_relay_node::relay::state::Level;
use agent_relay_node::relay::store::{CloudSession, CloudWorkspace};
use agent_relay_node::relay::workspace::{validated_base_url, Secret};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The setting that turns the check on. Off unless it says so.
pub const FLAG: &str = "AGENT_RELAY_DESKTOP_SESSION_BRIDGE";

/// How control plane access tokens begin; Cloud's do not.
const CONTROL_ACCESS_PREFIX: &str = "sh_at_";

/// How many times in all a binding check Cloud could not answer is tried.
pub(crate) const CHECK_ATTEMPTS: u32 = 3;

/// The bridge's state on one account. Only bookkeeping for the binding
/// check lives here; whether the account is on the control plane is read
/// from the store.
#[derive(Debug, Default)]
pub(crate) struct Bridge {
    enabled: bool,
    /// The last tick found a control plane session kept for this site.
    seen_control: AtomicBool,
    /// Binding check tries left for that session; zero once it is done.
    checks_left: AtomicU32,
}

impl Bridge {
    pub(crate) fn from_env(env: &BTreeMap<String, String>) -> Bridge {
        Bridge {
            enabled: env.get(FLAG).is_some_and(|value| is_on(value)),
            ..Bridge::default()
        }
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }
}

fn is_on(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "on" | "yes"
    )
}

/// Whether the control plane issued this access token.
pub(crate) fn is_control_issued(token: &Secret) -> bool {
    token.expose().starts_with(CONTROL_ACCESS_PREFIX)
}

/// Whether a rotation replaced a Cloud pair with a control plane pair.
pub(crate) fn crossed_to_control(old: &CloudSession, new: &CloudSession) -> bool {
    old.site_url == new.site_url
        && !is_control_issued(&old.access_token)
        && is_control_issued(&new.access_token)
}

/// How a resolve after a crossing differs from the stored binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Mismatch {
    /// The resolve names a relaygate URL; the install would go gate-only.
    Gate,
    /// The Relaycast origin changed.
    Origin,
    /// The workspace key changed or is gone.
    Key,
    /// The Relaycast workspace id changed.
    Workspace,
}

impl std::fmt::Display for Mismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match self {
            Mismatch::Gate => "now names a relaygate, which would make this computer gate-only",
            Mismatch::Origin => "names a different Relaycast address",
            Mismatch::Key => "shares a different Agent Relay key",
            Mismatch::Workspace => "names a different Relaycast workspace",
        };
        write!(
            f,
            "Your organization moved to its new Agent Relay control plane, but its workspace {what}. This computer keeps its current connection; tell your Agent Relay administrator."
        )
    }
}

/// Whether `after` (a resolve read after a crossing) keeps the binding
/// `before`. The first difference found, gate first, is the answer.
pub(crate) fn binding_kept(
    before: &CloudWorkspace,
    after: &CloudWorkspace,
) -> Result<(), Mismatch> {
    if after.gate_url.is_some() && before.gate_url.is_none() {
        return Err(Mismatch::Gate);
    }
    if !same_origin(&before.base_url, &after.base_url) {
        return Err(Mismatch::Origin);
    }
    if before.key != after.key {
        return Err(Mismatch::Key);
    }
    match (&before.workspace_id, &after.workspace_id) {
        (Some(a), Some(b)) if a != b => Err(Mismatch::Workspace),
        _ => Ok(()),
    }
}

fn same_origin(a: &str, b: &str) -> bool {
    match (validated_base_url(a), validated_base_url(b)) {
        (Some(a), Some(b)) => a == b,
        _ => a == b,
    }
}

impl Account {
    /// After a rotation `old` -> `new` was kept: announces a crossing to
    /// the control plane and wakes supervision, whose next tick checks the
    /// binding. Nothing when the bridge is off.
    pub(super) fn note_rotation(&self, old: &CloudSession, new: &CloudSession) {
        if !self.session_bridge.is_enabled() || !crossed_to_control(old, new) {
            return;
        }
        if let Some(core) = self.core() {
            core.hub.activity(
                Level::Info,
                "Your organization moved to its new Agent Relay control plane; this computer is now signed in there.".to_string(),
            );
        }
        self.wake.notify_one();
    }

    /// The kept session for this site, when the bridge is on and the
    /// control plane issued it.
    fn control_session(&self) -> Option<CloudSession> {
        if !self.session_bridge.is_enabled() {
            return None;
        }
        self.load_session()
            .filter(|s| s.site_url == self.site_url && is_control_issued(&s.access_token))
    }

    /// Supervision's part: one read-only resolve for a control plane
    /// session not yet checked, compared with the stored binding. Reports
    /// a mismatch; changes nothing.
    pub(crate) async fn check_bridged_binding(&self) {
        let bridge = &self.session_bridge;
        let Some(session) = self.control_session() else {
            bridge.seen_control.store(false, Ordering::SeqCst);
            return;
        };
        if !bridge.seen_control.swap(true, Ordering::SeqCst) {
            bridge.checks_left.store(CHECK_ATTEMPTS, Ordering::SeqCst);
        }
        let left = bridge.checks_left.load(Ordering::SeqCst);
        if left == 0 {
            return;
        }
        let Some(core) = self.core() else { return };
        let Some(before) = core.store.read(|store| store.cloud.clone()) else {
            return;
        };
        let Some(account_workspace) = before.account_workspace_id.clone() else {
            return;
        };
        let answer = crate::cloud_api::resolve_workspace(
            &session.site_url,
            &session.access_token,
            &account_workspace,
        )
        .await;
        let report = match answer {
            Ok(after) => binding_kept(&before, &after).err().map(|m| m.to_string()),
            Err(crate::cloud_api::ResolveError::Cloud(CloudError::Unreachable)) if left > 1 => {
                bridge.checks_left.store(left - 1, Ordering::SeqCst);
                return;
            }
            Err(error) => Some(format!(
                "Your organization moved to its new Agent Relay control plane, but its workspace could not be checked: {error}"
            )),
        };
        bridge.checks_left.store(0, Ordering::SeqCst);
        if let Some(report) = report {
            core.hub.activity(Level::Warning, report);
        }
    }
}

#[cfg(test)]
mod tests;
