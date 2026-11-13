//! The teammate-context SessionStart hook ([`crate::teammate_context`]) in
//! each shared Claude login's `settings.json`, written with the hosted MCP
//! entry while Claude Code is chosen and the setting is on.
//!
//! The person's choice is kept beside the provider choices
//! (`teammate-context-hook.json`): no file means no choice yet, which the
//! integration-default pass turns on once. Off takes the hook out of every
//! watched login; On puts it in every shared one, now and on each refresh.
use super::claude_hook::Kind;
use super::*;

const KIND: Kind = Kind::TeammateContext;

#[derive(serde::Deserialize, serde::Serialize)]
struct Choice {
    on: bool,
}

impl Installer {
    fn teammate_choice_path(&self) -> PathBuf {
        self.choices.with_file_name("teammate-context-hook.json")
    }

    /// The person's choice, or `None` before any.
    fn teammate_choice(&self) -> Option<bool> {
        let bytes = fs::read(self.teammate_choice_path()).ok()?;
        serde_json::from_slice::<Choice>(&bytes)
            .ok()
            .map(|choice| choice.on)
    }

    /// The setting as Settings shows it: on once chosen or defaulted on.
    pub fn teammate_context_enabled(&self) -> bool {
        self.teammate_choice() == Some(true)
    }

    /// Claude Code is on this computer or chosen, so the default may turn
    /// the hook on.
    pub fn teammate_context_available(&self) -> bool {
        self.claude_chosen() || self.detected("claude")
    }

    fn claude_chosen(&self) -> bool {
        self.selection()
            .ok()
            .flatten()
            .is_some_and(|ids| ids.iter().any(|id| id == "claude"))
    }

    /// The hook command this installation writes.
    pub(super) fn teammate_hook_command(&self) -> Option<String> {
        self.helper.teammate_context_hook()
    }

    fn writing(&self) -> std::sync::MutexGuard<'_, ()> {
        self.writing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// With `writing` held: saves the choice and reads it back.
    fn save_teammate_choice_locked(&self, on: bool) -> Result<(), String> {
        let text = serde_json::to_string(&Choice { on })
            .map_err(|_| "The teammate context choice could not be encoded.".to_string())?;
        files::update(&self.teammate_choice_path(), |_| Some(text.clone()))
            .map_err(|_| "The teammate context choice could not be saved.".to_string())?;
        if self.teammate_choice() != Some(on) {
            return Err(
                "The teammate context choice could not be saved. Check file permissions.".into(),
            );
        }
        Ok(())
    }

    /// Saves the choice only. The provider refresh that follows a chooser's
    /// save adds or removes the hook, and reports a failure there as a setup
    /// warning.
    pub fn save_teammate_context(&self, on: bool) -> Result<(), String> {
        let _writing = self.writing();
        self.save_teammate_choice_locked(on)
    }

    /// Saves the choice, then adds or removes the hook. A choice that cannot
    /// be saved changes no login.
    pub fn set_teammate_context(&self, on: bool) -> Result<(), String> {
        let _writing = self.writing();
        self.save_teammate_choice_locked(on)?;
        self.apply_teammate_hooks()
    }

    /// The integration default: on, unless the person already chose. The
    /// check, the save and the hook change happen under one hold of `writing`,
    /// so an Off saved meanwhile is never overwritten.
    pub fn default_teammate_context(&self) -> Result<(), String> {
        let _writing = self.writing();
        if self.teammate_choice().is_some() {
            return Ok(());
        }
        self.save_teammate_choice_locked(true)?;
        self.apply_teammate_hooks()
    }

    /// Puts the hook back where it is missing while it is on; leaves every
    /// file alone otherwise.
    pub fn reclaim_teammate_context(&self) {
        if !self.teammate_context_enabled() {
            return;
        }
        let _writing = self.writing();
        let _ = self.apply_teammate_hooks();
    }

    /// With `writing` held: the hook in every shared login while it is on
    /// and Claude Code is chosen, and in none otherwise.
    pub(super) fn apply_teammate_hooks(&self) -> Result<(), String> {
        if self.validate_scope().is_err() {
            return Ok(());
        }
        if self.teammate_context_enabled() && self.claude_chosen() {
            let Some(command) = self.teammate_hook_command() else {
                return Err(
                    "The teammate context hook needs Agent Relay's command-line helper.".into(),
                );
            };
            let added = self
                .claude_settings_files(true)
                .iter()
                .fold(true, |all, path| {
                    self.edit_claude_hook_kind(path, Some(&command), KIND) && all
                });
            if !added {
                return Err("The teammate context hook could not be added to Claude Code's settings.json. Check that it is a JSON object you can write.".into());
            }
            Ok(())
        } else {
            self.remove_teammate_hooks()
        }
    }

    /// Takes the hook out of every watched login.
    pub(super) fn remove_teammate_hooks(&self) -> Result<(), String> {
        let removed = self
            .claude_settings_files(false)
            .iter()
            .fold(true, |all, path| {
                self.edit_claude_hook_kind(path, None, KIND) && all
            });
        if removed {
            Ok(())
        } else {
            Err(
                "The teammate context hook could not be removed from Claude Code's settings.json."
                    .into(),
            )
        }
    }

    #[cfg(test)]
    pub(super) fn teammate_hook_present(&self) -> bool {
        self.claude_hook_kind_present(KIND)
    }
}
