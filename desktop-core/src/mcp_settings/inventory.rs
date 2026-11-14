use super::*;
use crate::contract::tool_integrations::{ToolIntegration, ToolsReport};
use toml_edit::{DocumentMut, Item};
mod capabilities;
mod removal;
use capabilities::capabilities;

impl Installer {
    pub fn report(&self, binding: Option<&Binding>, health: Option<String>) -> ToolsReport {
        let choices = self.selection();
        let endpoint = binding.and_then(Binding::endpoint);
        let helper = self.helper_context(binding);
        let mut tools: Vec<_> = manager::TOOLS
            .iter()
            .map(|(id, name)| {
                self.tool_report(
                    id,
                    name,
                    binding,
                    &choices,
                    endpoint.as_deref(),
                    helper.as_deref(),
                )
            })
            .collect();
        external::derive_host_connections(&mut tools, health.as_deref());
        tools.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        ToolsReport {
            onboarding_complete: choices.as_ref().is_ok_and(Option::is_some),
            account_health: health,
            tools,
        }
    }

    fn tool_report(
        &self,
        id: &str,
        name: &str,
        binding: Option<&Binding>,
        choices: &Result<Option<Vec<String>>, String>,
        endpoint: Option<&str>,
        helper: Option<&str>,
    ) -> ToolIntegration {
        let supported = Self::supported_provider(id);
        let external = external::setup(id, &self.home);
        let (detected, copilot) = self.row_detection(id);
        let selected = choices
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .is_some_and(|ids| ids.iter().any(|i| i == id));
        let (present, configured) = self.proof(id, endpoint.as_deref(), helper.as_deref());
        let configured = self.configuration_matches_login(id, configured, binding);
        let runtime_problem = self.runtime_problem(id, binding, selected);
        let failure = self
            .failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned()
            .or_else(|| self.config_error(id));
        let (state, label, detail) = if let Some(detail) =
            self.provider_signin_detail(id, binding, supported, selected, present)
        {
            ("signed_out", "Sign in needed", detail)
        } else if failure.is_some() {
            ("action_required", "Action needed", "Configuration could not be checked or changed. Check file permissions, invalid configuration, and conflicting Relay entries, then try again.")
        } else if let Some(setup) = &external {
            ("external_setup", setup.label, setup.detail)
        } else if !supported {
            ("unavailable", "Setup not available yet", "Agent Relay has not added automatic MCP setup for this provider yet. Existing delivery settings are unchanged.")
        } else if choices.is_err() {
            (
                "action_required",
                "Action needed",
                "Provider choices could not be read. Choose your providers again.",
            )
        } else if self.helper_pending(matches!(id, "codex" | "claude"), selected) {
            ("checking", "Checking", "Agent Relay is checking its command-line helper. Provider setup will continue automatically.")
        } else if self.helper_needed(matches!(id, "codex" | "claude"), selected, present) {
            ("action_required", "Setup needed", "Agent Relay's command-line helper is missing or cannot be run. Repair or reinstall Agent Relay, then repair this provider's MCP setup.")
        } else if let Some(detail) = runtime_problem.as_deref() {
            ("action_required", "MCP not ready", detail)
        } else if configured && self.copilot_messaging_unavailable(id) {
            ("action_required", "Messaging unavailable", "Copilot configuration is installed. Hosted Copilot chat verification is unavailable for this account; availability refreshes automatically.")
        } else if self.copilot_configuration_connected(id, configured) {
            ("configured", "Connected", "Copilot MCP configuration is installed. Local chat wake and verified messaging are unavailable on this platform.")
        } else if configured && self.needs_runtime_verification(id, binding, selected) {
            ("installed", "Setup saved", "Agent Relay MCP configuration is installed. Running chats have not yet been verified.")
        } else if configured && id == "claude" && !self.claude_attestation() {
            ("action_required", "Messaging unavailable", "Agent Relay MCP configuration and the chat identity hook are installed. The hosted service does not verify Claude chat identity yet, so sending is blocked until it does.")
        } else if configured {
            ("configured", "Configured", "Hosted MCP configuration verified. Availability in existing running sessions has not been verified.")
        } else if selected || present {
            ("action_required", "Setup needed", "Hosted MCP configuration is missing, disabled, or could not be reconciled. Repair checks permissions, conflicting entries, and authentication migration.")
        } else if choices.as_ref().is_ok_and(Option::is_none) {
            ("onboarding_needed", "Choose providers", "Choose which providers should receive Agent Relay's hosted sessions and messaging tools.")
        } else {
            (
                "not_installed",
                "Not installed",
                "Agent Relay MCP is not selected for this provider.",
            )
        };
        let (can_install, can_repair, can_uninstall) = if external.is_some() {
            (false, false, false)
        } else {
            capabilities(
                supported,
                detected,
                selected,
                present,
                configured,
                failure.is_some(),
            )
        };
        ToolIntegration {
            id: id.into(),
            name: name.into(),
            detected,
            onboarding_available: external::onboarding_available(id, &self.home, Path::new("/Applications")),
            supported,
            selected,
            state: state.into(),
            status_label: label.into(),
            detail: self.integration_detail(
                state,
                id,
                binding,
                selected,
                failure_detail(state, failure.as_deref(), detail),
                copilot.as_ref(),
            ),
            can_install,
            can_repair,
            can_uninstall,
        }
    }

    /// Checking shows only while setup will verify the helper: `install`
    /// ensures it for selected providers alone, so a leftover unselected
    /// entry keeps its setup or cleanup state instead of waiting forever.
    fn helper_pending(&self, supported: bool, selected: bool) -> bool {
        supported && selected && self.helper.pending()
    }

    fn provider_signin_detail(
        &self,
        id: &str,
        binding: Option<&Binding>,
        supported: bool,
        selected: bool,
        present: bool,
    ) -> Option<&'static str> {
        if needs_signin(binding, supported, selected, present) {
            Some("Sign in to Agent Relay to connect your providers. Agent Relay tools cannot authenticate while you are signed out.")
        } else if selected && self.native_login_required(id, binding) {
            Some("Sign in to Agent Relay again to connect this provider on this computer.")
        } else {
            None
        }
    }

    fn needs_runtime_verification(
        &self,
        id: &str,
        binding: Option<&Binding>,
        selected: bool,
    ) -> bool {
        Self::credential_provider(id) || self.runtime_unverified(id, binding, selected)
    }

    fn integration_detail(
        &self,
        state: &str,
        id: &str,
        binding: Option<&Binding>,
        selected: bool,
        fallback: &str,
        copilot: Option<&native::copilot::Detection>,
    ) -> String {
        let detail = if matches!(state, "configured" | "installed") {
            self.runtime_detail(id, binding, selected)
                .unwrap_or_else(|| fallback.into())
        } else {
            fallback.into()
        };
        let detail = self.copilot_detail(copilot, self.cursor_detail(id, state, selected, detail));
        let detail = self.copilot_connection_detail(id, state, detail);
        self.development_detail(id, detail)
    }

    pub(super) fn claude_files(&self) -> Vec<PathBuf> {
        self.claude_paths(false)
    }

    fn claude_paths(&self, shared_only: bool) -> Vec<PathBuf> {
        let mut paths: Vec<_> = self
            .claude
            .dirs
            .watched(&self.claude.home)
            .into_iter()
            .filter(|login| !shared_only || login.shared)
            .filter(|login| self.claude_login_allowed(&login.path))
            .map(|login| {
                if !self.helper.development_scope() && login.path == self.home.join(".claude") {
                    self.home.join(".claude.json")
                } else {
                    login.path.join(".claude.json")
                }
            })
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }

    fn json_paths(&self, id: &str) -> Vec<PathBuf> {
        match id {
            "claude" => self.claude_files(),
            "openclaw" => vec![self.home.join(".openclaw/openclaw.json")],
            "devin" => vec![self.home.join(".config/devin/mcp_config.json")],
            "opencode" => vec![
                self.opencode.config_dir.join("opencode.json"),
                self.opencode.config_dir.join("opencode.jsonc"),
            ],
            _ => Vec::new(),
        }
    }

    pub(super) fn proof(
        &self,
        id: &str,
        endpoint: Option<&str>,
        helper: Option<&str>,
    ) -> (bool, bool) {
        if Self::credential_provider(id) {
            let proof = self.native_proof(id, endpoint).unwrap_or_default();
            return (proof.present, proof.configured);
        }
        if id == "codex" {
            let document = fs::read_to_string(self.codex.home.join("config.toml"))
                .ok()
                .and_then(|text| text.parse::<DocumentMut>().ok());
            let entries = document
                .as_ref()
                .and_then(|doc| doc.get("mcp_servers"))
                .and_then(Item::as_table_like);
            let present = entries
                .is_some_and(|s| s.iter().any(|(_, s)| recognized::toml(s, endpoint, helper)));
            let configured = self.helper_usable()
                && entries.is_some_and(|servers| {
                    servers
                        .iter()
                        .filter(|(_, s)| recognized::toml(s, endpoint, helper))
                        .count()
                        == 1
                        && servers
                            .get(self.helper.server())
                            .is_some_and(|server| verification::codex(server, endpoint, helper))
                });
            return (present, configured);
        }
        let paths = self.json_paths(id);
        let roots: std::collections::BTreeMap<PathBuf, Value> = paths
            .iter()
            .filter_map(|path| {
                fs::read_to_string(path)
                    .ok()
                    .and_then(|text| parse_config(id, path, &text))
                    .map(|root| (path.clone(), root))
            })
            .collect();
        let scopes_clear = id != "claude"
            || self.helper.development_scope()
            || paths
                .iter()
                .all(|path| scopes::clear(path, endpoint, helper, false));
        let present = !scopes_clear
            || roots
                .values()
                .any(|root| json_present(root, endpoint, helper))
            || (id == "claude" && self.claude_hook_present());
        let required = if id == "claude" {
            self.claude_paths(true)
        } else {
            paths
        };
        let configured = self.helper_usable()
            && !required.is_empty()
            && scopes_clear
            && required.iter().all(|path| {
                roots.get(path).is_some_and(|root| {
                    verification::json_as(root, endpoint, helper, self.helper.server())
                })
            })
            && (id != "claude" || self.claude_hook_configured());
        (present, configured)
    }

    fn helper_usable(&self) -> bool {
        self.helper.usable()
    }

    fn helper_needed(&self, supported: bool, selected: bool, present: bool) -> bool {
        supported && (selected || present) && !self.helper_usable()
    }

    fn config_error(&self, id: &str) -> Option<String> {
        if Self::credential_provider(id) {
            return self.native_proof(id, None).err();
        }
        let paths = if id == "codex" {
            vec![self.codex.home.join("config.toml")]
        } else {
            self.json_paths(id)
        };
        for path in paths {
            match fs::read_to_string(&path) {
                Ok(text) => {
                    let valid = if id == "codex" {
                        text.parse::<DocumentMut>().is_ok()
                    } else {
                        parse_config(id, &path, &text).is_some()
                    };
                    if !valid {
                        return Some("Configuration is invalid or unsupported.".into());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(_) => return Some("Configuration could not be read.".into()),
            }
        }
        None
    }

    fn helper_context(&self, binding: Option<&Binding>) -> Option<String> {
        let signed_out = Binding {
            site_url: "https://agentrelay.com".into(),
            workspace_id: "signed-out".into(),
            account_id: "signed-out".into(),
        };
        self.helper.command(binding.unwrap_or(&signed_out))
    }

    pub(super) fn clear_reconciled_failures(&self, binding: &Binding) {
        let selected = self.selection().ok().flatten().unwrap_or_default();
        let endpoint = binding.endpoint();
        let helper = self.helper_context(Some(binding));
        self.failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|id, _| {
                !selected.contains(id)
                    || self.config_error(id).is_some()
                    || !self.proof(id, endpoint.as_deref(), helper.as_deref()).1
            });
    }
}

fn parse_config(id: &str, path: &Path, text: &str) -> Option<Value> {
    let relaxed = id == "openclaw"
        || path
            .extension()
            .is_some_and(|extension| extension == "jsonc");
    parse_with_fallback(text, relaxed, |text| json5::from_str(text).ok())
}

pub(super) fn parse_with_fallback(
    text: &str,
    relaxed: bool,
    fallback: impl FnOnce(&str) -> Option<Value>,
) -> Option<Value> {
    // Claude's user file can be large. JSON5's general grammar is expensive
    // even for ordinary JSON, so use it only after the fast parser rejects a
    // format explicitly known to allow comments or JSON5 syntax.
    serde_json::from_str(text)
        .ok()
        .or_else(|| relaxed.then(|| fallback(text)).flatten())
}

fn json_present(root: &Value, endpoint: Option<&str>, helper: Option<&str>) -> bool {
    if root["mcp"]["servers"]
        .as_object()
        .is_some_and(|s| s.values().any(|s| recognized::json(s, endpoint, helper)))
    {
        return true;
    }
    ["mcpServers", "mcp"].iter().any(|key| {
        root[*key]
            .as_object()
            .is_some_and(|s| s.values().any(|s| recognized::json(s, endpoint, helper)))
    }) || root["projects"].as_object().is_some_and(|projects| {
        projects
            .values()
            .any(|project| json_present(project, endpoint, helper))
    })
}

fn json_remove(root: &mut Value, endpoint: Option<&str>, helper: Option<&str>) -> bool {
    let mut changed = false;
    if let Some(servers) = root
        .get_mut("mcp")
        .and_then(|mcp| mcp.get_mut("servers"))
        .and_then(Value::as_object_mut)
    {
        let before = servers.len();
        servers.retain(|_, s| !recognized::json(s, endpoint, helper));
        changed |= before != servers.len();
    }
    for key in ["mcpServers", "mcp"] {
        if let Some(servers) = root.get_mut(key).and_then(Value::as_object_mut) {
            let before = servers.len();
            servers.retain(|_, s| !recognized::json(s, endpoint, helper));
            changed |= before != servers.len();
        }
    }
    if let Some(projects) = root.get_mut("projects").and_then(Value::as_object_mut) {
        for project in projects.values_mut() {
            changed |= json_remove(project, endpoint, helper);
        }
    }
    changed
}

fn needs_signin(binding: Option<&Binding>, supported: bool, selected: bool, present: bool) -> bool {
    binding.is_none() && supported && (selected || present)
}
fn failure_detail<'a>(state: &str, failure: Option<&'a str>, fallback: &'a str) -> &'a str {
    if state == "action_required" {
        failure.unwrap_or(fallback)
    } else {
        fallback
    }
}
