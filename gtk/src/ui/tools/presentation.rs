//! Concise provider setup guidance, separate from technical connection evidence.
//! The summary appears only when the status and action leave something unsaid.
use super::external::{is_external, AMP_CONNECTED, AMP_CONNECTS};
use agent_relay_desktop_core::contract::tool_integrations::ToolIntegration;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NextAction {
    Account,
    Check,
    Install,
    Enable,
    Repair,
}
impl NextAction {
    fn label(self, tool: &ToolIntegration) -> String {
        let name = &tool.name;
        match self {
            Self::Account => "Sign in to Agent Relay".to_string(),
            Self::Check => "Check connection".to_string(),
            Self::Install => format!("Connect {name}"),
            Self::Enable => format!("Enable {name}"),
            Self::Repair if !tool.selected => format!("Connect {name}"),
            Self::Repair => format!("Fix {name} connection"),
        }
    }
}
pub(super) struct Presentation {
    pub status: String,
    pub summary: Option<String>,
    pub action: Option<NextAction>,
    label: Option<String>,
}
impl Presentation {
    pub fn new(tool: &ToolIntegration, health: Option<&str>) -> Self {
        if let Some(presentation) = Self::connection_override(tool, health) {
            return presentation;
        }
        let (status, summary, action) = if is_external(&tool.id) {
            Self::external(tool)
        } else if !tool.supported {
            (
                "Setup not available yet",
                Some("Agent Relay hasn’t added automatic setup for this provider yet.".into()),
                None,
            )
        } else if tool.state == "checking" {
            (
                "Checking",
                Some("Agent Relay is checking setup. This will finish automatically.".into()),
                None,
            )
        } else if tool.selected
            && (tool.state == "signed_out" || health == Some("sign_in_required"))
        {
            ("Sign in required", None, Some(NextAction::Account))
        } else if tool.selected && health == Some("offline") {
            (
                "Can’t verify connection",
                Some("Check your internet connection, then try again.".into()),
                Some(NextAction::Check),
            )
        } else if !tool.selected && tool.state == "action_required" {
            (
                "Needs attention",
                Some("Agent Relay couldn’t finish changing this connection. Check Connection details and try again.".into()),
                None,
            )
        } else if !tool.selected {
            Self::unselected(tool)
        } else if tool.status_label == "Messaging unavailable" {
            ("Messaging unavailable", None, None)
        } else if tool.id == "codex" && tool.status_label == "MCP not ready" {
            (
                "Reload Codex chats",
                Some("Codex chats haven’t loaded Agent Relay. Reload or start a new Codex chat, then check the connection.".into()),
                Some(NextAction::Check),
            )
        } else if tool.state == "installed" && tool.status_label == "Connection not verified" {
            ("Connection not verified", Some(tool.detail.clone()), Some(NextAction::Check))
        } else if tool.state == "installed" {
            (
                "Setup saved",
                Some("Access from running chats hasn’t been verified yet.".into()),
                None,
            )
        } else if tool.state == "configured" && health == Some("ready") {
            ("Connected", (tool.status_label == "Connected").then(|| tool.detail.clone()), (tool.status_label == "Connected").then_some(NextAction::Check))
        } else {
            (
                "Needs attention",
                Some(tool.detail.clone()),
                tool.can_repair.then_some(NextAction::Repair),
            )
        };
        let label = action.map(|action| action.label(tool));
        Self {
            status: status.into(),
            summary,
            action,
            label,
        }
    }
    /// Remote connection proofs and provider-specific availability are authoritative.
    fn connection_override(tool: &ToolIntegration, health: Option<&str>) -> Option<Self> {
        if (matches!(tool.id.as_str(), "capy" | "grok-bot" | "dot") || tool.selected)
            && matches!(tool.id.as_str(), "capy" | "grok-bot" | "dot" | "conductor" | "paseo")
            && tool.state == "configured"
        {
            let action = matches!(tool.id.as_str(), "capy" | "grok-bot" | "dot").then_some(NextAction::Check);
            return Some(Self {
                status: if health == Some("ready") {
                    "Connected"
                } else {
                    "Connection not verified"
                }
                .into(),
                summary: Some(if health == Some("ready") {
                    tool.detail.clone()
                } else {
                    "Check your Agent Relay account connection, then check this provider again."
                        .into()
                }),
                label: action.map(|action| action.label(tool)),
                action,
            });
        }
        if matches!(tool.id.as_str(), "conductor" | "paseo")
            && tool.selected
            && tool.state == "action_required"
        {
            return Some(Self {
                status: tool.status_label.clone(),
                summary: Some(tool.detail.clone()),
                action: None,
                label: None,
            });
        }
        if matches!(tool.id.as_str(), "capy" | "grok-bot" | "dot") && tool.status_label != "Cloud integration" {
            return Some(Self {
                status: tool.status_label.clone(),
                summary: Some(tool.detail.clone()),
                action: None,
                label: None,
            });
        }
        if matches!(tool.id.as_str(), "cursor" | "copilot") && !tool.supported {
            return Some(Self {
                status: tool.status_label.clone(),
                summary: Some(tool.detail.clone()),
                action: None,
                label: None,
            });
        }
        None
    }

    fn external(tool: &ToolIntegration) -> (&'static str, Option<String>, Option<NextAction>) {
        let (status, summary) = match tool.id.as_str() {
            "conductor" => (
                "Uses Codex or Claude Code",
                "Manage the connection through the agent running inside Conductor.",
            ),
            "paseo" => (
                "Uses your coding agents",
                "Manage each agent’s connection below.",
            ),
            "capy" => (
                "Connect in Capy",
                "Add Agent Relay to your Capy threads to send messages and share sessions.",
            ),
            "grok-bot" | "dot" => ("Cloud integration", tool.detail.as_str()),
            "muse-assistant" => ("Personal assistant", tool.detail.as_str()),
            "amp" if tool.state == "configured" => (AMP_CONNECTED, tool.detail.as_str()),
            "amp" => (AMP_CONNECTS, tool.detail.as_str()),
            "replicas" => (
                "Managed in Agent Relay Cloud",
                "Replicas uses your cloud connection for launching agents and messaging.",
            ),
            _ => (
                "Uses your Claude account",
                "Manage cloud-session sharing in Claude Code settings.",
            ),
        };
        (
            status,
            Some(summary.into()),
            (matches!(tool.id.as_str(), "conductor" | "paseo") && !tool.selected)
                .then_some(NextAction::Enable),
        )
    }

    fn unselected(tool: &ToolIntegration) -> (&'static str, Option<String>, Option<NextAction>) {
        let summary = (!tool.detected)
            .then(|| format!("Install {} on this computer to connect it.", tool.name));
        let action = if tool.can_install {
            Some(NextAction::Install)
        } else if tool.can_repair {
            Some(NextAction::Repair)
        } else {
            None
        };
        ("Not connected", summary, action)
    }
    /// Names what the action connects or fixes.
    pub fn action_label(&self) -> Option<&str> {
        self.label.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detected_host_can_be_enabled_again_after_deselecting() {
        for id in ["conductor", "paseo"] {
            let mut tool = provider();
            tool.id = id.into();
            tool.name = id.into();
            tool.supported = false;
            tool.selected = false;
            assert_eq!(
                Presentation::new(&tool, Some("ready")).action,
                Some(NextAction::Enable)
            );
            tool.selected = true;
            assert_eq!(Presentation::new(&tool, Some("ready")).action, None);
        }
    }
    #[test]
    fn codex_runtime_failure_asks_for_reload_and_checks_instead_of_reinstalling() {
        let mut tool = provider();
        tool.state = "action_required".into();
        tool.status_label = "MCP not ready".into();
        let shown = Presentation::new(&tool, Some("ready"));
        assert_eq!(shown.status, "Reload Codex chats");
        assert_eq!(shown.action, Some(NextAction::Check));
        assert_eq!(shown.action_label(), Some("Check connection"));
        assert!(shown
            .summary
            .unwrap()
            .contains("Reload or start a new Codex chat"));
        assert_eq!(
            Presentation::new(&tool, Some("sign_in_required")).action,
            Some(NextAction::Account)
        );
    }
    #[test]
    fn missing_setup_is_an_agent_relay_gap_not_an_account_or_mcp_restriction() {
        for id in [
            "grok", "opencode", "devin", "muse", "pi", "openclaw", "hermes", "droid",
        ] {
            let mut tool = provider();
            tool.id = id.into();
            tool.selected = false;
            tool.supported = false;
            tool.state = "unavailable".into();
            for health in ["ready", "sign_in_required"] {
                let shown = Presentation::new(&tool, Some(health));
                assert_eq!(shown.status, "Setup not available yet");
                assert_eq!(
                    shown.summary.as_deref(),
                    Some("Agent Relay hasn’t added automatic setup for this provider yet.")
                );
                assert!(shown.action.is_none());
            }
        }
    }
    #[test]
    fn verified_external_connections_use_the_proof_without_local_selection() {
        let mut tool = provider();
        tool.id = "capy".into();
        tool.supported = false;
        tool.selected = false;
        let shown = Presentation::new(&tool, Some("ready"));
        assert_eq!(shown.status, "Connected");
        assert_eq!(shown.action, Some(NextAction::Check));
        assert_eq!(
            super::super::provider_status::ProviderStatus::for_tool(&tool, Some("ready")),
            super::super::provider_status::ProviderStatus::On
        );
        assert!(!tool.selected);
        tool.id = "grok-bot".into();
        assert_eq!(Presentation::new(&tool, Some("ready")).status, "Connected");
        assert_eq!(Presentation::new(&tool, Some("ready")).action, Some(NextAction::Check));
        assert_eq!(super::super::provider_status::ProviderStatus::for_tool(&tool, Some("ready")), super::super::provider_status::ProviderStatus::On);
        assert_eq!(Presentation::new(&tool, Some("offline")).status, "Connection not verified");
        tool.state = "external_setup".into();
        tool.status_label = "No active Grok Bot".into();
        assert_eq!(Presentation::new(&tool, Some("ready")).status, "No active Grok Bot");
        assert_ne!(super::super::provider_status::ProviderStatus::for_tool(&tool, Some("ready")), super::super::provider_status::ProviderStatus::On);
        tool.state = "configured".into();
        for host in ["conductor", "paseo"] {
            tool.id = host.into();
            tool.selected = true;
            assert_eq!(Presentation::new(&tool, Some("ready")).status, "Connected");
        }
    }

    #[test]
    fn cursor_preserves_core_availability() {
        let mut tool = provider();
        tool.id = "cursor".into();
        tool.supported = false;
        tool.status_label = "Unavailable on Windows".into();
        tool.detail = "Cursor session messaging is not available on Windows.".into();
        let shown = Presentation::new(&tool, Some("ready"));
        assert_eq!(shown.status, tool.status_label);
        assert_eq!(shown.summary.as_deref(), Some(tool.detail.as_str()));
        assert!(shown.action.is_none());
    }

    #[test]
    fn cursor_with_managed_setup_offers_install_and_repair() {
        let mut tool = provider();
        tool.id = "cursor".into();
        tool.selected = false;
        tool.state = "not_installed".into();
        tool.can_install = true;
        assert_eq!(Presentation::new(&tool, Some("ready")).action, Some(NextAction::Install));
        tool.selected = true;
        tool.state = "action_required".into();
        assert_eq!(Presentation::new(&tool, Some("ready")).action, Some(NextAction::Repair));
    }

    fn provider() -> ToolIntegration {
        serde_json::from_value(serde_json::json!({"id":"codex","name":"Codex","detected":true,"supported":true,"selected":true,"state":"configured","status_label":"Configured","detail":"Technical evidence","can_install":false,"can_repair":true,"can_uninstall":true})).unwrap()
    }
    #[test]
    fn next_action_respects_account_failure_before_configuration() {
        let mut tool = provider();
        tool.state = "action_required".into();
        assert_eq!(
            Presentation::new(&tool, Some("sign_in_required")).action,
            Some(NextAction::Account)
        );
        assert_eq!(
            Presentation::new(&tool, Some("offline")).action,
            Some(NextAction::Check)
        );
        assert_eq!(
            Presentation::new(&tool, Some("ready")).action_label(),
            Some("Fix Codex connection")
        );
        tool.selected = false;
        assert_eq!(Presentation::new(&tool, Some("ready")).action_label(), None);
        assert_eq!(
            Presentation::new(&tool, Some("ready")).status,
            "Needs attention"
        );
        assert_eq!(Presentation::new(&tool, Some("ready")).summary.as_deref(),
            Some("Agent Relay couldn’t finish changing this connection. Check Connection details and try again."));
        tool.state = "not_selected".into();
        assert_eq!(
            Presentation::new(&tool, Some("ready")).action_label(),
            Some("Connect Codex")
        );
    }
    #[test]
    fn actions_name_their_target_and_summaries_only_add_information() {
        let mut tool = provider();
        let connected = Presentation::new(&tool, Some("ready"));
        assert_eq!(connected.status, "Connected");
        assert_eq!(connected.summary, None);
        assert_eq!(
            Presentation::new(&tool, Some("sign_in_required")).action_label(),
            Some("Sign in to Agent Relay")
        );
        let offline = Presentation::new(&tool, Some("offline"));
        assert_eq!(offline.action_label(), Some("Check connection"));
        assert_eq!(
            offline.summary.as_deref(),
            Some("Check your internet connection, then try again.")
        );
        tool.selected = false;
        tool.can_install = true;
        let off = Presentation::new(&tool, Some("ready"));
        assert_eq!(off.status, "Not connected");
        assert_eq!(off.action_label(), Some("Connect Codex"));
        assert_eq!(off.summary, None);
        tool.detected = false;
        assert_eq!(
            Presentation::new(&tool, Some("ready")).summary.as_deref(),
            Some("Install Codex on this computer to connect it.")
        );
    }
    #[test]
    fn pending_helper_verification_is_neutral_without_a_repair_action() {
        let mut tool = provider();
        tool.state = "checking".into();
        for health in [None, Some("ready"), Some("offline")] {
            let shown = Presentation::new(&tool, health);
            assert_eq!(shown.status, "Checking");
            assert_eq!(
                shown.summary.as_deref(),
                Some("Agent Relay is checking setup. This will finish automatically.")
            );
            assert_eq!(shown.action, None);
            assert_eq!(shown.action_label(), None);
        }
    }
    #[test]
    fn saved_or_unavailable_messaging_does_not_offer_a_fake_repair() {
        let mut tool = provider();
        tool.state = "installed".into();
        let saved = Presentation::new(&tool, Some("ready"));
        assert_eq!(saved.status, "Setup saved");
        assert_eq!(
            saved.summary.as_deref(),
            Some("Access from running chats hasn’t been verified yet.")
        );
        assert_eq!(saved.action, None);
        tool.status_label = "Messaging unavailable".into();
        assert_eq!(
            Presentation::new(&tool, Some("ready")).status,
            "Messaging unavailable"
        );
        assert_eq!(Presentation::new(&tool, Some("ready")).action, None);
    }
    #[test]
    fn hosted_providers_do_not_claim_local_health_or_offer_local_repairs() {
        let mut tool = provider();
        tool.id = "replicas".into();
        tool.supported = false;
        let state = Presentation::new(&tool, Some("ready"));
        assert_eq!(state.status, "Managed in Agent Relay Cloud");
        assert_eq!(state.action, None);
    }
    #[test]
    fn amp_shows_its_cloud_connection_without_local_actions() {
        use super::super::provider_status::ProviderStatus;
        let mut tool = provider();
        tool.id = "amp".into();
        tool.name = "Amp".into();
        tool.supported = false;
        tool.detected = false;
        tool.selected = false;
        tool.state = "external_setup".into();
        tool.detail = "Amp connects through Agent Relay Cloud, in your workspace's Settings under Connections.".into();
        let shown = Presentation::new(&tool, Some("ready"));
        assert_eq!(shown.status, "Connects through Agent Relay Cloud");
        assert_eq!(shown.summary.as_deref(), Some(tool.detail.as_str()));
        assert_eq!(shown.action, None);
        assert_ne!(
            ProviderStatus::for_tool(&tool, Some("ready")),
            ProviderStatus::On
        );
        tool.state = "configured".into();
        let shown = Presentation::new(&tool, Some("ready"));
        assert_eq!(shown.status, "Connected in Agent Relay Cloud");
        // Amp's one action is Open Cloud connections, never Check connection.
        assert_eq!(shown.action, None);
        assert_eq!(
            ProviderStatus::for_tool(&tool, Some("ready")),
            ProviderStatus::On
        );
        assert_eq!(
            ProviderStatus::for_tool(&tool, Some("offline")),
            ProviderStatus::Off
        );
    }

    #[test]
    fn copilot_ready_setup_offers_connection_and_preserves_installed_limitations() {
        let mut tool = provider();
        tool.id = "copilot".into();
        tool.name = "GitHub Copilot (VS Code)".into();
        tool.supported = true;
        tool.selected = false;
        tool.detected = true;
        tool.can_install = true;
        tool.can_repair = false;
        tool.state = "onboarding_needed".into();
        tool.status_label = "Choose providers".into();
        tool.detail="GitHub Copilot needs VS Code with the GitHub Copilot Chat extension, signed in to GitHub inside VS Code. VS Code: detected at /usr/bin/code. GitHub Copilot Chat extension: detected at /home/person/.vscode/extensions/github.copilot-chat-0.72.0.".into();
        let ready = Presentation::new(&tool, Some("ready"));
        assert_eq!(ready.action, Some(NextAction::Install));
        assert_eq!(
            ready.action_label(),
            Some("Connect GitHub Copilot (VS Code)")
        );
        tool.selected = true;
        tool.can_install = false;
        tool.state = "installed".into();
        tool.status_label = "Setup saved".into();
        assert_eq!(
            Presentation::new(&tool, Some("ready")).status,
            "Setup saved"
        );
        tool.state = "action_required".into();
        tool.status_label = "Messaging unavailable".into();
        let unavailable = Presentation::new(&tool, Some("ready"));
        assert_eq!(unavailable.status, "Messaging unavailable");
        assert!(unavailable.action.is_none());
        tool.state = "configured".into();
        tool.status_label = "Connected".into();
        tool.detail="Copilot MCP configuration is installed. Local chat wake is unavailable on this platform.".into();
        let configured = Presentation::new(&tool, Some("ready"));
        assert_eq!(configured.status, "Connected");
        assert_eq!(configured.summary.as_deref(), Some(tool.detail.as_str()));
    }
}
