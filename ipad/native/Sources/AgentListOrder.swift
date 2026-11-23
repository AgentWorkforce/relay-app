import Foundation

/// What an agent is doing now, as the list's badge shows it. The desktop
/// Chat page also has Ended; iOS has no signal that a session ended, so it
/// is left out.
enum ChatActivity: Equatable, Sendable {
    /// Waiting or blocked on its turn, or a loaded session wrote within the
    /// last two minutes.
    case working
    /// Reachable and ready for a message.
    case idle
    /// Not reachable now; messages wait.
    case offline

    /// The list's order: working first, then idle, then offline.
    var rank: Int {
        switch self {
        case .working: 0
        case .idle: 1
        case .offline: 2
        }
    }

    var label: String {
        switch self {
        case .working: "Working"
        case .idle: "Idle"
        case .offline: "Offline"
        }
    }
}

/// The agent list's order, the desktop Chat page's: by activity, then by
/// recency in five-minute buckets, then by name. Agents heartbeat every few
/// seconds; ordering by the exact time would reshuffle the list on each
/// refresh, so a time only moves a row when it crosses a bucket.
enum AgentListOrder {
    /// How recently a loaded session must have written to count as working.
    static let workingWindow: TimeInterval = 120
    /// How coarsely recency orders agents.
    static let bucketLength: TimeInterval = 300

    /// What one row is ordered by.
    struct Entry: Equatable {
        var name: String
        var activity: ChatActivity
        /// The newest time the client knows the agent was active; nil when
        /// it knows none.
        var lastActivity: Date?
        /// Breaks a tie between equal names, so the order never depends on
        /// the input's.
        var id = ""
    }

    /// The activity from what iOS has: the record's `status`, whether the
    /// agent is reachable (`RelayAgent.isReachable`), and the newest event
    /// of its session when that session is already loaded. iOS has no turn
    /// state from a core and no roster `activity.turn`; a `waiting` or
    /// `blocked` status stands for the turn the desktop core reports as
    /// waiting, which the desktop also counts as working.
    static func activity(status: String, reachable: Bool, lastEventAt: Date?, now: Date) -> ChatActivity {
        guard reachable else { return .offline }
        switch status.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() {
        case "waiting", "blocked": return .working
        default: break
        }
        if let lastEventAt, now.timeIntervalSince(lastEventAt) <= workingWindow { return .working }
        return .idle
    }

    /// `date`'s five-minute bucket: `floor(ms / 300000)`.
    static func bucket(_ date: Date) -> Int {
        Int((date.timeIntervalSince1970 / bucketLength).rounded(.down))
    }

    /// Whether `a` comes before `b`: lower rank, then the newer bucket
    /// (a missing time last), then the lowercase name.
    static func precedes(_ a: Entry, _ b: Entry) -> Bool {
        if a.activity.rank != b.activity.rank { return a.activity.rank < b.activity.rank }
        let left = a.lastActivity.map(bucket), right = b.lastActivity.map(bucket)
        if left != right {
            switch (left, right) {
            case let (l?, r?): return l > r
            case (_?, nil): return true
            default: return false
            }
        }
        let leftName = a.name.lowercased(), rightName = b.name.lowercased()
        if leftName != rightName { return leftName < rightName }
        return a.id < b.id
    }

    /// `rows` with pinned rows first, each group in the list's order.
    static func sorted<Row>(_ rows: [Row], pinned: (Row) -> Bool = { _ in false },
                            entry: (Row) -> Entry) -> [Row] {
        let keyed = rows.map { (row: $0, pinned: pinned($0), entry: entry($0)) }
        return keyed.sorted { a, b in
            a.pinned != b.pinned ? a.pinned : precedes(a.entry, b.entry)
        }.map(\.row)
    }
}
