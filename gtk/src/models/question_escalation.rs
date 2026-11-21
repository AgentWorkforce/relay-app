//! Question escalation in words: the Settings row that turns the core's
//! `question_escalation` on or off, and what it shows from the core.

use agent_relay_desktop_core::history::session_model::kind_label;
use agent_relay_node::relay::contract::agents::{
    QuestionCoverage, QuestionCoverageState, SettingsValues,
};
use agent_relay_node::relay::question::coverage::KINDS;
use agent_relay_node::relay::store::SessionKind;
use std::collections::BTreeMap;

/// Question escalation (`question_escalation`, `off` or `on`). The row shows
/// only what the core reports: it has no default of its own, and the
/// availability line is the core's `question_escalation_status`. The Mac's
/// `QuestionEscalationCopy` matches these word for word.
pub const QUESTION_ESCALATION_TITLE: &str = "Send agents' questions to you";
pub const QUESTION_ESCALATION_DETAIL: &str = "While this is on, questions from the sessions Agent Relay can reach are sent to you over Agent Relay instead of being asked in the session. You answer by replying to them.";
pub const QUESTION_ESCALATION_UNAVAILABLE: &str = "Not available: this version of Agent Relay's background service does not have this setting. Update Agent Relay to use it.";
pub const QUESTION_ESCALATION_TURN_ON_HELP: &str = "Send agents' questions to you over Agent Relay";
pub const QUESTION_ESCALATION_TURN_OFF_HELP: &str = "Ask agents' questions in the session again";

/// The disclosure that holds the list of kinds of session.
pub const QUESTION_COVERAGE_TITLE: &str = "Which sessions this reaches";
/// The line under the list, outside the disclosure.
pub const QUESTION_COVERAGE_CLOSING: &str =
    "Where questions are not sent to you, they are asked in the session as usual.";

/// The heading of one group of the list.
/// `QuestionEscalationCopy.coverageHeading` on the Mac.
pub fn question_coverage_heading(state: QuestionCoverageState) -> &'static str {
    match state {
        QuestionCoverageState::Covered => "Sent to you",
        QuestionCoverageState::Partial => "Sent to you in some cases",
        QuestionCoverageState::NotCovered => "Asked in the session as usual",
        QuestionCoverageState::NotApplicable => "Does not apply",
    }
}

/// What tells apart two kinds that share a name, used only when both are
/// listed apart. `QuestionEscalationCopy.qualifier` on the Mac.
pub fn question_coverage_qualifier(kind: SessionKind) -> Option<&'static str> {
    match kind {
        SessionKind::OpenCode => Some("on Agent Relay's server"),
        SessionKind::OpenCodeOpen => Some("in a terminal"),
        SessionKind::Grok => Some("run by Agent Relay"),
        SessionKind::GrokOpen => Some("already open"),
        _ => None,
    }
}

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
    /// The list built from `question_escalation_coverage`; when it is not
    /// empty it stands in for `status`.
    pub coverage: Vec<QuestionCoverageGroup>,
}

/// One group of the list: the kinds of session in one state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionCoverageGroup {
    pub heading: &'static str,
    pub entries: Vec<QuestionCoverageEntry>,
}

/// One kind of session (or several with the same name, state and reason).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestionCoverageEntry {
    pub name: String,
    /// The core's reason, trimmed; `None` when it is blank.
    pub reason: Option<String>,
}

impl QuestionCoverageEntry {
    /// The entry as the list shows it. `QuestionCoverageEntry.line` on the Mac.
    pub fn line(&self) -> String {
        match &self.reason {
            Some(reason) => format!("{}: {reason}", self.name),
            None => self.name.clone(),
        }
    }
}

/// The list for `question_escalation_coverage`: grouped by state (covered,
/// partial, not covered, not applicable), each group in the core's `KINDS`
/// order with keys the app does not know after the known ones. Kinds with
/// the same name, state and reason are one entry; two entries that still
/// share a name get their kinds' qualifiers. A state the app does not know
/// cannot reach here: the contract's decode of the settings refuses it.
/// `QuestionCoverageList.groups` on the Mac.
pub fn question_coverage_groups(
    coverage: Option<&BTreeMap<String, QuestionCoverage>>,
) -> Vec<QuestionCoverageGroup> {
    let Some(coverage) = coverage else {
        return Vec::new();
    };
    let known: Vec<&str> = KINDS.iter().map(|kind| kind.as_str()).collect();
    let keys = known
        .iter()
        .copied()
        .filter(|key| coverage.contains_key(*key))
        .chain(
            coverage
                .keys()
                .map(String::as_str)
                .filter(|key| !known.contains(key)),
        );
    // (state, name, kind when the key is one, reason), merged.
    let mut merged: Vec<(
        QuestionCoverageState,
        String,
        Vec<Option<SessionKind>>,
        Option<String>,
    )> = Vec::new();
    for key in keys {
        let entry = &coverage[key];
        let kind =
            serde_json::from_value::<SessionKind>(serde_json::Value::String(key.into())).ok();
        let name = kind.map_or_else(|| key.to_string(), |kind| kind_label(kind).to_string());
        let reason = Some(entry.reason.trim())
            .filter(|reason| !reason.is_empty())
            .map(str::to_owned);
        match merged.iter_mut().find(|(state, other, _, other_reason)| {
            *state == entry.state && *other == name && *other_reason == reason
        }) {
            Some((_, _, kinds, _)) => kinds.push(kind),
            None => merged.push((entry.state, name, vec![kind], reason)),
        }
    }
    let shared = |name: &str| {
        merged
            .iter()
            .filter(|(_, other, _, _)| other == name)
            .count()
            > 1
    };
    let entries: Vec<(QuestionCoverageState, QuestionCoverageEntry)> = merged
        .iter()
        .map(|(state, name, kinds, reason)| {
            let qualifier = match kinds.as_slice() {
                [Some(kind)] if shared(name) => question_coverage_qualifier(*kind),
                _ => None,
            };
            let name = match qualifier {
                Some(qualifier) => format!("{name} ({qualifier})"),
                None => name.clone(),
            };
            (
                *state,
                QuestionCoverageEntry {
                    name,
                    reason: reason.clone(),
                },
            )
        })
        .collect();
    [
        QuestionCoverageState::Covered,
        QuestionCoverageState::Partial,
        QuestionCoverageState::NotCovered,
        QuestionCoverageState::NotApplicable,
    ]
    .into_iter()
    .filter_map(|state| {
        let group: Vec<QuestionCoverageEntry> = entries
            .iter()
            .filter(|(entry_state, _)| *entry_state == state)
            .map(|(_, entry)| entry.clone())
            .collect();
        (!group.is_empty()).then(|| QuestionCoverageGroup {
            heading: question_coverage_heading(state),
            entries: group,
        })
    })
    .collect()
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
    let coverage = match enabled {
        Some(_) => question_coverage_groups(settings.question_escalation_coverage.as_ref()),
        None => Vec::new(),
    };
    QuestionEscalationRow {
        enabled,
        status: enabled
            .filter(|_| coverage.is_empty())
            .and(present(&settings.question_escalation_status)),
        inbox: enabled
            .and(present(&settings.question_escalation_inbox))
            .map(|inbox| question_escalation_inbox_line(&inbox)),
        coverage,
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
            "While this is on, questions from the sessions Agent Relay can reach are sent to you over Agent Relay instead of being asked in the session. You answer by replying to them."
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
            "Ask agents' questions in the session again"
        );
        assert_eq!(
            question_escalation_inbox_line("lead"),
            "Questions go to lead."
        );
        assert_eq!(QUESTION_COVERAGE_TITLE, "Which sessions this reaches");
        assert_eq!(
            QUESTION_COVERAGE_CLOSING,
            "Where questions are not sent to you, they are asked in the session as usual."
        );
        let headings = [
            QuestionCoverageState::Covered,
            QuestionCoverageState::Partial,
            QuestionCoverageState::NotCovered,
            QuestionCoverageState::NotApplicable,
        ]
        .map(question_coverage_heading);
        assert_eq!(
            headings,
            [
                "Sent to you",
                "Sent to you in some cases",
                "Asked in the session as usual",
                "Does not apply",
            ]
        );
        let entry = |reason: Option<&str>| QuestionCoverageEntry {
            name: "Codex".into(),
            reason: reason.map(str::to_owned),
        };
        assert_eq!(
            entry(Some("Plan mode only.")).line(),
            "Codex: Plan mode only."
        );
        assert_eq!(entry(None).line(), "Codex");
    }

    /// The core's `KINDS` order, the names the app already shows for each
    /// kind (`kind_label`) and the qualifiers, word for word as the Mac's
    /// `QuestionCoverageList.order`, `SessionKind.label` and
    /// `QuestionEscalationCopy.qualifier`.
    #[test]
    fn question_coverage_names_match_the_mac() {
        let names: Vec<(&str, &str, Option<&str>)> = KINDS
            .iter()
            .map(|kind| {
                (
                    kind.as_str(),
                    kind_label(*kind),
                    question_coverage_qualifier(*kind),
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                ("claude-terminal", "Claude Code", None),
                ("claude-cloud", "Claude app", None),
                ("codex", "Codex", None),
                ("cursor-cli", "Cursor CLI", None),
                ("copilot", "Copilot", None),
                ("opencode", "OpenCode", Some("on Agent Relay's server")),
                ("opencode-open", "OpenCode", Some("in a terminal")),
                ("grok", "Grok", Some("run by Agent Relay")),
                ("grok-open", "Grok", Some("already open")),
                ("devin", "Devin", None),
                ("droid", "Factory Droid", None),
                ("pi", "Pi", None),
                ("omp", "Oh My Pi", None),
                ("muse-open", "Muse Code", None),
                ("openclaw", "OpenClaw", None),
                ("hermes", "Hermes", None),
            ]
        );
    }

    fn coverage(value: Value) -> BTreeMap<String, QuestionCoverage> {
        serde_json::from_value(value).unwrap()
    }

    fn names(groups: &[QuestionCoverageGroup]) -> Vec<(&'static str, Vec<String>)> {
        groups
            .iter()
            .map(|group| {
                (
                    group.heading,
                    group
                        .entries
                        .iter()
                        .map(QuestionCoverageEntry::line)
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn question_coverage_absent_or_empty_is_no_list() {
        assert_eq!(question_coverage_groups(None), Vec::new());
        assert_eq!(question_coverage_groups(Some(&BTreeMap::new())), Vec::new());
    }

    #[test]
    fn question_coverage_groups_by_state_in_the_core_order() {
        let groups = question_coverage_groups(Some(&coverage(json!({
            "hermes": {"state": "not_applicable", "reason": "No dialog."},
            "pi": {"state": "not_covered", "reason": " Not yet. "},
            "codex": {"state": "partial", "reason": "Plan mode only."},
            "devin": {"state": "partial", "reason": "Hooks off until turned on."},
            "claude-terminal": {"state": "covered", "reason": "Through the hook."},
            "claude-cloud": {"state": "not_covered", "reason": "Runs in the cloud."},
        }))));
        assert_eq!(
            names(&groups),
            [
                (
                    "Sent to you",
                    vec!["Claude Code: Through the hook.".to_string()]
                ),
                (
                    "Sent to you in some cases",
                    vec![
                        "Codex: Plan mode only.".to_string(),
                        "Devin: Hooks off until turned on.".to_string(),
                    ]
                ),
                (
                    "Asked in the session as usual",
                    vec![
                        "Claude app: Runs in the cloud.".to_string(),
                        "Pi: Not yet.".to_string()
                    ]
                ),
                ("Does not apply", vec!["Hermes: No dialog.".to_string()]),
            ]
        );
        // A group with no entry is not shown.
        let covered_only = question_coverage_groups(Some(&coverage(json!({
            "codex": {"state": "covered", "reason": "Yes."},
        }))));
        assert_eq!(
            names(&covered_only),
            [("Sent to you", vec!["Codex: Yes.".to_string()])]
        );
    }

    /// At the pinned core the two Grok kinds share a name, state and reason
    /// and are one entry. The two OpenCode kinds share a state but not a
    /// reason, so both are shown, each with its qualifier.
    #[test]
    fn question_coverage_at_the_pinned_core() {
        let served: BTreeMap<String, QuestionCoverage> =
            serde_json::from_value(agent_relay_node::relay::question::coverage::value()).unwrap();
        let groups = question_coverage_groups(Some(&served));
        let served_names: Vec<String> = groups
            .iter()
            .flat_map(|group| group.entries.iter().map(|entry| entry.name.clone()))
            .collect();
        assert_eq!(
            served_names.iter().filter(|name| *name == "Grok").count(),
            1,
            "{served_names:?}"
        );
        assert!(
            served_names.contains(&"OpenCode (on Agent Relay's server)".to_string()),
            "{served_names:?}"
        );
        assert!(
            served_names.contains(&"OpenCode (in a terminal)".to_string()),
            "{served_names:?}"
        );
        assert_eq!(served_names.len(), KINDS.len() - 1, "{served_names:?}");
        // The same name, state and reason, by hand.
        let merged = question_coverage_groups(Some(&coverage(json!({
            "opencode": {"state": "not_covered", "reason": "Not yet."},
            "opencode-open": {"state": "not_covered", "reason": "Not yet."},
        }))));
        assert_eq!(
            names(&merged),
            [(
                "Asked in the session as usual",
                vec!["OpenCode: Not yet.".to_string()]
            )]
        );
    }

    /// Shaped like relay pull request #2234's table: the OpenCode kinds
    /// differ in state, so each is shown with its qualifier.
    #[test]
    fn question_coverage_keeps_a_shared_name_apart_with_qualifiers() {
        let groups = question_coverage_groups(Some(&coverage(json!({
            "opencode": {"state": "partial", "reason": "On a server Agent Relay runs."},
            "opencode-open": {"state": "not_covered", "reason": "Not through the plugin yet."},
            "grok": {"state": "not_covered", "reason": "Not yet."},
            "grok-open": {"state": "not_covered", "reason": "Not yet."},
        }))));
        assert_eq!(
            names(&groups),
            [
                (
                    "Sent to you in some cases",
                    vec![
                        "OpenCode (on Agent Relay's server): On a server Agent Relay runs."
                            .to_string()
                    ]
                ),
                (
                    "Asked in the session as usual",
                    vec![
                        "OpenCode (in a terminal): Not through the plugin yet.".to_string(),
                        "Grok: Not yet.".to_string(),
                    ]
                ),
            ]
        );
        // Same state, different reason: both shown, both qualified.
        let reasons = question_coverage_groups(Some(&coverage(json!({
            "grok": {"state": "not_covered", "reason": "One."},
            "grok-open": {"state": "not_covered", "reason": "Two."},
        }))));
        assert_eq!(
            names(&reasons),
            [(
                "Asked in the session as usual",
                vec![
                    "Grok (run by Agent Relay): One.".to_string(),
                    "Grok (already open): Two.".to_string(),
                ]
            )]
        );
    }

    #[test]
    fn question_coverage_shows_an_unknown_key_by_its_key_last() {
        let groups = question_coverage_groups(Some(&coverage(json!({
            "zeta-cli": {"state": "covered", "reason": "New."},
            "alpha-cli": {"state": "covered", "reason": ""},
            "omp": {"state": "covered", "reason": "Through the extension."},
        }))));
        assert_eq!(
            names(&groups),
            [(
                "Sent to you",
                vec![
                    "Oh My Pi: Through the extension.".to_string(),
                    "alpha-cli".to_string(),
                    "zeta-cli: New.".to_string(),
                ]
            )]
        );
    }

    #[test]
    fn question_coverage_shows_a_blank_reason_as_the_name_alone() {
        let groups = question_coverage_groups(Some(&coverage(json!({
            "codex": {"state": "partial", "reason": "  "},
        }))));
        assert_eq!(
            groups,
            [QuestionCoverageGroup {
                heading: "Sent to you in some cases",
                entries: vec![QuestionCoverageEntry {
                    name: "Codex".into(),
                    reason: None
                }],
            }]
        );
    }

    /// The list stands in for the status paragraph, on or off; without a
    /// map, or on a core without the setting, there is no list.
    #[test]
    fn question_escalation_row_shows_the_list_instead_of_the_status() {
        for reported in ["on", "off"] {
            let row = question_escalation_row(&settings(json!({
                "question_escalation": reported,
                "question_escalation_status": "Available for Claude Code.",
                "question_escalation_coverage": {
                    "claude-terminal": {"state": "covered", "reason": "Through the hook."},
                },
            })));
            assert_eq!(row.status, None);
            assert_eq!(
                names(&row.coverage),
                [(
                    "Sent to you",
                    vec!["Claude Code: Through the hook.".to_string()]
                )]
            );
        }
        let empty = question_escalation_row(&settings(json!({
            "question_escalation": "on",
            "question_escalation_status": "Available for Claude Code.",
            "question_escalation_coverage": {},
        })));
        assert_eq!(empty.status.as_deref(), Some("Available for Claude Code."));
        assert!(empty.coverage.is_empty());
        let older = question_escalation_row(&settings(json!({
            "question_escalation_coverage": {
                "codex": {"state": "covered", "reason": "Yes."},
            },
        })));
        assert_eq!(older.enabled, None);
        assert!(older.coverage.is_empty());
    }

    #[test]
    fn question_escalation_row_shows_only_what_the_core_reports() {
        // An older core without the key: unavailable, not hidden or off.
        let older = question_escalation_row(&settings(json!({})));
        assert_eq!(
            older,
            QuestionEscalationRow {
                enabled: None,
                status: None,
                inbox: None,
                coverage: Vec::new()
            }
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
