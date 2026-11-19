import Foundation

/// Whether an agent is active: the one rule behind every place a person picks
/// an agent (the Agents list, Fork, Call and New message). An agent that stood
/// down is not offered anywhere; its computer would answer a fork with
/// `spawn_fork_not_found` and a call or DM would go unanswered.
///
/// An agent is active when a node lists it among its live agents, or when its
/// current roster record is reachable (`RelayAgent.isReachable`: a reachable
/// status inside the core's five-minute presence window). The copy a row or an
/// open conversation was built from is never evidence: only the current roster
/// and node lists are.
///
/// Presence is unknown until the first roster read of a session answers
/// (`presenceKnown` false). Nothing is hidden or refused for being inactive
/// then; Fork still needs an online node, so it stays unavailable until the
/// nodes are read. After that a failed refresh keeps the last roster, whose
/// five-minute window ages agents out on its own.
///
/// People are not agents: they share one identity across devices and stay
/// available to message while offline.
enum AgentPresence {
    static func isActive(_ agent: RelayAgent, roster: [RelayAgent], fleet: [RelayAgent],
                         presenceKnown: Bool = true, at now: Date = Date()) -> Bool {
        if agent.isHuman || agent.source == .channel { return true }
        if !presenceKnown { return true }
        if fleet.contains(where: { $0.name == agent.name && $0.attachable && (agent.node.isEmpty || $0.node == agent.node) }) {
            return true
        }
        // A node's live-worker entry is the only evidence for a `.drive` copy.
        guard agent.source != .drive,
              let current = agent.rosterMatch(in: roster) ?? roster.first(where: { $0.name == agent.name && !$0.isHuman })
        else { return false }
        return current.isReachable(at: now)
    }

    /// The Agents list: active agents, and people while they are reachable.
    static func listed(_ roster: [RelayAgent], fleet: [RelayAgent], presenceKnown: Bool = true,
                       at now: Date = Date()) -> [RelayAgent] {
        roster.filter { agent in
            agent.isHuman ? agent.isReachable(at: now)
                : isActive(agent, roster: roster, fleet: fleet, presenceKnown: presenceKnown, at: now)
        }
    }

    enum ComposeTarget: Equatable {
        case open(RelayAgent)
        /// The sentence to show instead of opening a conversation.
        case refused(String)
    }

    /// What New message opens for a typed name or Relay address. People are
    /// always messageable; an agent must be active. A name the loaded roster
    /// doesn't know is no agent at all.
    static func composeTarget(_ endpoint: String, roster: [RelayAgent], conversations: [RelayAgent],
                              fleet: [RelayAgent], presenceKnown: Bool, at now: Date = Date()) -> ComposeTarget {
        let clean = endpoint.trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "^@", with: "", options: .regularExpression)
        let recipient = RelayAgent.recipient(clean, in: roster + conversations)
        if recipient.isHuman || !presenceKnown { return .open(recipient) }
        let known = roster.contains { $0.name == recipient.name || $0.address == recipient.address }
            || fleet.contains { $0.name == recipient.name }
        guard known else {
            return .refused("\(recipient.name) isn't an active agent in this workspace. Check the name, or pick one below.")
        }
        guard isActive(recipient, roster: roster, fleet: fleet, at: now) else {
            return .refused("\(recipient.name) isn't active right now, so it can't receive messages.")
        }
        return .open(recipient)
    }

    /// Active agents and reachable people whose name, display name or
    /// address contains `query`, for New message to pick from. Empty query:
    /// all of them.
    static func composeSuggestions(_ query: String, roster: [RelayAgent], fleet: [RelayAgent],
                                   presenceKnown: Bool, at now: Date = Date()) -> [RelayAgent] {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "^@", with: "", options: .regularExpression)
        return listed(roster, fleet: fleet, presenceKnown: presenceKnown, at: now).filter {
            query.isEmpty || "\($0.name) \($0.displayName) \($0.address)".localizedCaseInsensitiveContains(query)
        }
    }
}
