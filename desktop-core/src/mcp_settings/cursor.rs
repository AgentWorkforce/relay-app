//! Cursor's existing messaging hooks share the provider installer lifecycle.
use super::*;
use agent_relay_node::relay::agents::SettingsExtension;
use agent_relay_node::relay::cursor_cli::{CursorConfig, CursorSettings};

impl Installer {
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
            format!("{detail} Cursor MCP setup is available; local Cursor session messaging hooks are unavailable on Windows.")
        } else if matches!(state, "configured" | "installed") {
            format!("{detail} Cursor messaging hooks are installed. Reload Cursor or start a new chat, and approve the MCP connection if Cursor asks.")
        } else if state == "action_required" && selected && !self.cursor_hooks_ready() {
            format!("{detail} Cursor messaging hooks are off or could not be installed. Repair reconnects the hooks and Agent Relay MCP configuration.")
        } else {
            detail
        }
    }

    fn cursor_settings(&self) -> CursorSettings {
        let config = CursorConfig::from_env(&|_| None, &self.home)
            .development(self.helper.development_scope());
        CursorSettings::new(&config, self.helper.cursor_hook())
    }

    pub(super) fn cursor_hooks_ready(&self) -> bool {
        cfg!(windows)
            || (self.helper.usable()
                && self.cursor_settings().get().get("cursor_direct_delivery")
                    == Some(&Value::Bool(true)))
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
        self.cursor_settings().set_enabled(enabled)
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
        assert_eq!(
            mcp["mcpServers"][NAME]["url"],
            "https://agentrelay.com/cloud/api/v1/mcp/shared-sessions"
        );
        assert_eq!(
            mcp["mcpServers"][NAME]["headers"]["Authorization"],
            "Bearer cursor-fixture-grant"
        );
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
        app.set_cursor_hooks(false).unwrap();
        app.select(vec!["cursor".into()], Some(&binding), None)
            .unwrap();
        app.refresh(Some(&binding));
        assert!(
            !app.cursor_hooks_ready(),
            "maintenance and repeated choices keep an explicit Off"
        );
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
        let repaired: Value = json5::from_str(&fs::read_to_string(mcp_path).unwrap()).unwrap();
        assert_eq!(
            repaired["mcpServers"][NAME]["url"],
            "https://agentrelay.com/cloud/api/v1/mcp/shared-sessions"
        );
        assert_eq!(
            repaired["mcpServers"][NAME]["headers"]["Authorization"],
            "Bearer cursor-fixture-grant"
        );
        assert_eq!(repaired["mcpServers"]["other"]["command"], "other-server");
    }
}
