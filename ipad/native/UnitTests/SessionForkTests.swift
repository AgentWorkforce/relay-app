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
        XCTAssertEqual(SessionFork.availability(for: planner, nodes: [studio, second, old]),
                       .available(harness: "claude", nodes: [studio, second]))
        XCTAssertEqual(SessionFork.availability(for: agent("reviewer", kind: "codex"), nodes: [studio, second]),
                       .available(harness: "codex", nodes: [studio]))
    }

    func testAComputerWithoutForkSupportIsReportedNotHidden() {
        guard case let .unavailable(reason) = SessionFork.availability(for: agent("planner", kind: "claude-terminal"),
                                                                       nodes: [old]) else {
            return XCTFail("an older computer must not be offered a fork it would ignore")
        }
        XCTAssertTrue(reason.contains("Studio Mac"))
        XCTAssertEqual(SessionFork.availability(for: agent("helper", kind: "opencode"), nodes: [studio]),
                       .unavailable("Only Claude and Codex sessions can be forked."))
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
    }

    func testForkRefusalsReadAsSentences() {
        let refused = SpawnFailure.message("spawn_fork_refused: only this computer's verified owner can fork its sessions",
                                           node: "studio-mac-desktop", name: "planner-fork", harness: "claude")
        XCTAssertTrue(refused.hasPrefix("Only the owner of studio-mac-desktop can fork its sessions."))
    }
}
