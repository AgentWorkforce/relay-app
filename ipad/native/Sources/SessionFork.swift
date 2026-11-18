import Foundation

/// Forking an agent's session from the phone: the agent's computer starts a
/// new headless agent in the same folder as a copy of the conversation
/// (Claude's `--fork-session`, Codex's `thread/fork`, Grok's own session
/// fork), and the original keeps
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

    /// An agent a node reports having spawned, and the harness it runs.
    struct Spawned: Equatable, Sendable {
        let node: String
        let harness: String
    }

    /// `agent`'s entries in the nodes' spawned agents, in node order: every
    /// node that reports its name, or only its own node when it names one.
    static func spawnedEntries(for agent: RelayAgent, in spawned: [String: [Spawned]]) -> [Spawned] {
        (spawned[agent.name] ?? []).filter { agent.node.isEmpty || $0.node == agent.node }
    }

    /// The harness `agent`'s nodes state, only when they all state the same
    /// one; nodes that disagree state nothing.
    static func spawnedHarness(for agent: RelayAgent, in spawned: [String: [Spawned]]) -> String {
        let harnesses = Set(spawnedEntries(for: agent, in: spawned).map(\.harness))
        return harnesses.count == 1 ? harnesses.first ?? "" : ""
    }

    /// The first harness stated about `agent`, in order: the nodes that
    /// spawned it, its current roster record, then the copy at hand. Empty
    /// when nothing states one. A name or persona is never a statement.
    /// The fork and the icon both decide from this.
    static func statedHarness(for agent: RelayAgent, roster: [RelayAgent] = [],
                              spawned: [String: [Spawned]] = [:]) -> String {
        [spawnedHarness(for: agent, in: spawned),
         roster.first { $0.name == agent.name }?.statedCLI ?? "",
         agent.statedCLI].first { !$0.isEmpty } ?? ""
    }

    /// The spawn harness that forks this agent's kind of session, from the
    /// first statement about it (`statedHarness(for:roster:spawned:)`). A
    /// name or persona is never evidence, since the node forks with exactly
    /// the harness asked for and a wrong one fails on the computer.
    static func harness(for agent: RelayAgent, roster: [RelayAgent] = [],
                        spawned: [String: [Spawned]] = [:]) -> String? {
        switch statedHarness(for: agent, roster: roster, spawned: spawned) {
        case "claude", "claudecode": return "claude"
        case "codex": return "codex"
        case "grok": return "grok"
        default: return nil
        }
    }

    /// Whether an agent row or conversation offers a fork at all: people and
    /// channels have no session to copy.
    static func offered(for agent: RelayAgent) -> Bool {
        !agent.isHuman && agent.source != .channel && !agent.name.isEmpty
    }

    /// Whether `agent` still runs on its computer, judged from the current
    /// roster and the nodes' live-agent lists, never from the copy a row or
    /// an open conversation was built from: a registration that stood down
    /// is not a fork source, and its computer would answer
    /// `spawn_fork_not_found`.
    static func isLive(_ agent: RelayAgent, roster: [RelayAgent], fleet: [RelayAgent], at now: Date = Date()) -> Bool {
        if fleet.contains(where: { $0.name == agent.name && $0.attachable && (agent.node.isEmpty || $0.node == agent.node) }) {
            return true
        }
        guard agent.source != .drive, let current = roster.first(where: { $0.name == agent.name }) else { return false }
        return current.isReachable(at: now)
    }

    /// `live`: `isLive(_:roster:fleet:at:)` for `agent`.
    /// `roster` and `spawned` state the agent's harness and, for a spawned
    /// agent with no node of its own, every node reporting it, in order
    /// (`harness(for:roster:spawned:)`).
    static func availability(for agent: RelayAgent, nodes: [RelaySpawnNode], live: Bool,
                             roster: [RelayAgent] = [], spawned: [String: [Spawned]] = [:]) -> Availability {
        guard live else {
            return .unavailable("\(agent.name) is no longer running. Only active agents can be forked.")
        }
        guard let harness = harness(for: agent, roster: roster, spawned: spawned) else {
            return .unavailable("Only Claude, Codex and Grok sessions can be forked.")
        }
        let capable = nodes.filter { $0.forkHarnesses.contains(harness) }
        let matched: [RelaySpawnNode]
        // A name two nodes report (one may be a stale entry) keeps both as
        // candidates, so a node answering `spawn_fork_not_found` passes to the next.
        let reported = agent.node.isEmpty ? spawnedEntries(for: agent, in: spawned).map(\.node) : [agent.node]
        if !reported.isEmpty {
            matched = reported.flatMap { node in capable.filter { $0.name == node } }
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
        return SpawnFailure.isForkNotFound(detail)
    }
}
