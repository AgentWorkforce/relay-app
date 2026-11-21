import Foundation

/// Wording for question escalation (`question_escalation`, `off` or `on`).
/// The row shows only what the core reports: it has no default of its own,
/// and the availability line is the core's `question_escalation_status`. The
/// Linux app's `QUESTION_ESCALATION_*` constants match these word for word.
public enum QuestionEscalationCopy {
    public static let title = "Send agents' questions to you"
    public static let detail = "While this is on, a question an agent asks is sent to you over Agent Relay instead of being shown in the agent's own window. You answer by replying to it."
    public static let unavailable = "Not available: this version of Agent Relay's background service does not have this setting. Update Agent Relay to use it."
    public static let turnOnHelp = "Send agents' questions to you over Agent Relay"
    public static let turnOffHelp = "Show agents' questions in their own window again"

    /// The line naming where questions go, from `question_escalation_inbox`;
    /// `question_escalation_inbox_line` on Linux.
    public static func inbox(_ inbox: String) -> String {
        "Questions go to \(inbox)."
    }
}

/// What the question escalation row shows, read from the core's settings.
/// The Linux app's `models::question_escalation_row` matches it.
public struct QuestionEscalationRow: Equatable, Sendable {
    /// `true` or `false` when the core reports the setting; `nil` for a core
    /// that does not, whose row reads as unavailable.
    public let enabled: Bool?
    /// The core's `question_escalation_status`, as it gave it.
    public let status: String?
    /// The `question_escalation_inbox` line, when one is set.
    public let inbox: String?

    public init(enabled: Bool?, status: String?, inbox: String?) {
        self.enabled = enabled
        self.status = status
        self.inbox = inbox
    }

    public init(_ settings: SettingsValues?) {
        func present(_ value: String?) -> String? {
            guard let trimmed = value?.trimmingCharacters(in: .whitespacesAndNewlines),
                  !trimmed.isEmpty else { return nil }
            return trimmed
        }
        let enabled = settings?.questionEscalation.map {
            $0.trimmingCharacters(in: .whitespacesAndNewlines) == "on"
        }
        self.enabled = enabled
        status = enabled == nil ? nil : present(settings?.questionEscalationStatus)
        inbox = enabled == nil ? nil : present(settings?.questionEscalationInbox).map(QuestionEscalationCopy.inbox)
    }
}
