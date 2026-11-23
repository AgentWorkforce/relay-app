import Foundation
import RelaySessionOverview

/// A session's title and one-line summary as the list rows load them.
struct AgentRowSummary: Equatable, Sendable {
    var title = ""
    var line = ""
}

/// What one row of the agent and conversation lists shows, built from the
/// agent's record and whatever the client has already loaded. No view and no
/// request: every field is decided here so it can be tested on its own.
struct AgentRowModel: Equatable {
    enum Status: Equatable {
        /// Reachable, with nothing more specific reported.
        case online
        /// Reachable and waiting on a person (`waiting` or `blocked`).
        case waiting
        /// Reachable and idle.
        case idle
        case offline
        /// The record reports no status.
        case unknown

        var label: String {
            switch self {
            case .online: "Online"
            case .waiting: "Waiting"
            case .idle: "Idle"
            case .offline: "Offline"
            case .unknown: "Status unknown"
            }
        }
    }

    /// What the client has loaded for the agent, beyond its own record.
    struct Loaded {
        var displayName = ""
        /// The harness key the avatar artwork uses (`RelayClient.displayCLI`).
        var cli = ""
        var model = ""
        /// The Cloud brief the agent overview loaded.
        var overview: SessionOverview?
        /// The brief the visible rows loaded.
        var rowSummary: AgentRowSummary?
        /// The session's recorded summary, when the detail has loaded one.
        var recordedSummary: SessionSummaryState?
        var goal = ""
        var unread = false
    }

    static let summaryLimit = 240

    let name: String
    /// A person's handle, shown beside their name. Nil for agents.
    let handle: String?
    let title: String?
    let summary: String?
    let cliLabel: String?
    let machine: String?
    /// Nil for a channel, which has no presence.
    let status: Status?
    let unread: Bool
    /// Text the row does not show but search has always matched.
    private let extraSearchText: String

    init(agent: RelayAgent, loaded: Loaded = Loaded(), now: Date = Date()) {
        let displayName = Self.oneLine(loaded.displayName)
        name = displayName.isEmpty ? agent.name : displayName
        handle = agent.isHuman && agent.name != name ? agent.name : nil
        unread = loaded.unread

        let isAgent = !agent.isHuman && agent.source != .channel
        // A brief, a provider title and the original request describe one
        // agent session. A person, a channel or a gateway registration has
        // none, whatever session ID its record carries.
        let hasSession = isAgent && !agent.sessionID.isEmpty
        var titles: [String] = []
        var lines: [String] = []
        if hasSession {
            let brief = loaded.overview?.hasBrief == true
                ? AgentRowSummary(title: loaded.overview?.title ?? "", line: loaded.overview?.description ?? "")
                : loaded.rowSummary ?? AgentRowSummary()
            var recordedShort = "", recordedLong = ""
            if case .ready(let text, let isShort)? = loaded.recordedSummary {
                if isShort { recordedShort = text } else { recordedLong = text }
            }
            titles = [brief.title, agent.sessionTitle, agent.objective, loaded.goal]
            lines = [recordedShort, brief.line, recordedLong, agent.sessionTitle, agent.objective, loaded.goal]
        } else if isAgent {
            titles = [agent.objective]
        } else if agent.source == .channel {
            lines = [agent.objective]
        }
        // A conversation's subtitle is its last message; a channel's is its
        // member count. A roster record's subtitle repeats the machine.
        if agent.source == .conversation || agent.source == .channel { lines.append(agent.subtitle) }

        let title = titles.lazy.map(Self.oneLine).first { !$0.isEmpty }
        self.title = title
        summary = lines.lazy.map(Self.oneLine).first { !$0.isEmpty && !Self.sameText($0, title) }

        cliLabel = isAgent ? Self.cliLabel(cli: loaded.cli, model: loaded.model) : nil
        let machine = MobileMachineFilter.displayName(agent.machine)
        self.machine = isAgent && !machine.isEmpty ? machine : nil
        status = agent.source == .channel ? nil
            : Self.status(agent.status, reachable: agent.isReachable(at: now))
        // The overview's title is matched even where the row does not show
        // it: an incomplete brief, a person or a channel.
        extraSearchText = [agent.name, agent.address, agent.machine, agent.subtitle, agent.objective,
                           agent.sessionTitle, loaded.goal, loaded.overview?.title ?? ""].joined(separator: " ")
    }

    /// The small line under the summary: the CLI and the machine.
    var caption: String? {
        let parts = [cliLabel, machine].compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    /// What VoiceOver reads for the row: name, title, CLI, machine, status
    /// and unread, in that order.
    var accessibilityLabel: String {
        var parts = [name]
        if let title { parts.append(title) }
        if let cliLabel { parts.append(cliLabel) }
        if let machine { parts.append(machine) }
        if let status { parts.append(status.label) }
        if unread { parts.append("Unread messages") }
        return parts.joined(separator: ", ")
    }

    /// Everything search matches: what the row shows, and the record's own
    /// fields it has always matched.
    var searchText: String {
        ([name, handle, title, summary, cliLabel, machine].compactMap { $0 } + [extraSearchText])
            .joined(separator: " ")
    }

    func matches(_ query: String) -> Bool {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        return query.isEmpty || searchText.localizedCaseInsensitiveContains(query)
    }

    static func status(_ raw: String, reachable: Bool) -> Status {
        let status = raw.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if status.isEmpty { return .unknown }
        guard reachable else { return .offline }
        switch status {
        case "waiting", "blocked": return .waiting
        case "idle": return .idle
        default: return .online
        }
    }

    /// The CLI's name as people write it, with the model when one is known.
    static func cliLabel(cli: String, model: String = "") -> String? {
        let key = cli.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let name: String
        switch key {
        case "": name = ""
        case "codex": name = "Codex"
        case "claude": name = "Claude"
        case "claudecode": name = "Claude Code"
        case "grok": name = "Grok"
        case "cursor": name = "Cursor"
        case "opencode": name = "OpenCode"
        case "openai": name = "OpenAI"
        case "muse": name = "Muse"
        default: name = key.prefix(1).uppercased() + key.dropFirst()
        }
        let model = oneLine(model)
        let parts = [name, sameText(model, name) ? "" : model].filter { !$0.isEmpty }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    /// `text` on one line: runs of whitespace and line breaks become one
    /// space, and a long text is cut at `summaryLimit` characters.
    static func oneLine(_ text: String) -> String {
        let line = text.split(whereSeparator: \.isWhitespace).joined(separator: " ")
        return line.count > summaryLimit ? String(line.prefix(summaryLimit - 1)) + "…" : line
    }

    private static func sameText(_ text: String, _ other: String?) -> Bool {
        guard let other else { return false }
        return text.compare(other, options: [.caseInsensitive, .diacriticInsensitive]) == .orderedSame
    }
}

/// Loads the briefs the visible rows show, a few at a time.
///
/// A row registers its session when it appears and withdraws it when it
/// scrolls away, so a session whose row left before its turn is never
/// requested. At most `maxConcurrent` loads run at once. A session is asked
/// for again only once its last attempt, successful or not, is
/// `refreshInterval` old. A failed load keeps what was loaded before.
/// Nothing is requested, and no late result is kept, while `canLoad` is
/// false.
@MainActor final class AgentRowSummaryLoader {
    typealias Fetch = @MainActor (_ sessionID: String) async -> AgentRowSummary?

    static let defaultMaxConcurrent = 2
    static let defaultRefreshInterval: TimeInterval = 5 * 60

    private struct Entry {
        var summary: AgentRowSummary?
        var fetchedAt: Date
    }

    let maxConcurrent: Int
    let refreshInterval: TimeInterval
    /// Called with each newly loaded or changed summary.
    var onUpdate: ((_ sessionID: String, _ summary: AgentRowSummary) -> Void)?

    private let fetch: Fetch
    private let canLoad: @MainActor () -> Bool
    private let now: () -> Date
    /// The session each visible row shows, by row.
    private var visibleRows: [String: String] = [:]
    /// Visible sessions in the order their rows appeared.
    private var queue: [String] = []
    private var entries: [String: Entry] = [:]
    private var inFlight: Set<String> = []
    /// Loads that have not returned, including ones `reset` abandoned.
    private(set) var running = 0
    private var generation = 0

    init(maxConcurrent: Int = AgentRowSummaryLoader.defaultMaxConcurrent,
         refreshInterval: TimeInterval = AgentRowSummaryLoader.defaultRefreshInterval,
         now: @escaping () -> Date = Date.init,
         canLoad: @escaping @MainActor () -> Bool,
         fetch: @escaping Fetch) {
        self.maxConcurrent = max(1, maxConcurrent)
        self.refreshInterval = refreshInterval
        self.now = now
        self.canLoad = canLoad
        self.fetch = fetch
    }

    func summary(for sessionID: String) -> AgentRowSummary? { entries[sessionID]?.summary }

    func rowAppeared(_ row: String, sessionID: String) {
        let id = sessionID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !id.isEmpty else {
            visibleRows[row] = nil
            return
        }
        visibleRows[row] = id
        if !queue.contains(id) { queue.append(id) }
        pump()
    }

    func rowDisappeared(_ row: String) {
        visibleRows[row] = nil
    }

    /// Loads what the visible rows still need, and again what has aged out.
    func refresh() { pump() }

    /// Forgets every loaded summary and abandons the loads in flight, at
    /// sign-out and when the workspace changes. Rows still on screen stay
    /// registered and load again once loading is allowed.
    func reset() {
        generation += 1
        entries = [:]
        inFlight = []
    }

    private func needsLoad(_ id: String) -> Bool {
        guard let entry = entries[id] else { return true }
        return now().timeIntervalSince(entry.fetchedAt) >= refreshInterval
    }

    private func pump() {
        let visible = Set(visibleRows.values)
        queue.removeAll { !visible.contains($0) }
        guard canLoad() else { return }
        for id in queue {
            guard running < maxConcurrent else { return }
            if !inFlight.contains(id), needsLoad(id) { start(id) }
        }
    }

    private func start(_ id: String) {
        running += 1
        inFlight.insert(id)
        let started = generation
        Task { @MainActor in
            let loaded = await self.fetch(id)
            self.running -= 1
            // A load `reset` abandoned stores nothing, and neither does one
            // that returns after loading stopped being allowed.
            if started == self.generation {
                self.inFlight.remove(id)
                if self.canLoad() {
                    let previous = self.entries[id]?.summary
                    self.entries[id] = Entry(summary: loaded ?? previous, fetchedAt: self.now())
                    if let loaded, loaded != previous { self.onUpdate?(id, loaded) }
                }
            }
            self.pump()
        }
    }
}
