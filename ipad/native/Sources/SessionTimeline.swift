import Foundation
import RelaySessionOverview

/// That a chat row came through Agent Relay, and which way.
enum RelayMark: Equatable {
    /// A message to the agent. A relayed prompt names its sender when the
    /// chat already shows a relay message of theirs; nil when only session
    /// text names one. A message read from the inbox (a frame in a tool's
    /// output) always carries nil: the one frame whose sender could be
    /// trusted, the copy of a message the chat holds, is not shown at all.
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

/// One run of the agent's tool work: its tool calls and their results,
/// shown together as one "Activity" group.
struct ActivityGroup: Identifiable, Equatable {
    /// Oldest first; never empty.
    let rows: [SessionEventRow]

    /// Named after the run's newest row, so older tool work joining the run
    /// from an older page never changes it: the chat's scroll back to the
    /// row that was first before the page (`timeline.first?.id`) still
    /// finds the group, as one of the lazy stack's own rows. Newer tool work
    /// joining the newest run changes it, which only makes a new view of
    /// that group; its open state is kept by row (`ActivityExpansion`).
    var id: String { "activity:\(rows[rows.count - 1].event.id)" }

    /// One line of the group: the tool's name and what it worked on (the
    /// first line of its content, else the files it touched).
    static func line(for row: SessionEventRow) -> (title: String, target: String) {
        let event = row.event
        let title = !event.toolName.isEmpty ? event.toolName : event.entryRole == .toolResult ? "Result" : "Tool"
        let first = event.content.split(whereSeparator: \.isNewline)
            .lazy.map { $0.trimmingCharacters(in: .whitespaces) }.first { !$0.isEmpty } ?? ""
        let target = first.isEmpty ? SessionTimeline.filesSummary(event.filesTouched) : first
        return (title, target.count > 120 ? String(target.prefix(120)) + "…" : target)
    }
}

/// Which Activity groups are open, and which of their lines, kept by the
/// rows' ids rather than the group's: a group whose id changes when newer
/// tool work joins its run, or that gains older rows, stays as it was.
struct ActivityExpansion: Equatable {
    private(set) var expandedRows: Set<String> = []
    private(set) var openedRows: Set<String> = []

    /// Open when any of its rows was in the group when it was opened.
    func isExpanded(_ group: ActivityGroup) -> Bool {
        group.rows.contains { expandedRows.contains($0.id) }
    }

    /// Opening marks every row the group has; closing clears them all, so
    /// no row of a closed group keeps it open.
    mutating func setExpanded(_ expanded: Bool, _ group: ActivityGroup) {
        for row in group.rows {
            if expanded { expandedRows.insert(row.id) } else { expandedRows.remove(row.id) }
        }
    }

    func isOpened(_ row: SessionEventRow) -> Bool { openedRows.contains(row.id) }

    /// Sets, rather than flips, so applying one change to several copies
    /// leaves them all the same.
    mutating func setOpened(_ row: SessionEventRow, _ opened: Bool) {
        if opened { openedRows.insert(row.id) } else { openedRows.remove(row.id) }
    }
}

/// One row of an agent's chat: a relay message, a session event, or a run
/// of tool work.
enum ChatTimelineItem: Identifiable {
    case message(RelayMessage)
    case event(SessionEventRow)
    case activity(ActivityGroup)

    /// A relay message keeps its own id, so delivery, pending reconciliation
    /// and scrolling see the same ids as before session events were shown.
    var id: String {
        switch self {
        case .message(let message): message.id
        case .event(let row): row.id
        case .activity(let group): group.id
        }
    }

    var date: Date? {
        switch self {
        case .message(let message): message.createdAt
        case .event(let row): row.event.ts
        case .activity(let group): group.rows[0].event.ts
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
    /// Events that only copy another are left out (`ChatEntryRules.withoutCopies`):
    /// the provider's prompt log (`history` events), a hook's context stored as
    /// a person's event, and a turn the transcript holds twice a moment apart.
    /// The stored events stay as they are; this is only how the chat reads them.
    ///
    /// A relay message inside the session is shown as one, marked
    /// (`RelayMark`): a relayed prompt and a relay frame in a tool's output
    /// (received), and a relay send in a tool call or the agent's text
    /// (sent). A frame in the agent's own text is its text.
    ///
    /// A relayed prompt or frame that is the session's copy of a relay
    /// message the chat holds is left out, so the message shows once: its
    /// envelope names that message and its text is that message's text. So
    /// is a prompt that copies a message the composer still shows as
    /// pending: sent by this device (`me`), with the pending text, within
    /// `pendingCopyWindow` of it.
    ///
    /// Frames and sends are also shown once by `relayKey` (direction, the
    /// other side as a bare handle, which matches any when one side does
    /// not name it, and text) within `relayRepeatWindow`, or at any distance
    /// when either has no time, counting the relay
    /// messages the chat holds: one to the agent (sent by `me`) is "in" from
    /// its sender, one from the agent "out" to `me`. So a send recorded
    /// twice shows once, while the same text from someone else, to someone
    /// else, or long after shows again.
    static func items(messages: [RelayMessage], events: [SessionEvent],
                      pending: [Pending] = [], me: String = "") -> [ChatTimelineItem] {
        var relayTexts: [String: String] = [:]
        for message in messages where message.hasServerID { relayTexts[message.id] = normalized(message.text) }
        var reader = Reader(relayTexts: relayTexts,
                            pending: pending.map { (text: normalized($0.text), createdAt: $0.createdAt) }, me: me)
        if !events.isEmpty {
            for message in messages {
                reader.senders.insert(message.sender)
                let key = isToAgent(message, me: me)
                    ? relayKey(incoming: true, counterpart: message.sender.isEmpty ? me : message.sender, message.text)
                    : relayKey(incoming: false, counterpart: me, message.text)
                reader.seen.record(key, at: message.createdAt)
            }
        }
        var rows: [SessionEventRow] = []
        for event in ChatEntryRules.withoutToolEchoes(ChatEntryRules.withoutCopies(events)) {
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

    /// `items` with each run of tool rows (calls and their results, not a
    /// relay message carried by one) folded into one `ActivityGroup`. Any
    /// other row ends the run: a message, a reply, a relay message, an
    /// activity note.
    static func grouped(_ items: [ChatTimelineItem]) -> [ChatTimelineItem] {
        var result: [ChatTimelineItem] = []
        var run: [SessionEventRow] = []
        func close() {
            if !run.isEmpty { result.append(.activity(ActivityGroup(rows: run))) }
            run = []
        }
        for item in items {
            if case .event(let row) = item, row.mark == nil, row.event.role == .tool {
                run.append(row)
                continue
            }
            close()
            result.append(item)
        }
        close()
        return result
    }

    /// Whether a relay message the chat holds went to the agent: sent by
    /// this device's handle (`me`), or marked as this device's own.
    static func isToAgent(_ message: RelayMessage, me: String) -> Bool {
        message.mine || (!me.isEmpty && message.sender == me)
    }

    /// How far apart two records of one relay message can be and still be
    /// taken for one: further apart, the same text is a new message (the
    /// desktop's window).
    static let relayRepeatWindow: TimeInterval = 10 * 60

    /// What tells one relay message from another in the chat (the
    /// desktop's `relay_seen`).
    struct RelayKey: Equatable {
        let incoming: Bool
        /// The other side as a bare handle: the sender of a message to the
        /// agent, the recipient of one from it. Empty when not known.
        let counterpart: String
        /// The text with whitespace runs made one space.
        let text: String

        /// The same direction and text, and the same other side, or one of
        /// them not known.
        func matches(_ other: RelayKey) -> Bool {
            incoming == other.incoming && text == other.text
                && (counterpart.isEmpty || other.counterpart.isEmpty || counterpart == other.counterpart)
        }
    }

    /// The key of a relay message: its direction, the other side reduced
    /// to a bare handle (no leading "@", nothing from an "@host" on, so
    /// `--address pilot@harbor` and `--to @pilot` are both "pilot"; a
    /// channel keeps its "#") and its text.
    static func relayKey(incoming: Bool, counterpart: String?, _ text: String) -> RelayKey {
        let handle = (counterpart ?? "").drop(while: { $0 == "@" }).prefix(while: { $0 != "@" })
        return RelayKey(incoming: incoming, counterpart: String(handle), text: normalized(text))
    }

    /// The `RelayKey`s shown so far and when.
    struct RelaySeen {
        /// By direction and text, so a lookup reads only that text's records.
        private var shown: [String: [(key: RelayKey, date: Date?)]] = [:]

        private static func bucket(_ key: RelayKey) -> String { (key.incoming ? "in:" : "out:") + key.text }

        /// Whether a key matching `key` was shown within `relayRepeatWindow`
        /// of `date`. When either has no time, the window cannot be told
        /// and the key alone decides.
        func isRepeat(_ key: RelayKey, at date: Date?) -> Bool {
            shown[Self.bucket(key), default: []].contains { seen in
                guard seen.key.matches(key) else { return false }
                guard let date, let seenDate = seen.date else { return true }
                return abs(date.timeIntervalSince(seenDate)) <= relayRepeatWindow
            }
        }

        mutating func record(_ key: RelayKey, at date: Date?) {
            shown[Self.bucket(key), default: []].append((key, date))
        }

        /// Records `key` unless it is a repeat; false for a repeat.
        mutating func show(_ key: RelayKey, at date: Date?) -> Bool {
            guard !isRepeat(key, at: date) else { return false }
            record(key, at: date)
            return true
        }
    }

    /// Reads events into rows in time order, remembering the relay
    /// messages already shown.
    private struct Reader {
        let relayTexts: [String: String]
        let pending: [(text: String, createdAt: Date)]
        let me: String
        /// Senders of relay messages the chat holds: the only names a
        /// relayed prompt's mark shows.
        var senders: Set<String> = []
        var seen = RelaySeen()

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
                // Shown: only the id and pending rules above take a prompt
                // for a copy. Recorded, so the inbox read of it shows once.
                seen.record(relayKey(incoming: true, counterpart: envelope.sender, body), at: event.ts)
                return SessionEventRow(event: event, text: envelope.body, mark: .from(trusted(envelope.sender)))
            }
            if role == .toolResult, let incoming = RelayLines.incoming(shown) {
                if let id = incoming.messageID, relayTexts[id] == normalized(incoming.body) { return nil }
                guard seen.show(relayKey(incoming: true, counterpart: incoming.sender, incoming.body), at: event.ts) else { return nil }
                return SessionEventRow(event: event, text: incoming.body, mark: .from(nil))
            }
            if role != .toolResult, let outgoing = RelayLines.outgoing(shown) {
                guard seen.show(relayKey(incoming: false, counterpart: outgoing.recipient, outgoing.body), at: event.ts) else { return nil }
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

/// Whether the agent is on its turn: the chat shows "Working…" at its end
/// while it is. The API has no in-progress flag, so this is read from what
/// the chat shows (the desktop's `chat_turn_open`).
enum SessionTurn {
    /// Past this age of the newest event the session is taken for idle or
    /// dead, and "Working…" is not shown.
    static let window: TimeInterval = 10 * 60
    /// The relay core's presence window: an agent not seen for longer is offline.
    static let presenceWindow: TimeInterval = 5 * 60

    /// Whether the agent's last row is not its own written reply. A message
    /// to the agent, a received relay message, a prompt and tool work leave
    /// the turn open; a reply, a relay message the agent sent and a relay
    /// message from it close it. Activity notes say nothing either way.
    /// Thinking, which the chat does not show, after the last row leaves it open.
    static func isOpen(_ items: [ChatTimelineItem], events: [SessionEvent], me: String = "") -> Bool {
        for item in items.reversed() {
            let open: Bool
            switch item {
            case .message(let message):
                open = SessionTimeline.isToAgent(message, me: me)
            case .activity:
                open = true
            case .event(let row):
                if let mark = row.mark {
                    open = mark.isIncoming
                } else {
                    switch row.event.role {
                    case .person, .tool: open = true
                    case .agent: open = false
                    case .activity: continue
                    }
                }
            }
            if !open, let date = item.date,
               events.contains(where: { $0.isThinking && ($0.ts ?? .distantPast) > date }) {
                return true
            }
            return open
        }
        return events.contains(where: \.isThinking)
    }

    /// Whether to show "Working…": the turn is open, the newest event is
    /// under `window` old, and the agent is not offline. With no events (the
    /// session is not available) it never shows.
    static func showsWorking(open: Bool, newestEventAt newest: Date?, offline: Bool, now: Date) -> Bool {
        guard open, !offline, let newest else { return false }
        let age = now.timeIntervalSince(newest)
        return age < window && age > -window
    }

    /// Whether the roster says the agent is gone: its status is offline, or
    /// it was last seen longer ago than the presence window. An unknown
    /// status is not offline.
    static func isOffline(status: String, lastSeen: Date?, now: Date) -> Bool {
        if status.lowercased() == "offline" { return true }
        if let lastSeen, now.timeIntervalSince(lastSeen) > presenceWindow { return true }
        return false
    }
}
