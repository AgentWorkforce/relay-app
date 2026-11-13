//! Teammate context in words: guidance Agent Relay adds to the start of
//! every session it reaches (Claude Code, Codex, Copilot, Devin) to check
//! teammates' session summaries first. The Mac's `TeammateContextCopy`
//! matches these word for word.

pub const TEAMMATE_CONTEXT_TITLE: &str = "Have agents check what teammates are working on";
pub const TEAMMATE_CONTEXT_DETAIL: &str =
    "At the start of each Claude Code, Codex, Copilot or Devin session, Agent Relay tells the agent to read teammates' session summaries before starting work.";
pub const TEAMMATE_CONTEXT_TURN_ON_HELP: &str =
    "Tell new agent sessions to check teammates' work first";
pub const TEAMMATE_CONTEXT_TURN_OFF_HELP: &str =
    "Stop telling new agent sessions to check teammates' work";

#[cfg(test)]
mod tests {
    use super::*;

    /// The Mac's wording, read from its source.
    const MAC: &str =
        include_str!("../../../macos/Sources/RelayDesktopCore/TeammateContextCopy.swift");

    fn mac(name: &str) -> &'static str {
        let declaration = format!("public static let {name} = \"");
        let start = MAC
            .find(&declaration)
            .unwrap_or_else(|| panic!("{name} in TeammateContextCopy.swift"))
            + declaration.len();
        let length = MAC[start..].find('"').unwrap();
        &MAC[start..start + length]
    }

    #[test]
    fn teammate_context_wording_matches_the_mac() {
        assert_eq!(TEAMMATE_CONTEXT_TITLE, mac("title"));
        assert_eq!(TEAMMATE_CONTEXT_DETAIL, mac("detail"));
        assert_eq!(TEAMMATE_CONTEXT_TURN_ON_HELP, mac("turnOnHelp"));
        assert_eq!(TEAMMATE_CONTEXT_TURN_OFF_HELP, mac("turnOffHelp"));
    }
}
