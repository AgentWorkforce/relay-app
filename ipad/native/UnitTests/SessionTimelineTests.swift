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

    private func rows(_ items: [ChatTimelineItem]) -> [SessionEventRow] {
        items.compactMap { if case .event(let row) = $0 { row } else { nil } }
    }

    func testThinkingIsNotShown() {
        let events = [event("e1", at: "2026-11-20T10:00:00Z", kind: "reasoning", content: "Weighing two fixes"),
                      SessionEvent(key: .init(source: "codex", machineID: "m1", kind: "x", eventID: "e2"),
                                   ts: SessionEventPage.date("2026-11-20T10:00:01Z"), kind: "x", type: "thinking",
                                   actorRole: "assistant", content: "Still weighing"),
                      event("e3", at: "2026-11-20T10:00:02Z", content: "Going with the smaller fix.")]
        let items = SessionTimeline.items(messages: [], events: events)
        XCTAssertEqual(ids(items), ["event:codex|m1|assistant|e3"])
        // Blank content shows the tool's name, or the event is left out.
        let blank = [event("t1", at: "2026-11-20T10:00:03Z", kind: "tool_use", content: " ", tool: "Read"),
                     event("t2", at: "2026-11-20T10:00:04Z", kind: "future_kind", role: "", content: "\n")]
        XCTAssertEqual(rows(SessionTimeline.items(messages: [], events: blank)).map(\.text), ["Read"])
    }

    func testAnOutgoingSendIsShownOnceAndMarked() {
        let send = "agent-relay-probe send --socket s --to @scout --body 'Release notes are ready.'"
        let events = [event("e1", at: "2026-11-20T10:00:00Z", kind: "tool_use", content: send, tool: "Bash"),
                      // Cloud's text copy of the call, and the call recorded again.
                      event("e2", at: "2026-11-20T10:00:01Z", content: send),
                      event("e3", at: "2026-11-20T10:00:02Z", kind: "tool_call", content: send, tool: "Bash"),
                      // Its output is not a send.
                      event("e4", at: "2026-11-20T10:00:03Z", kind: "tool_result", role: "tool_result", content: send)]
        let shown = rows(SessionTimeline.items(messages: [], events: events))
        XCTAssertEqual(shown.map(\.id), ["event:codex|m1|tool_use|e1", "event:codex|m1|tool_result|e4"])
        XCTAssertEqual(shown[0].text, "Release notes are ready.")
        XCTAssertEqual(shown[0].mark, .to("scout"))
        XCTAssertNil(shown[1].mark)
        // A relay tool call with no recipient is still a send.
        let post = event("e5", at: "2026-11-20T10:00:04Z", kind: "tool_use",
                         content: #"send_relay_message {"text": "Done."}"#, tool: "send_relay_message")
        XCTAssertEqual(rows(SessionTimeline.items(messages: [], events: [post])).first?.mark, .to(nil))
        // The agent's relay message the chat already holds stays; the session's copy goes.
        let reply = message("m_9", at: "2026-11-20T10:00:05Z", text: "Release notes are ready.", sender: "builder")
        XCTAssertEqual(ids(SessionTimeline.items(messages: [reply], events: Array(events.prefix(1)), me: "pilot")), ["m_9"])
        // Direct messages alone come through unchanged.
        XCTAssertEqual(ids(SessionTimeline.items(messages: [reply], events: [], me: "pilot")), ["m_9"])
    }

    func testAnIncomingFrameInToolOutputIsMarked() {
        let inbox = "1 unread\n[via Agent Relay — from @mapper (mapper@harbor) to @builder · ref 1a]\n[reply: x]\n\nThe map is updated.\n[end of Agent Relay message · ref 1a]"
        let read = event("e1", at: "2026-11-20T10:00:00Z", kind: "tool_result", role: "tool_result", content: inbox, tool: "Bash")
        let again = event("e2", at: "2026-11-20T10:00:01Z", kind: "tool_result", role: "tool_result", content: inbox, tool: "Bash")
        let shown = rows(SessionTimeline.items(messages: [], events: [read, again]))
        XCTAssertEqual(shown.map(\.id), ["event:codex|m1|tool_result|e1"])
        XCTAssertEqual(shown[0].text, "The map is updated.")
        // Only session text names mapper: the mark names no one.
        XCTAssertEqual(shown[0].mark, .from(nil))
        // Once the chat holds a relay message from mapper, the mark names them.
        let known = message("m_1", at: "2026-11-19T09:00:00Z", text: "Starting the map.", sender: "mapper")
        XCTAssertEqual(rows(SessionTimeline.items(messages: [known], events: [read])).first?.mark, .from("mapper"))
        // A relay message the chat holds is not shown again from the session.
        let mine = message("m_2", at: "2026-11-20T09:59:00Z", text: "The map is updated.", sender: "pilot")
        XCTAssertEqual(ids(SessionTimeline.items(messages: [mine], events: [read], me: "pilot")), ["m_2"])
        // A relayed prompt is a received message too.
        let prompt = event("e3", at: "2026-11-20T10:00:02Z", kind: "prompt", role: "user",
                           content: envelope(from: "mapper", id: "m_5", "Check the legend"))
        XCTAssertEqual(rows(SessionTimeline.items(messages: [], events: [prompt])).first?.mark, .from(nil))
        // The agent quoting a frame inside a sentence is its own text.
        let quote = event("e4", at: "2026-11-20T10:00:03Z", content: "Lines start with [via Agent Relay — from @x · ref 1]")
        XCTAssertNil(rows(SessionTimeline.items(messages: [], events: [quote])).first?.mark)
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
