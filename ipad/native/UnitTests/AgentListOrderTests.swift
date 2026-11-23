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

    func testActivityFromStatusThenLoadedSessionEvents() {
        func activity(_ status: String, reachable: Bool = true, eventSecondsAgo: TimeInterval? = nil) -> ChatActivity {
            AgentListOrder.activity(status: status, reachable: reachable,
                                    lastEventAt: eventSecondsAgo.map { now.addingTimeInterval(-$0) }, now: now)
        }
        // Not reachable is offline, whatever it reports or wrote.
        XCTAssertEqual(activity("waiting", reachable: false, eventSecondsAgo: 1), .offline)
        XCTAssertEqual(activity("", reachable: false), .offline)
        // Waiting or blocked on its turn counts as working.
        XCTAssertEqual(activity("waiting"), .working)
        XCTAssertEqual(activity(" Blocked "), .working)
        // A loaded session that wrote within two minutes is working.
        XCTAssertEqual(activity("active", eventSecondsAgo: 120), .working)
        XCTAssertEqual(activity("idle", eventSecondsAgo: 5), .working)
        XCTAssertEqual(activity("active", eventSecondsAgo: 121), .idle)
        // Reachable with no loaded events is idle.
        XCTAssertEqual(activity("active"), .idle)
        XCTAssertEqual(activity("online"), .idle)
        XCTAssertEqual(ChatActivity.working.label, "Working")
        XCTAssertEqual(ChatActivity.idle.label, "Idle")
        XCTAssertEqual(ChatActivity.offline.label, "Offline")
    }
}
