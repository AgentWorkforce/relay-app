import Foundation

extension RelayClient {
    /// The newest event time of `agent`'s session, only when its events are
    /// already loaded (the chat was opened). Nothing is requested for it.
    func loadedLastEventAt(for agent: RelayAgent) -> Date? {
        guard !agent.sessionID.isEmpty, let store = sessionEvents[agent.sessionID] else { return nil }
        return store.events.reversed().lazy.compactMap(\.ts).first
    }

    /// The agent list in its order: `pinned` agents first, then the rest,
    /// each group by activity, recency bucket and name.
    func listOrdered(_ agents: [RelayAgent], pinned: Set<String>, now: Date = Date()) -> [RelayAgent] {
        AgentListOrder.sorted(agents, pinned: { pinned.contains($0.id) }) { agent in
            AgentListOrder.entry(for: agent, name: displayName(for: agent),
                                 lastEventAt: loadedLastEventAt(for: agent), now: now)
        }
    }
}

extension AgentListOrder {
    /// What `agent` is ordered by, without building its row. An agent's
    /// activity is its row's badge (`AgentRowModel.activity(for:lastEventAt:now:)`),
    /// so the order and the badge always agree; a person, who has no badge,
    /// is ranked by the same rule without session events. Recency is the
    /// newer of the reported activity time and the loaded event, never the
    /// heartbeat (`lastSeen`).
    static func entry(for agent: RelayAgent, name: String, lastEventAt: Date?, now: Date) -> Entry {
        let activity = AgentRowModel.activity(for: agent, lastEventAt: lastEventAt, now: now)
            ?? Self.activity(turn: agent.activityTurn, reachable: agent.isReachable(at: now), lastEventAt: nil, now: now)
        return Entry(name: name, activity: activity,
                     lastActivity: [agent.activityUpdatedAt, lastEventAt].compactMap { $0 }.max(), id: agent.id)
    }
}
