//! Exact in-memory rollback of the provider-owned hook file. The public core
//! remains the owner of its schema and verifies the complete installed value.
use super::*;

pub(super) struct Backup {
    path: PathBuf,
    original: Option<String>,
    settings: CopilotSettings,
}
impl Backup {
    pub(super) fn capture(path: PathBuf, settings: CopilotSettings) -> Result<Self, String> {
        let original = read(&path)?;
        Ok(Self {
            path,
            original,
            settings,
        })
    }
    pub(super) fn restore(self) -> Result<(), String> {
        let current = read(&self.path)?;
        if current == self.original {
            return Ok(());
        }
        // enabled() checks equality with the core's ENTIRE managed JSON value,
        // not just an owner marker or hooks subset. Foreign additions fail it.
        if !self.settings.enabled() {
            return Err("Copilot hooks changed during setup; rollback left them unchanged".into());
        }
        if let Some(original) = self.original {
            files::update(&self.path, |observed| {
                (Some(observed) == current.as_deref()).then_some(original.clone())
            })
            .map_err(|_| "Cannot restore previous Copilot hooks")?;
            if read(&self.path)?.as_deref() != Some(&original) {
                return Err("Copilot hooks changed during rollback; repair is required".into());
            }
        } else {
            if read(&self.path)? != current {
                return Err("Copilot hooks changed during rollback; repair is required".into());
            }
            // Reuse the core's ownership check before removing an entry that
            // was absent before setup. Other hook files are never touched.
            self.settings.set_enabled(false)?;
            if read(&self.path)?.is_some() {
                return Err("Cannot remove failed Copilot hook setup".into());
            }
        }
        Ok(())
    }
}
fn read(path: &Path) -> Result<Option<String>, String> {
    providers::safe_parent(path)?;
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Cannot inspect Copilot hook configuration".into()),
    };
    if !metadata.is_file()
        || !private_fs::owned_by_current_user(&metadata)
        || !private_fs::private_to_owner(&metadata, 0o022)
    {
        return Err(
            "Copilot hooks must be an owned regular file without shared write access".into(),
        );
    }
    fs::read_to_string(path)
        .map(Some)
        .map_err(|_| "Cannot read Copilot hook configuration".into())
}
