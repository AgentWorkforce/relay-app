//! Whether sessions are shown teammates' overlapping work as their turn
//! ends (desktop-core's `coordination_advisories.*`). Matches the Mac's
//! CoordinationAdvisoriesCopy word for word.

pub const COORDINATION_ADVISORIES_TITLE: &str =
    "Show teammates' overlapping work at the end of a turn";
pub const COORDINATION_ADVISORIES_DETAIL: &str = "Off: nothing is added. On: when a Claude Code, Codex, Copilot, Devin or Cursor CLI session ends a turn, it is shown a short note about teammates' work that overlaps with its own, once per note. Your workspace's Cloud must offer these notes.";

#[cfg(test)]
mod tests {
    use super::*;

    /// The Mac's copy, read from its source so the two cannot drift.
    const MAC_COPY: &str =
        include_str!("../../../macos/Sources/RelayDesktopCore/CoordinationAdvisoriesCopy.swift");

    #[test]
    fn the_copy_matches_the_mac_word_for_word() {
        assert!(
            MAC_COPY.contains(&format!(
                "public static let title = \"{COORDINATION_ADVISORIES_TITLE}\"\n"
            )),
            "the Mac's title differs"
        );
        assert!(
            MAC_COPY.contains(&format!(
                "public static let detail = \"{COORDINATION_ADVISORIES_DETAIL}\"\n"
            )),
            "the Mac's detail differs"
        );
        assert_eq!(
            COORDINATION_ADVISORIES_TITLE,
            "Show teammates' overlapping work at the end of a turn"
        );
    }
}
