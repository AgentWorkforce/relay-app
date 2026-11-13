//! Existing host and cloud integrations are catalogued separately from local
//! shared-login MCP installation. Data-directory presence is never readiness.
use super::*;

pub(super) struct Setup {
    pub label: &'static str,
    pub detail: &'static str,
    pub detected: bool,
}

/// Host choices enable their providers through the normal installer lifecycle.
/// The host itself still has no separate MCP installation.
pub(super) fn host_providers(id: &str) -> &'static [&'static str] {
    match id {
        "conductor" => &["codex", "claude"],
        "paseo" => &["codex", "claude", "grok", "muse"],
        _ => &[],
    }
}

fn capy_installed(home: &Path, applications: &Path) -> bool {
    home.join(".capy").is_dir() || capy_application_present(home, applications)
}

/// Initial onboarding needs an installed application, not a paired computer's
/// retained data or an account connection. Keep the existing detection above
/// for history/presence and explicit provider settings.
pub(super) fn onboarding_available(id: &str, home: &Path, applications: &Path) -> Option<bool> {
    match id {
        "capy" => Some(capy_application_present(home, applications)),
        // Amp has only the supported Cloud handoff, no local install witness.
        "amp" => Some(false),
        _ => None,
    }
}

fn capy_application_present(home: &Path, applications: &Path) -> bool {
    cfg!(target_os = "macos")
        && [applications.join("Capy.app"), home.join("Applications/Capy.app")]
            .iter()
            .any(|path| path.is_dir())
}

pub(super) fn cursor_installed(home: &Path) -> bool {
    home.join(".cursor").is_dir()
        || (cfg!(target_os = "macos") && [Path::new("/Applications/Cursor.app").to_path_buf(), home.join("Applications/Cursor.app")].iter().any(|path| path.is_dir()))
}

pub(super) fn setup(id: &str, home: &Path) -> Option<Setup> {
    let (label, detail, detected) = match id {
        "conductor" => (
            "Uses Codex or Claude Code",
            if cfg!(target_os = "macos") {
                "Selecting Conductor enables Agent Relay for installed Codex and Claude Code providers. Conductor uses those providers’ settings; running chats may need a reload or a new session."
            } else {
                "Automatic desktop discovery is available on macOS. Conductor hosts Codex and Claude Code chats; configure those underlying provider integrations. This view does not install separate Conductor MCP or verify its live chats on this platform."
            },
            cfg!(target_os = "macos") && home.join("Library/Application Support/com.conductor.app").is_dir(),
        ),
        "paseo" => (
            "Uses its coding providers",
            "Selecting Paseo enables Agent Relay for installed Codex, Claude Code, Grok, and Muse Code providers. Paseo uses those providers’ settings; running chats may need a reload or a new session.",
            home.join(".paseo").is_dir(),
        ),
        "capy" => (
            "Cloud integration",
            "Capy connects cloud threads through Agent Relay Cloud. Use Set up in Capy to configure its connection. No local MCP installation is required; Capy's connection is not verified here.",
            capy_installed(home, Path::new("/Applications")),
        ),
        "grok-bot" => (
            "Cloud integration",
            "Connect a Grok Bot to Agent Relay. Set up in Grok Bot opens the app with the setup skill in your Bot’s input when available. Send the prompt to begin; its connection is not verified here.",
            cfg!(target_os = "macos") && [Path::new("/Applications/Grok Bot.app").to_path_buf(), home.join("Applications/Grok Bot.app")].iter().any(|path| path.is_dir()),
        ),
        "muse-assistant" => (
            "Personal assistant",
            "Copy the setup prompt below, open Muse, then paste and send it in a chat. The skill recommends scheduled inbox polling. Its connection and automatic history sync have not been verified here.",
            cfg!(target_os = "macos") && [Path::new("/Applications/Muse.app").to_path_buf(), home.join("Applications/Muse.app")].iter().any(|path| path.is_dir()),
        ),
        "dot" => (
            "Cloud integration",
            "Connect Dot to Agent Relay with our plugin. Open the plugin page to add it and approve the connection. You can set this up whether or not the app is installed; its connection is not verified here.",
            cfg!(target_os = "macos") && [Path::new("/Applications/Dot.app").to_path_buf(), home.join("Applications/Dot.app")].iter().any(|path| path.is_dir()),
        ),
        "replicas" => (
            "Cloud integration",
            "Replicas launches and messaging are managed through Agent Relay Cloud. Local MCP installation is not used, and its cloud connection is not verified here.",
            false,
        ),
        "amp" => (
            "Connects through Agent Relay Cloud",
            "Amp connects through Agent Relay Cloud, in your workspace's Settings under Connections. Open Cloud connections to add or change it there. Agent Relay Desktop never asks for or stores the Amp token, and Amp's connection is not verified here.",
            false,
        ),
        "claude-cloud" => (
            "Claude cloud integration",
            "Claude cloud sessions use the Claude account and the existing cloud-session sharing preference, separately from local Claude Code MCP setup. This desktop does not install MCP into cloud sessions or verify their connection here.",
            false,
        ),
        _ => return None,
    };
    Some(Setup {
        label,
        detail,
        detected,
    })
}

/// Hosts reuse their coding providers; selection stays the person's saved choice.
pub(crate) fn derive_host_connections(tools: &mut [crate::contract::tool_integrations::ToolIntegration], health: Option<&str>) {
    for host in ["conductor", "paseo"] {
        let ready = tools.iter().filter(|tool| host_providers(host).contains(&tool.id.as_str())
            && tool.selected && tool.state == "configured" && health == Some("ready"))
            .map(|tool| tool.name.clone()).collect::<Vec<_>>();
        let Some(tool) = tools.iter_mut().find(|tool| tool.id == host) else { continue };
        if !tool.selected { continue; }
        if !ready.is_empty() {
            tool.state = "configured".into(); tool.status_label = "Connected".into();
            tool.detail = format!("Connected through {}. Running chats may need a reload or a new session to load Agent Relay.", ready.join(" and "));
        } else {
            tool.state = "action_required".into(); tool.status_label = "Connect a coding provider".into();
            tool.detail = "Connect one of the coding providers below to use Agent Relay in this app.".into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_setup_handoffs_do_not_install_local_mcp_or_require_detection() {
        let root = tempfile::tempdir().unwrap();
        let app = installer(root.path());
        let report = app.report(None, None);
        for id in ["grok-bot", "dot"] {
            let row = report.tools.iter().find(|tool| tool.id == id).unwrap();
            assert_eq!(row.state, "external_setup");
            assert!(!row.supported && !row.selected && !row.can_install && !row.can_repair && !row.can_uninstall);
            assert!(row.detail.contains("not verified"));
            assert!(app.select(vec![id.into()], None, None).is_err());
            for action in ["install", "repair", "reinstall", "uninstall"] {
                assert!(app.manage(id, action, None, None).is_err());
            }
        }
        assert!(!app.choices.exists());
    }

    fn installer(home: &Path) -> Installer {
        Installer::new(
            home.into(),
            CodexConfig::from_env(&|_| None, home),
            ClaudeConfig::resolve(&|_| None, home, &[]),
            OpenCodeConfig::from_env(&|_| None, home),
        )
    }

    #[test]
    fn chooser_retains_existing_external_visibility_without_installing_it() {
        let root = tempfile::tempdir().unwrap();
        let app = installer(root.path());
        assert!(app.select(vec!["replicas".into()], None, None).is_err());
        app.manage("replicas", "enable", None, None).unwrap();
        app.select(vec!["replicas".into()], None, None).unwrap();
        assert_eq!(app.selection().unwrap().unwrap(), vec!["replicas"]);
        assert!(!Installer::credential_provider("replicas"));
        assert!(!root.path().join(".claude.json").exists());
        app.select(vec![], None, None).unwrap();
        assert!(app.selection().unwrap().unwrap().is_empty());
    }

    #[test]
    fn hosts_use_ready_providers_and_preserve_explicit_off() {
        let root = tempfile::tempdir().unwrap();
        let app = installer(root.path());
        let mut tools = app.report(None, None).tools;
        for tool in &mut tools {
            if tool.id == "claude" { tool.selected = true; tool.state = "configured".into(); }
            if tool.id == "conductor" { tool.selected = true; }
        }
        derive_host_connections(&mut tools, Some("ready"));
        let host = tools.iter().find(|tool| tool.id == "conductor").unwrap();
        assert_eq!(host.state, "configured");
        assert!(host.detail.contains("Claude Code"));
        assert!(!host.supported, "hosts never install a separate MCP server");
        let off = tools.iter().find(|tool| tool.id == "paseo").unwrap();
        assert!(!off.selected);
        assert_eq!(off.state, "external_setup");
        derive_host_connections(&mut tools, Some("offline"));
        assert_ne!(tools.iter().find(|tool| tool.id == "conductor").unwrap().state, "configured");
    }

    #[test]
    fn amp_is_a_cloud_connection_with_no_local_setup() {
        let root = tempfile::tempdir().unwrap();
        let app = installer(root.path());
        let report = app.report(None, None);
        let row = report.tools.iter().find(|tool| tool.id == "amp").unwrap();
        assert_eq!(row.name, "Amp");
        assert_eq!(row.state, "external_setup");
        assert_eq!(row.status_label, "Connects through Agent Relay Cloud");
        assert!(row.detail.starts_with("Amp connects through Agent Relay Cloud"));
        assert!(row.detail.contains("never asks for or stores the Amp token"));
        assert!(!row.detected && !row.supported && !row.can_install && !row.can_repair && !row.can_uninstall);
        assert!(app.select(vec!["amp".into()], None, None).is_err());
        assert!(app.manage("amp", "install", None, None).is_err());
        assert!(!app.choices.exists());
    }

    #[test]
    fn cursor_uses_the_local_provider_installer() {
        let root = tempfile::tempdir().unwrap();
        assert!(setup("cursor", root.path()).is_none());
        assert!(Installer::supported_provider("cursor"));
    }

    #[test]
    fn existing_hosts_and_cloud_integrations_have_explicit_external_rows() {
        let root = tempfile::tempdir().unwrap();
        let app = installer(root.path());
        let report = app.report(None, None);
        assert!(!report.tools.iter().any(|row| row.id == "claude-cloud"));
        assert!(report.tools.iter().any(|row| row.id == "claude"));
        for id in ["conductor", "paseo", "capy", "replicas"] {
            let row = report
                .tools
                .iter()
                .find(|row| row.id == id)
                .expect("existing integration is catalogued");
            assert_eq!(row.state, "external_setup", "{id}");
            assert!(!row.supported && !row.selected, "{id}");
            assert_eq!(
                row.detected,
                id == "capy" && capy_installed(root.path(), Path::new("/Applications")),
                "{id}"
            );
            assert!(
                !row.can_install && !row.can_repair && !row.can_uninstall,
                "{id}"
            );
            if host_providers(id).is_empty() {
                assert!(
                    row.detail.contains("not verified") || row.detail.contains("does not"),
                    "{id}"
                );
                assert!(app.select(vec![id.into()], None, None).is_err());
            }
            for action in ["install", "repair", "reinstall", "uninstall"] {
                assert!(app.manage(id, action, None, None).is_err(), "{id} {action}");
            }
        }
        assert!(
            !app.choices.exists(),
            "external rows do not create local MCP choices"
        );
        assert!(!root.path().join(".claude.json").exists());
        assert!(!app.codex.home.join("config.toml").exists());
    }

    #[test]
    fn host_choices_save_the_host_and_providers_without_installing_until_refresh() {
        for host in ["conductor", "paseo"] {
            let root = tempfile::tempdir().unwrap();
            let app = installer(root.path());
            fs::create_dir_all(&app.codex.home).unwrap();
            fs::create_dir_all(&app.claude.home).unwrap();
            let binding = Binding {
                site_url: "https://agentrelay.com".into(),
                workspace_id: "workspace-a".into(),
                account_id: "account-a".into(),
            };
            let report = app
                .select(vec![host.into(), "codex".into()], Some(&binding), None)
                .unwrap();
            assert!(report.onboarding_complete);
            let mut expected: Vec<String> = vec!["codex".into(), "claude".into()];
            expected.push(host.into());
            expected.sort();
            assert_eq!(app.selection().unwrap().unwrap(), expected);
            for id in &expected {
                assert!(
                    report
                        .tools
                        .iter()
                        .find(|row| &row.id == id)
                        .unwrap()
                        .selected
                );
            }
            let host_row = report.tools.iter().find(|row| row.id == host).unwrap();
            assert_eq!(host_row.state, "action_required");
            assert_eq!(host_row.status_label, "Connect a coding provider");
            assert!(!host_row.can_install && !host_row.supported);
            assert!(!root.path().join(".claude.json").exists());
            assert!(!app.codex.home.join("config.toml").exists());
            app.manage(host, "disable", None, None).unwrap();
            assert!(!app
                .selection()
                .unwrap()
                .unwrap()
                .contains(&host.to_string()));
            assert_eq!(
                app.selection().unwrap().unwrap(),
                vec!["claude".to_string(), "codex".to_string()]
            );
            // Skipping or clearing choices removes both the host and providers.
            app.select(vec![], None, None).unwrap();
            assert_eq!(app.selection().unwrap().unwrap(), Vec::<String>::new());
            app.manage(host, "enable", Some(&binding), None).unwrap();
            assert_eq!(app.selection().unwrap().unwrap(), expected);
            assert!(!root.path().join(".claude.json").exists());
            assert!(!app.codex.home.join("config.toml").exists());
        }
    }

    #[test]
    fn enabling_another_host_never_restores_a_disconnected_provider() {
        let root = tempfile::tempdir().unwrap();
        let executable = root
            .path()
            .join(if cfg!(windows) { "grok.exe" } else { "grok" });
        fs::write(&executable, "fixture executable").unwrap();
        agent_relay_node::platform::fs::set_mode(&executable, 0o700).unwrap();
        let app = installer(root.path()).with_environment(&|key| match key {
            "RELAY_DESKTOP_GROK" => Some(executable.to_string_lossy().into_owned()),
            "RELAY_DESKTOP_MUSE" => Some(
                root.path()
                    .join("missing-muse")
                    .to_string_lossy()
                    .into_owned(),
            ),
            _ => None,
        });
        fs::create_dir_all(&app.codex.home).unwrap();
        fs::create_dir_all(&app.claude.home).unwrap();
        app.select(vec!["paseo".into()], None, None).unwrap();
        app.manage("grok", "uninstall", None, None).unwrap();
        let choices = app.selection().unwrap().unwrap();
        assert!(!choices.contains(&"grok".into()) && choices.contains(&"paseo".into()));
        // Enabling a host expands only that host, never other saved host ids.
        fs::write(&app.choices, r#"["paseo"]"#).unwrap();
        app.manage("conductor", "enable", None, None).unwrap();
        assert!(!app.selection().unwrap().unwrap().contains(&"grok".into()));
    }

    #[test]
    fn host_data_presence_never_claims_a_live_connection() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(
            root.path()
                .join("Library/Application Support/com.conductor.app"),
        )
        .unwrap();
        fs::create_dir_all(root.path().join(".paseo")).unwrap();
        let report = installer(root.path()).report(None, Some("ready".into()));
        for id in ["conductor", "paseo"] {
            let row = report.tools.iter().find(|row| row.id == id).unwrap();
            assert_eq!(row.detected, id != "conductor" || cfg!(target_os = "macos"));
            assert_eq!(row.state, "external_setup");
            assert!(!row.selected && !row.supported);
        }
        for id in ["replicas"] {
            assert!(
                !report
                    .tools
                    .iter()
                    .find(|row| row.id == id)
                    .unwrap()
                    .detected,
                "desktop login health cannot prove remote provider connection"
            );
        }
    }

    #[test]
    fn onboarding_requires_the_supported_capy_app_not_data_or_saved_choices() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let applications = root.path().join("Applications");
        fs::create_dir_all(home.join(".capy")).unwrap();
        assert!(capy_installed(&home, &applications), "retain data-presence semantics");
        assert!(!capy_application_present(&home, &applications));
        let app = installer(&home);
        app.manage("capy", "enable", None, None).unwrap();
        app.manage("amp", "enable", None, None).unwrap();
        let before = fs::read(&app.choices).unwrap();
        let report = app.report(None, None);
        for id in ["amp", "capy"] {
            let tool = report.tools.iter().find(|tool| tool.id == id).unwrap();
            assert!(tool.selected, "explicit saved selection stays intact");
            assert_eq!(onboarding_available(id, &home, &applications), Some(false));
        }
        assert_eq!(fs::read(&app.choices).unwrap(), before);
        assert!(home.join(".capy").is_dir(), "never delete retained provider data");
        for path in [applications.join("Capy.app"), home.join("Applications/Capy.app")] {
            fs::create_dir_all(&path).unwrap();
            assert_eq!(capy_application_present(&home, &applications), cfg!(target_os = "macos"));
            fs::remove_dir(&path).unwrap();
        }
        let local_app = home.join("Applications/Capy.app");
        fs::create_dir_all(&local_app).unwrap();
        let installed = app.report(None, None);
        let capy = installed.tools.iter().find(|tool| tool.id == "capy").unwrap();
        assert_eq!(capy.onboarding_available, Some(cfg!(target_os = "macos")),
            "the actual report publishes the supported local-app witness");
        fs::remove_dir(local_app).unwrap();
        assert_eq!(fs::read(&app.choices).unwrap(), before);
        let codex = report.tools.iter().find(|tool| tool.id == "codex").unwrap();
        assert_eq!(codex.onboarding_available, None, "other providers keep existing policy");
    }

    #[test]
    fn capy_installation_is_detected_without_claiming_cloud_readiness() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let applications = root.path().join("Applications");
        assert!(!capy_installed(&home, &applications));
        for location in [
            applications.join("Capy.app"),
            home.join("Applications/Capy.app"),
            home.join(".capy"),
        ] {
            fs::create_dir_all(&location).unwrap();
            assert_eq!(
                capy_installed(&home, &applications),
                cfg!(target_os = "macos") || location == home.join(".capy")
            );
            fs::remove_dir(&location).unwrap();
        }
        fs::create_dir_all(home.join(".capy")).unwrap();
        let report = installer(&home).report(None, Some("ready".into()));
        let capy = report.tools.iter().find(|row| row.id == "capy").unwrap();
        assert!(capy.detected);
        assert!(!capy.selected && !capy.supported && !capy.can_install);
        assert_eq!(capy.state, "external_setup");
    }
}
