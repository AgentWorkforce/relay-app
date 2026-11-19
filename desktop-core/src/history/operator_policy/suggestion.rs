//! The sharing mode an organization suggests: shown, never applied.

use super::OperatorPolicy;
use crate::contract::account::SharingMode;
use crate::workspace_policy;

impl OperatorPolicy {
    /// The sharing mode the organization suggests for `workspace_id`: its
    /// workspace policy's default `sharing_mode`, only when Cloud answered
    /// with it since the last sign-in, nothing is enforced or withheld, the
    /// defaults do not stand in, and the key is not locked. It is only ever
    /// shown; nothing applies it.
    pub fn suggested_sharing(&self, workspace_id: &str) -> Option<SharingMode> {
        let workspace = Some(workspace_id);
        if self.enforced_sharing(workspace).is_some()
            || self.withholds(workspace)
            || self.defaulted(workspace_id)
        {
            return None;
        }
        // A default whose key is also locked was dropped when the document
        // was read, so a locked mode never shows up here.
        let policy = self.workspace.read_since_sign_in(workspace_id)?;
        let suggested = policy.default(workspace_policy::Key::SharingMode)?;
        serde_json::from_value(suggested.clone()).ok()
    }
}
