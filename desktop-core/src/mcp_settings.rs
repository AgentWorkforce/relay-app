//! Direct hosted MCP configuration authorized by the desktop login. Clients use
//! native HTTP header helpers or revocable, scoped provider grants. Copilot's
//! native bridge forwards inside Core. App access and refresh tokens never
//! enter provider configuration.
use agent_relay_node::platform::fs as private_fs;
use agent_relay_node::relay::{claude::ClaudeConfig, codex::CodexConfig, opencode::OpenCodeConfig};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

mod claude_hook;
mod codex;
mod copilot;
mod cursor;
pub(crate) mod development;
mod external;
pub(crate) use external::derive_host_connections;
mod files;
mod inventory;
mod json_config;
#[cfg(test)]
mod lifecycle_tests;
mod logout;
mod manager;
mod native;
#[cfg(test)]
mod profile_socket_tests;
pub(crate) mod providers;
mod recognized;
mod runtime;
mod scopes;
mod teammate_hook;
#[cfg(test)]
mod teammate_hook_tests;
#[cfg(test)]
mod tests;
mod verification;

pub(crate) const NAME: &str = agent_relay_node::relay::hosted_mcp::MCP;
/// Copilot flattens server and tool names with `-`; this identifier has no separator.
pub(crate) const COPILOT_MCP_SERVER: &str = "agentrelay";
/// Legacy VS Code HTTP imports must not override the native Copilot server.
pub(crate) const COPILOT_HISTORY_MCP_SERVER: &str = "relayhistory";
const ROUTE: &str = "/cloud/api/v1/mcp/shared-sessions";

/// Amp connects in Agent Relay Cloud, never on this computer. Confirmed Cloud
/// route: the settings page reads the `connections` hash and opens that tab,
/// with Amp's card first. It is relative to the Cloud app at `<site>/cloud`.
pub const AMP_CONNECTIONS_PATH: &str = "/dashboard/settings#connections";

/// The workspace's Cloud connections page for a site origin, built the way
/// sign-in builds Cloud URLs: the validated origin, then `/cloud` exactly once.
pub fn amp_connections_url(site_url: &str) -> Option<String> {
    let site = site_url.trim().trim_end_matches('/');
    let site = agent_relay_node::relay::workspace::validated_base_url(
        site.strip_suffix("/cloud").unwrap_or(site),
    )?;
    Some(format!("{site}/cloud{AMP_CONNECTIONS_PATH}"))
}

/// Exact desktop identity pinned into a credential-only client helper. A helper
/// from an older account cannot obtain a newly signed-in person's credential.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub site_url: String,
    pub workspace_id: String,
    pub account_id: String,
}

impl Binding {
    pub(crate) fn endpoint(&self) -> Option<String> {
        let site = agent_relay_node::relay::workspace::validated_base_url(&self.site_url)?;
        if [&self.workspace_id, &self.account_id].iter().any(|id| {
            id.is_empty()
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
        }) {
            return None;
        }
        Some(format!("{site}{ROUTE}"))
    }

    #[cfg(test)]
    fn helper(&self, command: &Path) -> Option<String> {
        self.helper_at(command, None)
    }

    fn helper_at(&self, command: &Path, socket: Option<&Path>) -> Option<String> {
        self.endpoint()?;
        let mut words = vec![
            command.to_str()?,
            "mcp-headers",
            "--site",
            &self.site_url,
            "--workspace",
            &self.workspace_id,
            "--account",
            &self.account_id,
        ];
        if let Some(socket) = socket {
            words.extend(["--socket", socket.to_str()?]);
        }
        Some(
            words
                .into_iter()
                .map(quote)
                .collect::<Option<Vec<_>>>()?
                .join(" "),
        )
    }
}

fn quote(word: &str) -> Option<String> {
    #[cfg(unix)]
    {
        Some(agent_relay_node::relay::outbound::shell_quote(word))
    }
    #[cfg(windows)]
    {
        // Header helpers execute through a shell. Refuse expansion/control
        // characters rather than letting an unusual profile path become code.
        if word.chars().any(|c| "\"%!^&|<>$`\r\n".contains(c)) {
            return None;
        }
        Some(format!("\"{word}\""))
    }
}

pub struct Installer {
    home: PathBuf,
    codex: CodexConfig,
    claude: ClaudeConfig,
    opencode: OpenCodeConfig,
    warnings: std::sync::Mutex<Vec<String>>,
    choices: PathBuf,
    writing: std::sync::Mutex<()>,
    failures: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
    runtime: std::sync::Mutex<runtime::Runtime>,
    helper: development::Helper,
    native: native::Native,
    claude_attestation: std::sync::Mutex<Option<claude_hook::Verified>>,
    copilot_attestation: std::sync::Mutex<Option<claude_hook::Verified>>,
    copilot_native: std::sync::OnceLock<agent_relay_node::relay::copilot::CopilotConfig>,
    copilot_epoch: std::sync::atomic::AtomicU64,
    copilot_selection: std::sync::Mutex<Option<Vec<String>>>,
}

impl Installer {
    pub fn copilot_helper(&self) -> PathBuf {
        self.helper.copilot_executable()
    }

    /// The Claude and Codex entry this build writes: [`NAME`], or a site
    /// profile build's own beside it.
    pub(crate) fn server(&self) -> &str {
        self.helper.server()
    }

    /// The helper and hook commands this installer writes, and their entry.
    #[cfg(test)]
    pub(crate) fn commands(&self, binding: &Binding) -> (Option<String>, Option<String>, &str) {
        (
            self.helper.command(binding),
            self.helper.attestation_hook(),
            self.helper.server(),
        )
    }

    pub fn new(
        home: PathBuf,
        codex: CodexConfig,
        claude: ClaudeConfig,
        opencode: OpenCodeConfig,
    ) -> Self {
        Self {
            home: home.clone(),
            codex,
            claude,
            opencode,
            warnings: std::sync::Mutex::new(Vec::new()),
            choices: home.join(".agent-relay/tool-integrations.json"),
            writing: std::sync::Mutex::new(()),
            failures: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            runtime: std::sync::Mutex::new(runtime::Runtime::default()),
            helper: development::Helper::production(&home),
            native: native::Native::empty(&home),
            claude_attestation: std::sync::Mutex::new(None),
            copilot_attestation: std::sync::Mutex::new(None),
            copilot_native: Default::default(),
            copilot_epoch: Default::default(),
            copilot_selection: Default::default(),
        }
    }

    /// A site profile build's core serves its own sockets (`build-app.sh`
    /// gives it `core.<bundle id>.sock`), so the helper and hook commands
    /// written for the person's agents name them, under its own entry name
    /// beside Agent Relay's. On the default socket, Agent Relay's own, the
    /// entries stay as they were.
    pub(crate) fn with_core_sockets(
        mut self,
        config: &agent_relay_node::relay::daemon::Config,
    ) -> Self {
        use agent_relay_node::relay::{outbound, paths, peer};
        if config.paths.socket != paths::default_socket(&config.home) {
            let env = |key: &str| config.env.get(key).cloned();
            let session = outbound::default_socket(&env, &config.app_data_dir, peer::current_uid());
            self.helper.set_profile_sockets(
                &config.paths.socket,
                &session,
                crate::claude_attestation::server_for(config),
            );
        }
        self
    }

    pub fn with_choices_path(mut self, path: PathBuf) -> Self {
        self.choices = path;
        self
    }

    /// A signed-out app invalidates cached provider grants. Credential helpers
    /// verify identity pins, while the account lifecycle revokes hosted grants.
    pub fn refresh(&self, binding: Option<&Binding>) -> Vec<String> {
        let _writing = self
            .writing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut current = binding
            .map(|binding| self.install(binding, &mut logout::run))
            .unwrap_or_default();
        // The retired teammate-context hook leaves on every refresh, signed
        // out, with no saved selection or no helper included: those paths
        // install nothing, so the cleanup cannot live in `install`.
        if let Err(warning) = self.remove_teammate_hooks() {
            current.push(warning);
            current.sort();
            current.dedup();
        }
        if let Some(binding) = binding {
            self.clear_reconciled_failures(binding);
            self.reconcile_runtime(binding);
        } else {
            self.clear_provider_credentials();
            self.invalidate_runtime();
        }
        let mut previous = self
            .warnings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let newly = current
            .iter()
            .filter(|warning| !previous.contains(warning))
            .cloned()
            .collect();
        *previous = current;
        newly
    }

    fn install(
        &self,
        binding: &Binding,
        logout: &mut dyn FnMut(&CodexConfig, &str) -> bool,
    ) -> Vec<String> {
        if self.validate_scope().is_err() {
            return vec!["Development provider profiles are not safely isolated; no client configuration was changed.".into()];
        }
        let selected = match self.selection() {
            Ok(Some(selected)) => selected,
            Ok(_) => return Vec::new(),
            Err(error) => return vec![error],
        };
        let mut warnings = self.install_native(binding, &selected);
        if !selected
            .iter()
            .any(|id| matches!(id.as_str(), "codex" | "claude"))
        {
            return warnings;
        }
        let Some(helper) = self.helper.command(binding) else {
            warnings.push(
                "Hosted Agent Relay MCP setup needs a valid desktop account and site.".into(),
            );
            return warnings;
        };
        if !self.helper.ensure() || !self.helper.usable() {
            warnings.push("Hosted Agent Relay MCP setup is waiting for the command-line probe to be installed.".into());
            return warnings;
        }
        if selected.iter().any(|id| id == "codex") {
            if !codex::install(&self.codex, binding, &helper, self.helper.server(), logout) {
                warnings.push("Codex hosted Agent Relay MCP setup could not use your Agent Relay sign-in. Existing configuration was preserved; check the Relay entry and that the installed Codex supports HTTP header helpers and MCP logout.".into());
            }
        }
        for login in self
            .claude
            .dirs
            .watched(&self.claude.home)
            .into_iter()
            .filter(|_| selected.iter().any(|id| id == "claude"))
            .filter(|login| self.claude_login_allowed(&login.path))
        {
            if !login.shared {
                continue;
            }
            let file =
                if !self.helper.development_scope() && login.path == self.home.join(".claude") {
                    self.home.join(".claude.json")
                } else {
                    login.path.join(".claude.json")
                };
            if !json_config::install(
                &file,
                binding,
                &helper,
                self.helper.server(),
                !self.helper.development_scope(),
            ) {
                warnings.push("Claude Code hosted Agent Relay MCP setup could not use your Agent Relay sign-in. Existing configuration was preserved; check .claude.json and support for headersHelper.".into());
            } else if !self.claude_hook_command().is_some_and(|hook| {
                self.edit_claude_hook(&login.path.join("settings.json"), Some(&hook))
            }) {
                warnings.push("Claude Code hosted Agent Relay MCP setup could not add the chat identity hook. Existing settings were preserved; check that settings.json is a JSON object you can write.".into());
            }
        }
        warnings.sort();
        warnings.dedup();
        warnings
    }
}

/// Only the exact current hosted endpoint or an existing desktop-managed
/// helper may be migrated. A same-name server on another origin stays intact.
fn managed_helper(existing: &str, expected: &str) -> bool {
    [" --site ", " \"--site\" "].iter().any(|separator| {
        existing
            .split_once(separator)
            .zip(expected.split_once(separator))
            .is_some_and(|((old, _), (new, _))| old == new || current_form(old) == new)
    })
}

/// A desktop-managed helper another build on this computer wrote: a site
/// profile build's names its own core with `--socket` and Agent Relay's names
/// none. Neither build migrates or removes the other's entry.
fn other_build(existing: &str, expected: &str) -> bool {
    managed_helper(existing, expected) && helper_socket(existing) != helper_socket(expected)
}

fn helper_socket(helper: &str) -> Option<&str> {
    [" --socket ", " \"--socket\" "]
        .iter()
        .find_map(|separator| helper.split_once(separator).map(|(_, socket)| socket))
}

/// A helper written in the earlier `<executable> relay mcp-headers` form, in
/// the current `<executable> mcp-headers` one, so setup replaces it.
fn current_form(helper: &str) -> String {
    [
        (" relay mcp-headers", " mcp-headers"),
        (" \"relay\" \"mcp-headers\"", " \"mcp-headers\""),
    ]
    .iter()
    .find_map(|(earlier, current)| {
        helper
            .strip_suffix(earlier)
            .map(|executable| format!("{executable}{current}"))
    })
    .unwrap_or_else(|| helper.to_string())
}
