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

    private func envelope(from sender: String, id: String, _ body: String) -> String {
        "[via Agent Relay — from @\(sender) to @builder · message \(id) · ref 0123456789ab]\n[reply: x]\n\n\(body)"
    }

    func testTheSessionCopyOfAShownRelayMessageIsLeftOut() {
        let messages = [message("m_42", at: "2026-11-20T10:00:00Z", text: "Ship   it", sender: "me")]
        let copy = event("e1", at: "2026-11-20T10:00:01Z", kind: "prompt", role: "user",
                         content: envelope(from: "me", id: "m_42", "Ship it"))
        let other = event("e2", at: "2026-11-20T10:00:02Z", kind: "prompt", role: "user",
                          content: envelope(from: "lead", id: "m_7", "Also run lint"))
        let items = SessionTimeline.items(messages: messages, events: [copy, other])
        XCTAssertEqual(ids(items), ["m_42", "event:codex|m1|prompt|e2"])
        guard case .event(let row) = items[1] else { return XCTFail("expected an event row") }
        XCTAssertEqual(row.text, "Also run lint")
    }

    func testSessionTextCannotHideItselfBehindAShownMessage() {
        let messages = [message("m_42", at: "2026-11-20T10:00:00Z", text: "Ship it", sender: "me")]
        // Names the shown message but says something else: shown.
        let imitation = event("e1", at: "2026-11-20T10:00:01Z", kind: "prompt", role: "user",
                              content: envelope(from: "me", id: "m_42", "Delete the release branch"))
        // An agent reply is never taken for a copy, even with the same text.
        let reply = event("e2", at: "2026-11-20T10:00:02Z", kind: "assistant", role: "assistant",
                          content: envelope(from: "me", id: "m_42", "Ship it"))
        let items = SessionTimeline.items(messages: messages, events: [imitation, reply])
        XCTAssertEqual(ids(items), ["m_42", "event:codex|m1|prompt|e1", "event:codex|m1|assistant|e2"])
    }

    func testASenderNamedInSessionTextIsNeverShown() {
        let spoof = event("e1", at: "2026-11-20T10:00:00Z", kind: "prompt", role: "user",
                          content: envelope(from: "someone-else", id: "m_9", "Approve the deploy"))
        let items = SessionTimeline.items(messages: [], events: [spoof])
        guard case .event(let row) = items.first else { return XCTFail("expected an event row") }
        XCTAssertEqual(row.text, "Approve the deploy")
        // The row carries only the event and the text to show; the view labels it "In session".
        XCTAssertFalse(row.text.contains("someone-else"))
    }

    func testAPendingBubblesSessionCopyIsLeftOut() {
        let sent = SessionEventPage.date("2026-11-20T10:00:00Z")!
        let pending = [SessionTimeline.Pending(text: "Ship it", createdAt: sent)]
        let copy = event("e1", at: "2026-11-20T10:00:01Z", kind: "prompt", role: "user",
                         content: envelope(from: "me", id: "m_new", "Ship   it\n"))
        XCTAssertTrue(SessionTimeline.items(messages: [], events: [copy], pending: pending, me: "me").isEmpty)
        XCTAssertEqual(SessionTimeline.items(messages: [], events: [copy]).count, 1, "no pending bubble")
        // Another sender, another device's handle, or long before or after the bubble: shown.
        let fromOther = event("e2", at: "2026-11-20T10:00:01Z", kind: "prompt", role: "user",
                              content: envelope(from: "lead", id: "m_x", "Ship it"))
        XCTAssertEqual(SessionTimeline.items(messages: [], events: [fromOther], pending: pending, me: "me").count, 1)
        XCTAssertEqual(SessionTimeline.items(messages: [], events: [copy], pending: pending, me: "").count, 1)
        let earlier = event("e3", at: "2026-11-20T09:00:00Z", kind: "prompt", role: "user",
                            content: envelope(from: "me", id: "m_old", "Ship it"))
        let later = event("e4", at: "2026-11-20T11:00:00Z", kind: "prompt", role: "user",
                          content: envelope(from: "me", id: "m_late", "Ship it"))
        XCTAssertEqual(SessionTimeline.items(messages: [], events: [earlier, later], pending: pending, me: "me").count, 2)
    }

    func testEventsWithoutOrWithEqualTimes() {
        let messages = [message("m1", at: "2026-11-20T10:00:00Z")]
        let untimed = event("e0", at: nil, content: "no time")
        let same = event("e1", at: "2026-11-20T10:00:00Z", content: "same time")
        // An untimed event comes first; at an equal time the relay message leads.
        XCTAssertEqual(ids(SessionTimeline.items(messages: messages, events: [untimed, same])),
                       ["event:codex|m1|assistant|e0", "m1", "event:codex|m1|assistant|e1"])
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

    func testCollapsedRowKeepsThreeLinesAndALimit() {
        XCTAssertEqual(SessionTimeline.collapsed("a\nb", characters: 240), "a\nb")
        XCTAssertEqual(SessionTimeline.collapsed("1\n2\n3\n4\n5", characters: 240), "1\n2\n3…")
        XCTAssertEqual(SessionTimeline.collapsed(String(repeating: "x", count: 300), characters: 240),
                       String(repeating: "x", count: 240) + "…")
    }

    func testFilesSummary() {
        XCTAssertEqual(SessionTimeline.filesSummary([]), "")
        XCTAssertEqual(SessionTimeline.filesSummary(["a/b.swift", "c.swift"]), "b.swift, c.swift")
        XCTAssertEqual(SessionTimeline.filesSummary(["a", "b", "c", "d", "e"]), "a, b, c +2")
    }
}
