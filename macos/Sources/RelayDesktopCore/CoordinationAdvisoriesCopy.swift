import Foundation

/// Matches the GTK coordination advisories setting word for word; the GTK
/// test `coordination_advisories::tests::the_copy_matches_the_mac_word_for_word`
/// reads this file.
public enum CoordinationAdvisoriesCopy {
    /// Where the row keeps a failed change's sentence in `settingErrors`.
    public static let settingKey = "coordination_advisories"
    public static let notificationsKey = "coordination_notifications"
    public static let notificationsTitle = "Urgent coordination system notifications"
    public static let notificationsDetail = "Off by default. When coordination notes are on, validated priority-one alerts can request a system notification, up to three batches per hour. Your system's notification permissions and Focus or Do Not Disturb settings still apply. Agent Relay has no separate quiet hours or snooze."
    public static let title = "Show agents teammates' overlapping work"
    public static let detail = "Off: nothing is added. On: urgent notes may reach registered agents before a turn ends. Other notes appear once at the end of a supported Claude Code, Codex, Copilot, Devin or Cursor CLI turn. Your workspace must provide coordination notes."
}
