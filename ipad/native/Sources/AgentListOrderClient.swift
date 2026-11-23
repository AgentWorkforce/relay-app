import Foundation

extension RelayClient {
    /// The newest event time of `agent`'s session, only when its events are
    /// already loaded (the chat was opened). Nothing is requested for it.
    func loadedLastEventAt(for agent: RelayAgent) -> Date? {
        guard !agent.sessionID.isEmpty, let store = sessionEvents[agent.sessionID] else { return nil }
        return store.events.reversed().lazy.compactMap(\.ts).first
    }

    /// The agent list in its order: `pinned` agents first, then the rest,
    /// each group by activity, recency bucket and name. An agent's activity
    /// is its row's badge (`AgentRowModel.activity`), so the order and the
    /// badge always agree; a person, who has no badge, is ranked by the
    /// same rule without session events.
    func listOrdered(_ agents: [RelayAgent], pinned: Set<String>, now: Date = Date()) -> [RelayAgent] {
        AgentListOrder.sorted(agents, pinned: { pinned.contains($0.id) }) { agent in
            let activity = rowModel(for: agent, now: now).activity
                ?? AgentListOrder.activity(turn: agent.activityTurn, reachable: agent.isReachable(at: now),
                                           lastEventAt: nil, now: now)
            return AgentListOrder.Entry(
                name: displayName(for: agent),
                activity: activity,
                lastActivity: [agent.lastSeen, loadedLastEventAt(for: agent)].compactMap { $0 }.max(),
                id: agent.id)
        }
    }
}
