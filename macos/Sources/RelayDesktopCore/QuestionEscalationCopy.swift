import Foundation

/// Wording for question escalation (`question_escalation`, `off` or `on`).
/// The row shows only what the core reports: it has no default of its own,
/// and the availability line is the core's `question_escalation_status`, or
/// the list built from `question_escalation_coverage` when the core serves
/// one. The Linux app's `QUESTION_ESCALATION_*` and `QUESTION_COVERAGE_*`
/// constants match these word for word.
public enum QuestionEscalationCopy {
    public static let title = "Send agents' questions to you"
    public static let detail = "While this is on, questions from the sessions Agent Relay can reach are sent to you over Agent Relay instead of being asked in the session. You answer by replying to them."
    public static let unavailable = "Not available: this version of Agent Relay's background service does not have this setting. Update Agent Relay to use it."
    public static let turnOnHelp = "Send agents' questions to you over Agent Relay"
    public static let turnOffHelp = "Ask agents' questions in the session again"

    /// The disclosure that holds the list of kinds of session.
    public static let coverageTitle = "Which sessions this reaches"
    /// The line under the list, outside the disclosure.
    public static let coverageClosing = "Where questions are not sent to you, they are asked in the session as usual."

    /// The line naming where questions go, from `question_escalation_inbox`;
    /// `question_escalation_inbox_line` on Linux.
    public static func inbox(_ inbox: String) -> String {
        "Questions go to \(inbox)."
    }

    /// The heading of one group of the list; `nil` for a state this build
    /// does not know, whose entries are left out.
    /// `question_coverage_heading` on Linux.
    public static func coverageHeading(_ state: QuestionCoverageState) -> String? {
        switch state {
        case .covered: return "Sent to you"
        case .partial: return "Sent to you in some cases"
        case .notCovered: return "Asked in the session as usual"
        case .notApplicable: return "Does not apply"
        case .unknown: return nil
        }
    }

    /// What tells apart two kinds that share a name, used only when both
    /// are listed apart. `question_coverage_qualifier` on Linux.
    public static func qualifier(_ kind: SessionKind) -> String? {
        switch kind {
        case .opencode: return "on Agent Relay's server"
        case .opencodeOpen: return "in a terminal"
        case .grok: return "run by Agent Relay"
        case .grokOpen: return "already open"
        default: return nil
        }
    }
}

/// One group of the coverage list: the kinds of session in one state.
public struct QuestionCoverageGroup: Equatable, Sendable {
    public let heading: String
    public let entries: [QuestionCoverageEntry]

    public init(heading: String, entries: [QuestionCoverageEntry]) {
        self.heading = heading
        self.entries = entries
    }
}

/// One kind of session (or several with the same name, state and reason).
public struct QuestionCoverageEntry: Equatable, Sendable {
    public let name: String
    /// The core's reason, trimmed; `nil` when it is blank.
    public let reason: String?

    public init(name: String, reason: String?) {
        self.name = name
        self.reason = reason
    }

    /// The entry as the list shows it. `QuestionCoverageEntry::line` on Linux.
    public var line: String {
        reason.map { "\(name): \($0)" } ?? name
    }
}

/// The list for `question_escalation_coverage`. The Linux app's
/// `models::question_coverage_groups` matches it.
public enum QuestionCoverageList {
    /// The core's `KINDS` order (relay `question/coverage.rs`).
    public static let order: [SessionKind] = [
        .claudeTerminal, .claudeCloud, .codex, .cursorCli, .copilot, .opencode, .opencodeOpen,
        .grok, .grokOpen, .devin, .droid, .pi, .omp, .museOpen, .openclaw, .hermes, .antigravity,
    ]

    private static let states: [QuestionCoverageState] = [.covered, .partial, .notCovered, .notApplicable]

    /// Grouped by state (covered, partial, not covered, not applicable),
    /// each group in the core's order with keys the app does not know after
    /// the known ones. Kinds with the same name, state and reason are one
    /// entry; two entries that still share a name get their kinds'
    /// qualifiers. An entry in a state this build does not know is left out.
    public static func groups(_ coverage: [String: QuestionCoverage]?) -> [QuestionCoverageGroup] {
        guard let coverage, !coverage.isEmpty else { return [] }
        let known = order.map(\.rawValue)
        let keys = known.filter { coverage[$0] != nil }
            + coverage.keys.filter { !known.contains($0) }.sorted()

        struct Merged {
            let state: QuestionCoverageState
            let name: String
            var kinds: [SessionKind]
            let reason: String?
        }
        var merged: [Merged] = []
        for key in keys {
            guard let entry = coverage[key] else { continue }
            if case .unknown = entry.state { continue }
            let kind = SessionKind(rawValue: key)
            // `SessionKind.label` names a kind this build does not know by its key.
            let name = kind.label
            let trimmed = entry.reason.trimmingCharacters(in: .whitespacesAndNewlines)
            let reason: String? = trimmed.isEmpty ? nil : trimmed
            if let index = merged.firstIndex(where: {
                $0.state == entry.state && $0.name == name && $0.reason == reason
            }) {
                merged[index].kinds.append(kind)
            } else {
                merged.append(Merged(state: entry.state, name: name, kinds: [kind], reason: reason))
            }
        }

        let all = merged
        func shared(_ name: String) -> Bool {
            all.filter { $0.name == name }.count > 1
        }
        let entries: [(QuestionCoverageState, QuestionCoverageEntry)] = all.map { item -> (QuestionCoverageState, QuestionCoverageEntry) in
            var name = item.name
            if item.kinds.count == 1, shared(item.name),
               let qualifier = QuestionEscalationCopy.qualifier(item.kinds[0]) {
                name = "\(item.name) (\(qualifier))"
            }
            return (item.state, QuestionCoverageEntry(name: name, reason: item.reason))
        }
        return states.compactMap { state -> QuestionCoverageGroup? in
            guard let heading = QuestionEscalationCopy.coverageHeading(state) else { return nil }
            let group = entries.filter { $0.0 == state }.map { $0.1 }
            return group.isEmpty ? nil : QuestionCoverageGroup(heading: heading, entries: group)
        }
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
    /// The list built from `question_escalation_coverage`; when it is not
    /// empty it stands in for `status`.
    public let coverage: [QuestionCoverageGroup]

    public init(enabled: Bool?, status: String?, inbox: String?, coverage: [QuestionCoverageGroup] = []) {
        self.enabled = enabled
        self.status = status
        self.inbox = inbox
        self.coverage = coverage
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
        let coverage: [QuestionCoverageGroup] = enabled == nil ? [] : QuestionCoverageList.groups(settings?.questionEscalationCoverage)
        self.enabled = enabled
        self.coverage = coverage
        status = enabled == nil || !coverage.isEmpty ? nil : present(settings?.questionEscalationStatus)
        inbox = enabled == nil ? nil : present(settings?.questionEscalationInbox).map(QuestionEscalationCopy.inbox)
    }
}
