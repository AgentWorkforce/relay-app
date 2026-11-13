//! Late-worker releases a sign-out left pending
//! (`docs/service-callers.md` section 3, in-flight calls).
//!
//! A sign-out deletes the whole `ServiceCallers` tree, so a worker an
//! invoke started across it cannot be kept in a registry. It is kept here
//! instead, in `<app data>/ServiceCallerReleases.json` beside the tree,
//! 0600, until its one terminal line: a cold start, signed in or not,
//! still releases it with the idempotent `service-caller-late-<invocation>`
//! key. Each entry carries the workspace key it was admitted under, and
//! only while its release is pending; the file is deleted once empty.

use super::registry::{self, Binding};
use agent_relay_node::relay::workspace::{Secret, Workspace};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub(crate) const FILE: &str = "ServiceCallerReleases.json";
const SCHEMA: u32 = 1;

#[derive(Serialize, Deserialize)]
struct ReleasesFile {
    schema: u32,
    releases: Vec<Release>,
}

/// One pending release. `Debug` is never derived: it holds a key.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Release {
    pub(crate) invocation_id: String,
    pub(crate) caller_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent_id: Option<String>,
    pub(crate) since_ms: i64,
    site_url: String,
    workspace_id: String,
    user_id: String,
    base_url: String,
    /// The Control origin brokering a node-scoped `workspace_key`; absent
    /// for a workspace key and in files written before it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    control_url: Option<String>,
    relaycast_workspace_id: String,
    workspace_key: Secret,
}

impl Release {
    pub(crate) fn new(
        binding: &Binding,
        invocation_id: &str,
        caller_id: &str,
        name: Option<String>,
        agent_id: Option<String>,
        since_ms: i64,
    ) -> Release {
        Release {
            invocation_id: invocation_id.to_string(),
            caller_id: caller_id.to_string(),
            name,
            agent_id,
            since_ms,
            site_url: binding.site_url.clone(),
            workspace_id: binding.workspace_id.clone(),
            user_id: binding.user_id.clone(),
            base_url: binding.workspace.base_url.clone(),
            control_url: binding.workspace.control_url.clone(),
            relaycast_workspace_id: binding.relaycast_workspace_id.clone(),
            workspace_key: binding.workspace.key.clone(),
        }
    }

    /// The binding the release was admitted under, rebuilt in `app_data`.
    pub(crate) fn binding(&self, app_data: &Path) -> Binding {
        Binding {
            key: registry::key(&self.site_url, &self.workspace_id),
            site_url: self.site_url.clone(),
            workspace_id: self.workspace_id.clone(),
            user_id: self.user_id.clone(),
            workspace: Workspace {
                key: self.workspace_key.clone(),
                base_url: self.base_url.clone(),
                workspace_id: Some(self.relaycast_workspace_id.clone()),
                source: Default::default(),
                control_url: self.control_url.clone(),
            },
            relaycast_workspace_id: self.relaycast_workspace_id.clone(),
            folder: app_data.join(registry::FOLDER),
        }
    }
}

/// The releases file of the profile `binding` lives in.
pub(crate) fn path_of(binding: &Binding) -> Option<PathBuf> {
    Some(binding.folder.parent()?.join(FILE))
}

/// Every kept release. An absent, unreadable or unknown file reads as
/// none; nothing is shown.
pub(crate) fn load(path: &Path) -> Vec<Release> {
    registry::read_private(path)
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<ReleasesFile>(&bytes).ok())
        .filter(|file| file.schema == SCHEMA)
        .map(|file| file.releases)
        .unwrap_or_default()
}

fn save(path: &Path, releases: Vec<Release>) {
    if releases.is_empty() {
        let _ = std::fs::remove_file(path);
        return;
    }
    let _ = registry::write(
        path,
        &ReleasesFile {
            schema: SCHEMA,
            releases,
        },
    );
}

/// Keeps each of `added` not already kept. Under the files lock.
pub(crate) fn keep(path: &Path, added: impl IntoIterator<Item = Release>) {
    let mut releases = load(path);
    let before = releases.len();
    for release in added {
        if !releases
            .iter()
            .any(|kept| kept.invocation_id == release.invocation_id)
        {
            releases.push(release);
        }
    }
    if releases.len() != before {
        save(path, releases);
    }
}

/// Drops `invocation_id`: its terminal line is written. Under the files
/// lock.
pub(crate) fn forget(path: &Path, invocation_id: &str) {
    if std::fs::symlink_metadata(path).is_err() {
        return;
    }
    let mut releases = load(path);
    let before = releases.len();
    releases.retain(|kept| kept.invocation_id != invocation_id);
    if releases.len() != before {
        save(path, releases);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A release admitted under a node-scoped key keeps its Control
    /// origin across the file, so the late release still goes to the broker.
    #[test]
    fn a_kept_release_rebuilds_its_control_origin() {
        let app_data = Path::new("/app");
        let binding = Binding {
            key: "k".into(),
            site_url: "https://agentrelay.example".into(),
            workspace_id: "ws_1".into(),
            user_id: "user_1".into(),
            workspace: Workspace {
                key: Secret::new("sh_wk_fixture0123"),
                base_url: "https://cast.example".into(),
                workspace_id: Some("rw_1".into()),
                source: Default::default(),
                control_url: Some("https://control.example".into()),
            },
            relaycast_workspace_id: "rw_1".into(),
            folder: app_data.join(registry::FOLDER),
        };
        let release = Release::new(&binding, "inv_1", "caller_1", None, None, 0);
        let json = serde_json::to_string(&release).unwrap();
        let kept: Release = serde_json::from_str(&json).unwrap();
        assert_eq!(
            kept.binding(app_data).workspace.control_url.as_deref(),
            Some("https://control.example")
        );

        let mut legacy: serde_json::Value = serde_json::from_str(&json).unwrap();
        legacy.as_object_mut().unwrap().remove("control_url");
        let kept: Release = serde_json::from_value(legacy).unwrap();
        assert_eq!(kept.binding(app_data).workspace.control_url, None);
    }

    /// `control_url` is written only when present, and a file written
    /// before the field existed reads back as absent.
    #[test]
    fn an_absent_control_origin_is_not_written_and_reads_back_absent() {
        let app_data = Path::new("/app");
        let release_with = |control_url: Option<&str>| {
            let binding = Binding {
                key: "k".into(),
                site_url: "https://agentrelay.example".into(),
                workspace_id: "ws_1".into(),
                user_id: "user_1".into(),
                workspace: Workspace {
                    key: Secret::new("sh_wk_fixture0123"),
                    base_url: "https://cast.example".into(),
                    workspace_id: Some("rw_1".into()),
                    source: Default::default(),
                    control_url: control_url.map(str::to_string),
                },
                relaycast_workspace_id: "rw_1".into(),
                folder: app_data.join(registry::FOLDER),
            };
            Release::new(&binding, "inv_1", "caller_1", None, None, 0)
        };

        let without = serde_json::to_value(release_with(None)).unwrap();
        assert!(!without.as_object().unwrap().contains_key("control_url"));

        let with = serde_json::to_value(release_with(Some("https://control.example"))).unwrap();
        assert_eq!(with["control_url"], "https://control.example");
        let kept: Release = serde_json::from_value(with).unwrap();
        assert_eq!(kept.control_url.as_deref(), Some("https://control.example"));

        // A document from before the field existed (no key at all).
        let kept: Release = serde_json::from_value(without).unwrap();
        assert_eq!(kept.control_url, None);
    }
}
