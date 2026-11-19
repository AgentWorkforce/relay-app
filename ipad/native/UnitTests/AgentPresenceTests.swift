import XCTest
@testable import AgentRelayiPad

/// One rule decides whether an agent is offered anywhere a person picks one:
/// the Agents list, Fork, Call and New message.
final class AgentPresenceTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    private func agent(_ name: String, status: String = "active", seen: TimeInterval? = -60,
                       desktop: Bool = false) -> RelayAgent {
        var metadata: JSONObject = ["host": "sf-mac-mini"]
        if desktop { metadata["source"] = "relay-desktop" }
        var data: JSONObject = ["name": name, "status": status, "address": "\(name)@sf-mac-mini", "metadata": metadata]
        if let seen { data["last_seen"] = now.addingTimeInterval(seen).ISO8601Format() }
        return RelayAgent(relay: data)
    }

    private func person(_ name: String, status: String) -> RelayAgent {
        RelayAgent(relay: ["name": name, "type": "human", "status": status, "persona": "Sam Lee"])
    }

    private func worker(_ name: String, liveness: String = "live") -> RelayAgent {
        RelayAgent(fleet: ["name": name, "node": "sf-mac-mini-desktop", "workerLiveness": liveness])
    }

    func testAStoodDownAgentIsNotActive() {
        let gone = agent("neon-exit-lead", status: "offline")
        XCTAssertFalse(AgentPresence.isActive(gone, roster: [gone], fleet: [], at: now))
        // Still marked active, but silent past the core's five-minute window.
        let stale = agent("neon-exit-lead", seen: -6 * 60)
        XCTAssertFalse(AgentPresence.isActive(stale, roster: [stale], fleet: [], at: now))
        // No longer on the roster at all.
        XCTAssertFalse(AgentPresence.isActive(stale, roster: [], fleet: [], at: now))
    }

    func testAReachableRegistrationOrALiveNodeWorkerIsActive() {
        for status in ["active", "online", "waiting", "blocked", "idle"] {
            let up = agent("builder", status: status)
            XCTAssertTrue(AgentPresence.isActive(up, roster: [up], fleet: [], at: now), status)
        }
        // An idle desktop registration is a parked session, not active.
        let parked = agent("builder", status: "idle", desktop: true)
        XCTAssertFalse(AgentPresence.isActive(parked, roster: [parked], fleet: [], at: now))
        // A node listing the worker overrides a stale roster record.
        let stale = agent("builder", status: "offline")
        XCTAssertTrue(AgentPresence.isActive(stale, roster: [stale], fleet: [worker("builder")], at: now))
        XCTAssertFalse(AgentPresence.isActive(stale, roster: [stale], fleet: [worker("builder", liveness: "offline")], at: now))
    }

    func testTheCopyAtHandIsNeverEvidence() {
        let opened = RelayAgent(conversation: ["participants": [["agent_name": "neon-exit-lead"]], "status": "active"])
        XCTAssertFalse(AgentPresence.isActive(opened, roster: [agent("other")], fleet: [], at: now))
        let gone = agent("neon-exit-lead", status: "offline")
        XCTAssertFalse(AgentPresence.isActive(opened, roster: [gone], fleet: [], at: now))
        XCTAssertTrue(AgentPresence.isActive(opened, roster: [agent("neon-exit-lead")], fleet: [], at: now))
    }

    func testFallbackWhilePresenceIsUnknown() {
        let opened = RelayAgent(conversation: ["participants": [["agent_name": "builder"]], "status": ""])
        XCTAssertTrue(AgentPresence.isActive(opened, roster: [], fleet: [], presenceKnown: false, at: now))
        XCTAssertFalse(AgentPresence.isActive(opened, roster: [], fleet: [], presenceKnown: true, at: now))
        // Fork still needs an online node that can fork, so it stays unavailable.
        XCTAssertFalse(SessionFork.availability(for: opened, nodes: [], live: true).isAvailable)
    }

    func testForkUsesTheSameRule() {
        let gone = agent("neon-exit-lead", status: "offline")
        let up = agent("builder")
        for candidate in [gone, up] {
            XCTAssertEqual(SessionFork.isLive(candidate, roster: [gone, up], fleet: [], at: now),
                           AgentPresence.isActive(candidate, roster: [gone, up], fleet: [], at: now))
        }
    }

    func testAgentsListHidesInactiveAgentsAndOfflinePeople() {
        let roster = [agent("builder"), agent("neon-exit-lead", status: "offline"),
                      agent("stale", seen: -10 * 60), person("sam-lee", status: "online"),
                      person("pat", status: "offline")]
        let names = AgentPresence.listed(roster, fleet: [], at: now).map(\.name)
        XCTAssertEqual(names, ["builder", "sam-lee"])
        // Nothing is hidden before the first roster read answers.
        XCTAssertEqual(AgentPresence.listed(roster, fleet: [], presenceKnown: false, at: now).count, 4)
    }

    func testNewMessageRefusesInactiveAndUnknownAgents() {
        let roster = [agent("builder"), agent("neon-exit-lead", status: "offline"), person("sam-lee", status: "offline")]
        func target(_ text: String, known: Bool = true) -> AgentPresence.ComposeTarget {
            AgentPresence.composeTarget(text, roster: roster, conversations: [], fleet: [], presenceKnown: known, at: now)
        }
        guard case .open(let builder) = target(" @builder ") else { return XCTFail("an active agent opens") }
        XCTAssertEqual(builder.name, "builder")
        guard case .refused(let reason) = target("neon-exit-lead") else { return XCTFail("a stood-down agent is refused") }
        XCTAssertEqual(reason, "neon-exit-lead isn't active right now, so it can't receive messages.")
        guard case .refused(let unknown) = target("nobody") else { return XCTFail("an unknown name is refused") }
        XCTAssertTrue(unknown.hasPrefix("nobody isn't an active agent"), unknown)
        // People stay messageable while offline.
        guard case .open(let sam) = target("sam-lee") else { return XCTFail("an offline person opens") }
        XCTAssertTrue(sam.isHuman)
        // Before presence is known nothing is refused.
        guard case .open = target("neon-exit-lead", known: false) else { return XCTFail("unknown presence must not refuse") }
    }

    func testNewMessageSuggestsOnlyActiveAgentsAndReachablePeople() {
        let roster = [agent("builder"), agent("builder-old", status: "offline"), agent("reviewer"),
                      person("sam-lee", status: "online")]
        let all = AgentPresence.composeSuggestions("", roster: roster, fleet: [], presenceKnown: true, at: now)
        XCTAssertEqual(all.map(\.name), ["builder", "reviewer", "sam-lee"])
        let matching = AgentPresence.composeSuggestions("@build", roster: roster, fleet: [], presenceKnown: true, at: now)
        XCTAssertEqual(matching.map(\.name), ["builder"])
        let byDisplayName = AgentPresence.composeSuggestions("sam lee", roster: roster, fleet: [], presenceKnown: true, at: now)
        XCTAssertEqual(byDisplayName.map(\.name), ["sam-lee"])
    }
}
