import XCTest
import RelaySessionOverview
@testable import AgentRelayiPad

final class AgentRowModelTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 1_800_000_000)

    private func seen(_ secondsAgo: TimeInterval = 10) -> String {
        Date.ISO8601FormatStyle(includingFractionalSeconds: true).format(now.addingTimeInterval(-secondsAgo))
    }

    private func agent(name: String = "builder", status: String = "active", metadata: JSONObject = [:],
                       extra: JSONObject = [:]) -> RelayAgent {
        var data: JSONObject = ["name": name, "status": status, "address": "\(name)@test-mini-123",
                                "last_seen": seen()]
        var merged: JSONObject = ["host": "Test Mini.local", "session_id": "session-1", "cli": "codex"]
        merged.merge(metadata) { _, new in new }
        data["metadata"] = merged
        data.merge(extra) { _, new in new }
        return RelayAgent(relay: data)
    }

    private func row(_ agent: RelayAgent, _ loaded: AgentRowModel.Loaded = .init()) -> AgentRowModel {
        AgentRowModel(agent: agent, loaded: loaded, now: now)
    }

    func testTitlePrefersTheBriefThenSessionTitleThenObjectiveThenGoal() {
        let full = agent(metadata: ["session_title": "Provider title", "objective": "Published objective"])
        var loaded = AgentRowModel.Loaded(goal: "Original request")
        loaded.overview = SessionOverview(status: .completed, title: "Brief title", description: "Brief line")
        XCTAssertEqual(row(full, loaded).title, "Brief title")

        loaded.overview = nil
        loaded.rowSummary = AgentRowSummary(title: "Row brief title", line: "Row brief line")
        XCTAssertEqual(row(full, loaded).title, "Row brief title")

        loaded.rowSummary = nil
        XCTAssertEqual(row(full, loaded).title, "Provider title")
        XCTAssertEqual(row(agent(metadata: ["objective": "Published objective"]), loaded).title, "Published objective")
        XCTAssertEqual(row(agent(), loaded).title, "Original request")
        XCTAssertNil(row(agent()).title)
    }

    func testABriefStillLoadingOrFailedFallsBackToTheRowsOwnBrief() {
        var loaded = AgentRowModel.Loaded()
        loaded.overview = SessionOverview(status: .failed, error: "Could not generate this summary.")
        loaded.rowSummary = AgentRowSummary(title: "Row brief title", line: "Row brief line")
        let model = row(agent(), loaded)
        XCTAssertEqual(model.title, "Row brief title")
        XCTAssertEqual(model.summary, "Row brief line")
    }

    func testSummaryPrefersTheRecordedShortSummaryThenTheBriefThenUnusedFallbacks() {
        let full = agent(metadata: ["session_title": "Provider title", "objective": "Published objective"])
        var loaded = AgentRowModel.Loaded(goal: "Original request")
        loaded.overview = SessionOverview(status: .completed, title: "Brief title", description: "Brief line")
        loaded.recordedSummary = .ready(text: "Recorded short summary", isShort: true)
        XCTAssertEqual(row(full, loaded).summary, "Recorded short summary")

        // A long summary is only used when there is no brief line.
        loaded.recordedSummary = .ready(text: "Recorded long summary", isShort: false)
        XCTAssertEqual(row(full, loaded).summary, "Brief line")
        loaded.overview = nil
        XCTAssertEqual(row(full, loaded).summary, "Recorded long summary")

        // No summary at all: the title took the session title, so the
        // summary is the next unused text.
        loaded.recordedSummary = .empty
        XCTAssertEqual(row(full, loaded).title, "Provider title")
        XCTAssertEqual(row(full, loaded).summary, "Published objective")
        XCTAssertEqual(row(agent(metadata: ["objective": "Published objective"]), loaded).summary, "Original request")
        XCTAssertNil(row(agent(metadata: ["objective": "Published objective"])).summary)
    }

    func testSummaryNeverRepeatsTheTitle() {
        var loaded = AgentRowModel.Loaded(goal: "ship  the\nrelease")
        loaded.overview = SessionOverview(status: .completed, title: "Ship the release", description: "Ship the release")
        let model = row(agent(metadata: ["session_title": "SHIP THE RELEASE", "objective": "Tag the build"]), loaded)
        XCTAssertEqual(model.title, "Ship the release")
        XCTAssertEqual(model.summary, "Tag the build")

        let onlyDuplicates = row(agent(metadata: ["session_title": "Ship the release", "objective": "ship the release"]))
        XCTAssertEqual(onlyDuplicates.title, "Ship the release")
        XCTAssertNil(onlyDuplicates.summary)
    }

    func testEmptyAndWhitespaceValuesFallThroughAndTextIsOneLine() {
        var loaded = AgentRowModel.Loaded(displayName: "  ", goal: " \n ")
        loaded.overview = SessionOverview(status: .completed, title: "  ", description: "\n\t")
        loaded.rowSummary = AgentRowSummary(title: " ", line: " ")
        loaded.recordedSummary = .ready(text: "First line.\n\n  Second   line.", isShort: true)
        let model = row(agent(metadata: ["session_title": "  Provider\n title "]), loaded)
        XCTAssertEqual(model.name, "builder")
        XCTAssertEqual(model.title, "Provider title")
        XCTAssertEqual(model.summary, "First line. Second line.")

        let long = String(repeating: "word ", count: 200)
        loaded.recordedSummary = .ready(text: long, isShort: true)
        let cut = row(agent(), loaded).summary ?? ""
        XCTAssertEqual(cut.count, AgentRowModel.summaryLimit)
        XCTAssertTrue(cut.hasSuffix("…"))
        XCTAssertFalse(cut.contains("\n"))
    }

    func testCLILabelNamesTheCLIAndAddsAKnownModel() {
        XCTAssertEqual(AgentRowModel.cliLabel(cli: "codex"), "Codex")
        XCTAssertEqual(AgentRowModel.cliLabel(cli: "claudecode"), "Claude Code")
        XCTAssertEqual(AgentRowModel.cliLabel(cli: "claude"), "Claude")
        XCTAssertEqual(AgentRowModel.cliLabel(cli: "opencode"), "OpenCode")
        XCTAssertEqual(AgentRowModel.cliLabel(cli: "openai"), "OpenAI")
        XCTAssertEqual(AgentRowModel.cliLabel(cli: "grok"), "Grok")
        XCTAssertEqual(AgentRowModel.cliLabel(cli: "cursor"), "Cursor")
        XCTAssertEqual(AgentRowModel.cliLabel(cli: "muse"), "Muse")
        XCTAssertEqual(AgentRowModel.cliLabel(cli: "newtool"), "Newtool")
        XCTAssertNil(AgentRowModel.cliLabel(cli: "  "))
        XCTAssertEqual(AgentRowModel.cliLabel(cli: "codex", model: " model-x "), "Codex · model-x")
        XCTAssertEqual(AgentRowModel.cliLabel(cli: "codex", model: "codex"), "Codex")
        XCTAssertEqual(AgentRowModel.cliLabel(cli: "", model: "model-x"), "model-x")

        let model = row(agent(), .init(cli: "claudecode", model: "model-x"))
        XCTAssertEqual(model.cliLabel, "Claude Code · model-x")
        XCTAssertEqual(model.caption, "Claude Code · model-x · Test Mini")
    }

    func testFleetRecordKeepsItsModel() {
        let fleet = RelayAgent(fleet: ["name": "builder", "node": "test-mini", "cli": "codex", "cliModel": "model-x"])
        XCTAssertEqual(fleet.model, "model-x")
        XCTAssertEqual(RelayAgent(fleet: ["name": "builder", "node": "test-mini"]).model, "")
    }

    func testMachineNameDropsTheLocalSuffixAndIsMissingWithoutAHost() {
        XCTAssertEqual(row(agent()).machine, "Test Mini")
        let addressed = RelayAgent(relay: ["name": "builder", "address": "builder@studio-mac-42", "status": "active"])
        XCTAssertEqual(row(addressed).machine, "studio-mac")
        let bare = RelayAgent(relay: ["name": "builder", "status": "active"])
        XCTAssertNil(row(bare).machine)
        XCTAssertNil(row(bare).caption)
        XCTAssertEqual(row(bare, .init(cli: "codex")).caption, "Codex")
    }

    func testPersonRowHasNameAndHandleAndNoSessionLines() {
        let person = RelayAgent(relay: ["name": "sam-lee", "type": "human", "persona": "Sam Lee", "status": "active",
                                        "address": "sam-lee@cloud-1234567890", "description": "A person",
                                        "last_seen": seen(), "metadata": ["session_id": "session-9"]])
        var loaded = AgentRowModel.Loaded(displayName: "Sam Lee", cli: "codex", goal: "Not theirs either")
        loaded.overview = SessionOverview(status: .completed, title: "Not theirs", description: "Not theirs either")
        loaded.rowSummary = AgentRowSummary(title: "Not theirs", line: "Not theirs either")
        loaded.recordedSummary = .ready(text: "Not theirs either", isShort: true)
        let model = row(person, loaded)
        XCTAssertEqual(model.name, "Sam Lee")
        XCTAssertEqual(model.handle, "sam-lee")
        // A person never gets session lines, even with a session ID.
        XCTAssertNil(model.title)
        XCTAssertNil(model.summary)
        XCTAssertNil(model.cliLabel)
        XCTAssertNil(model.machine)
        XCTAssertNil(model.caption)
        XCTAssertEqual(model.status, .online)
        XCTAssertEqual(model.accessibilityLabel, "Sam Lee, Online")

        let plain = RelayAgent(relay: ["name": "sam-lee", "type": "human", "persona": "Sam Lee", "status": "active",
                                       "last_seen": seen()])
        let plainRow = row(plain, .init(displayName: "Sam Lee"))
        XCTAssertNil(plainRow.title)
        XCTAssertNil(plainRow.summary)
        XCTAssertEqual(plainRow.accessibilityLabel, "Sam Lee, Online")
        XCTAssertNil(row(plain, .init(displayName: "sam-lee")).handle)
    }

    func testPersonConversationShowsTheLastMessage() {
        let thread = RelayAgent(conversation: [
            "participants": [["agent_name": "sam-lee"]], "is_human": true, "display_name": "Sam Lee",
            "last_message": ["text": "Lunch at\nnoon?"], "status": "active", "session_id": "session-9",
        ])
        var loaded = AgentRowModel.Loaded(displayName: "Sam Lee", unread: true)
        loaded.rowSummary = AgentRowSummary(title: "Not theirs", line: "Not theirs either")
        let model = row(thread, loaded)
        // Only the last message, never a session's lines.
        XCTAssertNil(model.title)
        XCTAssertEqual(model.summary, "Lunch at noon?")
        XCTAssertTrue(model.unread)
        XCTAssertEqual(model.accessibilityLabel, "Sam Lee, Online, Unread messages")
    }

    func testGatewayAgentHasNoSessionLinesButKeepsItsObjectiveCLIAndMachine() {
        let gateway = agent(name: "main", metadata: ["source": "relay-desktop", "session_kind": "openclaw",
                                                     "session_id": "main", "session_title": "A conversation title",
                                                     "objective": "Answers the front desk"])
        XCTAssertEqual(gateway.sessionID, "")
        var loaded = AgentRowModel.Loaded(cli: "", goal: "Someone's request")
        loaded.rowSummary = AgentRowSummary(title: "Not this agent's", line: "Nor this")
        loaded.recordedSummary = .ready(text: "Nor this", isShort: true)
        let model = row(gateway, loaded)
        XCTAssertEqual(model.title, "Answers the front desk")
        XCTAssertNil(model.summary)
        XCTAssertEqual(model.machine, "Test Mini")
        XCTAssertEqual(model.caption, "Test Mini")

        let silent = agent(name: "main", metadata: ["session_scope": "agent"])
        XCTAssertNil(row(silent).title)
        XCTAssertNil(row(silent).summary)
    }

    func testChannelRowHasNoStatusAndShowsItsTopicOrMembers() {
        let topic = RelayAgent(channel: ["name": "launch", "topic": "Launch prep", "member_count": 3], joined: true)
        XCTAssertNil(row(topic).status)
        XCTAssertNil(row(topic).title)
        XCTAssertEqual(row(topic).summary, "Launch prep")
        XCTAssertEqual(row(topic).accessibilityLabel, "launch")
        let plain = RelayAgent(channel: ["name": "general", "member_count": 12], joined: false)
        XCTAssertEqual(row(plain).summary, "Not joined · 12 members")
    }

    func testStatusComesFromReachabilityAndTheReportedState() {
        XCTAssertEqual(row(agent(status: "active")).status, .online)
        XCTAssertEqual(row(agent(status: "online")).status, .online)
        XCTAssertEqual(row(agent(status: "waiting")).status, .waiting)
        XCTAssertEqual(row(agent(status: "blocked")).status, .waiting)
        XCTAssertEqual(row(agent(status: "idle")).status, .idle)
        // A desktop registration that reports idle is not reachable.
        XCTAssertEqual(row(agent(status: "idle", metadata: ["source": "relay-desktop"])).status, .offline)
        XCTAssertEqual(row(agent(status: "offline")).status, .offline)
        XCTAssertEqual(row(agent(status: "")).status, .unknown)
        // Not seen within the presence window.
        let stale = agent(status: "active", extra: ["last_seen": seen(6 * 60)])
        XCTAssertEqual(row(stale).status, .offline)
        XCTAssertEqual(AgentRowModel.Status.unknown.label, "Status unknown")
    }

    func testUnreadIsCarriedAndRead() {
        XCTAssertFalse(row(agent()).unread)
        let model = row(agent(metadata: ["session_title": "Provider title"]), .init(cli: "codex", unread: true))
        XCTAssertTrue(model.unread)
        XCTAssertEqual(model.accessibilityLabel, "builder, Provider title, Codex, Test Mini, Online, Unread messages")
    }

    func testSearchMatchesWhatTheRowShowsAndTheRecordsOwnFields() {
        var loaded = AgentRowModel.Loaded(cli: "claudecode", goal: "Original request")
        loaded.rowSummary = AgentRowSummary(title: "Brief title", line: "Brief line")
        let model = row(agent(metadata: ["session_title": "Provider title", "objective": "Published objective"]), loaded)
        for query in ["builder", "brief TITLE", "Brief line", "claude code", "Test Mini", "test-mini-123",
                      "provider title", "published objective", "original request", "  builder ", ""] {
            XCTAssertTrue(model.matches(query), query)
        }
        XCTAssertFalse(model.matches("reviewer"))
        XCTAssertFalse(model.matches("codex"))
    }
}
