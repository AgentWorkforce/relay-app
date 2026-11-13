//! External onboarding handoffs; keep these links and words aligned with the Mac.
use super::{CAPY_SETUP_ACTION, CAPY_SETUP_COPIED, CAPY_SETUP_PROMPT, CAPY_URL};
use agent_relay_node::relay::state::{AccountStatus, SignInState};
use agent_relay_desktop_core::contract::tool_integrations::ToolIntegration;

/// Authentication is complete before the initial local history scan finishes.
pub fn provider_onboarding_ready(account: &AccountStatus) -> bool {
    account.state == SignInState::SignedIn
        || (account.state == SignInState::Preparing && account.phase.as_deref() == Some("preparing_local_sessions"))
}

/// Initial onboarding uses the core's local-install witness for Amp/Capy.
/// Saved choices, Cloud connections and retained data are not installation.
pub fn provider_onboarding_present(tool: &ToolIntegration) -> bool {
    !matches!(tool.id.as_str(), "amp" | "capy")
        || tool.onboarding_available == Some(true)
}

pub fn provider_onboarding_detail<'a>(tool: &'a ToolIntegration, account: &AccountStatus) -> &'a str {
    if account.state == SignInState::Preparing && account.phase.as_deref() == Some("preparing_local_sessions") && tool.state == "signed_out" {
        "Choose this provider now. Setup will finish after your local sessions are prepared."
    } else {
        &tool.detail
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloudProviderSetup {
    Capy,
    GrokBot,
    Dot,
    MuseAssistant,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_amp_and_capy_catalog_actions_are_not_presence() {
        for id in ["amp", "capy"] {
            let absent = ToolIntegration {
                id: id.into(), name: id.into(), detected: false, onboarding_available: None, supported: true,
                selected: false, state: "external_setup".into(), status_label: "Setup".into(),
                detail: "Setup is available".into(), can_install: true, can_repair: true, can_uninstall: true,
            };
            assert!(!provider_onboarding_present(&absent), "setup capability is not detection");
            for evidence in ["detected", "selected", "configured"] {
                let mut present = absent.clone();
                present.detected = evidence == "detected";
                present.selected = evidence == "selected";
                present.state = if evidence == "configured" { "configured" } else { "external_setup" }.into();
                assert!(!provider_onboarding_present(&present), "{id} {evidence} is not installation");
                present.onboarding_available = Some(false);
                assert!(!provider_onboarding_present(&present));
                present.onboarding_available = Some(true);
                assert!(provider_onboarding_present(&present), "the core's installed witness admits the row");
            }
        }
    }

    #[test]
    fn provider_setup_does_not_wait_for_history_but_waits_for_authentication() {
        for (state, phase, ready) in [
            (SignInState::SignedIn, None, true),
            (SignInState::Preparing, Some("preparing_local_sessions"), true),
            (SignInState::Preparing, Some("connecting_workspace"), false),
            (SignInState::PendingApproval, None, false),
            (SignInState::SignedOut, Some("preparing_local_sessions"), false),
        ] {
            let account = AccountStatus { state, phase: phase.map(String::from), ..Default::default() };
            assert_eq!(provider_onboarding_ready(&account), ready);
        }
    }

    #[test]
    fn muse_copies_the_published_skill_without_an_invented_prefill_url() {
        let muse = CloudProviderSetup::for_id("muse-assistant").unwrap();
        assert!(muse.copy_only());
        let skill = muse.skill_url().unwrap();
        assert_eq!(skill, "https://github.com/AgentWorkforce/skills/blob/main/skills/agent-relay-muse/SKILL.md");
        assert!(muse.prompt().unwrap().contains(skill));
        assert!(muse.app_url().is_none());
        assert_eq!(muse.web_url(), "https://muse.ai/");
        assert!(muse.copied().is_some());
    }

    #[test]
    fn app_links_prefill_the_setup_skill_without_sending() {
        for (setup, scheme, path, key) in [
            (CloudProviderSetup::Capy, "capy", "/new", "prompt"),
            (CloudProviderSetup::GrokBot, "grokbot", "/v1/prompt", "text"),
        ] {
            let url = setup.app_url().unwrap();
            assert_eq!(url.scheme(), scheme);
            assert_eq!(url.host_str(), Some("app"));
            assert_eq!(url.path(), path);
            assert_eq!(url.query_pairs().collect::<Vec<_>>(), vec![(key.into(), setup.prompt().unwrap().into())]);
        }
        let dot = CloudProviderSetup::for_id("dot").unwrap();
        assert!(dot.app_url().is_none() && dot.prompt().is_none());
        assert_eq!(dot.web_url(), "https://chatgpt.com/plugins/plugin_asdk_app_6ad1e2038840819189295d49199c2915");
        assert!(CloudProviderSetup::for_id("grok").is_none(), "Grok CLI is separate");
    }
}

impl CloudProviderSetup {
    pub fn for_id(id: &str) -> Option<Self> {
        match id {
            "capy" => Some(Self::Capy),
            "grok-bot" => Some(Self::GrokBot),
            "dot" => Some(Self::Dot),
            "muse-assistant" => Some(Self::MuseAssistant),
            _ => None,
        }
    }

    pub fn action(self) -> &'static str {
        match self {
            Self::Capy => CAPY_SETUP_ACTION,
            Self::GrokBot => "Set up in Grok Bot",
            Self::Dot => "Open Agent Relay plugin",
            Self::MuseAssistant => "Copy setup prompt",
        }
    }

    pub fn copy_only(self) -> bool {
        self == Self::MuseAssistant
    }

    pub fn prompt(self) -> Option<&'static str> {
        match self {
            Self::Capy => Some(CAPY_SETUP_PROMPT),
            Self::GrokBot => Some("Set me up on Agent Relay: https://github.com/AgentWorkforce/skills/blob/main/skills/agent-relay-grok-bot/SKILL.md"),
            Self::MuseAssistant => Some("Set me up on Agent Relay: https://github.com/AgentWorkforce/skills/blob/main/skills/agent-relay-muse/SKILL.md"),
            Self::Dot => None,
        }
    }

    pub fn skill_url(self) -> Option<&'static str> {
        (self == Self::MuseAssistant).then_some("https://github.com/AgentWorkforce/skills/blob/main/skills/agent-relay-muse/SKILL.md")
    }

    pub fn web_url(self) -> &'static str {
        match self {
            Self::Capy => CAPY_URL,
            Self::GrokBot => "https://cursor.com/bot",
            Self::Dot => "https://chatgpt.com/plugins/plugin_asdk_app_6ad1e2038840819189295d49199c2915",
            Self::MuseAssistant => "https://muse.ai/",
        }
    }

    pub fn app_url(self) -> Option<url::Url> {
        let (base, key) = match self {
            Self::Capy => ("capy://app/new", "prompt"),
            Self::GrokBot => ("grokbot://app/v1/prompt", "text"),
            Self::Dot | Self::MuseAssistant => return None,
        };
        let mut url = url::Url::parse(base).ok()?;
        url.query_pairs_mut().append_pair(key, self.prompt()?);
        Some(url)
    }

    pub fn copied(self) -> Option<&'static str> {
        match self {
            Self::Capy => Some(CAPY_SETUP_COPIED),
            Self::GrokBot => Some("Setup prompt copied. Send it in Grok Bot to begin. If you don’t have a Bot yet, create one and paste the prompt."),
            Self::MuseAssistant => Some("Setup prompt copied. Open Muse, then paste and send it to begin."),
            Self::Dot => None,
        }
    }
}
