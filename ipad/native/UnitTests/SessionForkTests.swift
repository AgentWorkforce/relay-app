import XCTest
@testable import AgentRelayiPad

final class SessionForkTests: XCTestCase {
    private func agent(_ name: String, kind: String, host: String = "Studio Mac.local") -> RelayAgent {
        RelayAgent(relay: ["name": name, "status": "active", "address": "\(name)@studio-mac-1",
                           "metadata": ["host": host, "session_kind": kind, "session_id": "s-\(name)"]])
    }

    private let studio = RelaySpawnNode(name: "studio-mac-desktop", harnesses: ["claude", "codex"],
                                        forkHarnesses: ["claude", "codex"], machine: "Studio-Mac")
    private let second = RelaySpawnNode(name: "studio-mac-desktop-2", harnesses: ["claude"],
                                        forkHarnesses: ["claude"], machine: "studio mac")
    private let old = RelaySpawnNode(name: "studio-mac-old", harnesses: ["claude"], machine: "")

    func testClaudeAndCodexSessionsForkWithTheirOwnHarness() {
        XCTAssertEqual(SessionFork.harness(for: agent("planner", kind: "claude-terminal")), "claude")
        XCTAssertEqual(SessionFork.harness(for: agent("reviewer", kind: "codex")), "codex")
        XCTAssertNil(SessionFork.harness(for: agent("helper", kind: "opencode")))
    }

    func testEveryInstallationOnTheAgentsMachineThatForksIsACandidate() {
        let planner = agent("planner", kind: "claude-terminal")
        XCTAssertEqual(SessionFork.availability(for: planner, nodes: [studio, second, old], live: true),
                       .available(harness: "claude", nodes: [studio, second]))
        XCTAssertEqual(SessionFork.availability(for: agent("reviewer", kind: "codex"), nodes: [studio, second], live: true),
                       .available(harness: "codex", nodes: [studio]))
    }

    func testAComputerWithoutForkSupportIsReportedNotHidden() {
        guard case let .unavailable(reason) = SessionFork.availability(for: agent("planner", kind: "claude-terminal"),
                                                                       nodes: [old], live: true) else {
            return XCTFail("an older computer must not be offered a fork it would ignore")
        }
        XCTAssertTrue(reason.contains("Studio Mac"))
        XCTAssertEqual(SessionFork.availability(for: agent("helper", kind: "opencode"), nodes: [studio], live: true),
                       .unavailable("Only Claude and Codex sessions can be forked."))
    }

    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    private func registration(_ name: String, status: String = "active", seen: Date?) -> RelayAgent {
        var data: JSONObject = ["name": name, "status": status, "address": "\(name)@studio-mac-1",
                                "metadata": ["source": "relay-desktop", "host": "Studio Mac.local",
                                             "session_kind": "claude-terminal", "session_id": "s-\(name)"]]
        if let seen { data["last_seen"] = seen.ISO8601Format() }
        return RelayAgent(relay: data)
    }

    func testOnlyALiveAgentIsAForkSource() {
        let live = registration("planner", seen: now.addingTimeInterval(-60))
        XCTAssertTrue(SessionFork.isLive(live, roster: [live], fleet: [], at: now))
        let offline = registration("planner", status: "offline", seen: now.addingTimeInterval(-60))
        XCTAssertFalse(SessionFork.isLive(offline, roster: [offline], fleet: [], at: now))
        // Past the core's five-minute presence window, even while still marked active.
        let stale = registration("planner", seen: now.addingTimeInterval(-6 * 60))
        XCTAssertFalse(SessionFork.isLive(stale, roster: [stale], fleet: [], at: now))
    }

    /// A conversation opened while the agent was up keeps its copy; the
    /// current roster decides, so an agent that stood down (and was replaced
    /// under another name) is not offered.
    func testAnOpenConversationIsJudgedByTheCurrentRoster() {
        let opened = RelayAgent(conversation: ["participants": [["agent_name": "neon-exit-lead"]],
                                               "status": "active", "machine": "sf-mac-mini", "cli": "claude"])
        let successor = registration("neon-exit-lead-2", seen: now)
        XCTAssertFalse(SessionFork.isLive(opened, roster: [successor], fleet: [], at: now))
        guard case let .unavailable(reason) = SessionFork.availability(for: opened, nodes: [studio], live: false) else {
            return XCTFail("a stood-down agent must not be offered as a fork source")
        }
        XCTAssertTrue(reason.contains("neon-exit-lead is no longer running"), reason)
        let current = registration("neon-exit-lead", seen: now.addingTimeInterval(-30))
        XCTAssertTrue(SessionFork.isLive(opened, roster: [successor, current], fleet: [], at: now))
    }

    func testANodesLiveWorkerListCountsAsLive() {
        let worker = RelayAgent(fleet: ["name": "builder", "node": "studio-mac-desktop", "workerLiveness": "live"])
        XCTAssertTrue(SessionFork.isLive(worker, roster: [], fleet: [worker], at: now))
        XCTAssertFalse(SessionFork.isLive(worker, roster: [], fleet: [], at: now))
        let stopped = RelayAgent(fleet: ["name": "builder", "node": "studio-mac-desktop", "workerLiveness": "offline"])
        XCTAssertFalse(SessionFork.isLive(stopped, roster: [], fleet: [stopped], at: now))
    }

    func testPeopleAndChannelsHaveNoSessionToFork() {
        XCTAssertTrue(SessionFork.offered(for: agent("planner", kind: "claude-terminal")))
        XCTAssertFalse(SessionFork.offered(for: RelayAgent(relay: ["name": "sam", "type": "human", "status": "active"])))
        XCTAssertFalse(SessionFork.offered(for: RelayAgent(channel: ["name": "launch"], joined: true)))
    }

    func testSuggestedNamesStayUniqueAndValid() {
        XCTAssertEqual(SessionFork.suggestedName(for: "planner", taken: []), "planner-fork")
        XCTAssertEqual(SessionFork.suggestedName(for: "planner", taken: ["planner-fork", "planner-fork-2"]), "planner-fork-3")
        let long = String(repeating: "a", count: 47)
        let name = SessionFork.suggestedName(for: long, taken: [String("\(long)-fork".prefix(48))])
        XCTAssertLessThanOrEqual(name.count, 48)
        XCTAssertTrue(name.hasSuffix("-2"))
        var crowded: Set<String> = ["planner-fork"]
        crowded.formUnion((2...99).map { "planner-fork-\($0)" })
        let next = SessionFork.suggestedName(for: "planner", taken: crowded)
        XCTAssertEqual(next, "planner-fork-100")
        XCTAssertLessThanOrEqual(next.count, 48)
    }

    func testALostForkReplyIsNotARejection() {
        let lost = SessionFork.unanswered(URLError(.timedOut))
        XCTAssertTrue(SessionFork.submissionUncertain(lost))
        XCTAssertTrue(lost.localizedDescription.contains("Check Agents"))
        let refused = RelayAPIError(message: "spawn_fork_refused", status: 403, code: "spawn_fork_refused")
        XCTAssertFalse(SessionFork.submissionUncertain(SessionFork.unanswered(refused)))
        XCTAssertFalse(SessionFork.submissionUncertain(SessionFork.unanswered(CancellationError())))
        XCTAssertFalse(SessionFork.triesNextNode(after: lost))
    }

    func testOnlyAMissingAgentMovesTheForkToTheNextInstallation() {
        XCTAssertTrue(SessionFork.triesNextNode(after: RelayAPIError(message: "x\nDetails: spawn_fork_not_found: @planner is not an agent")))
        XCTAssertFalse(SessionFork.triesNextNode(after: RelayAPIError(message: "Details: spawn_fork_refused: only the owner")))
        // The code replaced on the way; the node's sentence still says it.
        XCTAssertTrue(SessionFork.triesNextNode(after: RelayAPIError(message: "x\nDetails: [redacted] @planner is not an agent on this computer")))
    }

    func testForkRefusalsReadAsSentences() {
        let refused = SpawnFailure.message("spawn_fork_refused: only this computer's verified owner can fork its sessions",
                                           node: "studio-mac-desktop", name: "planner-fork", harness: "claude")
        XCTAssertTrue(refused.hasPrefix("Only the owner of studio-mac-desktop can fork its sessions."))
    }
}
