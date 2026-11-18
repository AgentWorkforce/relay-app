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

    func testGrokSessionsForkWhereTheComputerOffersIt() {
        let grokNode = RelaySpawnNode(name: "studio-mac-desktop", harnesses: ["claude", "codex", "grok"],
                                      forkHarnesses: ["claude", "codex", "grok"], machine: "Studio-Mac")
        for kind in ["grok", "grok-open"] {
            let session = agent("scout", kind: kind)
            XCTAssertEqual(SessionFork.harness(for: session), "grok")
            XCTAssertEqual(SessionFork.availability(for: session, nodes: [grokNode], live: true),
                           .available(harness: "grok", nodes: [grokNode]))
            // A computer that forks only Claude and Codex is reported, not hidden.
            XCTAssertEqual(SessionFork.availability(for: session, nodes: [studio], live: true),
                           .unavailable("\(session.machine) isn't online with a version of Agent Relay that can fork sessions. Update Agent Relay on it, then try again."))
        }
    }

    func testANameContainingGrokIsNotAStatedHarness() {
        let named = RelayAgent(relay: ["name": "devopsgrok", "status": "active"])
        XCTAssertEqual(named.cli, "grok")
        XCTAssertEqual(named.statedCLI, "")
        XCTAssertNil(SessionFork.harness(for: named, roster: [named]))
        XCTAssertEqual(SessionFork.availability(for: named, nodes: [studio], live: true, roster: [named]),
                       .unavailable("Only Claude, Codex and Grok sessions can be forked."))
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
                       .unavailable("Only Claude, Codex and Grok sessions can be forked."))
    }

    private let lab = RelaySpawnNode(name: "lab-desktop", harnesses: ["claude", "codex"],
                                     forkHarnesses: ["claude", "codex"], machine: "lab")

    /// A headless agent the node spawned, as Relaycast lists it: the node's
    /// `agent.register` stamps only `metadata.fleet`; nothing names Claude.
    private func spawnedRecord(_ name: String, persona: String? = nil) -> RelayAgent {
        var data: JSONObject = ["name": name, "status": "active", "address": "\(name)@lab-desktop",
                                "metadata": ["fleet": ["node_id": "node_lab", "invocation_id": "inv_1"],
                                             "renamed_to": "", "session_title": ""] as JSONObject]
        if let persona { data["persona"] = persona }
        return RelayAgent(relay: data)
    }

    /// The node's `release` capability, as `relay-node` advertises it.
    private func labCapabilities(_ agents: [(String, String)]) -> [JSONObject] {
        [["name": "spawn:claude", "kind": "capacity"],
         ["name": "release", "kind": "capacity",
          "metadata": ["live": ["observed_at": "2026-11-17T09:00:00Z",
                                "agents": agents.map { ["name": $0.0, "agent_id": "a_\($0.0)", "harness": $0.1,
                                                        "holds_slot": true] as JSONObject },
                                "reclaimed": [JSONObject]()] as JSONObject] as JSONObject]]
    }

    func testAHeadlessClaudeSpawnForksWithClaudeOnItsNode() {
        let spawned = SessionFork.spawned(nodes: [(name: "lab-desktop",
                                                   capabilities: labCapabilities([("events-shim-merge", "claude")]))])
        XCTAssertEqual(spawned, ["events-shim-merge": [SessionFork.Spawned(node: "lab-desktop", harness: "claude")]])
        let agent = spawnedRecord("events-shim-merge")
        XCTAssertEqual(agent.statedCLI, "")
        XCTAssertEqual(SessionFork.harness(for: agent, spawned: spawned), "claude")
        XCTAssertEqual(SessionFork.availability(for: agent, nodes: [studio, lab], live: true, spawned: spawned),
                       .available(harness: "claude", nodes: [lab]))
    }

    func testAnAgentWithNoStatedHarnessIsStillRefused() {
        let agent = spawnedRecord("events-shim-merge")
        XCTAssertNil(SessionFork.harness(for: agent, roster: [agent]))
        XCTAssertEqual(SessionFork.availability(for: agent, nodes: [lab], live: true, roster: [agent]),
                       .unavailable("Only Claude, Codex and Grok sessions can be forked."))
        // A name is not evidence of the harness either.
        let named = RelayAgent(relay: ["name": "claude-helper", "status": "active"])
        XCTAssertEqual(named.cli, "claude")
        XCTAssertNil(SessionFork.harness(for: named, roster: [named]))
    }

    func testAClaudeSessionNeverForksOnCodex() {
        // Named for Codex, registered as a Claude Code session.
        let renamed = agent("codex-review", kind: "claude-terminal")
        XCTAssertEqual(SessionFork.harness(for: renamed, roster: [renamed]), "claude")
        // Spawned on Claude, described as a Codex worker.
        let spawned = SessionFork.spawned(nodes: [(name: "lab-desktop",
                                                   capabilities: labCapabilities([("shim", "claude")]))])
        let described = spawnedRecord("shim", persona: "a codex worker")
        XCTAssertEqual(described.cli, "codex")
        XCTAssertEqual(SessionFork.harness(for: described, roster: [described], spawned: spawned), "claude")
        // A conversation copy saying Codex yields to the current registration.
        let opened = RelayAgent(conversation: ["participants": [["agent_name": "neon-exit-lead", "cli": "codex"]],
                                               "status": "active", "cli": "codex"])
        let current = agent("neon-exit-lead", kind: "claude-terminal")
        XCTAssertEqual(SessionFork.harness(for: opened, roster: [current]), "claude")
        XCTAssertEqual(SessionFork.availability(for: opened, nodes: [studio], live: true, roster: [current]),
                       .unavailable("This agent's computer isn't online with a version of Agent Relay that can fork sessions. Update Agent Relay on it, then try again."))
        guard case let .available(harness, _) = SessionFork.availability(for: current, nodes: [studio], live: true,
                                                                          roster: [current]) else {
            return XCTFail("a live Claude session on a forking computer is forkable")
        }
        XCTAssertEqual(harness, "claude")
    }

    private let annex = RelaySpawnNode(name: "annex-desktop", harnesses: ["claude", "codex"],
                                       forkHarnesses: ["claude", "codex"], machine: "annex")

    func testANameTwoNodesReportForksOnEachInNodeOrder() {
        // The entry on lab-desktop is stale; annex-desktop runs the agent.
        let spawned = SessionFork.spawned(nodes: [
            (name: "lab-desktop", capabilities: labCapabilities([("shim", "claude")])),
            (name: "annex-desktop", capabilities: labCapabilities([("shim", "claude")])),
        ])
        XCTAssertEqual(spawned, ["shim": [SessionFork.Spawned(node: "lab-desktop", harness: "claude"),
                                          SessionFork.Spawned(node: "annex-desktop", harness: "claude")]])
        let agent = spawnedRecord("shim")
        XCTAssertEqual(SessionFork.availability(for: agent, nodes: [annex, studio, lab], live: true, spawned: spawned),
                       .available(harness: "claude", nodes: [lab, annex]))
    }

    func testNodesThatDisagreeOnTheHarnessStateNone() {
        let spawned = SessionFork.spawned(nodes: [
            (name: "lab-desktop", capabilities: labCapabilities([("shim", "claude")])),
            (name: "annex-desktop", capabilities: labCapabilities([("shim", "codex")])),
        ])
        let record = spawnedRecord("shim")
        XCTAssertEqual(SessionFork.statedHarness(for: record, roster: [record], spawned: spawned), "")
        XCTAssertNil(SessionFork.harness(for: record, roster: [record], spawned: spawned))
        XCTAssertEqual(SessionFork.availability(for: record, nodes: [lab, annex], live: true, roster: [record],
                                                spawned: spawned),
                       .unavailable("Only Claude, Codex and Grok sessions can be forked."))
        // The roster's statement then decides, and both nodes stay candidates.
        let current = agent("shim", kind: "claude-terminal")
        XCTAssertEqual(SessionFork.availability(for: record, nodes: [lab, annex], live: true, roster: [current],
                                                spawned: spawned),
                       .available(harness: "claude", nodes: [lab, annex]))
        // A node of its own picks that node's entry alone.
        let pinned = RelayAgent(fleet: ["name": "shim", "node": "annex-desktop", "workerLiveness": "live"])
        XCTAssertEqual(SessionFork.harness(for: pinned, spawned: spawned), "codex")
    }

    func testTheIconShowsTheHarnessTheForkUses() {
        let spawned = SessionFork.spawned(nodes: [(name: "lab-desktop",
                                                   capabilities: labCapabilities([("shim", "claude")]))])
        // The copy at hand states Codex; the node and the roster state Claude.
        let copy = RelayAgent(conversation: ["participants": [["agent_name": "shim", "cli": "codex"]],
                                             "status": "active", "cli": "codex"])
        XCTAssertEqual(copy.statedCLI, "codex")
        let current = agent("shim", kind: "claude")
        let cases: [(roster: [RelayAgent], spawned: [String: [SessionFork.Spawned]])] = [
            (roster: [], spawned: spawned),
            (roster: [current], spawned: [:]),
            (roster: [current], spawned: spawned),
            (roster: [], spawned: [:]),
        ]
        for inputs in cases {
            let harness = SessionFork.harness(for: copy, roster: inputs.roster, spawned: inputs.spawned)
            XCTAssertEqual(SessionFork.displayCLI(for: copy, roster: inputs.roster, fleet: [], spawned: inputs.spawned),
                           harness)
        }
        XCTAssertEqual(SessionFork.displayCLI(for: copy, roster: [current], fleet: [], spawned: [:]), "claude")
        XCTAssertEqual(SessionFork.displayCLI(for: copy, roster: [], fleet: [], spawned: [:]), "codex")
        // With nothing stated, the icon still guesses from the name; the fork does not.
        let named = RelayAgent(relay: ["name": "codex-helper", "status": "active"])
        XCTAssertNil(SessionFork.harness(for: named, roster: [named]))
        XCTAssertEqual(SessionFork.displayCLI(for: named, roster: [named], fleet: [], spawned: [:]), "codex")
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
