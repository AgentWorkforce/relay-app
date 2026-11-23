#if DEBUG
import Foundation

/// Synthetic session events for the UI-test fixture agents, used only with
/// `-RelayUITestConversations` in Debug builds. Cloud calls bypass `api()`,
/// so the client asks this before the Cloud events route.
/// `-RelayUITestSessionEventsDelay` holds the first page for two seconds so
/// the loading row can be seen; `-RelayUITestSessionWorking` makes the
/// builder's newest event a tool call from a few seconds ago, so its turn is open.
enum SessionEventsFixture {
    static let sessionIDs: Set<String> = ["session-builder", "session-reviewer"]

    static func covers(_ sessionID: String) -> Bool {
        RelayUITestFixture.enabled && sessionIDs.contains(sessionID)
    }

    static var delaysFirstPage: Bool {
        RelayUITestFixture.enabled && ProcessInfo.processInfo.arguments.contains("-RelayUITestSessionEventsDelay")
    }

    /// The page for `spec`, as the Cloud route answers it: newest first for
    /// `order=desc`, with an older page behind the builder's cursor.
    static func response(sessionID: String, spec: SessionEventsRequest.Spec) -> Data? {
        guard covers(sessionID) else { return nil }
        let all = events(for: sessionID)
        let ordered = spec.order == .desc ? Array(all.reversed()) : all
        let older = spec.cursor == "fixture-older"
        let page = sessionID == "session-builder"
            ? (older ? Array(ordered.suffix(2)) : Array(ordered.dropLast(2)))
            : ordered
        let body: [String: Any] = ["sessionId": sessionID, "events": page,
                                   "nextCursor": sessionID == "session-builder" && !older ? "fixture-older" : NSNull()]
        return try? JSONSerialization.data(withJSONObject: body)
    }

    private static func events(for sessionID: String) -> [[String: Any]] {
        if sessionID == "session-reviewer" {
            return [event("r1", at: "2026-10-08T11:58:00.000Z", kind: "assistant", role: "assistant",
                          content: "Reading the diff before I leave comments.")]
        }
        var rows: [[String: Any]] = [
            event("b0", at: "2026-10-08T11:50:00.000Z", kind: "prompt", role: "user",
                  content: "<environment_context>\n<cwd>/work</cwd>\n</environment_context>\nSet up the release build"),
            event("b1", at: "2026-10-08T11:51:00.000Z", kind: "assistant", role: "assistant",
                  content: "Starting with the project settings."),
            // The session's copy of the relay message "Ship it": shown once, as the relay bubble.
            event("b2", at: "2026-10-08T11:58:00.000Z", kind: "prompt", role: "user",
                  content: "[via Agent Relay — from @ui-test-person to @builder · message m_me_builder · ref 0123456789ab]\n[reply: x]\n\nShip it"),
            event("b3", at: "2026-10-08T11:58:30.000Z", kind: "assistant", role: "assistant",
                  content: "Running the build checks now."),
            event("b4", at: "2026-10-08T11:59:00.000Z", kind: "tool_use", role: "assistant",
                  content: "make release\n" + String(repeating: "step output line\n", count: 40),
                  tool: "Bash", status: "completed", files: ["Sources/App.swift", "Sources/Build.swift"]),
            // A relay send in a tool call, Cloud's text copy of it (not shown),
            // a relay message read from the inbox, and thinking (not shown).
            event("b8", at: "2026-10-08T11:59:30.000Z", kind: "tool_use", role: "assistant",
                  content: "agent-relay-probe send --to scout --body 'Release build is ready for review.'", tool: "Bash"),
            event("b9", at: "2026-10-08T11:59:31.000Z", kind: "assistant", role: "assistant",
                  content: "agent-relay-probe send --to scout --body 'Release build is ready for review.'"),
            event("b10", at: "2026-10-08T11:59:40.000Z", kind: "tool_result", role: "tool_result",
                  content: "[via Agent Relay — from @scout to @builder · ref 0a1b2c3d4e5f]\n[reply: x]\n\nLooks good from here.\n[end of Agent Relay message · ref 0a1b2c3d4e5f]",
                  tool: "Bash"),
            event("b11", at: "2026-10-08T11:59:45.000Z", kind: "thinking", role: "assistant",
                  content: "Weighing whether to tag the release"),
            event("b5", at: "2026-10-08T12:00:30.000Z", kind: "assistant", role: "assistant",
                  content: "All release checks passed."),
            ["kind": "future_kind", "eventId": "b6", "ts": "2026-10-08T12:00:40.000Z", "content": "Checkpoint saved"],
        ]
        if ProcessInfo.processInfo.arguments.contains("-RelayUITestSessionWorking") {
            rows.append(event("b7", at: workingCallAt, kind: "tool_use", role: "assistant", content: "swift test",
                              tool: "Bash", status: "running"))
        }
        return rows
    }

    /// When the working fixture's running tool call started: a few seconds
    /// before launch, the same on every read, as a stored event is. Stamped
    /// per read, it changed the store on every poll.
    private static let workingCallAt = Date.ISO8601FormatStyle(includingFractionalSeconds: true)
        .format(Date().addingTimeInterval(-5))

    private static func event(_ id: String, at ts: String, kind: String, role: String, content: String,
                              tool: String? = nil, status: String? = nil, files: [String] = []) -> [String: Any] {
        var row: [String: Any] = ["eventId": id, "ts": ts, "kind": kind, "type": kind, "actorRole": role,
                                  "content": content, "contentTruncated": false, "filesTouched": files,
                                  "machineId": "fixture-machine", "source": "fixture"]
        if let tool { row["toolName"] = tool }
        if let status { row["toolStatus"] = status }
        return row
    }
}
#endif
