//! Question escalation in words: the Settings row that turns the core's
//! `question_escalation` on or off, and what it shows from the core.

use agent_relay_node::relay::contract::agents::SettingsValues;

/// Question escalation (`question_escalation`, `off` or `on`). The row shows
/// only what the core reports: it has no default of its own, and the
/// availability line is the core's `question_escalation_status`. The Mac's
/// `QuestionEscalationCopy` matches these word for word.
pub const QUESTION_ESCALATION_TITLE: &str = "Send agents' questions to you";
pub const QUESTION_ESCALATION_DETAIL: &str = "While this is on, a question an agent asks is sent to you over Agent Relay instead of being shown in the agent's own window. You answer by replying to it.";
pub const QUESTION_ESCALATION_UNAVAILABLE: &str = "Not available: this version of Agent Relay's background service does not have this setting. Update Agent Relay to use it.";
pub const QUESTION_ESCALATION_TURN_ON_HELP: &str = "Send agents' questions to you over Agent Relay";
pub const QUESTION_ESCALATION_TURN_OFF_HELP: &str = "Show agents' questions in their own window again";

/// The line naming where questions go, from `question_escalation_inbox`;
/// `QuestionEscalationCopy.inbox` on the Mac.
pub fn question_escalation_inbox_line(inbox: &str) -> String {
    format!("Questions go to {inbox}.")
}

/// What the question escalation row shows, read from the core's settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionEscalationRow {
    /// `Some(on)` when the core reports the setting; `None` for a core
    /// that does not, whose row reads as unavailable.
    pub enabled: Option<bool>,
    /// The core's `question_escalation_status`, as it gave it.
    pub status: Option<String>,
    /// The `question_escalation_inbox` line, when one is set.
    pub inbox: Option<String>,
}

pub fn question_escalation_row(settings: &SettingsValues) -> QuestionEscalationRow {
    let present = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let enabled = settings
        .question_escalation
        .as_deref()
        .map(|value| value.trim() == "on");
    QuestionEscalationRow {
        enabled,
        status: enabled.and(present(&settings.question_escalation_status)),
        inbox: enabled
            .and(present(&settings.question_escalation_inbox))
            .map(|inbox| question_escalation_inbox_line(&inbox)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn settings(value: Value) -> SettingsValues {
        let mut base = json!({"allow_self_registration": true, "auto_activate": true});
        if let (Some(base), Value::Object(changes)) = (base.as_object_mut(), value) {
            base.extend(changes);
        }
        serde_json::from_value(base).unwrap()
    }

    #[test]
    fn question_escalation_wording_matches_the_mac() {
        assert_eq!(QUESTION_ESCALATION_TITLE, "Send agents' questions to you");
        assert_eq!(
            QUESTION_ESCALATION_DETAIL,
            "While this is on, a question an agent asks is sent to you over Agent Relay instead of being shown in the agent's own window. You answer by replying to it."
        );
        assert_eq!(
            QUESTION_ESCALATION_UNAVAILABLE,
            "Not available: this version of Agent Relay's background service does not have this setting. Update Agent Relay to use it."
        );
        assert_eq!(
            QUESTION_ESCALATION_TURN_ON_HELP,
            "Send agents' questions to you over Agent Relay"
        );
        assert_eq!(
            QUESTION_ESCALATION_TURN_OFF_HELP,
            "Show agents' questions in their own window again"
        );
        assert_eq!(
            question_escalation_inbox_line("lead"),
            "Questions go to lead."
        );
    }

    #[test]
    fn question_escalation_row_shows_only_what_the_core_reports() {
        // An older core without the key: unavailable, not hidden or off.
        let older = question_escalation_row(&settings(json!({})));
        assert_eq!(
            older,
            QuestionEscalationRow { enabled: None, status: None, inbox: None }
        );
        // The core says which way an unset value reads; the app has no default.
        for (reported, enabled) in [("off", false), ("on", true)] {
            let row = question_escalation_row(&settings(json!({
                "question_escalation": reported,
                "question_escalation_status": "Covers Claude Code sessions.",
            })));
            assert_eq!(row.enabled, Some(enabled));
            assert_eq!(row.status.as_deref(), Some("Covers Claude Code sessions."));
            assert_eq!(row.inbox, None);
        }
        let routed = question_escalation_row(&settings(json!({
            "question_escalation": "on",
            "question_escalation_inbox": "lead",
        })));
        assert_eq!(routed.status, None);
        assert_eq!(routed.inbox.as_deref(), Some("Questions go to lead."));
        let empty_inbox = question_escalation_row(&settings(json!({
            "question_escalation": "off",
            "question_escalation_inbox": "",
            "question_escalation_status": " ",
        })));
        assert_eq!(empty_inbox.inbox, None);
        assert_eq!(empty_inbox.status, None);
    }
}
