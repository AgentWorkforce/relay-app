//! Presentation of a provider's configuration and account verification.
use crate::ui::widgets::Color;
use agent_relay_desktop_core::contract::tool_integrations::ToolIntegration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::ui) enum ProviderStatus {
    On,
    Attention,
    Unverified,
    Off,
}

impl ProviderStatus {
    pub(in crate::ui) fn for_tool(tool: &ToolIntegration, health: Option<&str>) -> Self {
        // Cloud-connected providers, Amp included, are on once Agent Relay Cloud
        // reports them configured and the account is ready, selected or not.
        if matches!(tool.id.as_str(), "capy" | "grok-bot" | "dot" | "amp") && tool.state == "configured" && health == Some("ready") {
            return Self::On;
        }
        if tool.state == "external_setup"
            || tool.state == "checking"
            || (tool.selected && !tool.supported && tool.state == "unavailable")
        {
            return Self::Unverified;
        }
        if tool.selected && tool.state == "configured" && health == Some("ready") {
            Self::On
        } else if tool.selected && tool.state == "installed" {
            Self::Unverified
        } else if tool.selected {
            Self::Attention
        } else {
            Self::Off
        }
    }
    pub(in crate::ui) fn color(self) -> Color {
        match self {
            Self::On => Color::Green,
            Self::Attention => Color::Orange,
            Self::Off | Self::Unverified => Color::Grey,
        }
    }
    pub(in crate::ui) fn key(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Attention => "attention",
            Self::Unverified => "unverified",
            Self::Off => "off",
        }
    }
    pub(in crate::ui) fn detail(self) -> &'static str {
        match self {
            Self::On => {
                "On. Configuration and account health are verified. Running chats are not verified."
            }
            Self::Attention => "Needs attention. Running chats are not verified.",
            Self::Unverified => "Not yet verified. Running chats are not verified.",
            Self::Off => "Off. Running chats are not verified.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn green_requires_selection_configuration_and_verified_account() {
        let mut tool = ToolIntegration {
            id: "codex".into(),
            name: "Codex".into(),
            detected: true,
            onboarding_available: None,
            supported: true,
            selected: true,
            state: "configured".into(),
            status_label: "Configured".into(),
            detail: String::new(),
            can_install: false,
            can_repair: true,
            can_uninstall: true,
        };
        assert_eq!(
            ProviderStatus::for_tool(&tool, Some("ready")),
            ProviderStatus::On
        );
        assert_eq!(
            ProviderStatus::for_tool(&tool, Some("offline")),
            ProviderStatus::Attention
        );
        assert_eq!(
            ProviderStatus::for_tool(&tool, None),
            ProviderStatus::Attention
        );
        tool.state = "checking".into();
        let checking = ProviderStatus::for_tool(&tool, Some("ready"));
        assert_eq!(checking, ProviderStatus::Unverified);
        assert_eq!(checking.color(), Color::Grey);
        tool.state = "action_required".into();
        assert_eq!(
            ProviderStatus::for_tool(&tool, Some("ready")),
            ProviderStatus::Attention
        );
        tool.supported = false;
        tool.state = "unavailable".into();
        assert_eq!(
            ProviderStatus::for_tool(&tool, Some("ready")),
            ProviderStatus::Unverified
        );
        tool.state = "installed".into();
        let unverified = ProviderStatus::for_tool(&tool, Some("ready"));
        assert_eq!(unverified, ProviderStatus::Unverified);
        assert_eq!(unverified.color(), Color::Grey);
        assert!(unverified.detail().starts_with("Not yet verified."));
        tool.selected = false;
        tool.state = "external_setup".into();
        assert_eq!(
            ProviderStatus::for_tool(&tool, None),
            ProviderStatus::Unverified
        );
        tool.state = "installed".into();
        assert_eq!(
            ProviderStatus::for_tool(&tool, Some("ready")),
            ProviderStatus::Off
        );
        assert!(ProviderStatus::On
            .detail()
            .contains("Running chats are not verified."));
    }
}
