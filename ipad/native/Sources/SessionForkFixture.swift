#if DEBUG
import Foundation

/// A closed, offline UI fixture for forking a session, enabled with
/// `-RelayUITestFork` alongside `-RelayUITestConversations`. One computer
/// accepts forks; the spawn POST is the only request that advances it.
/// `-RelayUITestForkOldComputer` makes that computer predate forking.
enum SessionForkFixture {
    private final class State: @unchecked Sendable {
        let lock = NSLock()
        var fork: JSONObject?
    }
    private static let state = State()
    private static let person = "ui-test-person"
    private static let node = "studio-mac-desktop"
    private static var enabled: Bool { ProcessInfo.processInfo.arguments.contains("-RelayUITestFork") }
    private static var oldComputer: Bool { ProcessInfo.processInfo.arguments.contains("-RelayUITestForkOldComputer") }

    static func response(_ path: String, method: String, body: JSONObject?) -> Any? {
        guard enabled else { return nil }
        state.lock.lock()
        defer { state.lock.unlock() }
        if path == "/nodes/\(node)/actions/spawn/invoke", method == "POST" {
            state.fork = body?["input"] as? JSONObject
            return ["invocation_id": "inv_fork_1", "status": "pending"]
        }
        if path == "/actions/spawn/invocations/inv_fork_1" {
            let name = state.fork?["name"] as? String ?? ""
            return ["status": "completed", "output": ["spawned": true, "ready": true, "name": name,
                                                       "worker_cwd": "/Users/sam/checkout"]] as JSONObject
        }
        let forkName = state.fork?["name"] as? String
        if path == "/nodes" { return [computer(forkName: forkName)] }
        if path == "/agents" || path.hasPrefix("/agents?status=") {
            var roster: [JSONObject] = [
                ["name": person, "type": "human", "persona": "Sam Lee", "status": "active"],
                agent("planner", kind: "claude-terminal", title: "Redesign the checkout flow"),
                agent("reviewer", kind: "codex", title: "Review the payments API"),
            ]
            if let forkName { roster.insert(agent(forkName, kind: "claude-terminal", title: "Redesign the checkout flow (fork)"), at: 1) }
            return roster
        }
        if path.hasPrefix("/agents/") {
            let name = String(path.dropFirst("/agents/".count))
            if name == "planner" { return agent("planner", kind: "claude-terminal", title: "Redesign the checkout flow") }
            if name == "reviewer" { return agent("reviewer", kind: "codex", title: "Review the payments API") }
            if name == forkName { return agent(name, kind: "claude-terminal", title: "Redesign the checkout flow (fork)") }
            return JSONObject()
        }
        if path == "/dm/conversations" {
            var threads = [thread("dm_planner", peer: "planner", last: plannerMessages.first!),
                           thread("dm_reviewer", peer: "reviewer", last: reviewerMessages.first!)]
            if let forkName { threads.insert(thread("dm_fork", peer: forkName, last: forkMessages(forkName).first!), at: 0) }
            return threads
        }
        if path.hasPrefix("/dm/dm_planner/messages") { return plannerMessages }
        if path.hasPrefix("/dm/dm_reviewer/messages") { return reviewerMessages }
        if path.hasPrefix("/dm/dm_fork/messages"), let forkName { return forkMessages(forkName) }
        if path.hasSuffix("/readers") { return [JSONObject]() }
        if path.hasPrefix("/dm/") || path.hasPrefix("/channels") { return [JSONObject]() }
        return nil
    }

    /// The spawn input the app sent, for tests.
    static var sentFork: JSONObject? {
        state.lock.lock()
        defer { state.lock.unlock() }
        return state.fork
    }

    private static func computer(forkName: String?) -> JSONObject {
        var capabilities: [JSONObject] = [
            ["name": "spawn:claude", "kind": "capacity"],
            ["name": "spawn:codex", "kind": "capacity"],
            ["name": "relay:live-agents:v1", "metadata": ["names": [forkName].compactMap { $0 }]],
        ]
        if !oldComputer {
            capabilities.append(["name": SessionFork.capability, "kind": "capacity",
                                 "metadata": ["harnesses": ["claude", "codex"], "machine": "Studio Mac"]])
        }
        return ["name": node, "status": "online", "live": true, "handlers_live": true, "tags": [String](),
                "capabilities": capabilities]
    }

    private static func agent(_ name: String, kind: String, title: String) -> JSONObject {
        ["name": name, "status": "active", "address": "\(name)@studio-mac-\(name.count)",
         "last_seen": Date.ISO8601FormatStyle(includingFractionalSeconds: true).format(Date()),
         "metadata": ["source": "relay-desktop", "host": "Studio Mac", "session_kind": kind,
                      "session_id": "session-\(name)", "session_title": title]]
    }

    private static func thread(_ id: String, peer: String, last: JSONObject) -> JSONObject {
        ["id": id,
         "participants": [["agent_name": person, "agent_id": "a_me"], ["agent_name": peer, "agent_id": "a_\(peer)"]],
         "last_message": last]
    }

    private static func message(_ id: String, from sender: String, _ text: String, at time: String) -> JSONObject {
        ["id": id, "agent_name": sender, "agent_id": "a_\(sender)", "text": text, "created_at": "2026-11-10T\(time).000Z"]
    }

    // Newest first, as Relaycast returns them.
    private static let plannerMessages: [JSONObject] = [
        message("p4", from: "planner", "Plan is in docs/checkout-plan.md. Next up: step 3, the payment sheet. Want me to keep going here?", at: "09:12:00"),
        message("p3", from: person, "Yes, and keep Apple Pay first in the list.", at: "09:10:00"),
        message("p2", from: "planner", "Steps: 1 cart summary, 2 address autofill, 3 payment sheet, 4 confirmation. Steps 1 and 2 are done and tested.", at: "09:08:00"),
        message("p1", from: person, "Can you redesign the checkout flow in four steps?", at: "09:00:00"),
    ]

    private static let reviewerMessages: [JSONObject] = [
        message("r1", from: "reviewer", "Payments API review is done: two comments on idempotency keys.", at: "08:40:00"),
    ]

    private static func forkMessages(_ name: String) -> [JSONObject] {
        [message("f1", from: name, "Picked up from planner's checkout plan with Apple Pay first. Steps 1 and 2 are done, so I'm starting on step 3, the payment sheet, in a new branch. Planner keeps its own copy of the work.", at: "09:15:00")]
    }
}
#endif
