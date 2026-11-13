//! One Copilot choice configures both VS Code harnesses and native wake hooks.
//! The GitHub sign-in stays with Copilot; Relay supplies its revocable grant.
use super::*;
use agent_relay_node::relay::copilot::{CopilotConfig, CopilotSettings};
use providers::{HostedConfig, Inspection, Provider, ProviderFile};
mod hooks;
mod observer;

impl Installer {
    pub(crate) fn copilot_epoch(&self) -> u64 {
        self.copilot_epoch.load(std::sync::atomic::Ordering::SeqCst)
    }
    pub(super) fn revoke_copilot_intents(&self) {
        self.copilot_epoch
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    /// The selected provider set is part of a pending call's authority; an
    /// unchanged set seen again by a background refresh is not a change.
    pub(super) fn note_copilot_selection(&self, selected: &[String]) {
        let mut selected = selected.to_vec();
        selected.sort();
        selected.dedup();
        let mut last = self
            .copilot_selection
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.as_ref() != Some(&selected) {
            self.revoke_copilot_intents();
            *last = Some(selected);
        }
    }
    /// Exact bytes of every file a Copilot write edits. Unreadable is never
    /// equal to anything, so it always counts as a change.
    fn copilot_snapshot(&self) -> Result<Vec<Option<Vec<u8>>>, ()> {
        let mut paths: Vec<PathBuf> = self.copilot_files().into_iter().map(|f| f.path).collect();
        if cfg!(target_os = "macos") {
            paths.push(self.copilot_config().hooks_file());
            paths.push(self.copilot_observer_path());
        }
        paths
            .iter()
            .map(|path| match fs::read(path) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(_) => Err(()),
            })
            .collect()
    }
    /// The running core supplies its own executable; an unrelated global CLI
    /// may be older and is never replaced merely to make a native hook work.
    pub(crate) fn with_copilot_executable(mut self, executable: Option<PathBuf>) -> Self {
        self.helper.set_copilot_executable(executable);
        self
    }

    pub(super) fn copilot_configuration_connected(&self, id: &str, configured: bool) -> bool {
        id == "copilot" && configured && !cfg!(target_os = "macos")
    }

    pub(super) fn copilot_messaging_unavailable(&self, id: &str) -> bool {
        id == "copilot" && cfg!(target_os = "macos") && !self.copilot_attestation()
    }

    pub fn set_copilot_attestation_source(&self, verified: super::claude_hook::Verified) {
        *self
            .copilot_attestation
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(verified);
    }
    pub fn copilot_attestation(&self) -> bool {
        self.copilot_attestation
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|verified| verified())
    }

    pub(super) fn copilot_files(&self) -> Vec<ProviderFile> {
        vec![
            ProviderFile::new(
                Provider::CopilotHarness,
                self.native.locations.copilot_harness.clone(),
            )
            .with_copilot_helper(self.helper.copilot_executable()),
            ProviderFile::new(Provider::Copilot, self.native.locations.copilot.clone())
                .with_copilot_helper(self.helper.copilot_executable()),
        ]
    }
    pub(crate) fn copilot_development(&self) -> bool {
        self.helper.development_scope()
    }
    pub(crate) fn copilot_native_config(&self) -> CopilotConfig {
        let config = self
            .copilot_native
            .get_or_init(|| self.build_copilot_config())
            .clone();
        config.set_helper(self.helper.copilot_executable());
        config
    }
    fn copilot_config(&self) -> CopilotConfig {
        self.copilot_native_config()
    }
    fn build_copilot_config(&self) -> CopilotConfig {
        let data = self
            .native
            .locations
            .copilot_harness
            .parent()
            .unwrap_or(&self.home)
            .to_path_buf();
        CopilotConfig::from_env(
            &|key| (key == "COPILOT_HOME").then(|| data.to_string_lossy().into_owned()),
            &self.home,
        )
        .development(self.helper.development_scope())
    }
    fn copilot_settings(&self) -> CopilotSettings {
        CopilotSettings::new(&self.copilot_config(), self.helper.copilot_executable())
    }
    fn copilot_observer_path(&self) -> PathBuf {
        self.copilot_config()
            .hooks_file()
            .with_file_name("agent-relay-desktop-mcp.json")
    }
    fn copilot_observer(&self) -> observer::Settings {
        observer::Settings::new(
            self.copilot_observer_path(),
            self.helper.copilot_executable(),
            self.helper.development_scope(),
        )
    }
    pub(super) fn copilot_hooks_ready(&self) -> bool {
        !cfg!(target_os = "macos")
            || (self.helper.copilot_usable()
                && self.copilot_settings().enabled()
                && self.copilot_observer().enabled())
    }
    pub(super) fn set_copilot_hooks(&self, enabled: bool) -> Result<(), String> {
        if !cfg!(target_os = "macos") {
            return Ok(());
        }
        let directory = self
            .native
            .locations
            .copilot_harness
            .parent()
            .ok_or("Copilot configuration directory is unavailable")?;
        if self.helper.development_scope()
            && !development::safe_directory(&self.home, &directory.join("hooks"))
        {
            return Err(
                "Development Copilot hooks are outside the private provider profile".into(),
            );
        }
        if enabled && (!self.helper.ensure() || !self.helper.copilot_usable()) {
            return Err("Copilot hook setup is waiting for Agent Relay’s command-line helper. Repair this provider after it is installed.".into());
        }
        let result = self.copilot_settings().set_enabled(enabled);
        if enabled {
            return result;
        }
        let observer = self.copilot_observer().set_enabled(false);
        match (result, observer) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(a), Err(b)) => Err(format!("{a}; {b}")),
        }
    }
    pub(super) fn copilot_proof(
        &self,
        hosted: Option<&HostedConfig>,
    ) -> Result<Inspection, String> {
        self.validate_copilot_scope()?;
        let mut proof = Inspection {
            present: false,
            configured: true,
        };
        for file in self.copilot_files() {
            let current = file.inspect(hosted)?;
            proof.present |= current.present;
            proof.configured &= current.configured;
        }
        proof.configured &= self.copilot_hooks_ready();
        Ok(proof)
    }
    pub(super) fn write_copilot(&self, hosted: &HostedConfig) -> Result<(), String> {
        self.write_copilot_with(hosted, |file| file.install(hosted))
    }
    fn write_copilot_with(
        &self,
        hosted: &HostedConfig,
        write: impl Fn(&ProviderFile) -> Result<(), String>,
    ) -> Result<(), String> {
        self.write_copilot_with_hooks(hosted, write, || self.set_copilot_hooks(true))
    }
    /// Every rediscover tick rewrites the selected providers. Pending calls are
    /// revoked only when the authority they were issued under changes: before
    /// a write that is not already in place, and after any write that changed
    /// a managed byte or failed. Binding, credential and selection changes
    /// revoke where they are recorded.
    fn write_copilot_with_hooks(
        &self,
        hosted: &HostedConfig,
        write: impl Fn(&ProviderFile) -> Result<(), String>,
        install_hooks: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        self.validate_copilot_scope()?;
        let before = self.copilot_snapshot();
        if !self
            .copilot_proof(Some(hosted))
            .is_ok_and(|proof| proof.configured)
        {
            self.revoke_copilot_intents();
        }
        let result = self.write_copilot_transaction(hosted, write, install_hooks);
        if result.is_err() || before.is_err() || self.copilot_snapshot() != before {
            self.revoke_copilot_intents();
        }
        result
    }
    fn write_copilot_transaction(
        &self,
        hosted: &HostedConfig,
        write: impl Fn(&ProviderFile) -> Result<(), String>,
        install_hooks: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let hooks = cfg!(target_os = "macos")
            .then(|| {
                hooks::Backup::capture(self.copilot_config().hooks_file(), self.copilot_settings())
            })
            .transpose()?;
        let observer = cfg!(target_os = "macos").then(|| self.copilot_observer());
        let observer_backup = observer
            .as_ref()
            .map(observer::Settings::snapshot)
            .transpose()?;
        let files = self.copilot_files();
        let backups = files
            .iter()
            .map(|file| file.copilot_backup(hosted))
            .collect::<Result<Vec<_>, _>>()?;
        let mut attempted = 0;
        let mut hooks_attempted = false;
        let result = (|| {
            for file in &files {
                attempted += 1;
                write(file)?;
            }
            hooks_attempted = true;
            install_hooks()?;
            if let Some(observer) = &observer {
                observer.set_enabled(true)?;
            }
            Ok::<(), String>(())
        })();
        if let Err(error) = result {
            // An install error can follow a successful write and failed final
            // inspection. Roll back every attempted file, not only prior Ok's.
            let mut errors = vec![error];
            for backup in backups[..attempted].iter().rev() {
                if let Err(error) = backup.restore(hosted) {
                    errors.push(error);
                }
            }
            if let Some(hooks) = hooks.filter(|_| hooks_attempted) {
                if let Err(error) = hooks.restore() {
                    errors.push(error);
                }
            }
            if let Some(observer) = observer_backup {
                if let Err(error) = observer.restore() {
                    errors.push(error);
                }
            }
            return Err(errors.join("; "));
        }
        Ok(())
    }
    pub(super) fn remove_copilot(&self, endpoint: Option<&str>) -> Result<(), String> {
        self.validate_copilot_scope()?;
        self.revoke_copilot_intents();
        let mut errors = Vec::new();
        if let Err(error) = self.set_copilot_hooks(false) {
            errors.push(error);
        }
        // One malformed/inaccessible file must not retain the grant in its
        // cleanable sibling. Every writer still enforces its own ownership.
        for file in self.copilot_files() {
            if let Err(error) = file.remove(endpoint) {
                errors.push(error);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }
    fn validate_copilot_scope(&self) -> Result<(), String> {
        if self.helper.development_scope()
            && self.copilot_files().iter().any(|f| {
                f.path
                    .parent()
                    .is_none_or(|p| !development::safe_directory(&self.home, p))
            })
        {
            return Err("Development Copilot configuration is outside its private profile".into());
        }
        Ok(())
    }
    pub(super) fn copilot_connection_detail(
        &self,
        id: &str,
        state: &str,
        detail: String,
    ) -> String {
        if id != "copilot" {
            return detail;
        }
        let guidance = if cfg!(target_os = "macos") {
            "Start a new VS Code chat with the Copilot harness and approve the Agent Relay MCP connection if asked. Incoming messages can resume the chat while its stop hook waits; VS Code and the chat must remain running. History uploads follow your sharing settings automatically. The legacy Local harness cannot verify its Relay identity and has no wake or history support."
        } else {
            "Copilot MCP configuration and native Copilot harness history are available. Verified messaging and waking local VS Code Copilot chats are currently unavailable on this platform. Start a new chat to load the MCP connection. The legacy Local harness cannot verify its Relay identity and has no history support."
        };
        if matches!(state, "configured" | "installed" | "action_required") {
            format!("{detail} {guidance}")
        } else {
            detail
        }
    }
}

#[cfg(test)]
mod tests;
