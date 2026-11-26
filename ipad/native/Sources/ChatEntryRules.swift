import Foundation

/// How a chat reads one session event: who wrote it and what it is, what
/// text it shows, which events only copy another event, and which agent
/// lines only repeat a tool call. Pure rules, so the timeline and the
/// working row agree on them.
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
    /// Content that only records a tool call (`isRecordedCall`) is a tool
    /// call unless a person wrote it.
    static func role(type: String, kind: String, actorRole: String, toolName: String,
                     content: String = "") -> Role {
        let names = [type.lowercased(), kind.lowercased()].filter { !$0.isEmpty }
        let actor = actorRole.lowercased()
        // What the event is.
        if names.contains("tool_result") || actor == "tool_result" { return .toolResult }
        if names.contains(where: toolCallKinds.contains) || !toolName.isEmpty || actor == "tool" { return .toolCall }
        if names.contains(where: thinkingKinds.contains) { return .thinking }
        let person = actor == "user" || actor == "human" || (actor.isEmpty && names.contains(where: personKinds.contains))
        if !person, isRecordedCall(content) { return .toolCall }
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

    /// Whether `content` is a tool call that an upload recorded as the
    /// agent's text, `<tool> {"<argument>": …}` (Codex's
    /// `exec {"input": …}`), with no tool name. The name is letters, digits,
    /// `_` or `.` and does not end with `.` (a sentence's last word does),
    /// and the brace opens a JSON object on the same line.
    static func isRecordedCall(_ content: String) -> Bool {
        guard let space = content.firstIndex(of: " ") else { return false }
        let name = content[..<space]
        guard !name.isEmpty, !name.hasSuffix("."),
              name.unicodeScalars.allSatisfy({ $0.isASCII && (CharacterSet.alphanumerics.contains($0) || $0 == "_" || $0 == ".") })
        else { return false }
        return content[content.index(after: space)...].hasPrefix("{\"")
    }

    /// The text an event shows: its content, or the tool's name when the
    /// content is blank, or nil (the event is left out) when it has neither.
    static func shownText(content: String, toolName: String) -> String? {
        if !content.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return content }
        return toolName.isEmpty ? nil : toolName
    }

    /// What a hook adds to a session when it starts. Cloud stores it as a
    /// person's event, but nobody wrote it.
    static let hookContextPrefix = "SessionStart hook additional context:"

    /// How far apart (in seconds) two records of the same turn can be.
    static let sameTurnWindow: TimeInterval = 10

    /// Who wrote a text, and what it says: what makes two records one turn.
    private struct TurnKey: Hashable {
        let isPerson: Bool
        let text: String
    }

    /// `events` in the same order without the ones that only copy another
    /// event: Cloud's session events hold more than the transcript's own
    /// lines.
    /// - An event whose `type` or `kind` is `history` (any case): the
    ///   provider's prompt log, a copy of each prompt the transcript has.
    /// - An event, whoever it names as its writer, whose content starts
    ///   (after any leading blank space) with `hookContextPrefix`: a hook's
    ///   context, not something a person wrote. The prefix is matched
    ///   exactly, in this case.
    /// - A repeat of one turn: a person's or the agent's text (never a tool
    ///   call, a result or thinking) with the same writer role and the same
    ///   text (trimmed at both ends) as an earlier event, at most
    ///   `sameTurnWindow` seconds from it. Every record of a turn, kept or
    ///   not, moves the time it is compared with, so a run of repeats that
    ///   are each within the window of the one before shows once, and the
    ///   first is the one kept. An event with no time is never a repeat. The
    ///   same text after longer, or from the other writer, is another turn.
    static func withoutCopies(_ events: [SessionEvent]) -> [SessionEvent] {
        var lastSeen: [TurnKey: Date] = [:]
        return events.filter { event in
            if event.type.lowercased() == "history" || event.kind.lowercased() == "history" { return false }
            let text = event.content.trimmingCharacters(in: .whitespacesAndNewlines)
            if text.hasPrefix(hookContextPrefix) { return false }
            let role = event.entryRole
            guard role == .person || role == .agent, !text.isEmpty else { return true }
            let key = TurnKey(isPerson: role == .person, text: text)
            let previous = lastSeen[key]
            if let time = event.ts { lastSeen[key] = time }
            guard let time = event.ts, let previous else { return true }
            return abs(time.timeIntervalSince(previous)) > sameTurnWindow
        }
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
        ChatEntryRules.role(type: type, kind: kind, actorRole: actorRole, toolName: toolName, content: content)
    }

    /// The agent thinking out loud. Drawn as activity; it still says the
    /// agent is working.
    var isThinking: Bool { entryRole == .thinking }
}
