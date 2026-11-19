//! How `sharing status` lists one install.
use super::*;

/// One install's sharing, from its configuration alone.
pub(super) fn sharing_entry(directory: &Path, config: &Config) -> Value {
    json!({
        "directory": directory,
        "site_url": config.site_url,
        "account_id": config.account_id,
        "workspace_id": config.workspace_id,
        "sharing_mode": mode(config),
        "sharing_unchosen": config.sharing_unchosen,
        "change_pending": directory.join("sharing-change.json").exists(),
        "upgrade_required": false,
    })
}
/// An install a newer build set up, in a configuration format this build
/// does not read: listed by identity, its sharing unknown here.
pub(super) fn newer_sharing_entry(directory: &Path) -> Option<Value> {
    let install = newer_install(directory)?;
    Some(json!({
        "directory": directory,
        "site_url": install["site_url"],
        "account_id": install["account_id"],
        "workspace_id": install["workspace_id"],
        "sharing_mode": null,
        "change_pending": directory.join("sharing-change.json").exists(),
        "upgrade_required": true,
    }))
}
