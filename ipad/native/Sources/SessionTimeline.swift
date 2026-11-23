import Foundation
import RelaySessionOverview

/// That a chat row came through Agent Relay, and which way.
enum RelayMark: Equatable {
    /// A message to the agent, from the sender when the chat already shows
    /// a relay message of theirs; nil when only session text names one.
    case from(String?)
    /// A message the agent sent, to the recipient its send names.
    case to(String?)

    /// The line over a marked row: "⇄ Agent Relay · from @sender",
    /// "⇄ Agent Relay · to @recipient" (a channel keeps its "#"),
    /// "⇄ Agent Relay · sent" with no recipient, and "⇄ Agent Relay" for a
    /// received message whose sender is not shown.
    var label: String {
        let mark = "⇄ Agent Relay"
        switch self {
        case .from(let sender):
            guard let sender, !sender.isEmpty else { return mark }
            return "\(mark) · from @\(sender)"
        case .to(let recipient):
            guard let recipient, !recipient.isEmpty else { return "\(mark) · sent" }
            return "\(mark) · to \(recipient.hasPrefix("#") ? "" : "@")\(recipient)"
        }
    }

    /// A received message is drawn as one to the agent, on the person's
    /// side; a sent one as the agent's.
    var isIncoming: Bool {
        if case .from = self { return true }
        return false
    }
}

/// A session event as the chat shows it: harness preamble and the relay
/// envelope removed. Who sent a relayed prompt is never read from the
/// session text alone: that text is the agent's, and could name anyone.
struct SessionEventRow: Identifiable, Equatable {
    let event: SessionEvent
    /// The text to show.
    let text: String
    /// Set when the row is a relay message: one the agent received (a
    /// relayed prompt, or one its tool read from its inbox) or one it sent
    /// with a command or tool.
    var mark: RelayMark? = nil
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
    /// Events are read by `ChatEntryRules` over the whole list: an agent
    /// line that repeats a tool call goes, thinking is not shown, and an
    /// event with no content shows its tool's name or is left out.
    ///
    /// A relay message inside the session is shown as one, marked
    /// (`RelayMark`): a relayed prompt, a relay frame in a tool's output or
    /// the agent's text (received), and a relay send in a tool call or the
    /// agent's text (sent). Each is shown once by direction and trimmed
    /// text, counting the relay messages the chat holds: a message to the
    /// agent (sent by `me`) is "in", one from it "out". A message the chat
    /// holds stays and its session copy goes; a send recorded twice shows
    /// once. A received message names its sender only when a relay message
    /// the chat holds is from that sender.
    ///
    /// A person event that is the session's copy of a relay message is left
    /// out, so the message shows once: its envelope names a relay message
    /// the chat holds and its text is that message's text. So is the copy of
    /// a message the composer still shows as pending: sent by this device
    /// (`me`), with the pending text, within `pendingCopyWindow` of it.
    static func items(messages: [RelayMessage], events: [SessionEvent],
                      pending: [Pending] = [], me: String = "") -> [ChatTimelineItem] {
        var relayTexts: [String: String] = [:]
        for message in messages where message.hasServerID { relayTexts[message.id] = normalized(message.text) }
        var reader = Reader(relayTexts: relayTexts,
                            pending: pending.map { (text: normalized($0.text), createdAt: $0.createdAt) }, me: me)
        if !events.isEmpty {
            for message in messages {
                reader.senders.insert(message.sender)
                reader.seen.insert(relayKey(incoming: isToAgent(message, me: me), message.text))
            }
        }
        var rows: [SessionEventRow] = []
        for event in ChatEntryRules.withoutToolEchoes(events) {
            if let row = reader.row(for: event) { rows.append(row) }
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

    /// Whether a relay message the chat holds went to the agent: sent by
    /// this device's handle (`me`), or marked as this device's own.
    static func isToAgent(_ message: RelayMessage, me: String) -> Bool {
        message.mine || (!me.isEmpty && message.sender == me)
    }

    /// What tells one relay message from another in the chat: its
    /// direction and its trimmed text (the desktop's `relay_seen`).
    static func relayKey(incoming: Bool, _ text: String) -> String {
        (incoming ? "in:" : "out:") + text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Reads events into rows in time order, remembering the relay
    /// messages already shown.
    private struct Reader {
        let relayTexts: [String: String]
        let pending: [(text: String, createdAt: Date)]
        let me: String
        /// Senders of relay messages the chat holds: the only names a
        /// received message's mark shows.
        var senders: Set<String> = []
        /// `relayKey`s already shown.
        var seen: Set<String> = []

        init(relayTexts: [String: String], pending: [(text: String, createdAt: Date)], me: String) {
            self.relayTexts = relayTexts
            self.pending = pending
            self.me = me
        }

        private func trusted(_ sender: String?) -> String? {
            guard let sender, !sender.isEmpty, senders.contains(sender) else { return nil }
            return sender
        }

        mutating func row(for event: SessionEvent) -> SessionEventRow? {
            let role = event.entryRole
            guard role != .thinking,
                  let shown = ChatEntryRules.shownText(content: event.content, toolName: event.toolName) else { return nil }
            if role == .person {
                guard let envelope = RelayEnvelope.parse(event.content) else {
                    let text = withoutPreamble(event.content)
                    // A prompt that was only harness preamble.
                    guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
                    return SessionEventRow(event: event, text: text)
                }
                let body = normalized(envelope.body)
                guard !body.isEmpty else { return nil }
                if let id = envelope.messageID, relayTexts[id] == body { return nil }
                if !me.isEmpty, envelope.sender == me, let ts = event.ts,
                   pending.contains(where: { $0.text == body && pendingCopyWindow.contains(ts.timeIntervalSince($0.createdAt)) }) {
                    return nil
                }
                // A relay message the chat holds shows it already.
                guard !seen.contains(relayKey(incoming: true, envelope.body)) else { return nil }
                return SessionEventRow(event: event, text: envelope.body, mark: .from(trusted(envelope.sender)))
            }
            if let incoming = RelayLines.incoming(shown) {
                guard seen.insert(relayKey(incoming: true, incoming.body)).inserted else { return nil }
                return SessionEventRow(event: event, text: incoming.body, mark: .from(trusted(incoming.sender)))
            }
            if role != .toolResult, let outgoing = RelayLines.outgoing(shown) {
                guard seen.insert(relayKey(incoming: false, outgoing.body)).inserted else { return nil }
                return SessionEventRow(event: event, text: outgoing.body, mark: .to(outgoing.recipient))
            }
            guard role == .agent else { return SessionEventRow(event: event, text: shown) }
            let text = withoutPreamble(event.content)
            guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return nil }
            return SessionEventRow(event: event, text: text)
        }
    }

    fileprivate static func normalized(_ text: String) -> String {
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
