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

    func testEntryAgreesWithTheRowBadgeWithoutBuildingARow() {
        let seen = Date.ISO8601FormatStyle(includingFractionalSeconds: true).format(now.addingTimeInterval(-10))
        func agent(_ status: String, turn: String = "") -> RelayAgent {
            RelayAgent(relay: ["name": "worker", "status": status, "address": "worker@test-mini-123", "last_seen": seen,
                               "metadata": ["session_id": "session-1", "activity": ["turn": turn]]])
        }
        let recent = now.addingTimeInterval(-30)
        let cases: [(RelayAgent, Date?)] = [
            (agent("active", turn: "working"), nil), (agent("active", turn: "idle"), recent),
            (agent("active"), recent), (agent("active"), nil), (agent("waiting"), nil), (agent("offline"), recent),
        ]
        for (agent, lastEventAt) in cases {
            let entry = AgentListOrder.entry(for: agent, name: "worker", lastEventAt: lastEventAt, now: now)
            let badge = AgentRowModel(agent: agent, loaded: .init(lastEventAt: lastEventAt), now: now).activity
            XCTAssertEqual(entry.activity, badge)
        }
        // A person has no badge and is ranked from presence alone.
        let person = RelayAgent(relay: ["name": "sam-lee", "type": "human", "persona": "Sam Lee", "status": "active",
                                        "last_seen": seen])
        XCTAssertNil(AgentRowModel(agent: person, now: now).activity)
        XCTAssertEqual(AgentListOrder.entry(for: person, name: "Sam Lee", lastEventAt: recent, now: now).activity, .idle)
        // Recency is the newer of the reported activity time and the loaded
        // event, never the heartbeat.
        let entry = AgentListOrder.entry(for: agent("active"), name: "worker", lastEventAt: recent, now: now)
        XCTAssertEqual(entry.lastActivity, recent)
        XCTAssertEqual(entry.id, agent("active").id)
        XCTAssertNil(AgentListOrder.entry(for: agent("active"), name: "worker", lastEventAt: nil, now: now).lastActivity)
        let reported = RelayAgent(relay: ["name": "worker", "status": "active", "last_seen": seen,
                                          "metadata": ["activity": ["turn": "idle", "updated_ms": 1_799_999_900_000]]])
        XCTAssertEqual(AgentListOrder.entry(for: reported, name: "worker", lastEventAt: nil, now: now).lastActivity,
                       Date(timeIntervalSince1970: 1_799_999_900))
        XCTAssertEqual(AgentListOrder.entry(for: reported, name: "worker", lastEventAt: recent, now: now).lastActivity,
                       recent)
        XCTAssertEqual(AgentListOrder.entry(for: reported, name: "worker",
                                            lastEventAt: now.addingTimeInterval(-3_600), now: now).lastActivity,
                       Date(timeIntervalSince1970: 1_799_999_900))
    }

    private func rosterAgent(_ name: String, seenAt: Date, metadata: JSONObject = [:]) -> RelayAgent {
        RelayAgent(relay: ["name": name, "status": "active", "address": "\(name)@test-mini-123",
                           "last_seen": Date.ISO8601FormatStyle(includingFractionalSeconds: true).format(seenAt),
                           "metadata": metadata])
    }

    private func listOrder(_ agents: [RelayAgent], at time: Date) -> [String] {
        AgentListOrder.sorted(agents) { AgentListOrder.entry(for: $0, name: $0.name, lastEventAt: nil, now: time) }
            .map(\.name)
    }

    func testHeartbeatsAcrossABucketBoundaryDoNotReorder() {
        // `now` starts a bucket; the two heartbeats are 4 s apart across it.
        let alpha = rosterAgent("alpha", seenAt: now.addingTimeInterval(-2))
        let zeta = rosterAgent("zeta", seenAt: now.addingTimeInterval(2))
        XCTAssertNotEqual(AgentListOrder.bucket(alpha.lastSeen!), AgentListOrder.bucket(zeta.lastSeen!))
        XCTAssertEqual(listOrder([zeta, alpha], at: now.addingTimeInterval(10)), ["alpha", "zeta"])
        XCTAssertEqual(listOrder([alpha, zeta], at: now.addingTimeInterval(10)), ["alpha", "zeta"])
    }

    func testAnAgentWithNoActivityTimeSortsAfterOneWithATime() {
        let reported = rosterAgent("zeta", seenAt: now.addingTimeInterval(-60),
                                   metadata: ["activity": ["turn": "idle", "updated_ms": 1_799_000_000_000]])
        let silent = rosterAgent("alpha", seenAt: now)
        XCTAssertEqual(listOrder([silent, reported], at: now), ["zeta", "alpha"])
    }

    func testActivityTimeDecodesWholePositiveMilliseconds() {
        XCTAssertEqual(AgentListOrder.activityTime(NSNumber(value: 1_800_000_000_000)), now)
        XCTAssertEqual(AgentListOrder.activityTime(NSNumber(value: 1_800_000_000_000.0)), now)
        XCTAssertEqual(AgentListOrder.activityTime(NSNumber(value: 1)), Date(timeIntervalSince1970: 0.001))
        XCTAssertNil(AgentListOrder.activityTime(nil))
        XCTAssertNil(AgentListOrder.activityTime(NSNumber(value: 0)))
        XCTAssertNil(AgentListOrder.activityTime(NSNumber(value: -5)))
        XCTAssertNil(AgentListOrder.activityTime(NSNumber(value: 1.5)))
        XCTAssertNil(AgentListOrder.activityTime(NSNumber(value: Double.nan)))
        XCTAssertNil(AgentListOrder.activityTime(NSNumber(value: Double.infinity)))
        XCTAssertNil(AgentListOrder.activityTime(NSNumber(value: 1e30)))
        XCTAssertNil(AgentListOrder.activityTime(NSNumber(value: true)))
        XCTAssertNil(AgentListOrder.activityTime("1800000000000"))
        // Through the roster record, as JSON decodes it.
        let json = Data(#"{"name":"worker","metadata":{"activity":{"updated_ms":1800000000000}}}"#.utf8)
        let record = (try? JSONSerialization.jsonObject(with: json)) as? JSONObject ?? [:]
        XCTAssertEqual(RelayAgent(relay: record).activityUpdatedAt, now)
        XCTAssertNil(RelayAgent(relay: ["name": "worker", "metadata": ["activity": ["turn": "idle"]]]).activityUpdatedAt)
        let boolean = Data(#"{"name":"worker","metadata":{"activity":{"updated_ms":true}}}"#.utf8)
        let booleanRecord = (try? JSONSerialization.jsonObject(with: boolean)) as? JSONObject ?? [:]
        XCTAssertNil(RelayAgent(relay: booleanRecord).activityUpdatedAt)
    }

    func testAFutureActivityTimeDoesNotOutrankAPeerActiveNow() {
        let future = rosterAgent("zeta", seenAt: now,
                                 metadata: ["activity": ["turn": "idle", "updated_ms": 1_800_086_400_000]])
        let current = rosterAgent("alpha", seenAt: now,
                                  metadata: ["activity": ["turn": "idle", "updated_ms": 1_800_000_000_000]])
        // A day ahead counts as now: same bucket, so the name decides.
        XCTAssertEqual(listOrder([future, current], at: now), ["alpha", "zeta"])
        XCTAssertEqual(AgentListOrder.entry(for: future, name: "zeta", lastEventAt: nil, now: now).lastActivity, now)
        // A past time is kept as it is.
        XCTAssertEqual(AgentListOrder.entry(for: current, name: "alpha", lastEventAt: nil,
                                            now: now.addingTimeInterval(60)).lastActivity, now)
    }
}
