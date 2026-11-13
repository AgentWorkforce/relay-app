import Foundation

/// Forking an agent's session from the phone: the agent's computer starts a
/// new headless agent in the same folder as a copy of the conversation
/// (Claude's `--fork-session`, Codex's `thread/fork`), and the original keeps
/// running. Only computers advertising `relay:spawn-fork:v1` accept a fork;
/// older ones would ignore the request and start an empty agent.
enum SessionFork {
    static let capability = "relay:spawn-fork:v1"

    enum Availability: Equatable {
        /// Nodes to try, best first, and the harness to fork with.
        case available(harness: String, nodes: [RelaySpawnNode])
        case unavailable(String)

        var isAvailable: Bool {
            if case .available = self { return true }
            return false
        }
    }

    /// Where a fork entry point appears. One placement ships; DEBUG builds
    /// can show the others with `-RelayForkPlacement header|menu|row`.
    enum Placement: String, CaseIterable {
        case header, menu, row

        static var current: Placement {
            #if DEBUG
            if let value = UserDefaults.standard.string(forKey: "RelayForkPlacement"),
               let placement = Placement(rawValue: value) { return placement }
            #endif
            return .header
        }
    }

    /// The spawn harness that forks this agent's kind of session.
    static func harness(for agent: RelayAgent) -> String? {
        switch agent.cli {
        case "claude", "claudecode": "claude"
        case "codex": "codex"
        default: nil
        }
    }

    /// Whether an agent row or conversation offers a fork at all: people and
    /// channels have no session to copy.
    static func offered(for agent: RelayAgent) -> Bool {
        !agent.isHuman && agent.source != .channel && !agent.name.isEmpty
    }

    static func availability(for agent: RelayAgent, nodes: [RelaySpawnNode]) -> Availability {
        guard let harness = harness(for: agent) else {
            return .unavailable("Only Claude and Codex sessions can be forked.")
        }
        let capable = nodes.filter { $0.forkHarnesses.contains(harness) }
        let matched: [RelaySpawnNode]
        if !agent.node.isEmpty {
            matched = capable.filter { $0.name == agent.node }
        } else {
            matched = capable.filter {
                !agent.machine.isEmpty && MobileMachineFilter.key($0.machine) == MobileMachineFilter.key(agent.machine)
            }
        }
        guard !matched.isEmpty else {
            let machine = agent.machine.isEmpty ? "This agent's computer" : agent.machine
            return .unavailable("\(machine) isn't online with a version of Agent Relay that can fork sessions. Update Agent Relay on it, then try again.")
        }
        return .available(harness: harness, nodes: matched)
    }

    /// `<source>-fork`, then `<source>-fork-2` and on, within the 48-character name rule.
    /// Keeps going past 99 until a free name remains; truncation can repeat a
    /// candidate, and that occupied name is not offered again.
    static func suggestedName(for source: String, taken: Set<String>) -> String {
        let base = String("\(source)-fork".prefix(48))
        if !taken.contains(base) { return base }
        var index = 2
        var seen = Set<String>()
        while true {
            let suffix = "-\(index)"
            if suffix.count >= 48 { return base }
            let candidate = String(base.prefix(48 - suffix.count)) + suffix
            if !taken.contains(candidate) { return candidate }
            if !seen.insert(candidate).inserted { return base }
            index += 1
        }
    }

    /// The spawn POST left the phone and no HTTP answer came back, so the
    /// computer may already be starting the fork. The sheet must not send it again.
    static let uncertainSubmission = "fork_submission_uncertain"

    static func unanswered(_ error: Error) -> Error {
        if error is CancellationError || error is RelayAPIError { return error }
        return RelayAPIError(
            message: "The fork may have started, but no reply arrived. Check Agents before trying again.",
            code: uncertainSubmission)
    }

    static func submissionUncertain(_ error: Error) -> Bool {
        (error as? RelayAPIError)?.code == uncertainSubmission
    }

    /// A node that answered that the agent isn't there: try the next one on
    /// the same machine (a computer can run more than one Agent Relay).
    static func triesNextNode(after error: Error) -> Bool {
        let detail = ((error as? RelayAPIError)?.message ?? error.localizedDescription).lowercased()
        return detail.contains("spawn_fork_not_found")
    }
}
