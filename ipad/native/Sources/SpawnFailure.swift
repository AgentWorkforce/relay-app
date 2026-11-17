import Foundation

/// How a start request ended when it did not fail.
enum SpawnOutcome: Equatable {
    /// The node's receipt confirmed the agent is ready.
    case ready
    /// No receipt, but the node lists the agent as live.
    case running
    /// The node accepted the request and nothing has confirmed it since.
    case unconfirmed
}

/// Turns the node's or Relay's raw start error into a sentence a person can
/// act on. `message` keeps the raw detail for classification; only `summary`
/// is shown (see `error(_:node:name:harness:status:code:)`).
enum SpawnFailure {
    static func message(_ detail: String, node: String, name: String, harness: String) -> String {
        let detail = detail.trimmingCharacters(in: .whitespacesAndNewlines)
        let summary = Self.summary(detail, node: node, name: name, harness: harness)
        let shown = Self.withoutRedactionMarker(detail)
        return shown.isEmpty ? summary : "\(summary)\nDetails: \(shown)"
    }

    /// A scrubber between the node and this app (not the app itself) can
    /// replace the node's error code with `[redacted]`, as it did with
    /// `[redacted] @planner is not an agent on this computer`. The marker
    /// says nothing to a person, so the Details line drops it.
    static func withoutRedactionMarker(_ detail: String) -> String {
        detail.replacingOccurrences(of: #"^\[redacted\]:?\s*"#, with: "", options: .regularExpression)
    }

    /// The node no longer has the fork's source agent: by its code, or by
    /// the node's sentence when the code was replaced on the way.
    static func isForkNotFound(_ detail: String) -> Bool {
        let lower = detail.lowercased()
        return lower.contains("spawn_fork_not_found") || lower.contains("is not an agent on this computer")
    }

    /// The failure as shown: the sentence alone, never a server's or node's
    /// raw detail. `fromServer` is false only when `detail` is the app's own copy.
    static func error(_ detail: String, node: String, name: String, harness: String,
                      status: Int? = nil, code: String = "", fromServer: Bool = true) -> RelayAPIError {
        let trimmed = detail.trimmingCharacters(in: .whitespacesAndNewlines)
        return RelayAPIError(message: Self.message(trimmed, node: node, name: name, harness: harness),
                             status: status, code: code, fromServer: fromServer && !trimmed.isEmpty,
                             summary: Self.summary(trimmed, node: node, name: name, harness: harness))
    }

    static func summary(_ detail: String, node: String, name: String, harness: String) -> String {
        let lower = detail.lowercased()
        let summary: String
        if lower.contains("spawn_fork_refused") {
            summary = "Only the owner of \(node) can fork its sessions. Sign in on this device as that person."
        } else if lower.contains("spawn_fork_unavailable") {
            summary = "\(node) can't copy this session. It may have been archived, or it was opened from an app Agent Relay can't resume."
        } else if isForkNotFound(lower) {
            summary = "\(node) no longer has this agent. Refresh Agents and try again."
        } else if lower.contains("spawn_fork_unsupported") {
            summary = "\(node) can't fork this kind of session yet."
        } else if lower.contains("d1_pressure") || lower.contains("liveness retry pending") {
            summary = "Agent Relay is busy and could not register \(name). Wait a minute, then try again."
        } else if lower.contains("spawn_readiness_timeout") || lower.contains("readiness timeout") {
            summary = "\(name) did not become ready on \(node) in time. Check Agents before trying again."
        } else if lower.contains("preflight") {
            summary = "\(node) could not set up Agent Relay tools for \(name), so it did not start the agent."
        } else if harness == "codex" && lower.contains("codex") && lower.contains("lock") {
            summary = "Codex on \(node) is in use by another session. Try again later or choose another agent type."
        } else if lower.contains("operation not permitted") || lower.contains("permission denied")
                    || lower.contains("eperm") || lower.contains("eacces") {
            summary = "\(node) cannot open that project folder. Folders on an external drive may not be readable by background apps; choose a folder on the internal disk."
        } else if lower.contains("enoent") || lower.contains("no such file") || lower.contains("not a directory") {
            summary = "That project folder does not exist on \(node). Check the saved path."
        } else if lower.contains("name_in_use") || lower.contains("name_taken")
                    || (lower.contains("already") && (lower.contains("exists") || lower.contains("taken") || lower.contains("in use"))) {
            summary = "The name \(name) is already in use, possibly by an earlier start that is still being cleaned up. Choose another name."
        } else {
            summary = "\(node) did not start \(name)."
        }
        return summary
    }
}

/// Checks a project folder typed for a node. The node receives the path as
/// is, so it must be absolute there; `~` is not expanded.
enum SpawnProjectPath {
    static func problem(_ path: String, node: String) -> String? {
        let path = path.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !path.isEmpty else { return nil }
        if path.hasPrefix("~") { return "Enter the full path on \(node), such as /Users/name/project. ~ is not expanded." }
        if !path.hasPrefix("/") { return "Enter the full path on \(node), starting with /." }
        if path.contains("\n") { return "Enter the path on one line." }
        return nil
    }
}
