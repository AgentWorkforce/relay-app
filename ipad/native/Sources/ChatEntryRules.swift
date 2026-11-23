import Foundation

/// How a chat reads one session event: who wrote it and what it is, what
/// text it shows, and which agent lines only repeat a tool call. Pure rules,
/// so the timeline and the working row agree on them.
enum ChatEntryRules {
    /// What an event is in the chat.
    enum Role: Equatable, Sendable {
        /// A prompt the person (or a relayed sender) gave the agent.
        case person
        /// Text the agent wrote.
        case agent
        /// The agent thinking out loud: its own activity, not a reply.
        case thinking
        /// A tool call the agent made.
        case toolCall
        /// What a tool returned.
        case toolResult
        /// Anything else: an unknown kind, a system note.
        case activity

        /// The role a session row is drawn with: thinking is activity, and
        /// a tool's call and result are both tool rows.
        var sessionRole: SessionEvent.Role {
            switch self {
            case .person: .person
            case .agent: .agent
            case .thinking, .activity: .activity
            case .toolCall, .toolResult: .tool
            }
        }
    }

    private static let toolCallKinds: Set<String> = ["tool_use", "tool_call", "tool"]
    private static let thinkingKinds: Set<String> = ["thinking", "reasoning"]
    private static let personKinds: Set<String> = ["prompt", "user", "user_message"]
    private static let agentKinds: Set<String> = ["assistant", "assistant_message", "agent_message", "response"]

    /// An event's role. Cloud may name what an event is in `type` or in
    /// `kind`, so both are read. The desktop's mapping comes first: what the
    /// event is (a tool result, a tool call, thinking, or any event naming a
    /// tool), then who wrote it (`actorRole` user or human is the person,
    /// system or developer a note, assistant the agent; with no actor, a
    /// `user` event is the person). What that leaves open (no actor, or one
    /// the desktop does not name) falls back to the kind rules: a prompt
    /// kind is the person, a kind that mentions reasoning or thinking is
    /// thinking, a reply kind is the agent, and anything else is activity.
    static func role(type: String, kind: String, actorRole: String, toolName: String) -> Role {
        let names = [type.lowercased(), kind.lowercased()].filter { !$0.isEmpty }
        let actor = actorRole.lowercased()
        // What the event is.
        if names.contains("tool_result") || actor == "tool_result" { return .toolResult }
        if names.contains(where: toolCallKinds.contains) || !toolName.isEmpty || actor == "tool" { return .toolCall }
        if names.contains(where: thinkingKinds.contains) { return .thinking }
        // Who wrote it.
        switch actor {
        case "user", "human": return .person
        case "system", "developer": return .activity
        default: break
        }
        if actor.isEmpty, names.contains(where: personKinds.contains) { return .person }
        if names.contains(where: { $0.contains("reason") || $0.contains("think") }) { return .thinking }
        if actor == "assistant" || names.contains(where: agentKinds.contains) { return .agent }
        return .activity
    }

    /// The text an event shows: its content, or the tool's name when the
    /// content is blank, or nil (the event is left out) when it has neither.
    static func shownText(content: String, toolName: String) -> String? {
        if !content.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return content }
        return toolName.isEmpty ? nil : toolName
    }

    /// `events` without the agent lines that only repeat a tool call: Cloud
    /// can record a call twice, as the call and as agent text with the same
    /// words. An agent line goes when its text is exactly a tool call's
    /// text anywhere in `events` (before it or after it); a person's line,
    /// and any line that differs at all, stays.
    static func withoutToolEchoes(_ events: [SessionEvent]) -> [SessionEvent] {
        let calls = Set(events.lazy.filter { $0.entryRole == .toolCall }
            .compactMap { shownText(content: $0.content, toolName: $0.toolName) })
        guard !calls.isEmpty else { return events }
        return events.filter { event in
            guard event.entryRole == .agent, let text = shownText(content: event.content, toolName: event.toolName) else { return true }
            return !calls.contains(text)
        }
    }
}

extension SessionEvent {
    /// The event's role by `ChatEntryRules`.
    var entryRole: ChatEntryRules.Role {
        ChatEntryRules.role(type: type, kind: kind, actorRole: actorRole, toolName: toolName)
    }

    /// The agent thinking out loud. Drawn as activity; it still says the
    /// agent is working.
    var isThinking: Bool { entryRole == .thinking }
}
