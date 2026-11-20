//! The setting, kept in this build's data folder.

use super::*;

#[derive(Default, Deserialize, Serialize)]
pub(super) struct Saved {
    #[serde(default)]
    pub(super) enabled: bool,
    #[serde(default)]
    pub(super) notifications_enabled: bool,
}

/// The saved setting: off when nothing is saved or it cannot be read.
pub fn load(app_data_dir: &Path) -> bool {
    load_saved(app_data_dir).enabled
}

pub(super) fn load_saved(app_data_dir: &Path) -> Saved {
    std::fs::read(app_data_dir.join(FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Saved>(&bytes).ok())
        .unwrap_or_default()
}

#[cfg(test)]
pub(super) fn save(app_data_dir: &Path, enabled: bool) -> Result<(), String> {
    save_choices(
        app_data_dir,
        enabled,
        load_saved(app_data_dir).notifications_enabled,
    )
}

/// Persist the complete live choice. Reading the other preference back
/// from a missing/corrupt file would silently reset it after recovery.
pub(super) fn save_choices(
    app_data_dir: &Path,
    enabled: bool,
    notifications_enabled: bool,
) -> Result<(), String> {
    const FAILED: &str = "The coordination advisories setting could not be saved.";
    let saved = Saved {
        enabled,
        notifications_enabled,
    };
    let bytes = serde_json::to_vec_pretty(&saved).map_err(|_| FAILED.to_string())?;
    agent_relay_node::platform::fs::create_private_dir_all(app_data_dir)
        .map_err(|_| FAILED.to_string())?;
    agent_relay_node::platform::fs::write_atomic(&app_data_dir.join(FILE), &bytes)
        .map_err(|_| FAILED.to_string())?;
    let actual = load_saved(app_data_dir);
    if actual.enabled != enabled || actual.notifications_enabled != notifications_enabled {
        return Err(FAILED.into());
    }
    Ok(())
}
