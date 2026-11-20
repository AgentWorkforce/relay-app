//! Desktop's side of moving an organization to its own control plane.
//!
//! The move is done by the server: for a moved organization, a refresh
//! of the kept session (`session_refresh`, the same refresh path as ever)
//! answers with a pair the control plane issued. Desktop stores that pair
//! like any other rotation, with or without this module, so a Desktop that
//! predates it keeps working.
//!
//! With `AGENT_RELAY_DESKTOP_SESSION_BRIDGE` on (it is off by default),
//! Desktop also checks the move took the binding with it:
//!
//! - it notices a rotation that crossed from a Cloud pair to a control
//!   plane pair (the new access token has the control plane's prefix and
//!   the old one did not);
//! - it then reads the account workspace's resolve once with the new
//!   bearer. The Relaycast workspace is meant to stay where it was, under
//!   the same key and with no gate, so the answer must match the stored
//!   binding. A mismatch is reported, never adopted: the binding is left
//!   as it is;
//! - for the rest of that sign-in, refused-key recovery does not adopt a
//!   resolve that moves the Relaycast origin or names a gate.
//!
//! The person's identity (user and workspace) is already compared with the
//! kept account by the auth health check (`health`), so it is not repeated
//! here.
//!
//! Off, nothing here runs: rotations are not inspected, no resolve is
//! read and recovery adopts what it always adopted.

use super::Account;
use agent_relay_node::relay::state::Level;
use agent_relay_node::relay::store::{CloudSession, CloudWorkspace};
use agent_relay_node::relay::workspace::{validated_base_url, Secret};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// The setting that turns the check on. Off unless it says so.
pub const FLAG: &str = "AGENT_RELAY_DESKTOP_SESSION_BRIDGE";

/// How control plane access tokens begin; Cloud's do not.
const CONTROL_ACCESS_PREFIX: &str = "sh_at_";

/// How many times an unanswered binding check is tried again.
const CHECK_ATTEMPTS: u32 = 3;

/// The bridge's state on one account.
#[derive(Debug, Default)]
pub(crate) struct Bridge {
    enabled: bool,
    /// A crossing was seen and its binding check has not run yet.
    pending: AtomicBool,
    /// Binding checks left for the pending crossing.
    attempts_left: AtomicU32,
    /// The sign-out epoch a crossing was seen in, plus one; zero for none.
    crossed_in: AtomicU64,
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

/// Whether recovery after a crossing may adopt `after`: a new key is what
/// recovery is for, but not a new origin or a gate.
pub(crate) fn recovery_allowed(
    before: &CloudWorkspace,
    after: &CloudWorkspace,
) -> Result<(), Mismatch> {
    if after.gate_url.is_some() && before.gate_url.is_none() {
        return Err(Mismatch::Gate);
    }
    if !same_origin(&before.base_url, &after.base_url) {
        return Err(Mismatch::Origin);
    }
    Ok(())
}

fn same_origin(a: &str, b: &str) -> bool {
    match (validated_base_url(a), validated_base_url(b)) {
        (Some(a), Some(b)) => a == b,
        _ => a == b,
    }
}

impl Account {
    /// After a rotation `old` -> `new` was kept: notes a crossing to the
    /// control plane and wakes supervision to check the binding. Nothing
    /// when the bridge is off.
    pub(super) fn note_rotation(&self, old: &CloudSession, new: &CloudSession) {
        if !self.session_bridge.is_enabled() || !crossed_to_control(old, new) {
            return;
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        self.session_bridge
            .crossed_in
            .store(epoch.wrapping_add(1), Ordering::SeqCst);
        self.session_bridge
            .attempts_left
            .store(CHECK_ATTEMPTS, Ordering::SeqCst);
        self.session_bridge.pending.store(true, Ordering::SeqCst);
        if let Some(core) = self.core() {
            core.hub.activity(
                Level::Info,
                "Your organization moved to its new Agent Relay control plane; this computer is now signed in there.".to_string(),
            );
        }
        self.wake.notify_one();
    }

    /// Whether a crossing was seen since the last sign-out.
    pub(super) fn crossed_this_sign_in(&self) -> bool {
        self.session_bridge.is_enabled()
            && self.session_bridge.crossed_in.load(Ordering::SeqCst)
                == self.epoch.load(Ordering::SeqCst).wrapping_add(1)
    }

    /// Supervision's part: one read-only resolve after a crossing, compared
    /// with the stored binding. Reports a mismatch; changes nothing.
    pub(super) async fn check_bridged_binding(&self) {
        if !self.session_bridge.is_enabled()
            || !self.session_bridge.pending.swap(false, Ordering::SeqCst)
        {
            return;
        }
        if !self.crossed_this_sign_in() {
            return;
        }
        let Some(core) = self.core() else { return };
        let Some(before) = core.store.read(|store| store.cloud.clone()) else {
            return;
        };
        let Some(account_workspace) = before.account_workspace_id.clone() else {
            return;
        };
        let Some(session) = self.load_session().filter(|s| s.site_url == self.site_url) else {
            return;
        };
        if !is_control_issued(&session.access_token) {
            return;
        }
        match crate::cloud_api::resolve_workspace(&session.site_url, &session.access_token, &account_workspace).await {
            Ok(after) => {
                if let Err(mismatch) = binding_kept(&before, &after) {
                    core.hub.activity(Level::Warning, mismatch.to_string());
                }
            }
            Err(crate::cloud_api::ResolveError::Cloud(
                agent_relay_node::relay::cloud_session::CloudError::Unreachable,
            )) => {
                // Try again on a later tick, a few times at most.
                let left = self.session_bridge.attempts_left.load(Ordering::SeqCst);
                if left > 1 {
                    self.session_bridge.attempts_left.store(left - 1, Ordering::SeqCst);
                    self.session_bridge.pending.store(true, Ordering::SeqCst);
                }
            }
            Err(error) => core.hub.activity(Level::Warning, format!(
                "Your organization moved to its new Agent Relay control plane, but its workspace could not be checked: {error}"
            )),
        }
    }

    /// Recovery's guard after a crossing: `Err` (already reported) when
    /// `after` would move the Relaycast origin or make the install
    /// gate-only.
    pub(super) fn guard_bridged_recovery(
        &self,
        before: &CloudWorkspace,
        after: &CloudWorkspace,
    ) -> Result<(), Mismatch> {
        if !self.crossed_this_sign_in() {
            return Ok(());
        }
        recovery_allowed(before, after).inspect_err(|mismatch| {
            if let Some(core) = self.core() {
                core.hub.activity(Level::Warning, mismatch.to_string());
            }
        })
    }
}

#[cfg(test)]
mod tests;
