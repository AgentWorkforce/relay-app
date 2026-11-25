#if DEBUG
import Foundation

/// Synthetic session events for the UI-test fixture agents, used only with
/// `-RelayUITestConversations` in Debug builds. Cloud calls bypass `api()`,
/// so the client asks this before the Cloud events route.
/// `-RelayUITestSessionEventsDelay` holds the first page until the test taps
/// the release control (`RelayUITestReadPostsProbe`), so the loading row can
/// be seen; `-RelayUITestSessionWorking` makes the
/// builder's newest event a tool call from a few seconds ago, so its turn is open.
enum SessionEventsFixture {
    static let sessionIDs: Set<String> = ["session-builder", "session-reviewer"]

    /// How long a held page waits for the test before the hold runs out.
    private static let holdLimit: TimeInterval = 30

    static func covers(_ sessionID: String) -> Bool {
        RelayUITestFixture.enabled && sessionIDs.contains(sessionID)
    }

    static var delaysFirstPage: Bool {
        RelayUITestFixture.enabled && ProcessInfo.processInfo.arguments.contains("-RelayUITestSessionEventsDelay")
    }

    /// Holds a first page until the test releases it: true once released,
    /// false when the hold ran out first. Then the release control reads
    /// "timed out" and the fetch fails, so the test fails instead of passing
    /// on a timer.
    static func holdFirstPage() async throws -> Bool {
        let deadline = Date().addingTimeInterval(holdLimit)
        while true {
            switch SessionEventsGate.shared.current {
            case .released: return true
            case .timedOut: return false
            case .held:
                if Date() >= deadline { return SessionEventsGate.shared.expire() == .released }
                try await Task.sleep(for: .milliseconds(50))
            }
        }
    }

    static func releaseFirstPage() {
        SessionEventsGate.shared.release()
    }

    /// The page for `spec`, as the Cloud route answers it: newest first for
    /// `order=desc`, with an older page behind the builder's cursor.
    static func response(sessionID: String, spec: SessionEventsRequest.Spec) -> Data? {
        guard covers(sessionID) else { return nil }
        let all = events(for: sessionID)
        let ordered = spec.order == .desc ? Array(all.reversed()) : all
        let older = spec.cursor == "fixture-older"
        let olderCount = olderPage.map(\.count) ?? 2
        let page = sessionID == "session-builder"
            ? (older ? Array(ordered.suffix(olderCount)) : Array(ordered.dropLast(olderCount)))
            : ordered
        let body: [String: Any] = ["sessionId": sessionID, "events": page,
                                   "nextCursor": sessionID == "session-builder" && !older ? "fixture-older" : NSNull()]
        return try? JSONSerialization.data(withJSONObject: body)
    }

    private static func events(for sessionID: String) -> [[String: Any]] {
        if sessionID == "session-reviewer" {
            if RelayUITestScreenshot.enabled {
                return [event("r_screenshot", at: "2026-10-08T12:10:00.000Z", kind: "prompt", role: "user",
                              content: """
                              Simulator screenshot fixture

                              Attachments:
                              - inline-fixture.png (image/png, 252 B) saved to /fixture/attachments/ui-test-screenshot/inline-fixture.png
                              """)]
            }
            return [event("r1", at: "2026-10-08T11:58:00.000Z", kind: "assistant", role: "assistant",
                          content: "Reading the diff before I leave comments.")]
        }
        if let olderPage {
            // The plain-row variant opens its first page with a reply.
            let opener = olderNotesOnly ? [event("w0", at: "2026-10-08T11:39:55.000Z", kind: "assistant", role: "assistant",
                                                 content: "Picking up the lantern checks.")] : []
            return olderPage + opener + [
                event("w1", at: "2026-10-08T11:40:00.000Z", kind: "tool_use", role: "assistant",
                      content: "swift test --filter Lantern", tool: "Bash"),
                event("w2", at: "2026-10-08T11:40:10.000Z", kind: "tool_result", role: "tool_result",
                      content: "Lantern tests passed", tool: "Bash"),
                event("w3", at: "2026-10-08T11:40:20.000Z", kind: "tool_use", role: "assistant",
                      content: "git status --short", tool: "Bash"),
                event("w4", at: "2026-10-08T11:41:00.000Z", kind: "assistant", role: "assistant",
                      content: "The lantern checks are green."),
            ]
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

    /// `-RelayUITestSessionOlderTools`: the builder's first page opens with
    /// a run of tool work (three steps), and its older page is tall notes
    /// followed by tool work that joins that run, so loading it grows the
    /// top Activity group at its start and puts more than a screen of rows
    /// above it. `-RelayUITestSessionOlderNotes`: the first page opens with
    /// a plain reply instead, and the older page is the notes alone.
    private static var olderTools: Bool {
        ProcessInfo.processInfo.arguments.contains("-RelayUITestSessionOlderTools")
    }

    private static var olderNotesOnly: Bool {
        ProcessInfo.processInfo.arguments.contains("-RelayUITestSessionOlderNotes")
    }

    /// The older page of those fixtures, oldest first; nil without them.
    private static var olderPage: [[String: Any]]? {
        if olderNotesOnly { return olderNotes }
        if olderTools { return olderNotes + olderToolRun }
        return nil
    }

    private static let olderNotes: [[String: Any]] = (1...6).map { index in
        event("n\(index)", at: "2026-10-08T11:3\(index):00.000Z", kind: "assistant", role: "assistant",
              content: (1...10).map { "Lantern survey note \(index), line \($0): the wick holds." }.joined(separator: "\n"))
    }

    private static let olderToolRun: [[String: Any]] = [
        event("o1", at: "2026-10-08T11:39:40.000Z", kind: "tool_use", role: "assistant",
              content: "swift build --product Lantern", tool: "Bash"),
        event("o2", at: "2026-10-08T11:39:50.000Z", kind: "tool_result", role: "tool_result",
              content: "Build complete", tool: "Bash"),
    ]

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

/// Whether the held first page was released by the test or ran out. It moves
/// from held once and never back; `state` mirrors it for the release control.
final class SessionEventsGate: ObservableObject, @unchecked Sendable {
    enum State: String {
        case held
        case released
        case timedOut = "timed out"
    }

    static let shared = SessionEventsGate()
    private let lock = NSLock()
    private var locked = State.held
    @Published private(set) var state = State.held

    var current: State {
        lock.lock()
        defer { lock.unlock() }
        return locked
    }

    func release() {
        move(to: .released)
    }

    /// Ends the hold unless the test released it first; returns the state it ended in.
    func expire() -> State {
        move(to: .timedOut)
    }

    @discardableResult
    private func move(to next: State) -> State {
        lock.lock()
        if locked == .held { locked = next }
        let now = locked
        lock.unlock()
        DispatchQueue.main.async { self.state = now }
        return now
    }
}
#endif
