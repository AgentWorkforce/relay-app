import XCTest
@testable import AgentRelayiPad

final class ChatEntryRulesTests: XCTestCase {
    private func role(type: String = "", kind: String = "", actor: String = "", tool: String = "") -> ChatEntryRules.Role {
        ChatEntryRules.role(type: type, kind: kind, actorRole: actor, toolName: tool)
    }

    private func event(_ id: String, at seconds: TimeInterval, type: String, actor: String,
                       content: String, tool: String = "") -> SessionEvent {
        SessionEvent(key: .init(source: "codex", machineID: "m1", kind: type, eventID: id),
                     ts: Date(timeIntervalSince1970: 1_790_000_000 + seconds), type: type, actorRole: actor,
                     content: content, toolName: tool)
    }

    private func ids(_ events: [SessionEvent]) -> [String] { events.map(\.key.eventID) }

    func testRoleFollowsTypeThenActor() {
        // What the event is decides first, whichever of type or kind names it.
        for field in ["type", "kind"] {
            func named(_ value: String, actor: String = "assistant", tool: String = "") -> ChatEntryRules.Role {
                field == "type" ? role(type: value, actor: actor, tool: tool) : role(kind: value, actor: actor, tool: tool)
            }
            XCTAssertEqual(named("tool_result", actor: "user"), .toolResult, field)
            XCTAssertEqual(named("tool_use"), .toolCall, field)
            XCTAssertEqual(named("tool_call"), .toolCall, field)
            XCTAssertEqual(named("thinking"), .thinking, field)
            XCTAssertEqual(named("reasoning", actor: "user"), .thinking, field)
            XCTAssertEqual(named("assistant", tool: "Bash"), .toolCall, field)
            // Then who wrote it.
            XCTAssertEqual(named("message", actor: "user"), .person, field)
            XCTAssertEqual(named("message", actor: "Human"), .person, field)
            XCTAssertEqual(named("message", actor: "system"), .activity, field)
            XCTAssertEqual(named("message", actor: "developer"), .activity, field)
            XCTAssertEqual(named("message"), .agent, field)
            XCTAssertEqual(named("user", actor: ""), .person, field)
            // With no actor the kind rules decide.
            XCTAssertEqual(named("prompt", actor: ""), .person, field)
            XCTAssertEqual(named("agent_reasoning", actor: "assistant"), .thinking, field)
            XCTAssertEqual(named("agent_message", actor: ""), .agent, field)
            XCTAssertEqual(named("future_kind", actor: ""), .activity, field)
        }
        XCTAssertEqual(role(actor: "tool_result"), .toolResult)
        XCTAssertEqual(role(actor: "tool"), .toolCall)
        // The desktop's type rule wins over a kind the iOS rules would read otherwise.
        XCTAssertEqual(role(type: "tool_result", kind: "assistant", actor: "assistant"), .toolResult)
        // The chat draws thinking as activity; the event still knows it is thinking.
        let thought = SessionEvent(key: .init(source: "", machineID: "", kind: "", eventID: "t"), ts: nil,
                                   type: "thinking", actorRole: "assistant", content: "Considering the options")
        XCTAssertEqual(thought.role, .activity)
        XCTAssertTrue(thought.isThinking)
        XCTAssertEqual(ChatEntryRules.Role.toolResult.sessionRole, .tool)
    }

    func testBlankContentTakesToolNameOrIsDropped() {
        XCTAssertEqual(ChatEntryRules.shownText(content: "Done.", toolName: "Bash"), "Done.")
        XCTAssertEqual(ChatEntryRules.shownText(content: "  \n", toolName: "Bash"), "Bash")
        XCTAssertEqual(ChatEntryRules.shownText(content: "", toolName: "Read"), "Read")
        XCTAssertNil(ChatEntryRules.shownText(content: " \n\t", toolName: ""))
        XCTAssertNil(ChatEntryRules.shownText(content: "", toolName: ""))
    }

    func testAnAgentLineEqualToAToolCallIsDropped() {
        let events = [event("1", at: 0, type: "tool_use", actor: "assistant", content: "swift test", tool: "Bash"),
                      event("2", at: 1, type: "assistant", actor: "assistant", content: "swift test"),
                      event("3", at: 2, type: "assistant", actor: "assistant", content: "All tests passed.")]
        XCTAssertEqual(ids(ChatEntryRules.withoutToolEchoes(events)), ["1", "3"])
        // A call with blank content is its tool's name, and so is its echo.
        let named = [event("1", at: 0, type: "tool_use", actor: "assistant", content: "", tool: "Read"),
                     event("2", at: 1, type: "assistant", actor: "assistant", content: "Read")]
        XCTAssertEqual(ids(ChatEntryRules.withoutToolEchoes(named)), ["1"])
    }

    func testAPersonLineEqualToAToolCallIsKept() {
        let events = [event("1", at: 0, type: "tool_use", actor: "assistant", content: "swift test", tool: "Bash"),
                      event("2", at: 1, type: "user", actor: "user", content: "swift test")]
        XCTAssertEqual(ids(ChatEntryRules.withoutToolEchoes(events)), ["1", "2"])
    }

    func testNearMatchesAreKept() {
        let events = [event("1", at: 0, type: "tool_use", actor: "assistant", content: "swift test", tool: "Bash"),
                      event("2", at: 1, type: "assistant", actor: "assistant", content: "swift test "),
                      event("3", at: 2, type: "assistant", actor: "assistant", content: "Swift test"),
                      event("4", at: 3, type: "assistant", actor: "assistant", content: "swift  test"),
                      // A tool's result is not a call: text equal to it stays.
                      event("5", at: 4, type: "tool_result", actor: "tool_result", content: "ok"),
                      event("6", at: 5, type: "assistant", actor: "assistant", content: "ok")]
        XCTAssertEqual(ids(ChatEntryRules.withoutToolEchoes(events)), ["1", "2", "3", "4", "5", "6"])
    }

    func testEchoBeforeItsToolCallIsDropped() {
        let events = [event("1", at: 0, type: "assistant", actor: "assistant", content: "git status"),
                      event("2", at: 1, type: "tool_call", actor: "assistant", content: "git status", tool: "Bash")]
        XCTAssertEqual(ids(ChatEntryRules.withoutToolEchoes(events)), ["2"])
    }
}
