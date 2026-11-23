import XCTest
@testable import AgentRelayiPad

final class OlderPageAnchorTests: XCTestCase {
    private func event(_ id: String, at time: String?, kind: String = "assistant", content: String = "x",
                       tool: String = "") -> SessionEvent {
        SessionEvent(key: .init(source: "codex", machineID: "m1", kind: kind, eventID: id),
                     ts: time.flatMap(SessionEventPage.date), kind: kind, actorRole: "assistant", content: content, toolName: tool)
    }

    private func message(_ id: String, at time: String) -> RelayMessage {
        RelayMessage(["id": id, "text": "Lantern lit", "agent_name": "quill", "created_at": time], index: 0)
    }

    private func timeline(_ events: [SessionEvent], messages: [RelayMessage] = []) -> [ChatTimelineItem] {
        SessionTimeline.grouped(SessionTimeline.items(messages: messages, events: events))
    }

    private let firstPage = [
        SessionEvent(key: .init(source: "codex", machineID: "m1", kind: "assistant", eventID: "p1"),
                     ts: SessionEventPage.date("2026-11-20T10:00:00Z"), kind: "assistant", actorRole: "assistant",
                     content: "Picking up the lantern checks."),
    ]

    func testARowThatWasFirstIsKeptOnceTheOlderPageLands() throws {
        let anchor = try XCTUnwrap(OlderPageAnchor(sessionID: "s1", oldest: firstPage.first))
        let older = [event("o1", at: "2026-11-20T09:00:00Z", content: "Older note")]
        let after = timeline(older + firstPage)
        XCTAssertEqual(anchor.step(sessionID: "s1", oldestID: "codex|m1|assistant|o1", items: after),
                       .scroll("event:codex|m1|assistant|p1"))
        // Before the page lands there is nothing to do yet.
        XCTAssertEqual(anchor.step(sessionID: "s1", oldestID: "codex|m1|assistant|p1", items: timeline(firstPage)), .wait)
    }

    func testAGroupJoinedByOlderToolWorkIsKept() throws {
        let calls = [event("w1", at: "2026-11-20T10:00:00Z", kind: "tool_use", content: "swift test", tool: "Bash"),
                     event("w2", at: "2026-11-20T10:00:01Z", kind: "tool_use", content: "git status", tool: "Bash")]
        let anchor = try XCTUnwrap(OlderPageAnchor(sessionID: "s1", oldest: calls.first))
        let older = [event("n1", at: "2026-11-20T09:00:00Z", content: "Older note"),
                     event("o1", at: "2026-11-20T09:59:00Z", kind: "tool_use", content: "swift build", tool: "Bash")]
        let after = timeline(older + calls)
        // The older note is new; the group holds rows from before, under the same id.
        XCTAssertEqual(anchor.step(sessionID: "s1", oldestID: "codex|m1|assistant|n1", items: after),
                       .scroll(timeline(calls)[0].id))
    }

    func testARelayMessageThatWasFirstIsKept() throws {
        let anchor = try XCTUnwrap(OlderPageAnchor(sessionID: "s1", oldest: firstPage.first))
        let held = message("m_1", at: "2026-11-20T09:30:00Z")
        let older = [event("o1", at: "2026-11-20T09:00:00Z", content: "Older note")]
        XCTAssertEqual(anchor.step(sessionID: "s1", oldestID: "codex|m1|assistant|o1",
                                   items: timeline(older + firstPage, messages: [held])), .scroll("m_1"))
    }

    func testAnotherChatOrNoRowFromBeforeDropsIt() throws {
        let anchor = try XCTUnwrap(OlderPageAnchor(sessionID: "s1", oldest: firstPage.first))
        let older = [event("o1", at: "2026-11-20T09:00:00Z")]
        XCTAssertEqual(anchor.step(sessionID: "s2", oldestID: "codex|m1|assistant|o1", items: timeline(older)), .drop)
        XCTAssertEqual(anchor.step(sessionID: "s1", oldestID: nil, items: []), .drop)
        // The store was replaced: nothing from before is shown.
        XCTAssertEqual(anchor.step(sessionID: "s1", oldestID: "codex|m1|assistant|o1", items: timeline(older)), .drop)
        // No events, no anchor.
        XCTAssertNil(OlderPageAnchor(sessionID: "s1", oldest: nil))
    }
}
