import Foundation

/// One stored event of an agent's session, as Cloud's
/// `GET /workspaces/:ws/reflex/sessions/:id/events` returns it. The kind and
/// role values are not published yet, so every field is optional on the wire
/// and an unknown kind reads as a plain activity row.
struct SessionEvent: Identifiable, Hashable, Codable, Sendable {
    /// What identifies one event across pages: the same event read twice
    /// (a poll overlapping the last page) merges into one row.
    struct Key: Hashable, Codable, Comparable, Sendable {
        var source: String
        var machineID: String
        var kind: String
        var eventID: String

        static func < (lhs: Key, rhs: Key) -> Bool {
            (lhs.source, lhs.machineID, lhs.kind, lhs.eventID) < (rhs.source, rhs.machineID, rhs.kind, rhs.eventID)
        }
    }

    enum Role: String, Codable, Sendable {
        /// A prompt the person (or a relayed sender) gave the agent.
        case person
        /// Text the agent wrote.
        case agent
        /// A tool call or its result.
        case tool
        /// Anything else: an unknown kind, a system note.
        case activity
    }

    /// The longest content kept in memory for one event. A tool result can be
    /// megabytes; the row shows its start and says it was cut.
    static let maximumStoredContent = 16_000

    let key: Key
    let ts: Date?
    let kind: String
    let type: String
    let actorRole: String
    let actorName: String
    let content: String
    /// The server or this app cut the content.
    let contentTruncated: Bool
    let toolName: String
    let toolStatus: String
    let filesTouched: [String]
    let subagentID: String

    var id: String { "\(key.source)|\(key.machineID)|\(key.kind)|\(key.eventID)" }

    init(key: Key, ts: Date?, kind: String = "", type: String = "", actorRole: String = "", actorName: String = "",
         content: String = "", contentTruncated: Bool = false, toolName: String = "", toolStatus: String = "",
         filesTouched: [String] = [], subagentID: String = "") {
        self.key = key
        self.ts = ts
        self.kind = kind
        self.type = type
        self.actorRole = actorRole
        self.actorName = actorName
        let cut = content.count > Self.maximumStoredContent
        self.content = cut ? String(content.prefix(Self.maximumStoredContent)) : content
        self.contentTruncated = contentTruncated || cut
        self.toolName = toolName
        self.toolStatus = toolStatus
        self.filesTouched = filesTouched
        self.subagentID = subagentID
    }

    private static let toolKinds: Set<String> = ["tool_use", "tool_call", "tool_result", "tool"]
    private static let personKinds: Set<String> = ["prompt", "user", "user_message"]
    private static let agentKinds: Set<String> = ["assistant", "assistant_message", "agent_message", "response"]

    var role: Role {
        let kind = kind.lowercased(), actor = actorRole.lowercased()
        if !toolName.isEmpty || Self.toolKinds.contains(kind) || actor == "tool_result" || actor == "tool" { return .tool }
        if actor == "user" || Self.personKinds.contains(kind) { return .person }
        // The agent thinking out loud is activity, not a reply.
        if kind.contains("reason") || kind.contains("think") { return .activity }
        if actor == "assistant" || Self.agentKinds.contains(kind) { return .agent }
        return .activity
    }

    /// Whether this is the agent's own activity (its tools, its thinking),
    /// which says it is still working.
    var isAgentActivity: Bool {
        switch role {
        case .tool: true
        case .activity: actorRole.lowercased() == "assistant" || actorRole.isEmpty
        case .agent: content.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        case .person: false
        }
    }
}

/// One page of a session's events.
struct SessionEventPage: Equatable, Sendable {
    let sessionID: String
    let events: [SessionEvent]
    /// The cursor for the next page in the requested order; nil on the last.
    let nextCursor: String?

    enum DecodeError: Error, Equatable {
        /// Cloud answered for another session.
        case wrongSession
        /// The body is not a page at all.
        case malformed
    }

    /// Reads a page leniently: an event with missing or mistyped fields keeps
    /// what it has, and only an event with nothing to key it by is dropped.
    /// A body for another session, or one that is not an object with an
    /// `events` array, throws.
    static func decode(_ data: Data, sessionID: String) throws -> SessionEventPage {
        guard let body = try? JSONDecoder().decode(Body.self, from: data) else { throw DecodeError.malformed }
        guard body.sessionId == sessionID else { throw DecodeError.wrongSession }
        return SessionEventPage(sessionID: sessionID, events: body.events.compactMap(\.event), nextCursor: body.nextCursor)
    }

    private struct Body: Decodable {
        let sessionId: String
        let events: [LenientEvent]
        let nextCursor: String?

        enum CodingKeys: String, CodingKey { case sessionId, events, nextCursor }
        init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            sessionId = try container.decode(String.self, forKey: .sessionId)
            events = try container.decode([LenientEvent].self, forKey: .events)
            let cursor = try? container.decodeIfPresent(String.self, forKey: .nextCursor)
            nextCursor = (cursor ?? nil).flatMap { $0.isEmpty ? nil : $0 }
        }
    }

    /// Never fails to decode, so one odd event cannot drop its page.
    private struct LenientEvent: Decodable {
        let event: SessionEvent?

        private struct AnyKey: CodingKey {
            var stringValue: String
            var intValue: Int? { nil }
            init(_ string: String) { stringValue = string }
            init?(stringValue: String) { self.stringValue = stringValue }
            init?(intValue: Int) { nil }
        }

        init(from decoder: Decoder) throws {
            guard let container = try? decoder.container(keyedBy: AnyKey.self) else {
                event = nil
                return
            }
            func text(_ name: String) -> String {
                let key = AnyKey(name)
                if let value = try? container.decode(String.self, forKey: key) { return value }
                if let value = try? container.decode(Int64.self, forKey: key) { return String(value) }
                return ""
            }
            let ts: Date? = {
                let key = AnyKey("ts")
                if let value = try? container.decode(String.self, forKey: key) { return SessionEventPage.date(value) }
                if let millis = try? container.decode(Double.self, forKey: key) { return Date(timeIntervalSince1970: millis / 1000) }
                return nil
            }()
            let kind = text("kind"), content = text("content")
            var eventID = text("eventId")
            if eventID.isEmpty {
                // No id: key it by what it says, so the same event read on
                // two pages still merges. Without a time there is nothing.
                guard let ts else {
                    event = nil
                    return
                }
                eventID = "ts:\(Int64(ts.timeIntervalSince1970 * 1000)):\(SessionEventPage.fingerprint(kind + "\u{1F}" + content))"
            }
            let files = ((try? container.decode([LenientString].self, forKey: AnyKey("filesTouched"))) ?? [])
                .compactMap(\.value).filter { !$0.isEmpty }
            event = SessionEvent(
                key: .init(source: text("source"), machineID: text("machineId"), kind: kind, eventID: eventID),
                ts: ts, kind: kind, type: text("type"), actorRole: text("actorRole"), actorName: text("actorName"),
                content: content, contentTruncated: (try? container.decode(Bool.self, forKey: AnyKey("contentTruncated"))) ?? false,
                toolName: text("toolName"), toolStatus: text("toolStatus"), filesTouched: files,
                subagentID: text("subagentId"))
        }
    }

    private struct LenientString: Decodable {
        let value: String?
        init(from decoder: Decoder) throws { value = try? decoder.singleValueContainer().decode(String.self) }
    }

    static func date(_ value: String) -> Date? {
        (try? Date.ISO8601FormatStyle(includingFractionalSeconds: true).parse(value))
            ?? (try? Date.ISO8601FormatStyle().parse(value))
    }

    /// A stable FNV-1a hash; Swift's `Hasher` changes every launch.
    static func fingerprint(_ text: String) -> String {
        var hash: UInt64 = 0xcbf2_9ce4_8422_2325
        for byte in text.utf8 {
            hash ^= UInt64(byte)
            hash = hash &* 0x0000_0100_0000_01b3
        }
        return String(hash, radix: 16)
    }
}

/// The events request. The route has no tail and no `since`: the app reads
/// the newest page (`order=desc`) and follows its cursor to older pages.
enum SessionEventsRequest {
    enum Order: String, Sendable { case asc, desc }

    static let pageLimit = 100
    static let maximumLimit = 1000

    struct Spec: Equatable, Sendable {
        let order: Order
        let limit: Int
        let cursor: String?
    }

    static let newest = Spec(order: .desc, limit: pageLimit, cursor: nil)

    /// `/workspaces/<ws>/reflex/sessions/<id>/events?…`, or nil when the
    /// workspace is not a Cloud workspace id or the session id is unusable.
    static func path(workspaceID: String, sessionID: String, spec: Spec) -> String? {
        guard UUID(uuidString: workspaceID) != nil, !sessionID.isEmpty, sessionID.count <= 512 else { return nil }
        let segment = CharacterSet.urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))
        let query = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-._~"))
        guard let session = sessionID.addingPercentEncoding(withAllowedCharacters: segment) else { return nil }
        var items = ["limit=\(min(max(spec.limit, 1), maximumLimit))", "order=\(spec.order.rawValue)"]
        if let cursor = spec.cursor, !cursor.isEmpty {
            guard let encoded = cursor.addingPercentEncoding(withAllowedCharacters: query) else { return nil }
            items.append("cursor=\(encoded)")
        }
        return "/workspaces/\(workspaceID.lowercased())/reflex/sessions/\(session)/events?" + items.joined(separator: "&")
    }

    /// The server no longer has the session: its cached copy is deleted;
    /// what is on screen stays until the chat closes.
    static func sessionGone(status: Int?) -> Bool { status == 404 }

    /// This account may no longer read the session (403, or a 401 that
    /// outlived the credential refresh): its events leave the screen and disk.
    static func accessRevoked(status: Int?) -> Bool { status == 401 || status == 403 }

    /// The server dropped the cursor (409 `cursor_backend_transition`, 410
    /// `cursor_expired`, 501 `canonical_cursor_scope`): paging restarts
    /// from the newest page.
    static func cursorLost(status: Int?, code: String) -> Bool {
        status == 410 || status == 409 || code == "cursor_expired" || code == "cursor_backend_transition"
            || (status == 501 && code == "canonical_cursor_scope")
    }

    /// The server will not serve this session's events (any other 501, with
    /// or without a code, since the route in front may drop it). The answer
    /// is final: the store reads the session no more until it is dropped.
    static func unavailable(status: Int?, code: String) -> Bool {
        status == 501 && !cursorLost(status: status, code: code)
    }
}

/// What a session's event store holds, as one Codable value: what the
/// on-disk cache reads and writes.
struct SessionEventSnapshot: Codable, Equatable, Sendable {
    var sessionID: String
    var events: [SessionEvent]
    var olderCursor: String?
    var reachedStart: Bool

    /// The newest `count` events. Dropping older ones loses the place the
    /// older cursor points after, so paging restarts from the newest page.
    func keepingNewest(_ count: Int) -> SessionEventSnapshot {
        guard events.count > count else { return self }
        return SessionEventSnapshot(sessionID: sessionID, events: Array(events.suffix(max(count, 0))),
                                    olderCursor: nil, reachedStart: false)
    }
}

/// Where snapshots persist between launches: the one persistence seam.
/// `save`, `remove` and `removeAll` keep their order; `save` returns at once.
protocol SessionEventSnapshotStore: AnyObject, Sendable {
    func load(workspaceID: String, sessionID: String) async -> SessionEventSnapshot?
    func save(_ snapshot: SessionEventSnapshot, workspaceID: String)
    func remove(workspaceID: String, sessionID: String)
    func removeAll()
}

/// Keeps nothing: UI tests use it so every launch starts from the fixture.
final class MemoryOnlySessionEventSnapshots: SessionEventSnapshotStore, @unchecked Sendable {
    func load(workspaceID: String, sessionID: String) async -> SessionEventSnapshot? { nil }
    func save(_ snapshot: SessionEventSnapshot, workspaceID: String) {}
    func remove(workspaceID: String, sessionID: String) {}
    func removeAll() {}
}

/// One session's events in time order, merged from pages by key.
struct SessionEventStore: Equatable, Sendable {
    let sessionID: String
    /// Oldest first, by time and then key.
    private(set) var events: [SessionEvent] = []
    /// The cursor for the next older page; nil before the first page and
    /// after a lost cursor.
    private(set) var olderCursor: String?
    /// The oldest page has been read.
    private(set) var reachedStart = false
    /// A newest page has been read (or restored from a snapshot).
    private(set) var loaded = false
    /// Failed reads in a row; a success resets it.
    private(set) var failures = 0
    /// The last read failed and the store's paging has to restart.
    private var restarting = false
    /// Restarts after a lost cursor since older paging last added events.
    private var restarts = 0
    /// Older pages stop loading on their own (a tap still loads one) after
    /// an older page added nothing, an older read failed, or the cursor was
    /// lost twice in a row. An older page that adds events resumes them.
    private(set) var olderAutoLoadStopped = false
    /// The server answered that it will not serve this session (a 501):
    /// no further read, newest or older, and no timer. In memory only; it
    /// ends when the store is dropped and is not part of the snapshot.
    private(set) var unavailable = false
    private var keys: Set<SessionEvent.Key> = []

    init(sessionID: String) { self.sessionID = sessionID }

    init(snapshot: SessionEventSnapshot) {
        sessionID = snapshot.sessionID
        olderCursor = snapshot.olderCursor
        reachedStart = snapshot.reachedStart
        loaded = true
        // A trimmed snapshot has no cursor: older paging restarts from the newest page.
        restarting = snapshot.olderCursor == nil && !snapshot.reachedStart
        merge(snapshot.events)
    }

    var snapshot: SessionEventSnapshot {
        SessionEventSnapshot(sessionID: sessionID, events: events, olderCursor: olderCursor, reachedStart: reachedStart)
    }

    var newest: SessionEvent? { events.last }
    var hasOlder: Bool { loaded && !reachedStart && !unavailable }

    /// The request that reads the next older page: the newest page again
    /// when the cursor was lost, nil when the start was reached or the
    /// session is unavailable.
    var olderRequest: SessionEventsRequest.Spec? {
        guard loaded, !reachedStart, !unavailable else { return nil }
        guard let olderCursor, !restarting else { return SessionEventsRequest.newest }
        return .init(order: .desc, limit: SessionEventsRequest.pageLimit, cursor: olderCursor)
    }

    /// Merges a page read with `spec`. A newest page sets the older cursor
    /// the first time, after a restart, and when it shares no event with
    /// what is held (a gap: older pages are read again and merge in).
    /// Returns whether anything a snapshot holds changed.
    @discardableResult
    mutating func apply(_ page: SessionEventPage, for spec: SessionEventsRequest.Spec) -> Bool {
        guard page.sessionID == sessionID else { return false }
        failures = 0
        let before = (olderCursor, reachedStart)
        if spec.cursor == nil {
            let overlaps = page.events.contains { keys.contains($0.key) }
            if !loaded || restarting || (!overlaps && !events.isEmpty && !page.events.isEmpty) {
                olderCursor = page.nextCursor
                reachedStart = page.nextCursor == nil
            }
            loaded = true
            restarting = false
        }
        let countBefore = events.count
        let merged = merge(page.events)
        if spec.cursor != nil {
            olderCursor = page.nextCursor
            reachedStart = page.nextCursor == nil
            if events.count > countBefore {
                olderAutoLoadStopped = false
                restarts = 0
            } else {
                olderAutoLoadStopped = true
            }
        }
        return merged || before.0 != olderCursor || before.1 != reachedStart
    }

    /// Records a failed read. A lost cursor restarts paging from the newest
    /// page; a 501 makes the store final. What is held stays.
    /// `older`: the read was for older history.
    mutating func failed(status: Int?, code: String, older: Bool = false) {
        failures += 1
        if older { olderAutoLoadStopped = true }
        if SessionEventsRequest.unavailable(status: status, code: code) {
            unavailable = true
            return
        }
        if SessionEventsRequest.cursorLost(status: status, code: code) {
            restarting = true
            olderCursor = nil
            restarts += 1
            if restarts >= 2 { olderAutoLoadStopped = true }
        }
    }

    /// Drops every event and cursor, keeping the failure count, so the chat
    /// shows nothing from a session this account may no longer read.
    mutating func forgetEvents() {
        events = []
        keys = []
        olderCursor = nil
        reachedStart = false
        loaded = false
        restarting = false
        restarts = 0
        olderAutoLoadStopped = false
    }

    @discardableResult
    private mutating func merge(_ incoming: [SessionEvent]) -> Bool {
        var changed = false
        for event in incoming {
            if keys.contains(event.key) {
                guard let index = events.firstIndex(where: { $0.key == event.key }), events[index] != event else { continue }
                events[index] = event
            } else {
                keys.insert(event.key)
                events.append(event)
            }
            changed = true
        }
        if changed { events.sort(by: Self.ordered) }
        return changed
    }

    static func ordered(_ lhs: SessionEvent, _ rhs: SessionEvent) -> Bool {
        let left = lhs.ts ?? .distantPast, right = rhs.ts ?? .distantPast
        return left != right ? left < right : lhs.key < rhs.key
    }
}

/// How often the open chat re-reads its session's newest page, by how old
/// the session's activity is.
enum SessionEventPolling {
    static let activeInterval: TimeInterval = 5
    static let idleInterval: TimeInterval = 15
    static let quietInterval: TimeInterval = 60
    static let dormantInterval: TimeInterval = 5 * 60
    static let activeWindow: TimeInterval = 2 * 60
    static let idleWindow: TimeInterval = 60 * 60
    static let quietWindow: TimeInterval = 24 * 60 * 60
    /// At this age or older there is no timer.
    static let dormantWindow: TimeInterval = 7 * 24 * 60 * 60
    /// With nothing loaded and no chat messages.
    static let unknownInterval: TimeInterval = idleInterval
    static let maximumBackoff: TimeInterval = 2 * 60
    /// The shortest gap between two newest-page reads of one session, so a
    /// burst of chat messages reads at most once per interval.
    static let minimumSpacing: TimeInterval = activeInterval

    /// Age under the window → interval, youngest first.
    static let tiers: [(window: TimeInterval, interval: TimeInterval)] = [
        (activeWindow, activeInterval), (idleWindow, idleInterval),
        (quietWindow, quietInterval), (dormantWindow, dormantInterval),
    ]

    /// The wait before the next read, or nil for no timer. The reference is
    /// the later of the newest event and the chat's newest message: 5 s
    /// under 2 minutes old, 15 s under an hour, 60 s under a day, 5 minutes
    /// under a week, no timer after; 15 s with no reference. Failures in a
    /// row double it, up to 2 minutes, never below the base. An unavailable
    /// session has no timer.
    static func interval(newestEventAt newest: Date?, chatActivityAt chat: Date? = nil, now: Date, failures: Int,
                         unavailable: Bool = false) -> TimeInterval? {
        guard !unavailable, let base = baseInterval(reference: [newest, chat].compactMap { $0 }.max(), now: now) else {
            return nil
        }
        guard failures > 0 else { return base }
        return max(base, min(base * pow(2, Double(min(failures, 10))), maximumBackoff))
    }

    static func baseInterval(reference: Date?, now: Date) -> TimeInterval? {
        guard let reference else { return unknownInterval }
        let age = now.timeIntervalSince(reference)
        return tiers.first { age < $0.window }?.interval
    }
}

/// Whether the agent looks busy. The API has no in-progress flag, so this
/// is read from the newest event: the agent's own tool call or activity
/// under a minute old, with no reply of its own after it.
enum SessionWorking {
    static let window: TimeInterval = 60

    struct State: Equatable {
        /// The latest tool in the run, empty when none was named.
        let tool: String
        var label: String { tool.isEmpty ? "Working…" : "Working: \(tool)" }
    }

    static func state(events: [SessionEvent], now: Date) -> State? {
        guard let newest = events.last, newest.isAgentActivity, let ts = newest.ts,
              now.timeIntervalSince(ts) < window, now.timeIntervalSince(ts) > -window else { return nil }
        for event in events.reversed() {
            if !event.toolName.isEmpty { return State(tool: event.toolName) }
            if !event.isAgentActivity { break }
        }
        return State(tool: "")
    }
}
