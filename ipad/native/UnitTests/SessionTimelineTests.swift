import XCTest
@testable import AgentRelayiPad

final class SessionTimelineTests: XCTestCase {
    private func message(_ id: String, at time: String?, text: String = "hi", sender: String = "agent") -> RelayMessage {
        var data: JSONObject = ["id": id, "text": text, "agent_name": sender]
        if let time { data["created_at"] = time }
        return RelayMessage(data, index: 0)
    }

    private func event(_ id: String, at time: String?, kind: String = "assistant", role: String = "assistant",
                       content: String, tool: String = "") -> SessionEvent {
        SessionEvent(key: .init(source: "codex", machineID: "m1", kind: kind, eventID: id),
                     ts: time.flatMap(SessionEventPage.date), kind: kind, actorRole: role, content: content, toolName: tool)
    }

    private func ids(_ items: [ChatTimelineItem]) -> [String] { items.map(\.id) }

    func testMergesBothSourcesInTimeOrder() {
        let messages = [message("m1", at: "2026-11-20T10:01:00Z"), message("m2", at: "2026-11-20T10:03:00Z")]
        let events = [event("e0", at: "2026-11-20T10:00:00Z", content: "first"),
                      event("e2", at: "2026-11-20T10:02:00Z", kind: "tool_use", content: "ls", tool: "Bash"),
                      event("e4", at: "2026-11-20T10:04:00Z", content: "last")]
        XCTAssertEqual(ids(SessionTimeline.items(messages: messages, events: events)),
                       ["event:codex|m1|assistant|e0", "m1", "event:codex|m1|tool_use|e2", "m2", "event:codex|m1|assistant|e4"])
    }

    func testRelayOrderIsKeptAndUntimedMessagesStayPut() {
        let messages = [message("m1", at: "2026-11-20T10:05:00Z"), message("m2", at: nil), message("m3", at: "2026-11-20T10:01:00Z")]
        let events = [event("e1", at: "2026-11-20T10:02:00Z", content: "x")]
        XCTAssertEqual(ids(SessionTimeline.items(messages: messages, events: events)),
                       ["event:codex|m1|assistant|e1", "m1", "m2", "m3"])
        XCTAssertEqual(ids(SessionTimeline.items(messages: messages, events: [])), ["m1", "m2", "m3"])
    }

    func testTheSessionCopyOfAShownRelayMessageIsLeftOut() {
        let messages = [message("m_42", at: "2026-11-20T10:00:00Z", text: "Ship it", sender: "me")]
        let copy = event("e1", at: "2026-11-20T10:00:01Z", kind: "prompt", role: "user",
                         content: "[via Agent Relay — from @me to @builder · message m_42 · ref 0123456789ab]\n[reply: x]\n\nShip it")
        let other = event("e2", at: "2026-11-20T10:00:02Z", kind: "prompt", role: "user",
                          content: "[via Agent Relay — from @lead to @builder · message m_7 · ref 0123456789ab]\n\nAlso run lint")
        let items = SessionTimeline.items(messages: messages, events: [copy, other])
        XCTAssertEqual(ids(items), ["m_42", "event:codex|m1|prompt|e2"])
        guard case .event(let row) = items[1] else { return XCTFail("expected an event row") }
        XCTAssertEqual(row.text, "Also run lint")
        XCTAssertEqual(row.relayedFrom, "lead")
    }

    func testAPendingBubblesSessionCopyIsLeftOut() {
        let copy = event("e1", at: "2026-11-20T10:00:01Z", kind: "prompt", role: "user",
                         content: "[via Agent Relay — from @me to @builder · message m_new · ref 0123456789ab]\n\nShip   it\n")
        XCTAssertTrue(SessionTimeline.items(messages: [], events: [copy], pendingTexts: ["Ship it"]).isEmpty)
        XCTAssertEqual(SessionTimeline.items(messages: [], events: [copy]).count, 1)
    }

    func testPreambleIsStrippedAndPreambleOnlyPromptsHidden() {
        let prompt = event("e1", at: "2026-11-20T10:00:00Z", kind: "prompt", role: "user",
                           content: "<environment_context>\n<cwd>/x</cwd>\n</environment_context>\nFix the build")
        let only = event("e2", at: "2026-11-20T10:00:01Z", kind: "prompt", role: "user",
                         content: "<system_instruction>setup</system_instruction>")
        let items = SessionTimeline.items(messages: [], events: [prompt, only])
        XCTAssertEqual(items.count, 1)
        guard case .event(let row) = items[0] else { return XCTFail("expected an event row") }
        XCTAssertEqual(row.text, "Fix the build")
        XCTAssertNil(row.relayedFrom)
    }

    func testPlaceholder() {
        // Loading until relay history is checked and the session settled.
        XCTAssertEqual(SessionTimeline.placeholder(hasRows: false, historyChecked: false, sessionApplies: false, sessionSettled: false), .loading)
        XCTAssertEqual(SessionTimeline.placeholder(hasRows: true, historyChecked: true, sessionApplies: true, sessionSettled: false), .loading)
        XCTAssertEqual(SessionTimeline.placeholder(hasRows: false, historyChecked: true, sessionApplies: true, sessionSettled: true), .empty)
        XCTAssertEqual(SessionTimeline.placeholder(hasRows: false, historyChecked: true, sessionApplies: false, sessionSettled: false), .empty)
        XCTAssertEqual(SessionTimeline.placeholder(hasRows: true, historyChecked: true, sessionApplies: true, sessionSettled: true), .none)
    }

    func testPreviewCutsLongContent() {
        XCTAssertEqual(SessionTimeline.preview("short").text, "short")
        XCTAssertFalse(SessionTimeline.preview("short").isCut)
        let long = SessionTimeline.preview(String(repeating: "x", count: 700))
        XCTAssertTrue(long.isCut)
        XCTAssertEqual(long.text.count, 601)
        let tall = SessionTimeline.preview((1...20).map(String.init).joined(separator: "\n"))
        XCTAssertTrue(tall.isCut)
        XCTAssertEqual(tall.text, (1...12).map(String.init).joined(separator: "\n") + "…")
    }

    func testFilesSummary() {
        XCTAssertEqual(SessionTimeline.filesSummary([]), "")
        XCTAssertEqual(SessionTimeline.filesSummary(["a/b.swift", "c.swift"]), "b.swift, c.swift")
        XCTAssertEqual(SessionTimeline.filesSummary(["a", "b", "c", "d", "e"]), "a, b, c +2")
    }
}
