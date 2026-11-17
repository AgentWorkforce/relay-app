//! Cursor's existing messaging hooks share the provider installer lifecycle.
use super::*;
use agent_relay_node::relay::agents::SettingsExtension;
use agent_relay_node::relay::cursor_cli::{CursorConfig, CursorSettings};
use cursor_guide::GuideHooks;
use providers::{HostedConfig, Provider, ProviderFile};

impl Installer {
    /// Cursor's hosted configuration while Cursor is selected and its
    /// `mcp.json` entry is this build's: what the `cursor-mcp` route
    /// forwards with. The grant never leaves Core.
    pub(crate) fn cursor_dispatch_hosted(&self, binding: Option<&Binding>) -> Option<HostedConfig> {
        if !self
            .selection()
            .ok()
            .flatten()?
            .iter()
            .any(|id| id == "cursor")
        {
            return None;
        }
        let config = self.hosted("cursor", binding)?;
        let file = self.native_files("cursor").ok()?.into_iter().next()?;
        file.inspect(Some(&config))
            .ok()?
            .configured
            .then_some(config)
    }

    /// Whether Cursor's calls reach the hosted MCP through this build's
    /// `cursor-mcp` helper with the hooks that announce each chat's call in
    /// place: never on Windows, where Cursor's hooks are unavailable.
    pub(crate) fn cursor_attested_transport(&self) -> bool {
        !cfg!(windows)
            && self
                .cursor_dispatch_hosted(None)
                .is_some_and(|config| config.cursor_stdio.is_some())
            && self.cursor_hooks_ready()
    }

    /// Cursor's `mcp.json`, whose Agent Relay entry may be this build's
    /// helper command.
    pub(super) fn cursor_file(&self) -> ProviderFile {
        ProviderFile::new(Provider::Cursor, self.native.locations.cursor.clone())
            .with_copilot_helper(self.cursor_helper())
    }

    /// The helper entry Cursor runs instead of holding the grant: Core
    /// forwards its calls and signs each one a chat's hook announced.
    /// Windows keeps the URL entry: Cursor's hooks, which name the chat,
    /// are unavailable there.
    pub(super) fn cursor_stdio(&self, pinned: &Binding) -> Option<providers::CopilotStdio> {
        (!cfg!(windows)).then(|| providers::CopilotStdio {
            command: self.cursor_helper(),
            args: crate::cursor_dispatch::arguments(
                pinned,
                self.cursor_development(),
                self.cursor_session_socket().as_deref(),
            ),
        })
    }

    pub(super) fn cursor_detail(
        &self,
        id: &str,
        state: &str,
        selected: bool,
        detail: String,
    ) -> String {
        if id != "cursor" {
            return detail;
        }
        if cfg!(windows) {
            format!("{detail} Cursor MCP setup is available; local Cursor session messaging hooks and per-chat attested MCP calls are unavailable on Windows.")
        } else if matches!(state, "configured" | "installed") {
            format!("{detail} Cursor messaging hooks are installed, and Cursor reaches Agent Relay's MCP through agent-relay-probe, which attests each call per chat once Agent Relay Cloud supports it. Each new Cursor chat is also told the agent-relay-probe commands for its relay identity. Reload Cursor or start a new chat, and approve the MCP connection if Cursor asks.")
        } else if state == "action_required" && selected && !self.cursor_hooks_ready() {
            format!("{detail} Cursor messaging hooks are off or could not be installed. Repair reconnects the hooks and Agent Relay MCP configuration.")
        } else {
            detail
        }
    }

    /// The probe, which Cursor's `mcp.json` entry runs as `cursor-mcp`.
    pub(crate) fn cursor_helper(&self) -> PathBuf {
        self.helper.probe().to_path_buf()
    }

    /// The session socket a site profile build's Cursor helper names.
    pub(crate) fn cursor_session_socket(&self) -> Option<PathBuf> {
        self.helper.session_socket().map(Path::to_path_buf)
    }

    /// Where status reads learn whether Cloud verifies Cursor chat proofs.
    pub fn set_cursor_attestation_source(&self, verified: super::claude_hook::Verified) {
        *self
            .cursor_attestation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(verified);
    }

    /// Cloud verifies Cursor chat proofs for the current account; false
    /// until the core has said where to ask.
    pub fn cursor_attestation(&self) -> bool {
        let verified = self
            .cursor_attestation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        verified.is_some_and(|verified| verified())
    }

    /// Cursor's chats call the hosted MCP with a signed per-call proof: the
    /// helper and the hooks are this build's and Cloud verifies the proofs.
    pub fn cursor_calls_attested(&self) -> bool {
        self.cursor_attested_transport() && self.cursor_attestation()
    }

    pub(crate) fn cursor_development(&self) -> bool {
        self.helper.development_scope()
    }

    pub(crate) fn cursor_config(&self) -> CursorConfig {
        CursorConfig::from_env(&|_| None, &self.home).development(self.helper.development_scope())
    }

    fn cursor_settings(&self) -> CursorSettings {
        CursorSettings::new(&self.cursor_config(), self.helper.cursor_hook())
    }

    /// This build's own `cursor-guide-hook` entries (`cursor_guide`).
    fn cursor_guide(&self) -> Result<GuideHooks, String> {
        self.helper
            .cursor_guide_hook()
            .and_then(|(command, socket)| GuideHooks::new(&self.cursor_config(), command, socket))
            .ok_or_else(|| "Agent Relay's command-line helper path cannot be written into Cursor hooks.".into())
    }

    /// The core's messaging hooks for this build are on.
    fn cursor_delivery_on(&self) -> bool {
        self.cursor_settings().get().get("cursor_direct_delivery") == Some(&Value::Bool(true))
    }

    pub(super) fn cursor_hooks_ready(&self) -> bool {
        cfg!(windows)
            || (self.helper.usable()
                && self.cursor_delivery_on()
                && self.cursor_guide().is_ok_and(|guide| guide.ready()))
    }

    /// Maintenance: where this build's messaging hooks are on, brings its
    /// guide entries up to date, so an install made before them gets them
    /// without a repair. An explicit Off has taken the messaging hooks out,
    /// so it stays off. The error is why the entries could not be added.
    pub(super) fn refresh_cursor_guide(&self) -> Result<(), String> {
        if cfg!(windows) || !self.helper.usable() || !self.cursor_delivery_on() {
            return Ok(());
        }
        if self.cursor_guide().is_ok_and(|guide| guide.ready()) {
            return Ok(());
        }
        self.set_cursor_hooks(true)
    }

    pub(super) fn set_cursor_hooks(&self, enabled: bool) -> Result<(), String> {
        // MCP is available on Windows; its local messaging hooks are not.
        if cfg!(windows) {
            return Ok(());
        }
        let directory = self.home.join(".cursor");
        if self.helper.development_scope()
            && (!development::safe_directory(&self.home, &directory)
                || fs::symlink_metadata(directory.join("hooks.json"))
                    .is_ok_and(|m| m.file_type().is_symlink()))
        {
            return Err("Development Cursor hooks are outside the private provider profile".into());
        }
        if enabled && (!self.helper.ensure() || !self.helper.usable()) {
            return Err("Cursor hook setup is waiting for Agent Relay’s command-line helper. Repair this provider after it is installed.".into());
        }
        if enabled {
            let guide = self.cursor_guide()?;
            self.cursor_settings().set_enabled(true)?;
            return guide.set(true);
        }
        // Both go, even when one of them cannot.
        let delivery = self.cursor_settings().set_enabled(false);
        let guide = self.cursor_guide().and_then(|guide| guide.set(false));
        match (delivery, guide) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(first), Err(second)) => Err(format!("{first}; {second}")),
        }
    }

    pub(super) fn request_cursor_hooks(&self) {
        let result = self.set_cursor_hooks(true);
        let mut failures = self
            .failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        failures.remove("cursor");
        if let Err(error) = result {
            failures.insert("cursor".into(), error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn fixture(home: &Path) -> (Installer, Binding) {
        let command = crate::probe_copy::destination(home);
        fs::create_dir_all(command.parent().unwrap()).unwrap();
        fs::write(&command, "synthetic executable").unwrap();
        private_fs::set_mode(&command, 0o700).unwrap();
        let app = Installer::new(
            home.into(),
            CodexConfig::from_env(&|_| None, home),
            ClaudeConfig::resolve(&|_| None, home, &[]),
            OpenCodeConfig::from_env(&|_| None, home),
        );
        let binding = Binding {
            site_url: "https://agentrelay.com".into(),
            workspace_id: "workspace".into(),
            account_id: "alice".into(),
        };
        (app, binding)
    }

    #[test]
    fn cursor_selection_installs_scoped_mcp_and_preserves_other_servers_and_hooks() {
        let root = tempfile::tempdir().unwrap();
        let (app, binding) = fixture(root.path());
        let profile = root.path().join(".cursor");
        fs::create_dir_all(&profile).unwrap();
        fs::write(
            profile.join("mcp.json"),
            "{ // keep this comment\n\"mcpServers\":{\"other\":{\"command\":\"other-server\"}}}",
        )
        .unwrap();
        fs::write(
            profile.join("hooks.json"),
            r#"{"version":1,"hooks":{"stop":[{"command":"other-hook"}]}}"#,
        )
        .unwrap();
        assert!(
            !profile.join("projects").exists(),
            "setup does not require a first chat"
        );
        app.select(vec!["cursor".into()], Some(&binding), None)
            .unwrap();
        app.set_provider_credentials(
            &binding,
            BTreeMap::from([("cursor".into(), "cursor-fixture-grant".into())]),
        );
        assert!(app.refresh(Some(&binding)).is_empty());
        let installed = fs::read_to_string(profile.join("mcp.json")).unwrap();
        let mcp: Value = json5::from_str(&installed).unwrap();
        assert!(installed.contains("keep this comment"));
        assert_eq!(mcp["mcpServers"]["other"]["command"], "other-server");
        assert_cursor_entry(&app, &binding, &mcp["mcpServers"][NAME]);
        let report = app.report(Some(&binding), Some("ready".into()));
        let cursor = report
            .tools
            .iter()
            .find(|tool| tool.id == "cursor")
            .unwrap();
        assert!(cursor.supported && cursor.selected && cursor.can_uninstall);
        assert_eq!(cursor.state, "installed");
        if cfg!(windows) {
            assert!(cursor.detail.contains("unavailable on Windows"));
        } else {
            let hooks: Value =
                serde_json::from_str(&fs::read_to_string(profile.join("hooks.json")).unwrap())
                    .unwrap();
            for event in ["stop", "postToolUse", "beforeSubmitPrompt"] {
                assert!(hooks["hooks"][event]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|hook| hook["command"]
                        .as_str()
                        .is_some_and(|s| s.contains("cursor-hook"))));
            }
            for event in crate::cursor_guide::EVENTS {
                assert_eq!(guide_hooks(&hooks, event), 1, "{event}");
            }
            assert_eq!(hooks["hooks"]["stop"][0]["command"], "other-hook");
            assert!(app.cursor_hooks_ready());
        }
        app.manage("cursor", "uninstall", Some(&binding), None)
            .unwrap();
        let mcp: Value =
            json5::from_str(&fs::read_to_string(profile.join("mcp.json")).unwrap()).unwrap();
        assert!(mcp["mcpServers"].get(NAME).is_none());
        assert_eq!(mcp["mcpServers"]["other"]["command"], "other-server");
        let hooks = fs::read_to_string(profile.join("hooks.json")).unwrap();
        assert!(hooks.contains("other-hook"));
        assert!(!hooks.contains("cursor-hook"));
        assert!(!hooks.contains("cursor-guide-hook"));
    }

    const ENDPOINT: &str = "https://agentrelay.com/cloud/api/v1/mcp/shared-sessions";

    /// Cursor's entry runs this build's `cursor-mcp` helper and holds no
    /// grant; Windows, without Cursor's hooks, keeps the URL entry.
    fn assert_cursor_entry(app: &Installer, binding: &Binding, entry: &Value) {
        if cfg!(windows) {
            assert_eq!(entry["url"], ENDPOINT);
            assert_eq!(
                entry["headers"]["Authorization"],
                "Bearer cursor-fixture-grant"
            );
            return;
        }
        assert_eq!(
            entry,
            &json!({
                "command": app.cursor_helper(),
                "args": crate::cursor_dispatch::arguments(binding, false, None),
            })
        );
        assert!(!entry.to_string().contains("cursor-fixture-grant"));
    }

    #[cfg(not(windows))]
    #[test]
    fn a_url_entry_migrates_to_the_helper_and_the_helper_is_what_forwards() {
        let root = tempfile::tempdir().unwrap();
        let (app, binding) = fixture(root.path());
        let profile = root.path().join(".cursor");
        fs::create_dir_all(&profile).unwrap();
        fs::write(
            profile.join("mcp.json"),
            json!({"mcpServers": {
                NAME: {"url": ENDPOINT, "headers": {"Authorization": "Bearer cursor-fixture-grant"}},
                "other": {"command": "other-server"},
            }})
            .to_string(),
        )
        .unwrap();
        app.select(vec!["cursor".into()], Some(&binding), None)
            .unwrap();
        app.set_provider_credentials(
            &binding,
            BTreeMap::from([("cursor".into(), "cursor-fixture-grant".into())]),
        );
        app.refresh(Some(&binding));
        let mcp_path = profile.join("mcp.json");
        let migrated: Value = json5::from_str(&fs::read_to_string(&mcp_path).unwrap()).unwrap();
        assert_cursor_entry(&app, &binding, &migrated["mcpServers"][NAME]);
        assert_eq!(migrated["mcpServers"]["other"]["command"], "other-server");
        let hosted = app.cursor_dispatch_hosted(Some(&binding)).unwrap();
        assert_eq!(hosted.endpoint, ENDPOINT);
        assert!(app.cursor_attested_transport());
        assert!(
            !app.cursor_calls_attested(),
            "Cloud has not said it verifies proofs"
        );
        app.set_cursor_attestation_source(std::sync::Arc::new(|| true));
        assert!(app.cursor_calls_attested());

        // A hand-edited entry is no longer this build's: nothing forwards.
        fs::write(
            &mcp_path,
            json!({"mcpServers": {NAME: {
                "command": app.cursor_helper(),
                "args": ["cursor-mcp", "--site", "https://agentrelay.com", "--workspace",
                    "workspace", "--account", "mallory"],
            }}})
            .to_string(),
        )
        .unwrap();
        assert!(app.cursor_dispatch_hosted(Some(&binding)).is_none());
        assert!(!app.cursor_calls_attested());
    }

    /// How many of this build's guide entries `event` holds.
    fn guide_hooks(hooks: &Value, event: &str) -> usize {
        hooks["hooks"][event].as_array().map_or(0, |entries| {
            entries
                .iter()
                .filter(|hook| hook["command"].as_str().is_some_and(|s| s.contains(" cursor-guide-hook")))
                .count()
        })
    }

    fn hooks_file(root: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(root.join(".cursor/hooks.json")).unwrap_or_default())
            .unwrap_or_default()
    }

    #[cfg(not(windows))]
    #[test]
    fn refresh_respects_hooks_off_until_explicit_repair() {
        let root = tempfile::tempdir().unwrap();
        let (app, binding) = fixture(root.path());
        app.select(vec!["cursor".into()], Some(&binding), None)
            .unwrap();
        app.set_provider_credentials(
            &binding,
            BTreeMap::from([("cursor".into(), "cursor-fixture-grant".into())]),
        );
        app.refresh(Some(&binding));
        let guide = &crate::cursor_guide::EVENTS;
        assert!(guide.iter().all(|event| guide_hooks(&hooks_file(root.path()), event) == 1));
        app.set_cursor_hooks(false).unwrap();
        assert!(guide.iter().all(|event| guide_hooks(&hooks_file(root.path()), event) == 0));
        app.select(vec!["cursor".into()], Some(&binding), None)
            .unwrap();
        app.refresh(Some(&binding));
        assert!(
            !app.cursor_hooks_ready(),
            "maintenance and repeated choices keep an explicit Off"
        );
        assert!(guide.iter().all(|event| guide_hooks(&hooks_file(root.path()), event) == 0));
        assert_ne!(
            app.report(Some(&binding), None)
                .tools
                .into_iter()
                .find(|t| t.id == "cursor")
                .unwrap()
                .state,
            "configured"
        );
        let mcp_path = root.path().join(".cursor/mcp.json");
        fs::write(
            &mcp_path,
            r#"{"mcpServers":{"other":{"command":"other-server"}}}"#,
        )
        .unwrap();
        app.manage("cursor", "repair", Some(&binding), None)
            .unwrap();
        assert!(app.cursor_hooks_ready());
        assert!(guide.iter().all(|event| guide_hooks(&hooks_file(root.path()), event) == 1));
        let repaired: Value = json5::from_str(&fs::read_to_string(mcp_path).unwrap()).unwrap();
        assert_cursor_entry(&app, &binding, &repaired["mcpServers"][NAME]);
        assert_eq!(repaired["mcpServers"]["other"]["command"], "other-server");
    }

    #[cfg(not(windows))]
    #[test]
    fn maintenance_adds_the_guide_to_an_install_with_only_the_messaging_hooks() {
        let root = tempfile::tempdir().unwrap();
        let (app, binding) = fixture(root.path());
        app.select(vec!["cursor".into()], Some(&binding), None)
            .unwrap();
        app.set_provider_credentials(
            &binding,
            BTreeMap::from([("cursor".into(), "cursor-fixture-grant".into())]),
        );
        app.refresh(Some(&binding));
        app.cursor_guide().unwrap().set(false).unwrap();
        let hooks = hooks_file(root.path());
        let guide = &crate::cursor_guide::EVENTS;
        assert!(guide.iter().all(|event| guide_hooks(&hooks, event) == 0));
        assert!(hooks["hooks"]["stop"]
            .as_array()
            .unwrap()
            .iter()
            .any(|hook| hook["command"]
                .as_str()
                .is_some_and(|s| s.contains(" cursor-hook"))));
        assert!(!app.cursor_hooks_ready());
        assert!(app.refresh(Some(&binding)).is_empty());
        assert!(guide
            .iter()
            .all(|event| guide_hooks(&hooks_file(root.path()), event) == 1));
        assert!(app.cursor_hooks_ready());
    }

    #[cfg(not(windows))]
    #[test]
    fn maintenance_warns_when_the_guide_cannot_be_added() {
        let root = tempfile::tempdir().unwrap();
        let (app, binding) = fixture(root.path());
        app.select(vec!["cursor".into()], Some(&binding), None)
            .unwrap();
        app.set_provider_credentials(
            &binding,
            BTreeMap::from([("cursor".into(), "cursor-fixture-grant".into())]),
        );
        app.refresh(Some(&binding));
        let path = root.path().join(".cursor/hooks.json");
        let mut hooks = hooks_file(root.path());
        hooks["hooks"]["sessionStart"] = json!({"command": "not a list"});
        fs::write(&path, serde_json::to_vec(&hooks).unwrap()).unwrap();
        assert!(!app.cursor_hooks_ready());
        let warnings = app.refresh(Some(&binding));
        assert!(
            warnings
                .iter()
                .any(|warning| warning.starts_with("cursor: ") && warning.contains("sessionStart")),
            "{warnings:?}"
        );
        let cursor = app
            .report(Some(&binding), None)
            .tools
            .into_iter()
            .find(|tool| tool.id == "cursor")
            .unwrap();
        assert_eq!(cursor.state, "action_required");
        assert_eq!(
            hooks_file(root.path())["hooks"]["sessionStart"],
            json!({"command": "not a list"})
        );
    }
}
