import XCTest
@testable import AgentRelayiPad

final class AgentListOrderTests: XCTestCase {
    /// The start of a five-minute bucket (1_800_000_000 / 300 is whole).
    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    private func entry(_ name: String, _ activity: ChatActivity = .idle,
                       secondsAgo: TimeInterval? = 0) -> AgentListOrder.Entry {
        AgentListOrder.Entry(name: name, activity: activity,
                             lastActivity: secondsAgo.map { now.addingTimeInterval(-$0) }, id: name)
    }

    private func order(_ entries: [AgentListOrder.Entry], pinned: Set<String> = []) -> [String] {
        AgentListOrder.sorted(entries, pinned: { pinned.contains($0.name) }, entry: { $0 }).map(\.name)
    }

    func testWorkingFirstThenMostRecentThenName() {
        let entries = [
            entry("offline-new", .offline, secondsAgo: 0),
            entry("idle-old", .idle, secondsAgo: 3_600),
            entry("idle-Beta", .idle, secondsAgo: 30),
            entry("idle-alpha", .idle, secondsAgo: 60),
            entry("working-old", .working, secondsAgo: 7_200),
        ]
        XCTAssertEqual(order(entries), ["working-old", "idle-alpha", "idle-Beta", "idle-old", "offline-new"])
        XCTAssertEqual(ChatActivity.working.rank, 0)
        XCTAssertEqual(ChatActivity.idle.rank, 1)
        XCTAssertEqual(ChatActivity.offline.rank, 2)
    }

    func testAHeartbeatInsideFiveMinutesDoesNotReorder() {
        // Both in the bucket that starts at `now`; "zeta" heartbeats last.
        var zeta = entry("zeta", secondsAgo: -10)
        let alpha = entry("alpha", secondsAgo: -200)
        XCTAssertEqual(order([zeta, alpha]), ["alpha", "zeta"])
        zeta.lastActivity = now.addingTimeInterval(299)
        XCTAssertEqual(order([zeta, alpha]), ["alpha", "zeta"])
        XCTAssertEqual(AgentListOrder.bucket(now), 6_000_000)
        XCTAssertEqual(AgentListOrder.bucket(now.addingTimeInterval(299.9)), 6_000_000)
        XCTAssertEqual(AgentListOrder.bucket(now.addingTimeInterval(-0.001)), 5_999_999)
        // Crossing into the next bucket does move it.
        zeta.lastActivity = now.addingTimeInterval(300)
        XCTAssertEqual(order([alpha, zeta]), ["zeta", "alpha"])
    }

    func testMissingTimeSortsLast() {
        let entries = [entry("aaa", secondsAgo: nil), entry("old", secondsAgo: 86_400), entry("new")]
        XCTAssertEqual(order(entries), ["new", "old", "aaa"])
        // Rank still comes first: a working agent with no time leads.
        XCTAssertEqual(order(entries + [entry("zz", .working, secondsAgo: nil)]), ["zz", "new", "old", "aaa"])
    }

    func testPinnedStayFirst() {
        let entries = [
            entry("worker", .working),
            entry("pinned-offline", .offline),
            entry("idle"),
            entry("pinned-working", .working, secondsAgo: 3_600),
        ]
        XCTAssertEqual(order(entries, pinned: ["pinned-offline", "pinned-working"]),
                       ["pinned-working", "pinned-offline", "worker", "idle"])
        XCTAssertEqual(order([]), [])
    }

    func testActivityFromTurnThenLoadedSessionEvents() {
        func activity(_ turn: AgentTurn?, reachable: Bool = true, eventSecondsAgo: TimeInterval? = nil) -> ChatActivity {
            AgentListOrder.activity(turn: turn, reachable: reachable,
                                    lastEventAt: eventSecondsAgo.map { now.addingTimeInterval(-$0) }, now: now)
        }
        // Not reachable is offline, whatever turn it reports or wrote.
        XCTAssertEqual(activity(.working, reachable: false, eventSecondsAgo: 1), .offline)
        XCTAssertEqual(activity(nil, reachable: false), .offline)
        // A reported turn decides.
        XCTAssertEqual(activity(.working), .working)
        XCTAssertEqual(activity(.working, eventSecondsAgo: 3_600), .working)
        // An idle turn beats a recent event.
        XCTAssertEqual(activity(.idle, eventSecondsAgo: 1), .idle)
        // With no turn, a loaded session that wrote within two minutes is working.
        XCTAssertEqual(activity(nil, eventSecondsAgo: 120), .working)
        XCTAssertEqual(activity(nil, eventSecondsAgo: 121), .idle)
        XCTAssertEqual(activity(nil), .idle)
        // The reported value: case and spaces do not matter; an unknown one is no turn.
        XCTAssertEqual(AgentTurn(reported: " Working "), .working)
        XCTAssertEqual(AgentTurn(reported: "idle"), .idle)
        XCTAssertNil(AgentTurn(reported: "waiting"))
        XCTAssertNil(AgentTurn(reported: ""))
        XCTAssertEqual(ChatActivity.working.label, "Working")
        XCTAssertEqual(ChatActivity.idle.label, "Idle")
        XCTAssertEqual(ChatActivity.offline.label, "Offline")
    }
}
