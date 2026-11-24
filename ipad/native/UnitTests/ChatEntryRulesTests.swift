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

    // MARK: Copies of session events

    /// An event with a kind as well as a type, and a time that may be absent.
    private func copy(_ id: String, at seconds: TimeInterval? = 0, type: String = "user", kind: String = "",
                      actor: String = "user", content: String, tool: String = "") -> SessionEvent {
        SessionEvent(key: .init(source: "codex", machineID: "m1", kind: type, eventID: id),
                     ts: seconds.map { Date(timeIntervalSince1970: 1_790_000_000 + $0) }, kind: kind, type: type,
                     actorRole: actor, content: content, toolName: tool)
    }

    func testTheHistoryCopyAndTheHookContextAreLeftOut() {
        let events = [copy("1", at: 0, type: "history", content: "ping"),
                      copy("2", at: 0.1, content: "SessionStart hook additional context: This session joins"),
                      copy("3", at: 0.2, content: "ping")]
        XCTAssertEqual(ids(ChatEntryRules.withoutCopies(events)), ["3"])
    }

    func testOnlyAnEventNamedHistoryIsAHistoryCopy() {
        let events = [copy("1", type: "", kind: "history", content: "Ship the harbor notes"),
                      copy("2", type: "History", content: "Ship the harbor notes 2"),
                      copy("3", type: "HISTORY", content: "Ship the harbor notes 3"),
                      copy("4", type: "message", kind: "History", content: "Ship the harbor notes 4"),
                      copy("5", type: "history_note", content: "Ship the harbor notes 5"),
                      copy("6", type: "message", kind: "histories", content: "Ship the harbor notes 6")]
        XCTAssertEqual(ids(ChatEntryRules.withoutCopies(events)), ["5", "6"])
    }

    func testHookContextIsFoundAtTheStartOfTheTextOnly() {
        let prefix = ChatEntryRules.hookContextPrefix
        XCTAssertEqual(prefix, "SessionStart hook additional context:")
        let events = [copy("1", at: 0, content: "\n  " + prefix + " first"),
                      copy("2", at: 20, content: "Note: " + prefix + " in the middle"),
                      copy("3", at: 40, content: "sessionstart hook additional context: lower case"),
                      copy("4", at: 60, type: "assistant", actor: "assistant", content: prefix + " from the agent"),
                      copy("5", at: 80, type: "tool_result", actor: "tool_result", content: prefix + " in a result"),
                      copy("6", at: 100, content: prefix)]
        XCTAssertEqual(ids(ChatEntryRules.withoutCopies(events)), ["2", "3"])
    }

    func testTwoRecordsOfOneTurnWithinTheWindowAreOne() {
        XCTAssertEqual(ChatEntryRules.sameTurnWindow, 10)
        let close = [copy("1", at: 0, content: "ping"), copy("2", at: 0.081, content: "ping")]
        XCTAssertEqual(ids(ChatEntryRules.withoutCopies(close)), ["1"])
        let edge = [copy("1", at: 0, content: "ping"), copy("2", at: 10, content: "ping")]
        XCTAssertEqual(ids(ChatEntryRules.withoutCopies(edge)), ["1"])
        let past = [copy("1", at: 0, content: "ping"), copy("2", at: 10.001, content: "ping")]
        XCTAssertEqual(ids(ChatEntryRules.withoutCopies(past)), ["1", "2"])
        // Either order inside the window is a repeat.
        let backwards = [copy("1", at: 5, content: "ping"), copy("2", at: 4.9, content: "ping")]
        XCTAssertEqual(ids(ChatEntryRules.withoutCopies(backwards)), ["1"])
    }

    func testAChainOfRepeatsIsOneRowButTheSameTextLaterIsANewTurn() {
        let chain = [copy("1", at: 0, content: "ping"), copy("2", at: 8, content: "ping"),
                     copy("3", at: 16, content: "ping"), copy("4", at: 24, content: "ping")]
        XCTAssertEqual(ids(ChatEntryRules.withoutCopies(chain)), ["1"])
        let later = chain + [copy("5", at: 40, content: "ping")]
        XCTAssertEqual(ids(ChatEntryRules.withoutCopies(later)), ["1", "5"])
    }

    func testTheOtherWriterAndOtherTextAreAnotherTurn() {
        let events = [copy("1", at: 0, content: "ok"),
                      copy("2", at: 0.5, type: "assistant", actor: "assistant", content: "ok"),
                      copy("3", at: 1, content: " ok \n"),
                      copy("4", at: 1.5, type: "assistant", actor: "assistant", content: "\nok"),
                      copy("5", at: 2, content: "o k"),
                      copy("6", at: 2.5, content: "Ok")]
        XCTAssertEqual(ids(ChatEntryRules.withoutCopies(events)), ["1", "2", "5", "6"])
    }

    func testOnlyAPersonsAndTheAgentsTextCanBeARepeat() {
        let events = [copy("1", at: 0, type: "tool_use", actor: "assistant", content: "swift test", tool: "Bash"),
                      copy("2", at: 0.5, type: "tool_use", actor: "assistant", content: "swift test", tool: "Bash"),
                      copy("3", at: 1, type: "tool_result", actor: "tool_result", content: "ok"),
                      copy("4", at: 1.5, type: "tool_result", actor: "tool_result", content: "ok"),
                      copy("5", at: 2, type: "thinking", actor: "assistant", content: "Weighing two fixes"),
                      copy("6", at: 2.5, type: "thinking", actor: "assistant", content: "Weighing two fixes"),
                      copy("7", at: 3, type: "note", actor: "system", content: "Resumed"),
                      copy("8", at: 3.5, type: "note", actor: "system", content: "Resumed"),
                      copy("9", at: 4, content: "  "), copy("10", at: 4.5, content: "  ")]
        XCTAssertEqual(ids(ChatEntryRules.withoutCopies(events)), ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"])
    }

    func testAnEventWithoutATimeIsNeverARepeatAndHidesNothing() {
        let events = [copy("1", at: nil, content: "ping"), copy("2", at: nil, content: "ping"),
                      copy("3", at: 0, content: "ping"), copy("4", at: 1, content: "ping"),
                      copy("5", at: nil, content: "ping")]
        // The untimed ones stay; the two timed ones are one turn.
        XCTAssertEqual(ids(ChatEntryRules.withoutCopies(events)), ["1", "2", "3", "5"])
    }

    func testRemovedCopiesTakeNoPartInTheRepeatRule() {
        // A history copy takes no part: the first real prompt stays, its repeat 6 s later goes.
        let events = [copy("1", at: 0, type: "history", content: "ping"),
                      copy("2", at: 9, content: "ping"),
                      copy("3", at: 15, content: "ping")]
        XCTAssertEqual(ids(ChatEntryRules.withoutCopies(events)), ["2"])
        // The kept order is the given order.
        let mixed = [copy("a", at: 30, content: "later"), copy("b", at: 0, type: "history", content: "x"),
                     copy("c", at: 10, content: "earlier")]
        XCTAssertEqual(ids(ChatEntryRules.withoutCopies(mixed)), ["a", "c"])
    }
}
