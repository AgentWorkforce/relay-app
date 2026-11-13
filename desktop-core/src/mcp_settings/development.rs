//! Explicit development profiles. Never changes HOME, copies credentials, or
//! installs a helper/configuration into the person's production directories.
use super::*;
mod codex;
use agent_relay_node::relay::daemon::Config;
use codex::CodexScope;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Clients {
    pub home: PathBuf,
    pub env: BTreeMap<String, String>,
    root: PathBuf,
    codex: CodexScope,
}

impl Clients {
    pub fn new(config: &Config) -> Self {
        let home = config.app_data_dir.join("mcp/clients");
        let codex = CodexScope::new(&config.home, &config.app_data_dir);
        let mut env = BTreeMap::new();
        for key in ["PATH", "PATHEXT", "PROGRAMFILES", "PROGRAMFILES(X86)"] {
            if let Some(value) = config.env.get(key) {
                env.insert(key.into(), value.clone());
            }
        }
        let lookup = |key: &str| config.env.get(key).cloned();
        let executables = [
            (
                "RELAY_DESKTOP_CODEX",
                CodexConfig::from_env(&lookup, &config.home).executable,
            ),
            (
                "RELAY_DESKTOP_CLAUDE",
                ClaudeConfig::from_env(&lookup, &config.home).executable,
            ),
        ];
        for (key, executable) in executables {
            if let Some(executable) = executable {
                env.insert(key.into(), executable.to_string_lossy().into_owned());
            }
        }
        // Detection reads the person's actual installation locations. Only the
        // executable paths cross into Dev; all provider configuration stays in
        // the private profiles below.
        env.insert(
            "AGENT_RELAY_PROVIDER_DETECTION_HOME".into(),
            config.home.to_string_lossy().into_owned(),
        );
        for id in [
            "grok", "opencode", "devin", "muse", "pi", "openclaw", "hermes", "droid",
        ] {
            let key = format!("RELAY_DESKTOP_{}", id.to_uppercase());
            if let Some(value) = config.env.get(&key) {
                env.insert(key, value.clone());
            }
        }
        for (id, executable) in native::machine_executables(&config.home, &lookup) {
            env.insert(
                format!("RELAY_DESKTOP_{}", id.to_uppercase()),
                executable.to_string_lossy().into_owned(),
            );
        }
        for (key, relative) in [
            ("CLAUDE_CONFIG_DIR", ".claude"),
            ("XDG_CONFIG_HOME", ".config"),
            ("XDG_DATA_HOME", ".local/share"),
            ("XDG_STATE_HOME", ".local/state"),
            ("APPDATA", "AppData/Roaming"),
            ("LOCALAPPDATA", "AppData/Local"),
            ("GROK_HOME", ".grok"),
            ("HERMES_HOME", ".hermes"),
            ("OPENCLAW_HOME", ".openclaw"),
            ("OPENCLAW_STATE_DIR", ".openclaw"),
            ("OPENCLAW_CONFIG_PATH", ".openclaw/openclaw.json"),
            ("OPENCODE_CONFIG", ".config/opencode/opencode.json"),
            ("OPENCODE_CONFIG_DIR", ".config/opencode"),
            ("PI_CODING_AGENT_DIR", ".pi/agent"),
            ("PASEO_HOME", ".paseo"),
        ] {
            env.insert(
                key.into(),
                home.join(relative).to_string_lossy().into_owned(),
            );
        }
        // Factory Droid reads `$FACTORY_HOME_OVERRIDE/.factory`, so the
        // override is the private home itself.
        env.insert(
            "FACTORY_HOME_OVERRIDE".into(),
            home.to_string_lossy().into_owned(),
        );
        for key in ["CODEX_HOME", "CODEX_SQLITE_HOME"] {
            env.insert(key.into(), codex.profile.to_string_lossy().into_owned());
        }
        env.insert("AGENT_RELAY_CLAUDE_CONFIG_DIRS".into(), String::new());
        env.insert(
            agent_relay_node::relay::paths::SOCKET_ENV.into(),
            config.paths.socket.to_string_lossy().into_owned(),
        );
        Self {
            home,
            env,
            root: config.app_data_dir.clone(),
            codex,
        }
    }

    pub fn safe(&self) -> bool {
        safe_profiles(&self.root) && self.codex.valid()
    }

    pub fn agent_environment(
        &self,
        previous: Option<agent_relay_node::relay::claude::AgentEnvironment>,
    ) -> agent_relay_node::relay::claude::AgentEnvironment {
        let overrides = self.env.clone();
        std::sync::Arc::new(move |harness, mut env| {
            if let Some(previous) = &previous {
                env = previous(harness, env);
            }
            env.extend(overrides.clone());
            env.remove("CLOUD_API_ACCESS_TOKEN");
            env
        })
    }
}

pub(super) struct Helper {
    path: PathBuf,
    copilot: Option<PathBuf>,
    socket: Option<PathBuf>,
    /// The session socket the Claude hook asks, named only for a site
    /// profile build; otherwise the hook finds it through `--dev` or not.
    session: Option<PathBuf>,
    /// The Claude and Codex entry name of a site profile build, so its entry
    /// sits beside Agent Relay's [`NAME`] rather than replacing it.
    server: Option<String>,
    source: Option<PathBuf>,
    root: Option<PathBuf>,
    digest: Option<String>,
    ready: AtomicBool,
    checked: AtomicBool,
    codex: Option<CodexScope>,
}

impl Helper {
    pub fn production(home: &Path) -> Self {
        Self {
            path: crate::probe_copy::destination(home),
            copilot: None,
            socket: None,
            session: None,
            server: None,
            source: None,
            root: None,
            digest: None,
            ready: AtomicBool::new(true),
            checked: AtomicBool::new(true),
            codex: None,
        }
    }

    fn development(root: &Path, socket: &Path, source: &Path, codex: Option<CodexScope>) -> Self {
        let digest = digest(source);
        let name = if cfg!(windows) {
            "agent-relay-probe.exe"
        } else {
            "agent-relay-probe"
        };
        Self {
            path: root
                .join("mcp/bin")
                .join(digest.as_deref().unwrap_or("unavailable"))
                .join(name),
            copilot: None,
            socket: Some(socket.into()),
            session: None,
            server: None,
            source: Some(source.into()),
            root: Some(root.into()),
            digest,
            ready: AtomicBool::new(false),
            checked: AtomicBool::new(false),
            codex,
        }
    }

    pub fn command(&self, binding: &Binding) -> Option<String> {
        binding.helper_at(&self.path, self.socket.as_deref())
    }
    /// A site profile build's own core socket, session socket and entry
    /// name, which the person's agents must name to reach it rather than
    /// Agent Relay's.
    pub(super) fn set_profile_sockets(&mut self, core: &Path, session: &Path, server: String) {
        if !self.development_scope() {
            self.socket = Some(core.into());
            self.session = Some(session.into());
            self.server = Some(server);
        }
    }
    /// The Claude and Codex entry this build writes and owns.
    pub fn server(&self) -> &str {
        self.server.as_deref().unwrap_or(NAME)
    }
    /// Which Claude hook is this build's own.
    pub(super) fn hook_build(&self) -> Option<super::claude_hook::Build> {
        super::claude_hook::Build::new(self.session.as_deref(), self.server())
    }
    /// The PreToolUse hook run by this same executable.
    pub fn attestation_hook(&self) -> Option<String> {
        super::claude_hook::command(
            &self.path,
            self.development_scope(),
            self.session.as_deref(),
        )
    }
    pub(super) fn set_copilot_executable(&mut self, executable: Option<PathBuf>) {
        if !self.development_scope() {
            self.copilot = executable.filter(|path| path.is_absolute());
        }
    }
    pub(super) fn copilot_executable(&self) -> PathBuf {
        if self.development_scope() {
            self.path.clone()
        } else {
            self.copilot.clone().unwrap_or_default()
        }
    }
    pub(super) fn copilot_usable(&self) -> bool {
        if self.development_scope() {
            self.usable()
        } else {
            self.copilot
                .as_ref()
                .is_some_and(|path| agent_relay_node::platform::exe::is_executable(path))
        }
    }
    pub fn cursor_hook(&self) -> String {
        agent_relay_node::relay::cursor_cli::CursorSettings::command_for(
            &self.path,
            self.development_scope(),
        )
    }
    pub fn usable(&self) -> bool {
        self.ready.load(Ordering::Acquire)
            && agent_relay_node::platform::exe::is_executable(&self.path)
    }
    pub fn pending(&self) -> bool {
        !self.checked.load(Ordering::Acquire)
    }
    pub fn development_scope(&self) -> bool {
        self.root.is_some()
    }
    pub fn valid(&self) -> bool {
        self.root.as_ref().is_none_or(|root| {
            safe_profiles(root)
                && self.codex.as_ref().is_some_and(CodexScope::valid)
                && self
                    .path
                    .parent()
                    .is_some_and(|path| safe_directory(root, path))
        })
    }

    pub fn ensure(&self) -> bool {
        let Some(source) = &self.source else {
            return true;
        };
        if !self.valid() || !self.codex.as_ref().is_some_and(CodexScope::ensure) {
            self.checked.store(true, Ordering::Release);
            return false;
        }
        if self.usable() {
            return true;
        }
        if fs::symlink_metadata(&self.path).is_ok_and(|meta| meta.file_type().is_symlink()) {
            self.checked.store(true, Ordering::Release);
            return false;
        }
        let _ = crate::probe_copy::install_if_needed(source, &self.path, false, |_, _| false);
        let identical = self.digest.is_some() && digest(&self.path) == self.digest;
        if identical {
            let _ = private_fs::set_mode(&self.path, 0o700);
        }
        self.ready.store(identical, Ordering::Release);
        self.checked.store(true, Ordering::Release);
        self.usable()
    }
}

impl Installer {
    pub(super) fn claude_login_allowed(&self, path: &Path) -> bool {
        !self.helper.development_scope() || safe_directory(&self.home, path)
    }

    pub(super) fn development_detail(&self, id: &str, detail: String) -> String {
        if !self.helper.development_scope() {
            return detail;
        }
        match id {
            "codex" => format!("{detail} Development profile only: CODEX_HOME={} and CODEX_SQLITE_HOME={}. Global client profiles are unchanged.", self.codex.home.display(), self.codex.sqlite_home.display()),
            "claude" => format!("{detail} Development profile only: CLAUDE_CONFIG_DIR={}. Global client profiles are unchanged.", self.claude.home.display()),
            _ => detail,
        }
    }
    pub fn with_development(mut self, root: &Path, socket: &Path, source: &Path) -> Self {
        self.copilot_native = Default::default();
        self.helper = Helper::development(
            root,
            socket,
            source,
            (self.codex.sqlite_home == self.codex.home)
                .then(|| CodexScope::from_profile(&self.codex.home, root))
                .flatten(),
        );
        self.choices = root.join("tool-integrations.json");
        self
    }

    pub(super) fn validate_scope(&self) -> Result<(), agent_relay_node::relay::protocol::RpcError> {
        if self.helper.valid() {
            Ok(())
        } else {
            Err(agent_relay_node::relay::protocol::RpcError::invalid_params("Development provider profiles must stay in their private app-data or owner-scoped Dev directories without symbolic links."))
        }
    }
}

fn digest(path: &Path) -> Option<String> {
    Some(format!("{:x}", Sha256::digest(fs::read(path).ok()?)))
}

fn safe_profiles(root: &Path) -> bool {
    root.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".dev"))
        && [
            "mcp/clients/.codex",
            "mcp/clients/.claude",
            "mcp/clients/.config",
            "mcp/clients/.local",
            "mcp/clients/.grok",
            "mcp/clients/.pi",
            "mcp/clients/.hermes",
            "mcp/clients/.factory",
            "mcp/clients/.openclaw",
            "mcp/clients/AppData",
        ]
        .iter()
        .all(|relative| safe_directory(root, &root.join(relative)))
}

pub(super) fn safe_directory(root: &Path, directory: &Path) -> bool {
    let Ok(relative) = directory.strip_prefix(root) else {
        return false;
    };
    // `..` could walk from the written prefix back into a normal profile.
    if !relative
        .components()
        .all(|part| matches!(part, std::path::Component::Normal(_)))
    {
        return false;
    }
    let mut path = root.to_path_buf();
    for part in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(part) = part {
            path.push(part);
        }
        match fs::symlink_metadata(&path) {
            Ok(meta)
                if !meta.is_dir()
                    || meta.file_type().is_symlink()
                    || !private_fs::owned_by_current_user(&meta) =>
            {
                return false
            }
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(_) => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests;
