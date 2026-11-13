import Foundation

/// Matches the GTK coordination advisories setting word for word; the GTK
/// test `coordination_advisories::tests::the_copy_matches_the_mac_word_for_word`
/// reads this file.
public enum CoordinationAdvisoriesCopy {
    /// Where the row keeps a failed change's sentence in `settingErrors`.
    public static let settingKey = "coordination_advisories"
    public static let title = "Show teammates' overlapping work at the end of a turn"
    public static let detail = "Off: nothing is added. On: when a Claude Code, Codex, Copilot, Devin or Cursor CLI session ends a turn, it is shown a short note about teammates' work that overlaps with its own, once per note. Your workspace's Cloud must offer these notes."
}
