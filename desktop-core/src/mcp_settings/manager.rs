use super::*;
use crate::contract::tool_integrations::ToolsReport;
use agent_relay_node::relay::protocol::RpcError;

pub(super) const TOOLS: &[(&str, &str)] = &[
    ("codex", "Codex"),
    ("claude", "Claude Code"),
    ("conductor", "Conductor"),
    ("paseo", "Paseo"),
    ("capy", "Capy"),
    ("grok-bot", "Grok Bot"),
    ("dot", "Dot"),
    ("replicas", "Replicas"),
    ("opencode", "OpenCode"),
    ("grok", "Grok"),
    ("devin", "Devin"),
    ("cursor", "Cursor"),
    ("muse", "Muse Code"),
    ("muse-assistant", "Muse"),
    ("pi", "Pi"),
    ("openclaw", "OpenClaw"),
    ("hermes", "Hermes"),
    ("droid", "Factory Droid"),
    ("copilot", "GitHub Copilot (VS Code)"),
    ("amp", "Amp"),
];

/// Providers set up from a native config file with an app-issued credential.
pub(super) const CREDENTIAL_PROVIDERS: &[&str] = &[
    "cursor", "opencode", "grok", "devin", "muse", "pi", "openclaw", "hermes", "droid",
];

impl Installer {
    /// Expand host choices before requesting grants as well as before saving.
    pub fn expand_selection(&self, mut ids: Vec<String>) -> Vec<String> {
        let providers: Vec<String> = ids
            .iter()
            .flat_map(|id| {
                external::host_providers(id)
                    .iter()
                    .filter(|id| self.detected(id))
                    .map(|id| (*id).to_string())
            })
            .collect();
        ids.extend(providers);
        ids.sort();
        ids.dedup();
        ids
    }

    pub(super) fn selection(&self) -> Result<Option<Vec<String>>, String> {
        match fs::read(&self.choices) {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|_| {
                "Provider choices could not be read. Choose your providers again.".into()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("Provider choices could not be read. Check file permissions.".into()),
        }
    }

    /// The saved choices an explicit new choice replaces. A damaged file is
    /// `None` rather than an error, so choosing again repairs it; an
    /// unreadable one still refuses, since saving would fail too.
    fn replaced_selection(&self) -> Result<Option<Vec<String>>, RpcError> {
        match fs::read(&self.choices) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Some(Vec::new())),
            Err(_) => Err(RpcError::invalid_params(
                "Provider choices could not be read. Check file permissions.",
            )),
        }
    }

    fn save_selection(&self, ids: &[String]) -> Result<(), RpcError> {
        let text = serde_json::to_string(ids)
            .map_err(|_| RpcError::invalid_params("Provider choices could not be encoded."))?;
        files::update(&self.choices, |_| Some(text.clone()))
            .map_err(|_| RpcError::invalid_params("Provider choices could not be saved."))?;
        if self.selection().ok().flatten().as_deref() != Some(ids) {
            return Err(RpcError::invalid_params(
                "Provider choices could not be saved. Check file permissions.",
            ));
        }
        Ok(())
    }

    /// Selection persists immediately. The lifecycle-owned refresh reconciles
    /// selected tools; onboarding never waits for a native CLI subprocess.
    pub fn select(
        &self,
        mut ids: Vec<String>,
        binding: Option<&Binding>,
        health: Option<String>,
    ) -> Result<ToolsReport, RpcError> {
        self.validate_scope()?;
        let _writing = self
            .writing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let saved = self.replaced_selection()?;
        if ids.iter().any(|id| {
            !Self::supported_provider(id) && external::host_providers(id).is_empty()
                // A reopened chooser may retain an existing external sidebar
                // choice, but cannot request a new unsupported installation.
                && !(external::setup(id, &self.home).is_some()
                    && saved.as_ref().is_some_and(|previous| previous.contains(id)))
        }) {
            return Err(RpcError::invalid_params(
                "Agent Relay has not added automatic MCP setup for this provider yet.",
            ));
        }
        ids = self.expand_selection(ids);
        // Damaged choices name nothing reliably: every supported provider
        // left out of the new choice is cleaned up.
        let cursor_was_selected = saved
            .as_ref()
            .is_some_and(|ids| ids.iter().any(|id| id == "cursor"));
        let previous = saved.unwrap_or_else(|| {
            TOOLS
                .iter()
                .map(|(id, _)| (*id).to_string())
                .filter(|id| Self::supported_provider(id))
                .collect()
        });
        self.save_selection(&ids)?;
        for id in previous
            .iter()
            .filter(|id| Self::supported_provider(id) && !ids.contains(id))
        {
            let result = self.remove(id, binding);
            let mut failures = self
                .failures
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            failures.remove(id);
            if let Err(error) = result {
                failures.insert(id.clone(), error);
            }
        }
        if ids.iter().any(|id| id == "cursor") && !cursor_was_selected {
            self.request_cursor_hooks();
        }
        Ok(self.report(binding, health))
    }

    fn managed_selection(&self, id: &str, action: &str) -> Result<Vec<String>, RpcError> {
        // Damaged choices are replaced by this one action's result.
        let mut selected = self.replaced_selection()?.unwrap_or_default();
        // Host visibility is independent of a provider's connection. Only this
        // requested host is expanded below, so saved hosts cannot restore it.
        selected.retain(|existing| existing != id);
        if !matches!(action, "disable" | "uninstall") {
            selected.push(id.into());
        }
        if action == "enable" && !external::host_providers(id).is_empty() {
            selected.extend(self.expand_selection(vec![id.into()]));
        }
        selected.sort();
        selected.dedup();
        Ok(selected)
    }

    /// Enable records sidebar visibility without claiming local installation.
    /// Supported providers reconcile through the existing refresh lifecycle;
    /// unsupported providers can disable visibility without touching their files.
    pub fn manage(
        &self,
        id: &str,
        action: &str,
        binding: Option<&Binding>,
        health: Option<String>,
    ) -> Result<ToolsReport, RpcError> {
        self.validate_scope()?;
        if external::setup(id, &self.home).is_some() && !matches!(action, "enable" | "disable") {
            return Err(RpcError::invalid_params(
                "This provider's setup is managed outside local MCP configuration.",
            ));
        }
        if !TOOLS.iter().any(|(known, _)| *known == id)
            || !matches!(
                action,
                "enable" | "disable" | "install" | "repair" | "reinstall" | "uninstall"
            )
        {
            return Err(RpcError::invalid_params("Unknown provider or action."));
        }
        if action == "disable" && Self::supported_provider(id) {
            return Err(RpcError::invalid_params(
                "Use uninstall to remove this provider and clean up its MCP configuration.",
            ));
        }
        if !matches!(action, "enable" | "disable" | "uninstall") && !Self::supported_provider(id) {
            return Err(RpcError::invalid_params(
                "Agent Relay has not added automatic MCP setup for this provider yet.",
            ));
        }
        let _writing = self
            .writing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let selected = self.managed_selection(id, action)?;
        self.save_selection(&selected)?;
        self.failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
        if id == "cursor" && !matches!(action, "disable" | "uninstall") {
            self.request_cursor_hooks();
        }
        if action == "uninstall" {
            if id == "codex" {
                self.request_runtime_cleanup();
            }
            if let Err(error) = self.remove(id, binding) {
                self.failures
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(id.into(), error);
            }
        } else if !matches!(action, "enable" | "disable") {
            if let Some(binding) = binding {
                // Repair remains selected if a permission/conflict/auth migration
                // fails, so a later lifecycle refresh can retry it.
                self.install(binding, &mut logout::run);
            }
        }
        if !matches!(action, "enable" | "disable") {
            if let Some(binding) = binding {
                self.reconcile_runtime(binding);
            }
        }
        Ok(self.report(binding, health))
    }
}
