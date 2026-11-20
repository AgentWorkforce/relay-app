//! The setting, kept in this build's data folder.

use super::*;

#[derive(Default, Deserialize, Serialize)]
pub(super) struct Saved {
    #[serde(default)]
    pub(super) enabled: bool,
}

/// The saved setting: off when nothing is saved or it cannot be read.
pub fn load(app_data_dir: &Path) -> bool {
    std::fs::read(app_data_dir.join(FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Saved>(&bytes).ok())
        .is_some_and(|saved| saved.enabled)
}

pub(super) fn save(app_data_dir: &Path, enabled: bool) -> Result<(), String> {
    const FAILED: &str = "The coordination advisories setting could not be saved.";
    let bytes = serde_json::to_vec_pretty(&Saved { enabled }).map_err(|_| FAILED.to_string())?;
    agent_relay_node::platform::fs::create_private_dir_all(app_data_dir)
        .map_err(|_| FAILED.to_string())?;
    agent_relay_node::platform::fs::write_atomic(&app_data_dir.join(FILE), &bytes)
        .map_err(|_| FAILED.to_string())?;
    if load(app_data_dir) != enabled {
        return Err(FAILED.into());
    }
    Ok(())
}
