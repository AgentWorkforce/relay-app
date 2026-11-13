//! The teammate-context hook in words: a Claude Code SessionStart hook that
//! tells each session to check teammates' session summaries before it
//! starts. The Mac's `TeammateContextCopy` matches these word for word.

pub const TEAMMATE_CONTEXT_TITLE: &str = "Have agents check what teammates are working on";
pub const TEAMMATE_CONTEXT_DETAIL: &str =
    "Adds a session-start hook so Claude Code reads teammates' session summaries before starting work.";
pub const TEAMMATE_CONTEXT_TURN_ON_HELP: &str = "Add the teammate-context hook to Claude Code";
pub const TEAMMATE_CONTEXT_TURN_OFF_HELP: &str = "Remove the teammate-context hook from Claude Code";

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
