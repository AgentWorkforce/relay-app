//! The provider setup action that prefills a new Capy thread. The Mac's
//! `CapySetup` is the reference, word for word.

pub const CAPY_SETUP_TITLE: &str = "Capy detected";
pub const CAPY_SETUP_MESSAGE: &str = "Put your Capy threads on Agent Relay so other agents can message them and their conversations appear in Sessions. Capy asks you to approve the Agent Relay plugin once.";
pub const CAPY_SETUP_ACTION: &str = "Set up in Capy";
pub const CAPY_SETUP_NOT_NOW: &str = "Not now";
/// Prefilled in the new thread, and copied as a fallback.
pub const CAPY_SETUP_PROMPT: &str = "Set me up on Agent Relay: https://github.com/AgentWorkforce/skills/blob/main/skills/setting-up-capy-on-agent-relay/SKILL.md";
pub const CAPY_SETUP_COPIED: &str = "Setup prompt copied. Send it in Capy to begin.";
pub const CAPY_URL: &str = "https://capy.ai/new?prompt=Set%20me%20up%20on%20Agent%20Relay%3A%20https%3A%2F%2Fgithub.com%2FAgentWorkforce%2Fskills%2Fblob%2Fmain%2Fskills%2Fsetting-up-capy-on-agent-relay%2FSKILL.md";

/// The card shows while Capy is detected and the person has not chosen
/// Not now.
pub fn capy_setup_shows(installed: bool, dismissed: bool) -> bool {
    installed && !dismissed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capy_setup_shows_only_while_detected_and_not_dismissed() {
        assert!(capy_setup_shows(true, false));
        assert!(!capy_setup_shows(true, true));
        assert!(!capy_setup_shows(false, false));
    }

    #[test]
    fn capy_setup_wording_matches_the_mac() {
        assert_eq!(CAPY_SETUP_TITLE, "Capy detected");
        assert_eq!(
            CAPY_SETUP_MESSAGE,
            "Put your Capy threads on Agent Relay so other agents can message them and their conversations appear in Sessions. Capy asks you to approve the Agent Relay plugin once."
        );
        assert_eq!(CAPY_SETUP_ACTION, "Set up in Capy");
        assert_eq!(CAPY_SETUP_NOT_NOW, "Not now");
        assert_eq!(
            CAPY_SETUP_PROMPT,
            "Set me up on Agent Relay: https://github.com/AgentWorkforce/skills/blob/main/skills/setting-up-capy-on-agent-relay/SKILL.md"
        );
        assert_eq!(
            CAPY_SETUP_COPIED,
            "Setup prompt copied. Send it in Capy to begin."
        );
        assert_eq!(CAPY_URL, "https://capy.ai/new?prompt=Set%20me%20up%20on%20Agent%20Relay%3A%20https%3A%2F%2Fgithub.com%2FAgentWorkforce%2Fskills%2Fblob%2Fmain%2Fskills%2Fsetting-up-capy-on-agent-relay%2FSKILL.md");
    }
}
