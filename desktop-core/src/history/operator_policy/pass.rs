//! The operator policy a delivery pass sends under: the upload hold and the
//! revision of the same cached document, read together, so the revision a
//! batch names (`REVISION_HEADER`) is the one its eligibility was decided
//! under.

use super::{
    compiled_defaults, hold_of, read_saved, standing_with, unread, Unreadable, UploadHold, FILE,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

/// The revision each workspace's current delivery pass decided eligibility
/// under (see [`pass_policy`]), for [`REVISION_HEADER`](super::REVISION_HEADER).
static PASS_REVISIONS: Mutex<BTreeMap<String, i64>> = Mutex::new(BTreeMap::new());

/// The revision `workspace_id`'s current delivery pass decided eligibility
/// under: every upload of the pass (auth and connection retries included)
/// and its direct-upload credentials asks name it, even if a policy read
/// meanwhile cached a newer one. `None` when the pass's policy was not read
/// from Cloud (defaults, withheld or unreadable), or before any pass.
pub fn pass_revision(workspace_id: &str) -> Option<i64> {
    let revisions = PASS_REVISIONS.lock().unwrap_or_else(|e| e.into_inner());
    revisions.get(workspace_id).copied()
}

/// A delivery pass's [`UploadHold`] for `workspace_id`, from one read of the
/// cache with the revision of the same document, which is recorded as the
/// pass's [`pass_revision`]: a batch is only ever labelled with the revision
/// its eligibility (the oops window) was decided under. A newer revision is
/// named only once the next pass decides under its settings.
pub fn pass_policy(app_data_dir: &Path, workspace_id: &str) -> UploadHold {
    pass_policy_with(app_data_dir, workspace_id, compiled_defaults())
}

/// [`pass_policy`] with `defaults` in place of the build's.
pub fn pass_policy_with(
    app_data_dir: &Path,
    workspace_id: &str,
    defaults: Option<&BTreeMap<String, Value>>,
) -> UploadHold {
    let (hold, revision) = policy_read(app_data_dir, workspace_id, defaults);
    let mut revisions = PASS_REVISIONS.lock().unwrap_or_else(|e| e.into_inner());
    match revision {
        Some(revision) => revisions.insert(workspace_id.to_string(), revision),
        None => revisions.remove(workspace_id),
    };
    hold
}

/// The hold and, when the standing comes from a document read from Cloud,
/// that document's revision (zero included), from one read of the cache.
pub(super) fn policy_read(
    app_data_dir: &Path,
    workspace_id: &str,
    defaults: Option<&BTreeMap<String, Value>>,
) -> (UploadHold, Option<i64>) {
    if workspace_id.is_empty() {
        return (UploadHold::None, None);
    }
    let (standing, revision) = match read_saved(&app_data_dir.join(FILE)) {
        Ok(saved) => {
            let entry = saved.workspaces.get(workspace_id);
            let read = entry.filter(|entry| entry.fetched_at.is_some());
            (
                standing_with(entry, defaults),
                read.and_then(|entry| entry.revision),
            )
        }
        Err(Unreadable) => (unread(defaults), None),
    };
    (hold_of(standing), revision)
}
