import Foundation
import RelaySessionOverview

/// A session event as the chat shows it: harness preamble and the relay
/// envelope removed. Who sent a relayed prompt is never read from the
/// session text: that text is the agent's, and could name anyone.
struct SessionEventRow: Identifiable, Equatable {
    let event: SessionEvent
    /// The text to show.
    let text: String
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
    /// A message the composer still shows as sending.
    struct Pending: Equatable {
        let text: String
        let createdAt: Date
    }

    /// How long after a pending bubble was made its session copy can arrive.
    static let pendingCopyWindow: ClosedRange<TimeInterval> = -5...600

    /// Relay messages and session events in one time-ordered list. Relay
    /// messages keep their order; events without a time come first.
    ///
    /// A person event that is the session's copy of a relay message is left
    /// out, so the message shows once: its envelope names a relay message
    /// the chat holds and its text is that message's text. So is the copy of
    /// a message the composer still shows as pending: sent by this device
    /// (`me`), with the pending text, within `pendingCopyWindow` of it.
    /// Agent, tool and activity events are never hidden this way.
    static func items(messages: [RelayMessage], events: [SessionEvent],
                      pending: [Pending] = [], me: String = "") -> [ChatTimelineItem] {
        var relayTexts: [String: String] = [:]
        for message in messages where message.hasServerID { relayTexts[message.id] = normalized(message.text) }
        let pending = pending.map { (text: normalized($0.text), createdAt: $0.createdAt) }
        let rows: [SessionEventRow] = events.compactMap { event in
            row(for: event, relayTexts: relayTexts, pending: pending, me: me)
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

    private static func row(for event: SessionEvent, relayTexts: [String: String],
                            pending: [(text: String, createdAt: Date)], me: String) -> SessionEventRow? {
        switch event.role {
        case .person, .agent:
            if let envelope = RelayEnvelope.parse(event.content) {
                let body = normalized(envelope.body)
                guard !body.isEmpty else { return nil }
                if event.role == .person {
                    if let id = envelope.messageID, relayTexts[id] == body { return nil }
                    if !me.isEmpty, envelope.sender == me, let ts = event.ts,
                       pending.contains(where: { $0.text == body && pendingCopyWindow.contains(ts.timeIntervalSince($0.createdAt)) }) {
                        return nil
                    }
                }
                return SessionEventRow(event: event, text: envelope.body)
            }
            let text = withoutPreamble(event.content)
            // A prompt that was only harness preamble, or an empty reply.
            guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
            return SessionEventRow(event: event, text: text)
        case .tool, .activity:
            return SessionEventRow(event: event, text: event.content)
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

    /// A collapsed compact row: the first three lines and at most
    /// `characters` characters, with "…" when that cut anything.
    static func collapsed(_ text: String, characters: Int) -> String {
        let lines = text.split(separator: "\n", omittingEmptySubsequences: false)
        var head = lines.prefix(3).joined(separator: "\n")
        if head.count > characters { head = String(head.prefix(characters)) }
        return head.count < text.count ? head + "…" : head
    }

    /// A tool row's file list: the first three names and how many more.
    static func filesSummary(_ files: [String]) -> String {
        guard !files.isEmpty else { return "" }
        let names = files.prefix(3).map { ($0 as NSString).lastPathComponent }
        return files.count > 3 ? names.joined(separator: ", ") + " +\(files.count - 3)" : names.joined(separator: ", ")
    }
}
