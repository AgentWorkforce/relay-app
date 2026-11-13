//! Cleanup of the retired teammate-context SessionStart hook
//! ([`crate::teammate_context`]). Nothing writes it any more: every refresh,
//! whether or not providers are installed then, and the core's reclaim pass
//! take it out of every watched Claude login and delete the saved choice
//! (`teammate-context-hook.json`).
use super::claude_hook::Kind;
use super::*;

const KIND: Kind = Kind::TeammateContext;

impl Installer {
    fn teammate_choice_path(&self) -> PathBuf {
        self.choices.with_file_name("teammate-context-hook.json")
    }

    /// Takes the hook out of every watched login, and deletes the old choice
    /// file once no login holds the hook. A login that cannot be written is
    /// tried again on the next refresh or reclaim.
    pub(super) fn remove_teammate_hooks(&self) -> Result<(), String> {
        if self.validate_scope().is_err() {
            return Ok(());
        }
        let removed = self
            .claude_settings_files(false)
            .iter()
            .fold(true, |all, path| {
                self.edit_claude_hook_kind(path, None, KIND) && all
            });
        if !removed {
            return Err(
                "The retired teammate context hook could not be removed from Claude Code's settings.json."
                    .into(),
            );
        }
        match fs::remove_file(self.teammate_choice_path()) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(
                "The retired teammate context choice could not be deleted.".into(),
            ),
            _ => Ok(()),
        }
    }

    /// [`Self::remove_teammate_hooks`] for the core's reclaim pass: holds
    /// `writing`, and keeps a failure to itself.
    pub fn remove_legacy_teammate_context(&self) {
        let _writing = self
            .writing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = self.remove_teammate_hooks();
    }

    #[cfg(test)]
    pub(super) fn teammate_hook_present(&self) -> bool {
        self.claude_hook_kind_present(KIND)
    }

    #[cfg(test)]
    pub(super) fn teammate_choice_saved(&self) -> bool {
        self.teammate_choice_path().exists()
    }
}
