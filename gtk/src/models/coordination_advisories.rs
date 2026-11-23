//! Whether sessions are shown teammates' overlapping work as their turn
//! ends (desktop-core's `coordination_advisories.*`). Matches the Mac's
//! CoordinationAdvisoriesCopy word for word.

pub const COORDINATION_ADVISORIES_TITLE: &str = "Show agents teammates' overlapping work";
pub const COORDINATION_ADVISORIES_DETAIL: &str = "Off: nothing is added. On: urgent notes may reach registered agents before a turn ends. Other notes appear once at the end of a supported Claude Code, Codex, Copilot, Devin or Cursor CLI turn. Your workspace must provide coordination notes.";

pub const COORDINATION_NOTIFICATIONS_TITLE: &str = "Urgent coordination system notifications";
pub const COORDINATION_NOTIFICATIONS_DETAIL: &str = "Off by default. When coordination notes are on, validated priority-one alerts can request a system notification, up to three batches per hour. Your system's notification permissions and Focus or Do Not Disturb settings still apply. Agent Relay has no separate quiet hours or snooze.";

#[cfg(test)]
mod tests {
    use super::*;

    /// The Mac's copy, read from its source so the two cannot drift.
    const MAC_COPY: &str =
        include_str!("../../../macos/Sources/RelayDesktopCore/CoordinationAdvisoriesCopy.swift");

    /// The request the urgent-notifications switch sends, as the core reads it.
    #[test]
    fn the_notifications_switch_sends_its_method_and_enabled_flag() {
        use agent_relay_desktop_core::contract::coordination::CoordinationAdvisoriesParams;
        use agent_relay_desktop_core::contract::method;
        use agent_relay_node::relay::contract::Method;
        assert_eq!(
            method::CoordinationNotificationsSet::NAME,
            "coordination_advisories.notifications.set"
        );
        for enabled in [true, false] {
            assert_eq!(
                serde_json::to_value(CoordinationAdvisoriesParams { enabled }).unwrap(),
                serde_json::json!({ "enabled": enabled })
            );
        }
    }

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
        for (key, value) in [
            ("notificationsTitle", COORDINATION_NOTIFICATIONS_TITLE),
            ("notificationsDetail", COORDINATION_NOTIFICATIONS_DETAIL),
        ] {
            assert!(MAC_COPY.contains(&format!("public static let {key} = \"{value}\"\n")));
        }
        assert_eq!(
            COORDINATION_ADVISORIES_TITLE,
            "Show agents teammates' overlapping work"
        );
    }
}
