import Foundation
import RelaySessionOverview

/// A session event as the chat shows it: harness preamble and the relay
/// envelope removed, and who relayed it when it came through Agent Relay.
struct SessionEventRow: Identifiable, Equatable {
    let event: SessionEvent
    /// The text to show.
    let text: String
    /// The sender of a relayed message the open chat does not hold.
    let relayedFrom: String?
    var id: String { "event:\(event.id)" }
}

/// One row of an agent's chat: a relay message or a session event.
enum ChatTimelineItem: Identifiable {
    case message(RelayMessage)
    case event(SessionEventRow)

    /// A relay message keeps its own id, so delivery, pending reconciliation
    /// and scrolling see the same ids as before session events were shown.
    var id: String {
        switch self {
        case .message(let message): message.id
        case .event(let row): row.id
        }
    }

    var date: Date? {
        switch self {
        case .message(let message): message.createdAt
        case .event(let row): row.event.ts
        }
    }
}

enum SessionTimeline {
    /// Relay messages and session events in one time-ordered list. A
    /// session's copy of a relay message shown in the chat (its envelope
    /// names the message id) is left out, and so is the copy of a message
    /// the composer still shows as pending (the same text). Relay messages
    /// keep their order; events without a time come first.
    static func items(messages: [RelayMessage], events: [SessionEvent],
                      pendingTexts: [String] = []) -> [ChatTimelineItem] {
        let relayIDs = Set(messages.filter(\.hasServerID).map(\.id))
        let pending = Set(pendingTexts.map(normalized))
        let rows: [SessionEventRow] = events.compactMap { event in
            row(for: event, relayIDs: relayIDs, pending: pending)
        }
        var result: [ChatTimelineItem] = []
        result.reserveCapacity(messages.count + rows.count)
        var m = 0, e = 0
        while m < messages.count || e < rows.count {
            guard e < rows.count else {
                result.append(.message(messages[m])); m += 1; continue
            }
            guard m < messages.count else {
                result.append(.event(rows[e])); e += 1; continue
            }
            let eventDate = rows[e].event.ts ?? .distantPast
            if let messageDate = messages[m].createdAt, messageDate > eventDate {
                result.append(.event(rows[e])); e += 1
            } else {
                result.append(.message(messages[m])); m += 1
            }
        }
        return result
    }

    private static func row(for event: SessionEvent, relayIDs: Set<String>, pending: Set<String>) -> SessionEventRow? {
        switch event.role {
        case .person, .agent:
            if let envelope = RelayEnvelope.parse(event.content) {
                if let id = envelope.messageID, relayIDs.contains(id) { return nil }
                if pending.contains(normalized(envelope.body)) { return nil }
                guard !envelope.body.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
                return SessionEventRow(event: event, text: envelope.body, relayedFrom: envelope.sender)
            }
            let text = withoutPreamble(event.content)
            // A prompt that was only harness preamble, or an empty reply.
            guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
            return SessionEventRow(event: event, text: text, relayedFrom: nil)
        case .tool, .activity:
            return SessionEventRow(event: event, text: event.content, relayedFrom: nil)
        }
    }

    private static func normalized(_ text: String) -> String {
        text.components(separatedBy: .whitespacesAndNewlines).filter { !$0.isEmpty }.joined(separator: " ")
    }

    /// What the chat shows besides its rows.
    enum Placeholder: Equatable {
        /// A first load is running: a loading row, never the empty text.
        case loading
        /// Every source has loaded and none has a row.
        case empty
        case none
    }

    /// `historyChecked`: the relay history read finished (or found no
    /// thread); `sessionApplies`: the agent has a session this app can
    /// read; `sessionSettled`: its first page loaded or failed.
    static func placeholder(hasRows: Bool, historyChecked: Bool, sessionApplies: Bool,
                            sessionSettled: Bool) -> Placeholder {
        if !historyChecked || (sessionApplies && !sessionSettled) { return .loading }
        return hasRows ? .none : .empty
    }

    /// The start of long content and whether it was cut there.
    static func preview(_ text: String, characters: Int = 600, lines: Int = 12) -> (text: String, isCut: Bool) {
        var cut = text.count > characters
        var head = cut ? String(text.prefix(characters)) : text
        let split = head.split(separator: "\n", omittingEmptySubsequences: false)
        if split.count > lines {
            head = split.prefix(lines).joined(separator: "\n")
            cut = true
        }
        return (cut ? head.trimmingCharacters(in: .whitespacesAndNewlines) + "…" : head, cut)
    }

    /// A tool row's file list: the first three names and how many more.
    static func filesSummary(_ files: [String]) -> String {
        guard !files.isEmpty else { return "" }
        let names = files.prefix(3).map { ($0 as NSString).lastPathComponent }
        return files.count > 3 ? names.joined(separator: ", ") + " +\(files.count - 3)" : names.joined(separator: ", ")
    }
}
