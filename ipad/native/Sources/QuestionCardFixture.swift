#if DEBUG
import Foundation

/// An offline chat with one synthetic question, enabled with
/// `-RelayUITestConversations -RelayUITestQuestionCard`. A POST /dm to the
/// asking agent is recorded and appears in that chat's history, as Relaycast
/// would show it; nothing leaves the simulator.
enum QuestionCardFixture {
    private final class State: @unchecked Sendable {
        let lock = NSLock()
        var sent: [String] = []
        var notes: [String] = []
    }
    private static let state = State()
    private static let person = "ui-test-person"
    static let agent = "planner"
    static let ref = "q1a2b3c4d"

    static var enabled: Bool {
        RelayUITestFixture.enabled && ProcessInfo.processInfo.arguments.contains("-RelayUITestQuestionCard")
    }

    /// Asked a minute before launch; it expires 30 minutes after that.
    private static let askedAt = Date().addingTimeInterval(-60)

    static let questionText = """
        [Question \(ref) from @\(agent) on test-mini]
        Working on: Picking a release window
        session fixture-session
        Release window: Which day should the release go out?
        1. Tuesday
        2. Thursday — after the freeze lifts
        Choose a number, or answer in free text.
        Reply: answer \(ref) <n|text>. Expires in 30 min; the agent continues without it.
        """

    // MARK: Diagnostics (fixture launches only)

    /// Records one step of the composer, kept for the on-screen report and
    /// printed to the log.
    static func note(_ line: String) {
        guard enabled else { return }
        NSLog("Question card fixture: %@", line)
        state.lock.lock()
        defer { state.lock.unlock() }
        state.notes.append(line)
        if state.notes.count > 30 { state.notes.removeFirst(state.notes.count - 30) }
    }

    /// One outgoing bubble as the report shows it.
    static func describe(_ message: OutgoingMessage) -> String {
        "\(message.id.prefix(8)):\(message.state):\(message.messageID ?? "-"):agent=\(message.agentID)"
    }

    /// What the composer holds and what history shows, for the UI test's
    /// kept accessibility tree.
    static func report(outgoing: [OutgoingMessage], historyIDs: [String], selected: String?) -> String {
        state.lock.lock()
        defer { state.lock.unlock() }
        return ["posts=\(state.sent.count)", "selected=\(selected ?? "nil")",
                "outgoing=[\(outgoing.map(describe).joined(separator: ", "))]",
                "history=[\(historyIDs.joined(separator: ", "))]",
                "notes:"] .joined(separator: " | ") + " " + state.notes.joined(separator: " || ")
    }

    static func response(_ path: String, method: String, body: JSONObject?) -> Any? {
        guard enabled else { return nil }
        state.lock.lock()
        defer { state.lock.unlock() }
        let stamp = Date.ISO8601FormatStyle(includingFractionalSeconds: true)
        if path == "/dm", method == "POST" {
            guard body?["to"] as? String == agent, let text = body?["text"] as? String else { return nil }
            state.sent.append(text)
            NSLog("Question card fixture: POST /dm #%d", state.sent.count)
            return ["id": "question-answer-\(state.sent.count)", "conversation_id": "dm_\(agent)"]
        }
        let question: JSONObject = ["id": "question-1", "agent_name": agent, "agent_id": "a_\(agent)",
                                    "text": questionText, "created_at": stamp.format(askedAt)]
        let answers: [JSONObject] = state.sent.enumerated().map {
            ["id": "question-answer-\($0.offset + 1)", "agent_name": person, "agent_id": "a_me", "text": $0.element,
             "created_at": stamp.format(askedAt.addingTimeInterval(Double($0.offset + 1)))]
        }
        let roster: JSONObject = ["name": agent, "status": "active", "address": "\(agent)@test-mini-789",
                                  "last_seen": stamp.format(Date()), "metadata": ["host": "test-mini", "cli": "codex"]]
        if path == "/agents" || path.hasPrefix("/agents?status=") { return [roster] }
        if path == "/agents/\(agent)" { return roster }
        if path == "/dm/conversations" {
            return [["id": "dm_\(agent)",
                     "participants": [["agent_name": person, "agent_id": "a_me"],
                                      ["agent_name": agent, "agent_id": "a_\(agent)"]],
                     "last_message": answers.last ?? question] as JSONObject]
        }
        // Newest first, as Relaycast returns them.
        if path.hasPrefix("/dm/dm_\(agent)/messages") { return Array((answers.reversed()) + [question]) }
        if path.hasSuffix("/readers") { return [JSONObject]() }
        return nil
    }
}
#endif
