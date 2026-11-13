//! Global provider adapters share revocable grants issued through the app login.
//! Grants are pinned to an account/workspace and never participate in Debug output.
use super::*;
use providers::{HostedConfig, Inspection, Locations, Provider, ProviderFile};
use std::{
    collections::BTreeMap,
    sync::{Mutex, PoisonError},
};
pub(crate) mod copilot;
mod discovery;
#[cfg(test)]
mod tests;

pub(super) struct Native {
    cursor_detection_home: PathBuf,
    pub locations: Locations,
    executables: discovery::Search,
    copilot: copilot::Search,
    versions: discovery::Versions,
    credentials: Mutex<Option<(Binding, BTreeMap<String, String>)>>,
    login_required: Mutex<Option<(Binding, Vec<String>)>>,
}

impl Native {
    pub(super) fn empty(home: &Path) -> Self {
        Self {
            cursor_detection_home: home.to_path_buf(),
            locations: Locations::from_env(home, &|_| None),
            executables: Default::default(),
            copilot: copilot::Search::new(home, &|_| None),
            versions: Default::default(),
            credentials: Mutex::new(None),
            login_required: Mutex::new(None),
        }
    }
    pub(super) fn new(home: &Path, env: &impl Fn(&str) -> Option<String>) -> Self {
        // Dev searches the person's installation locations while Locations
        // continues resolving every writable profile against its private home.
        let detection_home = env("AGENT_RELAY_PROVIDER_DETECTION_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.to_path_buf());
        Self {
            cursor_detection_home: detection_home.clone(),
            locations: Locations::from_env(home, env),
            executables: discovery::Search::new(&detection_home, env),
            copilot: copilot::Search::new(&detection_home, env),
            versions: Default::default(),
            credentials: Mutex::new(None),
            login_required: Mutex::new(None),
        }
    }
}

impl Installer {
    /// The app-owned bridge keeps the selected provider's child grant inside
    /// Core. It never borrows the person's general desktop MCP credential.
    pub(crate) fn copilot_dispatch_hosted(&self, binding: &Binding) -> Option<HostedConfig> {
        if !self
            .selection()
            .ok()
            .flatten()?
            .iter()
            .any(|id| id == "copilot")
        {
            return None;
        }
        let config = self.hosted("copilot", Some(binding))?;
        self.copilot_proof(Some(&config))
            .ok()?
            .configured
            .then_some(config)
    }
    pub fn with_environment(mut self, env: &impl Fn(&str) -> Option<String>) -> Self {
        self.native = Native::new(&self.home, env);
        self.copilot_native = Default::default();
        self
    }

    pub fn credential_provider(id: &str) -> bool {
        manager::CREDENTIAL_PROVIDERS.contains(&id)
            || (copilot::COPILOT_CONNECTION_READY && id == "copilot")
    }

    pub(super) fn supported_provider(id: &str) -> bool {
        matches!(id, "codex" | "claude") || Self::credential_provider(id)
    }

    pub fn provider_ids(&self) -> Vec<String> {
        self.selection()
            .ok()
            .flatten()
            .unwrap_or_default()
            .into_iter()
            .filter(|id| Self::credential_provider(id))
            .collect()
    }

    /// Expected scoped credentials, only while their saved native configuration
    /// and explicit provider selection still match this account and workspace.
    /// Callers must never print these values or use a URL read from a config file.
    pub(crate) fn provider_probe_tokens(
        &self,
        binding: &Binding,
        ids: &[String],
    ) -> BTreeMap<String, String> {
        let selected = self.selection().ok().flatten().unwrap_or_default();
        ids.iter()
            .filter_map(|id| {
                if !Self::credential_provider(id) || !selected.contains(id) {
                    return None;
                }
                let config = self.hosted(id, Some(binding))?;
                if !self
                    .native_proof(id, Some(&config.endpoint))
                    .ok()?
                    .configured
                {
                    return None;
                }
                let token = config
                    .headers
                    .get("Authorization")?
                    .strip_prefix("Bearer ")?;
                Some((id.clone(), token.to_string()))
            })
            .collect()
    }

    pub fn set_provider_credentials(
        &self,
        binding: &Binding,
        credentials: BTreeMap<String, String>,
    ) {
        let credentials: BTreeMap<_, _> = credentials
            .into_iter()
            .filter(|(id, _)| Self::credential_provider(id))
            .collect();
        let mut current = self
            .native
            .credentials
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let old = current
            .as_ref()
            .and_then(|(b, t)| t.get("copilot").map(|token| (b, token)));
        let next = credentials.get("copilot").map(|token| (binding, token));
        if old != next {
            self.revoke_copilot_intents();
        }
        *current = Some((binding.clone(), credentials));
        drop(current);
        *self
            .native
            .login_required
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
    }

    pub fn clear_provider_credentials(&self) {
        self.revoke_copilot_intents();
        *self
            .native
            .credentials
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        *self
            .native
            .login_required
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        let _ = providers::token_file::remove(&self.grok_credentials_path());
    }

    pub fn set_provider_login_required(&self, binding: &Binding, mut ids: Vec<String>) {
        ids.retain(|id| Self::credential_provider(id));
        ids.sort();
        ids.dedup();
        *self
            .native
            .login_required
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some((binding.clone(), ids));
    }

    /// A selected provider needs a new app login for this account: the
    /// account's sign-in renews it even while the login itself is healthy.
    pub fn provider_login_required(&self, binding: &Binding) -> bool {
        self.native
            .login_required
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|(pinned, ids)| pinned == binding && !ids.is_empty())
    }

    pub(super) fn native_login_required(&self, id: &str, binding: Option<&Binding>) -> bool {
        self.native
            .login_required
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|(pinned, ids)| {
                binding == Some(pinned) && ids.iter().any(|required| required == id)
            })
    }

    pub(super) fn native_binding_matches(&self, binding: Option<&Binding>) -> bool {
        self.native
            .credentials
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|(pinned, _)| binding == Some(pinned))
    }

    pub(super) fn configuration_matches_login(
        &self,
        id: &str,
        configured: bool,
        binding: Option<&Binding>,
    ) -> bool {
        configured && (!Self::credential_provider(id) || self.native_binding_matches(binding))
    }

    fn hosted(&self, id: &str, binding: Option<&Binding>) -> Option<HostedConfig> {
        let lock = self
            .native
            .credentials
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let (pinned, grants) = lock.as_ref()?;
        if binding.is_some_and(|binding| binding != pinned) {
            return None;
        }
        let mut headers = BTreeMap::from([(
            "Authorization".into(),
            format!("Bearer {}", grants.get(id)?),
        )]);
        if id == "grok" {
            headers.insert(
                "X-Agent-Relay-Native-Session-Id".into(),
                "{{session_id}}".into(),
            );
        }
        Some(HostedConfig {
            bearer_token_file: (id == "grok").then(|| self.grok_credentials_path()),
            endpoint: pinned.endpoint()?,
            headers,
            copilot_stdio: (id == "copilot" && cfg!(target_os = "macos")).then(|| {
                providers::CopilotStdio {
                    command: self.copilot_helper(),
                    args: crate::copilot_dispatch::peer::arguments(
                        pinned,
                        self.copilot_development(),
                    ),
                }
            }),
        })
    }

    pub(super) fn native_proof(
        &self,
        id: &str,
        endpoint: Option<&str>,
    ) -> Result<Inspection, String> {
        let config = self
            .hosted(id, None)
            .filter(|config| endpoint == Some(config.endpoint.as_str()));
        if id == "pi" {
            self.validate_pi_scope()?;
            return providers::pi::inspect(
                &self.native.locations.pi,
                &self.pi_credentials_path(),
                config.as_ref(),
            );
        }
        if id == "copilot" {
            return self.copilot_proof(config.as_ref());
        }
        let files = self.native_files(id)?;
        let mut proof = Inspection::default();
        for (index, file) in files.iter().enumerate() {
            let current = file.inspect(config.as_ref())?;
            proof.present |= current.present;
            if index == 0 {
                proof.configured = current.configured;
            } else if current.present {
                proof.configured = false;
            }
        }
        if id == "grok" {
            proof.configured &= config
                .as_ref()
                .is_some_and(|config| providers::token_file::ready(config).unwrap_or(false));
        }
        if id == "cursor" {
            proof.present |= !cfg!(windows) && self.cursor_hooks_ready();
            proof.configured &= self.cursor_hooks_ready();
        }
        Ok(proof)
    }

    pub(super) fn install_native(&self, binding: &Binding, selected: &[String]) -> Vec<String> {
        if !self.native_binding_matches(Some(binding)) {
            self.clear_provider_credentials();
        }
        self.note_copilot_selection(selected);
        let mut warnings = Vec::new();
        for id in selected.iter().filter(|id| Self::credential_provider(id)) {
            let result=self.hosted(id,Some(binding)).ok_or_else(||"Agent Relay could not authorize this provider. Check the connection and try again.".to_string())
                .and_then(|config|self.write_native(id,&config));
            if let Err(error) = result {
                self.failures
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(id.clone(), error.clone());
                warnings.push(format!("{id}: {error}"));
            }
        }
        warnings
    }

    fn write_native(&self, id: &str, config: &HostedConfig) -> Result<(), String> {
        if id == "copilot" {
            return self.write_copilot(config);
        }
        if id == "pi" {
            self.validate_pi_scope()?;
            return providers::pi::install(
                &self.native.locations.pi,
                &self.pi_credentials_path(),
                config,
            );
        }
        let files = self.native_files(id)?;
        // Validate every effective file before making any edits. Legacy entries
        // in higher-precedence files must not override the installed credential.
        files
            .first()
            .ok_or("Provider configuration location is unavailable")?
            .validate_install(config)?;
        for file in &files {
            file.inspect(Some(config))?;
        }
        if id == "grok" {
            providers::token_file::install(config)?;
        }
        for file in files.iter().skip(1) {
            file.remove(Some(&config.endpoint))?;
        }
        files
            .first()
            .ok_or("Provider configuration location is unavailable")?
            .install(config)
    }

    pub(super) fn remove_native(&self, id: &str, endpoint: Option<&str>) -> Result<(), String> {
        if id == "pi" {
            self.validate_pi_scope()?;
            return providers::pi::remove(
                &self.native.locations.pi,
                &self.pi_credentials_path(),
                endpoint,
            );
        }
        let files = self.native_files(id)?;
        for file in &files {
            file.inspect(None)?;
        }
        for file in files {
            file.remove(endpoint)?;
        }
        if id == "grok" {
            providers::token_file::remove(&self.grok_credentials_path())?;
        }
        Ok(())
    }

    fn validate_pi_scope(&self) -> Result<(), String> {
        if self.helper.development_scope()
            && !development::safe_directory(&self.home, &self.native.locations.pi)
        {
            return Err("Development Pi configuration is outside its private profile".into());
        }
        Ok(())
    }

    fn grok_credentials_path(&self) -> PathBuf {
        self.choices
            .parent()
            .unwrap_or(&self.home)
            .join("mcp/providers/grok.token")
    }

    fn pi_credentials_path(&self) -> PathBuf {
        self.choices
            .parent()
            .unwrap_or(&self.home)
            .join("mcp/providers/pi.json")
    }

    /// A row's detected flag and, for Copilot, the one detection behind both
    /// that flag and its detail, so the two never disagree.
    pub(super) fn row_detection(&self, id: &str) -> (bool, Option<copilot::Detection>) {
        if id != "copilot" {
            return (self.detected(id), None);
        }
        let detection = self.native.copilot.detect();
        (detection.ready(), Some(detection))
    }

    pub(super) fn detected(&self, id: &str) -> bool {
        if let Some(setup) = external::setup(id, &self.home) {
            return setup.detected;
        }
        if Self::credential_provider(id) || id == "copilot" {
            return self.native_detected(id);
        }
        match id {
            "codex" => self.codex.executable.is_some() || self.codex.home.is_dir(),
            "claude" => self.claude.executable.is_some() || self.claude.home.is_dir(),
            "opencode" => self.opencode.installed(),
            _ => {
                self.home.join(format!(".{id}")).is_dir()
                    || self.home.join(".config").join(id).is_dir()
            }
        }
    }

    pub(super) fn native_detected(&self, id: &str) -> bool {
        if id == "cursor" {
            return super::external::cursor_installed(&self.native.cursor_detection_home);
        }
        if id == "copilot" {
            return self.native.copilot.detect().ready();
        }
        self.native.executables.resolve(id).is_some()
    }

    fn native_files(&self, id: &str) -> Result<Vec<ProviderFile>, String> {
        let locations = &self.native.locations;
        let file = |kind, path: &PathBuf| ProviderFile::new(kind, path.clone());
        let files = match id {
            "cursor" => vec![file(Provider::Cursor, &locations.cursor)],
            "copilot" => self.copilot_files(),
            "grok" => vec![file(Provider::Grok, &locations.grok)],
            "muse" => vec![file(Provider::Muse, &locations.muse)],
            "openclaw" => vec![file(Provider::OpenClaw, &locations.openclaw)],
            "hermes" => vec![file(Provider::Hermes, &locations.hermes)],
            "droid" => vec![file(Provider::Droid, &locations.droid)],
            "opencode" => self.opencode_files()?,
            "devin" => {
                let modern = self
                    .native
                    .versions
                    .devin(&self.native.executables.all())
                    .unwrap_or_else(|| {
                        locations.devin.exists() || !locations.devin_legacy.exists()
                    });
                let (primary, other) = if modern {
                    (&locations.devin, &locations.devin_legacy)
                } else {
                    (&locations.devin_legacy, &locations.devin)
                };
                let mut files = vec![file(Provider::Devin, primary)];
                if other.exists() {
                    files.push(file(Provider::Devin, other));
                }
                files
            }
            _ => Vec::new(),
        };
        if self.helper.development_scope()
            && files.iter().any(|file| {
                !file
                    .path
                    .parent()
                    .is_some_and(|parent| development::safe_directory(&self.home, parent))
            })
        {
            return Err("Development provider configuration is outside its private profile".into());
        }
        Ok(files)
    }

    fn opencode_files(&self) -> Result<Vec<ProviderFile>, String> {
        let paths = &self.native.locations.opencode;
        let primary = paths
            .iter()
            .rev()
            .find(|path| path.exists())
            .or_else(|| paths.first())
            .ok_or("Missing OpenCode configuration location")?;
        let major = self
            .native
            .versions
            .opencode(&self.native.executables.all());
        let mut files = Vec::new();
        for path in std::iter::once(primary).chain(
            paths
                .iter()
                .filter(|path| *path != primary && path.exists()),
        ) {
            let existing = fs::read_to_string(path)
                .ok()
                .and_then(|text| json5::from_str::<Value>(&text).ok());
            let v2 = major.map(|major| major >= 2).unwrap_or_else(|| {
                existing
                    .as_ref()
                    .is_some_and(|value| value["mcp"].get("servers").is_some())
            });
            files.push(ProviderFile::new(
                if v2 {
                    Provider::OpenCodeV2
                } else {
                    Provider::OpenCodeV1
                },
                path.clone(),
            ));
        }
        Ok(files)
    }
}

impl Installer {
    /// What Copilot needs and what was found, appended to its row's detail;
    /// while its connection is not ready, the reason setup is unavailable.
    pub(super) fn copilot_detail(
        &self,
        detection: Option<&copilot::Detection>,
        detail: String,
    ) -> String {
        let Some(detection) = detection else {
            return detail;
        };
        let found = self.native.copilot.detail(detection);
        format!("{} {detail} {found}", copilot::REQUIREMENTS)
    }
}

pub(super) fn machine_executables(
    home: &Path,
    env: &impl Fn(&str) -> Option<String>,
) -> BTreeMap<String, PathBuf> {
    discovery::Search::new(home, env).all()
}
