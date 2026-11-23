import Foundation
import SwiftUI
import Security
import UniformTypeIdentifiers
import RelaySessionOverview
import UserNotifications

typealias JSONObject = [String: Any]

private func object(_ value: Any?) -> JSONObject { value as? JSONObject ?? [:] }
private func string(_ value: Any?) -> String {
    if let value = value as? String { return value }
    if let value = value as? NSNumber { return value.stringValue }
    return ""
}
private func objects(_ value: Any?) -> [JSONObject] { value as? [JSONObject] ?? [] }
private func machineIn(_ address: String) -> String {
    guard let at = address.firstIndex(of: "@") else { return "" }
    let host = String(address[address.index(after: at)...])
    guard let dash = host.lastIndex(of: "-"), host[host.index(after: dash)...].allSatisfy(\.isNumber) else { return host }
    return String(host[..<dash])
}
private func cliInName(_ value: String) -> String {
    let words = value.lowercased().split(whereSeparator: { !$0.isLetter && !$0.isNumber }).map(String.init)
    for (index, word) in words.enumerated() {
        switch word {
        case "claudecode": return "claudecode"
        case "claude": return words.indices.contains(index + 1) && ["code", "terminal"].contains(words[index + 1]) ? "claudecode" : "claude"
        case "opencode": return "opencode"
        case "open": if words.indices.contains(index + 1) && words[index + 1] == "code" { return "opencode" }
        case "codex", "grok", "cursor", "openai", "muse": return word
        // Grok Bot inboxes are often named for their job, like `devopsgrok`.
        default: if word.count > 4 && word.hasSuffix("grok") { return "grok" }
        }
    }
    return ""
}
/// The harness a record states in a structured field, never guessed from a name.
private func cliStated(in hints: [String]) -> String {
    hints.lazy.map(cliInName).first { !$0.isEmpty } ?? ""
}
/// The harness a record states for a session on a computer. A hosted bot's kind
/// (`grok-bot`) names a product, not a session a computer can fork, so it states none.
private func harnessStated(in hints: [String]) -> String {
    let namesBot = hints.contains { hint in
        hint.lowercased().split(whereSeparator: { !$0.isLetter && !$0.isNumber }).contains { $0.hasSuffix("bot") }
    }
    return namesBot ? "" : cliStated(in: hints)
}
private func agentCLI(structured: [String], name: String, persona: String = "") -> String {
    let stated = cliStated(in: structured)
    if !stated.isEmpty { return stated }
    let named = cliInName(name)
    if !named.isEmpty { return named }
    let description = persona.lowercased()
    for (phrase, cli) in [("claude code", "claudecode"), ("open code", "opencode"),
                          ("codex", "codex"), ("claude", "claude"),
                          ("grok", "grok"), ("cursor", "cursor"),
                          ("openai", "openai"), ("muse", "muse")] {
        if description.contains("a \(phrase) agent") || description.contains("a \(phrase) worker") {
            return cli
        }
    }
    return ""
}

struct RelayAgent: Identifiable, Hashable, Sendable {
    enum Source: String, Sendable { case relay, drive, conversation, channel }
    let id: String
    let name: String
    let address: String
    let source: Source
    let node: String
    let machine: String
    let sessionID: String
    let objective: String
    /// The provider's title, independent from a user-authored objective.
    var sessionTitle = ""
    var mobileHandoff: MobileHandoffNotice?
    var mobileHandoffEnabled = false
    var mobileNativeContinuationEnabled = false
    var mobileNativeCommandFileApprovals = false
    let cli: String
    /// `cli` only when a structured field of the record states it, not when
    /// it is inferred from the name or persona. A fork must not guess.
    var statedCLI = ""
    /// The model a fleet record states (`cliModel`); empty when it states none.
    var model = ""
    let subtitle: String
    let status: String
    let attachable: Bool
    /// A person (Relaycast `type: "human"`) rather than an agent.
    var isHuman = false
    /// A person's display name; empty for agents.
    var displayName = ""
    var lastSeen: Date?
    var isDesktopRegistration = false
    /// Renamed registrations only forward messages to the current name.
    var isListed = true
    /// A channel this person belongs to, so its messages reach them.
    var isJoined = false
    var memberCount = 0

    /// Matches the relay core's five-minute presence window.
    func isReachable(at now: Date = Date()) -> Bool {
        if let lastSeen, now.timeIntervalSince(lastSeen) > 5 * 60 { return false }
        switch status {
        case "active", "online", "waiting", "blocked": return true
        case "idle": return !isDesktopRegistration
        default: return false
        }
    }

    static func recipient(_ endpoint: String, in roster: [RelayAgent]) -> RelayAgent {
        let clean = endpoint.trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "^@", with: "", options: .regularExpression)
        if let known = roster.first(where: { clean.contains("@") ? $0.address == clean : $0.name == clean }) {
            return known
        }
        let name = clean.split(separator: "@", maxSplits: 1).first.map(String.init) ?? clean
        return RelayAgent(relay: ["name": name, "address": clean, "status": "active"])
    }

    func rosterMatch(in roster: [RelayAgent]) -> RelayAgent? {
        if let exact = roster.first(where: { $0.id == id }) { return exact }
        guard source == .relay else { return nil }
        if !address.contains("@"), let named = roster.first(where: { $0.name == name }) { return named }
        // A person's conversation is independent from online registrations.
        return isHuman ? self : nil
    }

    init(relay data: JSONObject) {
        name = string(data["name"])
        address = string(data["address"]).isEmpty ? name : string(data["address"])
        id = "relay:\(address)"
        source = .relay
        node = ""
        let metadata = object(data["metadata"])
        mobileHandoff = MobileHandoffNotice(metadata: metadata)
        mobileHandoffEnabled = metadata["mobile_handoff_enabled"] as? Bool ?? false
        mobileNativeContinuationEnabled = metadata["mobile_native_continuation_enabled"] as? Bool ?? false
        mobileNativeCommandFileApprovals = metadata["mobile_native_continuation_approvals"] as? String == "command_file"
        isDesktopRegistration = string(metadata["source"]) == "relay-desktop"
        isListed = string(metadata["renamed_to"]).isEmpty
        let seen = string(data["last_seen"])
        lastSeen = (try? Date.ISO8601FormatStyle(includingFractionalSeconds: true).parse(seen))
            ?? (try? Date.ISO8601FormatStyle().parse(seen))
        let hints = [string(metadata["session_kind"]), string(metadata["cli"]), string(metadata["tool"]),
                     string(metadata["harness"]), string(metadata["provider"])]
        cli = agentCLI(structured: hints, name: name, persona: string(data["persona"]))
        statedCLI = harnessStated(in: hints)
        // Gateway registrations name an agent (for example OpenClaw's `main`),
        // not any one of its conversations. Never request a brief for that ID.
        let gateway = string(metadata["session_scope"]) == "agent" ||
            (isDesktopRegistration && ["openclaw", "hermes"].contains(string(metadata["session_kind"])))
        sessionID = gateway ? "" : string(metadata["session_id"])
        sessionTitle = string(metadata["session_title"]).trimmingCharacters(in: .whitespacesAndNewlines)
        let publishedObjective = string(metadata["objective"])
        objective = (publishedObjective.isEmpty ? string(data["description"]) : publishedObjective)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let host = string(metadata["host"])
        isHuman = PersonIdentity.isHuman(data)
        // A person is one identity across devices: no machine, and the
        // persona is their name, not an agent role.
        machine = isHuman ? "" : (host.isEmpty ? machineIn(address) : host)
        displayName = isHuman ? string(data["persona"]).trimmingCharacters(in: .whitespacesAndNewlines) : ""
        subtitle = isHuman ? name : [machine, string(data["persona"])].filter { !$0.isEmpty }.joined(separator: " · ")
        status = string(data["status"])
        attachable = false
    }
    init(fleet data: JSONObject) {
        name = string(data["name"])
        node = string(data["node"])
        address = name
        id = "drive:\(node)/\(name)"
        source = .drive
        machine = node
        sessionID = string(data["session_id"])
        objective = (string(data["objective"]).isEmpty ? string(data["description"]) : string(data["objective"]))
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let hints = [string(data["cli"]), string(data["harness"]), string(data["providerLabel"]), string(data["cliModel"])]
        cli = agentCLI(structured: hints, name: name)
        statedCLI = harnessStated(in: hints)
        model = string(data["cliModel"]).trimmingCharacters(in: .whitespacesAndNewlines)
        subtitle = [node, string(data["cliModel"]).isEmpty ? string(data["providerLabel"]) : string(data["cliModel"])].filter { !$0.isEmpty }.joined(separator: " · ")
        let state = string(data["workerLiveness"])
        status = state.isEmpty ? string(data["state"]) : state
        attachable = status != "offline" && !name.isEmpty && !node.isEmpty
    }
    init(conversation data: JSONObject) {
        let peers = objects(data["participants"])
        let peer = peers.first { string($0["agent_name"]) != UserDefaults.standard.string(forKey: "relay.mobileName") }
        name = string(peer?["agent_name"])
        address = name
        id = "conversation:\(address)"
        source = .conversation
        node = ""
        isHuman = data["is_human"] as? Bool ?? false
        displayName = string(data["display_name"])
        machine = isHuman ? "" : (string(data["machine"]).isEmpty ? machineIn(address) : string(data["machine"]))
        sessionID = string(data["session_id"])
        objective = string(data["objective"]).trimmingCharacters(in: .whitespacesAndNewlines)
        sessionTitle = string(data["session_title"]).trimmingCharacters(in: .whitespacesAndNewlines)
        mobileHandoff = MobileHandoffNotice(metadata: object(data["handoff_metadata"]))
        mobileHandoffEnabled = data["mobile_handoff_enabled"] as? Bool ?? false
        mobileNativeContinuationEnabled = data["mobile_native_continuation_enabled"] as? Bool ?? false
        mobileNativeCommandFileApprovals = data["mobile_native_continuation_approvals"] as? String == "command_file"
        isDesktopRegistration = data["desktop_registration"] as? Bool ?? false
        cli = isHuman ? "" : agentCLI(structured: [string(peer?["cli"]), string(data["cli"])], name: name)
        // `data["cli"]` is copied from a roster entry and may be a guess.
        statedCLI = isHuman ? "" : harnessStated(in: [string(peer?["cli"])])
        subtitle = string(object(data["last_message"])["text"])
        status = string(data["status"])
        attachable = false
    }
    /// A workspace channel: a group chat shared by people and agents.
    init(channel data: JSONObject, joined: Bool) {
        name = string(data["name"])
        address = "#\(name)"
        id = "channel:\(name)"
        source = .channel
        node = ""
        machine = ""
        sessionID = ""
        objective = string(data["topic"]).trimmingCharacters(in: .whitespacesAndNewlines)
        cli = ""
        let count = (data["member_count"] as? NSNumber)?.intValue ?? 0
        let members = count == 1 ? "1 member" : "\(count.formatted()) members"
        subtitle = joined ? members : "Not joined · \(members)"
        status = ""
        attachable = false
        memberCount = count
        isJoined = joined
    }
}

struct RelayMessage: Identifiable {
    let id: String
    let text: String
    let sender: String
    let mine: Bool
    let createdAt: Date?
    /// False when Relaycast sent no id and the row index stands in for one.
    let hasServerID: Bool
    let attachments: [RelayAttachment]
    init(_ data: JSONObject, index: Int) {
        hasServerID = !string(data["id"]).isEmpty
        id = hasServerID ? string(data["id"]) : String(index)
        text = string(data["text"])
        sender = string(data["agent_name"])
        mine = sender == UserDefaults.standard.string(forKey: "relay.mobileName")
        let timestamp = string(data["created_at"])
        createdAt = (try? Date.ISO8601FormatStyle(includingFractionalSeconds: true).parse(timestamp))
            ?? (try? Date.ISO8601FormatStyle().parse(timestamp))
        attachments = objects(data["attachments"]).map { RelayAttachment($0) }
    }
}

struct RelayAttachment: Identifiable, Hashable {
    let id: String
    let filename: String
    let contentType: String
    let sizeBytes: Int

    init(fileID: String, filename: String, contentType: String, sizeBytes: Int) {
        id = fileID
        self.filename = filename
        self.contentType = contentType
        self.sizeBytes = sizeBytes
    }

    init(_ data: JSONObject) {
        id = string(data["file_id"])
        filename = string(data["filename"])
        contentType = string(data["content_type"])
        sizeBytes = (data["size_bytes"] as? NSNumber)?.intValue ?? 0
    }
}

struct RelaySpawnNode: Identifiable, Hashable {
    let name: String
    let harnesses: [String]
    /// Harnesses whose spawns here can start as a copy of a session (`SessionFork`).
    var forkHarnesses: [String] = []
    /// The computer's name as its agents' `host` metadata gives it.
    var machine = ""
    /// Newest-first working folders of agents started here, when the node
    /// publishes them (`SpawnFolderSuggestions`).
    var recentFolders: [String] = []
    var id: String { name }
}

struct VoiceLiveGrant {
    let sessionID: String
    /// The WebRTC SDP answer.
    let answer: String
    let remainingSeconds: Int?
}

struct RelayAPIError: LocalizedError {
    let message: String
    var status: Int? = nil
    var code: String = ""
    /// The server's `error.reason`: what tells apart answers that share a
    /// code (every session events 501 has `unsupported_by_backend`).
    var reason: String = ""
    var retryAfterSeconds: Int? = nil
    /// A paid GPT-Live session the voice broker created but couldn't confirm
    /// closed; the call reports usage for it until the close is confirmed.
    var voiceSessionID: String? = nil
    /// `message` holds text from a server's answer, not copy the app wrote.
    /// It is kept for classifying the failure and never shown; see UserFacingError.
    var fromServer = false
    /// What to show instead of a server's `message`, when the app worded the
    /// failure from it (SpawnFailure).
    var summary: String? = nil
    var errorDescription: String? { UserFacingError.apiMessage(self) }
}

/// The identifiers in an error body. Cloud routes report `{error, code}`;
/// Relaycast nests the code in `error`; the reason is `error.reason`.
enum RelayErrorBody {
    static func identifiers(in json: JSONObject) -> (code: String, reason: String) {
        let problem = object(json["error"])
        let code = string(problem["code"]).isEmpty ? string(json["code"]) : string(problem["code"])
        return (code, string(problem["reason"]))
    }
}

private enum SecureStore {
    static func read(_ key: String) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: "com.agentworkforce.relayipad",
            kSecAttrAccount as String: key,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne
        ]
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        #if DEBUG && targetEnvironment(simulator)
        if status == errSecMissingEntitlement { return UserDefaults.standard.string(forKey: "relay.simulator.\(key)") }
        #endif
        if status != errSecSuccess && status != errSecItemNotFound {
            NSLog("Agent Relay Keychain read failed: %d", status)
        }
        guard status == errSecSuccess,
              let data = item as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }
    static func write(_ value: String, for key: String) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: "com.agentworkforce.relayipad",
            kSecAttrAccount as String: key
        ]
        SecItemDelete(query as CFDictionary)
        var item = query
        item[kSecValueData as String] = Data(value.utf8)
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let status = SecItemAdd(item as CFDictionary, nil)
        #if DEBUG && targetEnvironment(simulator)
        if status == errSecMissingEntitlement {
            UserDefaults.standard.set(value, forKey: "relay.simulator.\(key)")
            return
        }
        #endif
        if status != errSecSuccess { NSLog("Agent Relay Keychain write failed: %d", status) }
    }
    static func remove(_ key: String) {
        #if DEBUG && targetEnvironment(simulator)
        UserDefaults.standard.removeObject(forKey: "relay.simulator.\(key)")
        #endif
        SecItemDelete([
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: "com.agentworkforce.relayipad",
            kSecAttrAccount as String: key
        ] as CFDictionary)
    }
}

@MainActor final class RelayClient: ObservableObject {
    @Published var paired = RelayClient.startsPaired
    @Published var connected = false
    @Published var busy = false
    @Published var error: String?
    /// A small inline line for the open conversation while its latest
    /// automatic read failed; never an alert. Cleared by the next good read.
    @Published var refreshNotice: String?
    @Published var workspace = UserDefaults.standard.string(forKey: "relay.workspaceName") ?? "Workspace"
    @Published var account = UserDefaults.standard.string(forKey: "relay.accountName") ?? "Agent Relay"
    /// The bare handle agents use to reach this person (`to=<handle>`).
    var messagingHandle: String { UserDefaults.standard.string(forKey: "relay.mobileName") ?? "" }
    @Published var verificationURL: URL?
    /// Counts sign-ins that stopped with an error (polling for the device
    /// grant included), so the approval page can close to show the failure.
    @Published private(set) var signInFailures = 0
    @Published var userCode = ""
    @Published var signInStatus = ""
    @Published var relayAgents: [RelayAgent] = []
    /// False until the first roster request of this session has answered,
    /// so the lists say they are loading instead of being empty.
    @Published var rosterLoaded = false
    @Published var fleetAgents: [RelayAgent] = []
    /// Agents the online nodes report having spawned, by name, with the harness
    /// they run: every reporting node, in node order.
    @Published var spawnedSessions: [String: [SessionFork.Spawned]] = [:]
    @Published var spawnNodes: [RelaySpawnNode] = []
    /// Every online node that accepts forks, including a machine's second installation.
    @Published var forkNodes: [RelaySpawnNode] = []
    @Published var conversations: [RelayAgent] = []
    @Published var channels: [RelayAgent] = []
    @Published var unreadConversationIDs: Set<String> = []
    @Published var hiddenConversationIDs: Set<String> = []
    @Published var messages: [RelayMessage] = []
    @Published var summaryCredentialReady = SecureStore.read("cloudRefreshToken") != nil
    @Published var cloudWorkspaceID = UserDefaults.standard.string(forKey: "relay.workspaceID") {
        didSet {
            if cloudWorkspaceID != oldValue {
                resetPeople()
                resetRowSummaries()
                resetSessionEvents()
            }
        }
    }
    @Published private var workspacePeople = WorkspacePeopleDirectory()
    @Published var summaries: [String: SessionOverview] = [:]
    @Published var goals: [String: String] = [:]
    /// The briefs the list rows loaded for their sessions, by session ID.
    /// Kept apart from `summaries`, which the agent overview owns.
    @Published private(set) var rowSummaries: [String: AgentRowSummary] = [:]
    private lazy var rowSummaryLoader: AgentRowSummaryLoader = {
        let loader = AgentRowSummaryLoader(canLoad: { [weak self] in self?.canLoadRowSummaries ?? false },
                                           fetch: { [weak self] id in await self?.fetchRowSummary(id) })
        loader.onUpdate = { [weak self] id, summary in self?.rowSummaries[id] = summary }
        return loader
    }()
    /// Each session's stored events read so far, by session id, for the app
    /// run and in the disk cache; both cleared at sign-out and workspace switch.
    @Published private(set) var sessionEvents: [String: SessionEventStore] = [:]
    private struct SessionEventRead {
        let token: UUID
        let task: Task<Void, Never>
    }
    private var sessionEventReads: [String: SessionEventRead] = [:]
    /// When each session's newest page was last asked for, so a burst of
    /// chat messages reads at most once per `SessionEventPolling.minimumSpacing`.
    private var sessionEventLastRead: [String: Date] = [:]
    private var sessionEventsGeneration = 0
    /// Where session events persist between launches: the on-disk cache,
    /// or nothing in UI tests.
    private let sessionEventSnapshots: SessionEventSnapshotStore
    /// Reads one page of a session's events instead of Cloud; unit tests
    /// set it. With it set, every agent with a session id can load events.
    typealias SessionEventFetch = @MainActor (_ sessionID: String, _ spec: SessionEventsRequest.Spec) async throws -> SessionEventPage
    private let sessionEventFetch: SessionEventFetch?

    init(sessionEventSnapshots: SessionEventSnapshotStore? = nil, sessionEventFetch: SessionEventFetch? = nil) {
        self.sessionEventFetch = sessionEventFetch
        #if DEBUG
        let fixture = RelayUITestFixture.enabled
        #else
        let fixture = false
        #endif
        if let sessionEventSnapshots {
            self.sessionEventSnapshots = sessionEventSnapshots
        } else if !fixture, let disk = SessionEventDiskCache.standard() {
            self.sessionEventSnapshots = disk
        } else {
            self.sessionEventSnapshots = MemoryOnlySessionEventSnapshots()
        }
    }
    /// The conversation whose relay history read has finished, found no
    /// thread or failed, so the chat can tell loading from empty.
    @Published private(set) var historyCheckedConversation: String?
    /// Hosted session summaries (`GET /sessions/:id/summary`), by session ID.
    @Published var sessionSummaries: [String: SessionSummaryCacheEntry] = [:]
    /// The summary load running for each session; later callers await it.
    private let sessionSummaryLoads = SessionSummaryLoads()
    /// The content revision the summary list last reported for each session.
    @Published private(set) var sessionRevisions: [String: String] = [:]
    /// The canonical stored source the summary list reported for each
    /// session, for `GET /sessions/:id/summary`. A session the list never
    /// returned has none, and its summary section stays hidden.
    @Published private(set) var sessionSources: [String: String] = [:]
    /// Session IDs Cloud History has no copy of (a 404 for their analysis).
    @Published var sessionsMissingFromHistory: Set<String> = []
    /// Outgoing messages Relaycast reports as read by someone other than the
    /// sender, which happens when the broker puts them into the agent's session.
    @Published var deliveredMessageIDs: Set<String> = []
    @Published var attachedID: String?
    @Published var attachedMode: String?
    @Published var terminalEnded = false
    @Published var screenHistory: [String] = []
    var terminalBacklog = ""
    var onTerminalOutput: ((String) -> Void)?
    var onTerminalScroll: ((Int) -> Void)?
    private var terminalTask: Task<Void, Never>?
    private var terminalSocket: URLSessionWebSocketTask?
    private var terminalSessionID: String?
    private var conversationIDs: [String: String] = [:]
    /// When a voice call last found no DM thread with an agent (see directHistory).
    private var threadMisses = VoiceThreadMisses()
    private var joinedChannels: Set<String> = []
    /// Channel senders no longer on the roster, looked up once so a person
    /// who left still shows as a person.
    @Published private var channelSenders: [String: RelayAgent] = [:]
    private var channelSenderRetryAfter: [String: Date] = [:]
    /// Joins this device made, kept until Relaycast lists them (or two
    /// minutes pass) so a lagging membership read can't undo a join.
    private var recentJoins: [String: Date] = [:]
    private var latestMessageIDs: [String: String] = [:]
    private var localMessageNotices = LocalMessageNotices()
    private var latestIncomingMessageIDs: [String: String] = [:]
    /// The server's read state (`/inbox`), for the session `readStateSession`.
    private var readState = ConversationReadState()
    private var readStateSession = 0
    private var carryingOverSeen = false
    var isAppActive: @MainActor () -> Bool = { UIApplication.shared.applicationState == .active }
    var notificationCenter = RelayNotificationCenter.system
    private var conversationsInitialized = false
    private var registeredPushToken: String?
    private var selectedConversation: String?
    /// The conversation whose history `messages` holds from a successful
    /// read; nil while it has not loaded since the conversation was opened.
    private var loadedHistoryConversation: String?
    private var loadingSummaryIDs: Set<String> = []
    private var loadingGoalIDs: Set<String> = []
    private var cloudRefreshTask: Task<String, Error>?
    private var cloudRefreshTaskID: UUID?
    private let workspaceKeyRecovery = WorkspaceKeyRecovery()
    private var peopleRefreshTask: Task<Void, Never>?
    private var conversationAgentMetadata: [String: RelayAgent] = [:]
    /// Failed handoff notices the person dismissed; in memory only.
    @Published private var handoffDismissals = MobileHandoffDismissals()
    private var conversationRoster = ConversationRoster()
    private var conversationAgentRetryAfter: [String: Date] = [:]
    private var latestConversationThreads: [JSONObject] = []
    private var conversationMetadataWorkerID: UUID?
    private var conversationRefreshGeneration = 0
    private var refreshGeneration = 0
    /// Which failed automatic reads are reported (never in the alert): not one
    /// from before the app was last suspended, nor a cancelled or superseded one.
    private var connectionReports = ConnectionErrorReports()
    /// Reads the server asked the app to hold back (429, Retry-After, DM reads shed).
    private var readBackoff = ReadBackoff()
    /// A sign-out is under way: the session is already invalid while its network calls finish.
    private(set) var signingOut = false
    /// Changes whenever the signed-in identity does, so file work started for
    /// one account or workspace is dropped instead of landing in the next.
    @Published private(set) var sessionGeneration = 0
    private var lastScreenCapture = Date.distantPast
    private let cloud = "https://agentrelay.com/cloud/api/v1"
    private let defaultRelaycast = "https://cast.agentrelay.com"

    private static var startsPaired: Bool {
        #if DEBUG
        RelayUITestFixture.prepare()
        if RelayUITestFixture.enabled { return true }
        #endif
        return SecureStore.read("workspaceKey") != nil
    }
    private var relaycast: String { SecureStore.read("relaycastURL") ?? defaultRelaycast }
    private var workspaceKey: String? { SecureStore.read("workspaceKey") }
    private var agentToken: String? { SecureStore.read("agentToken") }
    private var unreadStoragePrefix: String {
        let workspace = cloudWorkspaceID ?? "unknown"
        let mobileName = UserDefaults.standard.string(forKey: "relay.mobileName") ?? "unknown"
        return "relay.mobileUnread.\(workspace).\(mobileName)"
    }
    private var unreadSeenKey: String { unreadStoragePrefix + ".seen" }
    private var unreadBaselineKey: String { unreadStoragePrefix + ".baseline" }
    private var hiddenConversationsKey: String { unreadStoragePrefix + ".hidden" }

    func captureScreen(_ screen: String) {
        let text = screen.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, text != screenHistory.last else { return }
        let now = Date()
        guard now.timeIntervalSince(lastScreenCapture) >= 2 else { return }
        lastScreenCapture = now
        screenHistory.append(String(text.prefix(20_000)))
        if screenHistory.count > 120 { screenHistory.removeFirst(screenHistory.count - 120) }
    }

    private func call(_ url: URL, token: String? = nil, method: String = "GET", body: JSONObject? = nil) async throws -> JSONObject {
        try await call(RelayRequestSpec(url: url, token: token, method: method, body: body))
    }
    private func call(_ spec: RelayRequestSpec) async throws -> JSONObject {
        let maximumAttempts = RelayRequestRetry.automaticAttempts(method: spec.method)
        for attempt in 0..<maximumAttempts {
            let request = try spec.urlRequest()
            let (data, response) = try await URLSession.shared.data(for: request)
            guard let http = response as? HTTPURLResponse else { throw RelayAPIError(message: "No response from Agent Relay") }
            let json = (try? JSONSerialization.jsonObject(with: data)) as? JSONObject ?? [:]
            guard !(200..<300).contains(http.statusCode) else { return json }
            let problem = object(json["error"])
            let (code, reason) = RelayErrorBody.identifiers(in: json)
            let headerDelay = http.value(forHTTPHeaderField: "Retry-After")
                .flatMap(Int.init).map { max($0, 0) }
            let retryDelay = headerDelay ?? min(2 << attempt, 10)
            if http.statusCode == 503, code == "database_overloaded",
               headerDelay.map({ $0 <= 10 }) ?? true,
               attempt + 1 < maximumAttempts {
                try await Task.sleep(for: .seconds(retryDelay))
                continue
            }
            let serverDetail = string(problem["message"]).isEmpty ? string(json["error"]) : string(problem["message"])
            let detail = code == "database_overloaded"
                ? "Agent Relay is busy. Try again in a moment."
                : (serverDetail.isEmpty ? "Agent Relay returned HTTP \(http.statusCode)" : serverDetail)
            let voiceSession = string(json["sessionId"])
            throw RelayAPIError(message: detail, status: http.statusCode, code: code, reason: reason,
                                retryAfterSeconds: headerDelay,
                                voiceSessionID: voiceSession.isEmpty ? nil : voiceSession,
                                fromServer: code != "database_overloaded")
        }
        throw RelayAPIError(message: "Agent Relay is busy. Try again in a moment.", status: 503,
                            code: "database_overloaded")
    }
    private func api(_ path: String, agent: Bool = false, method: String = "GET", body: JSONObject? = nil,
                     idempotencyKey: String? = nil) async throws -> Any {
        #if DEBUG
        if let fixture = RelayUITestFixture.response(path, method: method, body: body) { return fixture }
        #endif
        guard let token = agent ? agentToken : workspaceKey else {
            throw RelayAPIError(message: "Sign in to Agent Relay")
        }
        let base = relaycast.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        guard let url = URL(string: base + "/v1" + path) else {
            throw RelayAPIError(message: "Invalid Relaycast URL")
        }
        let request = RelayRequestSpec(url: url, token: token, method: method, body: body, idempotencyKey: idempotencyKey)
        let readScope = ReadBackoff.scope(path: path, method: method)
        if let readScope, let wait = readBackoff.remaining(readScope, token: token) {
            // The server asked for a pause; reading before it ends lengthens it.
            throw RelayAPIError(message: UserFacingError.busy, code: "read_paused", retryAfterSeconds: wait)
        }
        let result: JSONObject
        do {
            result = try await call(request)
        } catch let failure as RelayAPIError where agent && (failure.status == 401 || failure.status == 403) {
            guard let workspaceID = cloudWorkspaceID else { throw failure }
            let person = try await cloudPersonCredential(workspaceID: workspaceID)
            guard person.baseURL == relaycast.trimmingCharacters(in: CharacterSet(charactersIn: "/")) else {
                throw RelayAPIError(message: "Cloud returned a different Relaycast server for this workspace")
            }
            SecureStore.write(person.token, for: "agentToken")
            guard agentToken == person.token else {
                throw RelayAPIError(message: "Could not save your messaging credential")
            }
            UserDefaults.standard.set(person.name, forKey: "relay.mobileName")
            registeredPushToken = nil
            result = try await resend(request, with: person.token, pausing: readScope)
        } catch let failure as RelayAPIError where !agent && failure.status == 401 {
            // Relaycast answers 401 for a workspace key it no longer knows: a
            // rotation whose grace has lapsed. Ask Cloud for the current key
            // and retry this request once with it.
            let replacement: String
            do {
                replacement = try await recoverWorkspaceKey(refused: token, base: base)
            } catch WorkspaceKeyRecoveryError.superseded {
                throw failure
            }
            result = try await resend(request, with: replacement, pausing: readScope)
        } catch {
            if let readScope { readBackoff.failed(readScope, token: token, error: error) }
            throw error
        }
        if let readScope { readBackoff.succeeded(readScope) }
        return result["data"] ?? result
    }
    /// Sends `request` again with a recovered `token`. A wait the server asks
    /// for pauses `readScope` for that token, the one the resend used.
    private func resend(_ request: RelayRequestSpec, with token: String, pausing readScope: String?) async throws -> JSONObject {
        do {
            return try await call(request.resent(with: token))
        } catch {
            if let readScope { readBackoff.failed(readScope, token: token, error: error) }
            throw error
        }
    }
    /// The current key for a workspace key Relaycast refused, resolved from
    /// Cloud with this device's session, checked like a sign-in's and saved.
    /// One recovery runs at a time, at most once for each refused key. Only
    /// a key for `base`, the Relaycast server the refused request went to,
    /// is ever returned.
    private func recoverWorkspaceKey(refused: String, base: String) async throws -> String {
        guard let workspaceID = cloudWorkspaceID, let uuid = UUID(uuidString: workspaceID) else {
            throw WorkspaceKeyRecoveryError.signInAgain
        }
        let generation = sessionGeneration
        let isCurrent = { [weak self] () -> Bool in
            guard let self else { return false }
            return !self.signingOut && self.sessionGeneration == generation && self.cloudWorkspaceID == workspaceID &&
                self.relaycast.trimmingCharacters(in: CharacterSet(charactersIn: "/")) == base
        }
        guard isCurrent() else { throw WorkspaceKeyRecoveryError.superseded }
        // A concurrent request already replaced the refused key.
        if let current = workspaceKey, current != refused { return current }
        return try await workspaceKeyRecovery.replacement(
            for: refused, currentBaseURL: base, defaultBaseURL: defaultRelaycast,
            resolve: { [weak self] in
                guard let self, isCurrent() else { throw WorkspaceKeyRecoveryError.superseded }
                guard SecureStore.read("cloudRefreshToken") != nil else {
                    throw WorkspaceKeyRecovery.CloudSessionRejected()
                }
                do {
                    return try await self.authorizedCloudCall("/workspaces/\(uuid.uuidString.lowercased())/resolve")
                } catch let failure as RelayAPIError where failure.status == 401 || failure.status == 403 ||
                            SecureStore.read("cloudRefreshToken") == nil {
                    // Cloud refused the session, or renewing it did and the
                    // saved session was cleared.
                    throw WorkspaceKeyRecovery.CloudSessionRejected()
                }
            },
            save: { [weak self] refused, replacement in
                guard let self, isCurrent(), self.workspaceKey == refused else {
                    throw WorkspaceKeyRecoveryError.superseded
                }
                SecureStore.write(replacement, for: "workspaceKey")
                guard self.workspaceKey == replacement else {
                    SecureStore.write(refused, for: "workspaceKey")
                    return false
                }
                return true
            })
    }
    private func cloudCall(_ path: String, token: String? = nil, method: String? = nil, body: JSONObject? = nil) async throws -> JSONObject {
        guard let url = URL(string: cloud + path) else { throw RelayAPIError(message: "Invalid Cloud URL") }
        return try await call(url, token: token, method: method ?? (body == nil ? "GET" : "POST"), body: body)
    }
    private func cloudTokenNeedsRefresh() -> Bool {
        guard let value = SecureStore.read("cloudAccessExpiresAt") else { return true }
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        let expires = formatter.date(from: value) ?? ISO8601DateFormatter().date(from: value)
        return expires.map { $0.timeIntervalSinceNow < 60 } ?? true
    }
    private func invalidateCloudRefresh() {
        cloudRefreshTask?.cancel()
        cloudRefreshTask = nil
        cloudRefreshTaskID = nil
    }
    private func refreshCloudToken() async throws -> String {
        guard !signingOut else { throw CancellationError() }
        if let cloudRefreshTask { return try await cloudRefreshTask.value }
        guard let refresh = SecureStore.read("cloudRefreshToken") else {
            throw RelayAPIError(message: "Sign in again to view session summaries")
        }
        let request = CloudCredentialRefresh(sessionGeneration: sessionGeneration, refreshToken: refresh)
        cloudRefreshTaskID = request.id
        let task = Task { try await performCloudRefresh(request) }
        cloudRefreshTask = task
        defer {
            if cloudRefreshTaskID == request.id {
                cloudRefreshTask = nil
                cloudRefreshTaskID = nil
            }
        }
        return try await task.value
    }
    private func requireCurrentRefresh(_ request: CloudCredentialRefresh) throws {
        guard !Task.isCancelled,
              request.isCurrent(requestID: cloudRefreshTaskID, sessionGeneration: sessionGeneration,
                                refreshToken: SecureStore.read("cloudRefreshToken"), signingOut: signingOut) else {
            throw CancellationError()
        }
    }
    private func performCloudRefresh(_ request: CloudCredentialRefresh) async throws -> String {
        try requireCurrentRefresh(request)
        do {
            let result = try await cloudCall("/auth/token/refresh", body: ["refreshToken": request.refreshToken])
            try requireCurrentRefresh(request)
            let access = string(result["accessToken"])
            let nextRefresh = string(result["refreshToken"])
            guard !access.isEmpty, !nextRefresh.isEmpty else {
                throw RelayAPIError(message: "Cloud did not renew the summary credential")
            }
            SecureStore.write(nextRefresh, for: "cloudRefreshToken")
            SecureStore.write(access, for: "cloudAccessToken")
            SecureStore.write(string(result["accessTokenExpiresAt"]), for: "cloudAccessExpiresAt")
            return access
        } catch let failure as RelayAPIError where failure.status == 403 || failure.status == 401 {
            try requireCurrentRefresh(request)
            for key in ["cloudAccessToken", "cloudRefreshToken", "cloudAccessExpiresAt"] { SecureStore.remove(key) }
            summaryCredentialReady = false
            throw RelayAPIError(message: "Sign in again to view session summaries")
        }
    }
    private func authorizedCloudCall(_ path: String, method: String = "GET", body: JSONObject? = nil) async throws -> JSONObject {
        let access: String
        if cloudTokenNeedsRefresh() || SecureStore.read("cloudAccessToken") == nil {
            access = try await refreshCloudToken()
        } else {
            access = SecureStore.read("cloudAccessToken") ?? ""
        }
        do { return try await cloudCall(path, token: access, method: method, body: body) }
        catch let failure as RelayAPIError where failure.status == 401 {
            let renewed = try await refreshCloudToken()
            return try await cloudCall(path, token: renewed, method: method, body: body)
        }
    }
    /// Asks the Cloud voice broker to start a GPT-Live session for a call. The
    /// broker holds the OpenAI key and the session settings; the app sends only
    /// its WebRTC offer, the agent's handle and recent call text.
    func createVoiceLiveSession(offer: String, agent: RelayAgent,
                                history: [(fromPerson: Bool, text: String)],
                                voice: String, persona: VoicePersona) async throws -> VoiceLiveGrant {
        guard let workspaceID = cloudWorkspaceID, UUID(uuidString: workspaceID) != nil else {
            throw RelayAPIError(message: "No Agent Relay workspace is selected", status: 404, code: "not_found")
        }
        let response = try await authorizedCloudCall(
            "/workspaces/\(workspaceID)/voice/live-sessions", method: "POST",
            body: ["sdp": offer, "agent": agent.name, "voice": voice, "persona": persona.rawValue,
                   "history": history.map { ["role": $0.fromPerson ? "user" : "assistant", "text": $0.text] }])
        let grant = VoiceLiveGrant(sessionID: string(response["sessionId"]), answer: string(response["sdp"]),
                                   remainingSeconds: (response["remainingSeconds"] as? NSNumber)?.intValue)
        guard !grant.sessionID.isEmpty, !grant.answer.isEmpty else {
            throw RelayAPIError(message: "Cloud did not return a voice session", status: 502, code: "voice_live_upstream")
        }
        return grant
    }
    /// Reports a finished GPT-Live session's billed seconds against the monthly
    /// quota. Returns whether Cloud accepted it, so failures can be retried.
    func reportVoiceLiveUsage(sessionID: String, seconds: Double) async -> Bool {
        guard let workspaceID = cloudWorkspaceID, UUID(uuidString: workspaceID) != nil,
              let id = sessionID.addingPercentEncoding(withAllowedCharacters: .alphanumerics.union(CharacterSet(charactersIn: "-_."))) else { return false }
        do {
            _ = try await authorizedCloudCall("/workspaces/\(workspaceID)/voice/live-sessions/\(id)/usage",
                                              method: "POST", body: ["seconds": seconds])
            return true
        } catch let failure as RelayAPIError where failure.status == 404 || failure.status == 400 {
            // Cloud will never accept it; don't retry.
            return true
        } catch {
            return false
        }
    }
    private func cloudPersonCredential(workspaceID: String) async throws -> (name: String, token: String, baseURL: String) {
        guard UUID(uuidString: workspaceID) != nil else {
            throw RelayAPIError(message: "No Agent Relay workspace is selected")
        }
        let response = try await authorizedCloudCall(
            "/workspaces/\(workspaceID)/messaging/identity/credential", method: "POST")
        let identity = object(response["identity"])
        let name = string(identity["name"])
        let token = string(identity["token"])
        let base = string(identity["relaycastUrl"])
        let components = URLComponents(string: base)
        guard !name.isEmpty, token.hasPrefix("at_live_"),
              !string(identity["agentId"]).isEmpty,
              components?.scheme == "https", components?.host != nil,
              components?.user == nil, components?.password == nil,
              components?.query == nil, components?.fragment == nil,
              (components?.path.isEmpty == true || components?.path == "/"),
              (components?.port == nil || components?.port == 443) else {
            throw RelayAPIError(message: "Cloud did not return a valid messaging identity")
        }
        return (name, token, base.trimmingCharacters(in: CharacterSet(charactersIn: "/")))
    }
    func checkConnection() async {
        guard paired, !signingOut else { return }
        if UserDefaults.standard.string(forKey: "relay.messagingIdentitySource") != "cloud" {
            // Existing installations have an agent unique to this device.
            // Upgrade before loading or sending any of its old messages.
            paired = false
            guard SecureStore.read("cloudRefreshToken") != nil else {
                error = "Sign in again to sync Messages with your other devices."
                return
            }
            do {
                try await finishSavedCloudSignIn()
            } catch {
                reportFailure(error)
            }
            return
        }
        await refresh()
    }
    private func clearPendingCloudGrant() {
        for key in ["cloudPendingDeviceCode", "cloudPendingExpiresAt", "cloudPendingUserCode", "cloudPendingURL"] {
            SecureStore.remove(key)
        }
        verificationURL = nil
        userCode = ""
        signInStatus = ""
    }
    private func pollCloudGrant(code: String, deadline: Date, initialInterval: Int) async throws {
        var interval = initialInterval
        while Date() < deadline {
            try await Task.sleep(for: .seconds(interval))
            let token: JSONObject
            let background = BackgroundRequest(name: "Cloud sign-in")
            defer { background.end() }
            do {
                token = try await cloudCall("/auth/device/token", body: [
                    "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
                    "device_code": code
                ])
            } catch let failure as RelayAPIError {
                switch CloudDeviceGrantPoll.classify(message: failure.message, status: failure.status) {
                case .keepPolling: continue
                case .slowDown: interval = min(interval + 5, 60); continue
                case .startOver(let message):
                    clearPendingCloudGrant()
                    throw RelayAPIError(message: message, status: failure.status, code: failure.code)
                case .fail: throw failure
                }
            } catch {
                if Task.isCancelled { throw error }
                signInStatus = "Waiting for the connection to return…"
                continue
            }
            guard !string(token["access_token"]).isEmpty else { continue }
            // A successful token response consumes the device code, even if
            // the remaining workspace setup fails and needs a later retry.
            clearPendingCloudGrant()
            try await finishSignIn(token)
            return
        }
        clearPendingCloudGrant()
        throw RelayAPIError(message: "Sign-in expired. Start again.")
    }
    func resumePendingSignIn() async {
        guard !busy, !signingOut else { return }
        busy = true
        defer { busy = false }
        do {
            if let code = SecureStore.read("cloudPendingDeviceCode"),
               let rawDeadline = SecureStore.read("cloudPendingExpiresAt"),
               let timestamp = Double(rawDeadline) {
                let deadline = Date(timeIntervalSince1970: timestamp)
                if deadline > Date() {
                    userCode = SecureStore.read("cloudPendingUserCode") ?? ""
                    verificationURL = SecureStore.read("cloudPendingURL").flatMap(URL.init(string:))
                    signInStatus = "Checking device approval…"
                    try await pollCloudGrant(code: code, deadline: deadline, initialInterval: 5)
                    return
                }
                clearPendingCloudGrant()
            }
            if SecureStore.read("cloudRefreshToken") != nil &&
                (!canLoadSummaries || UserDefaults.standard.string(forKey: "relay.messagingIdentitySource") != "cloud") {
                signInStatus = "Finishing Cloud sign-in…"
                try await finishSavedCloudSignIn()
                signInStatus = ""
            }
        } catch {
            reportFailure(error)
            signInFailures += 1
        }
    }
    func beginSignIn() async {
        guard !busy, !signingOut else { return }
        busy = true
        error = nil
        signInStatus = "Starting sign-in…"
        defer { busy = false }
        do {
            let grant = try await cloudCall("/auth/device/start", body: ["client_name": "Agent Relay Mobile"])
            let code = string(grant["device_code"])
            guard !code.isEmpty else { throw RelayAPIError(message: "Sign-in could not start") }
            userCode = string(grant["user_code"])
            let link = string(grant["verification_uri_complete"]).isEmpty
                ? string(grant["verification_uri"]) : string(grant["verification_uri_complete"])
            guard let url = URL(string: link), url.scheme == "https", url.host == "agentrelay.com" else {
                throw RelayAPIError(message: "Cloud returned an invalid sign-in link")
            }
            verificationURL = url
            signInStatus = "Tap Continue sign-in and approve this device. The app finishes on its own."
            let seconds = min(max(grant["expires_in"] as? Int ?? 600, 30), 900)
            let interval = min(max(grant["interval"] as? Int ?? 5, 1), 60)
            let deadline = Date().addingTimeInterval(TimeInterval(seconds))
            SecureStore.write(code, for: "cloudPendingDeviceCode")
            SecureStore.write(String(deadline.timeIntervalSince1970), for: "cloudPendingExpiresAt")
            SecureStore.write(userCode, for: "cloudPendingUserCode")
            SecureStore.write(url.absoluteString, for: "cloudPendingURL")
            try await pollCloudGrant(code: code, deadline: deadline, initialInterval: interval)
        } catch {
            reportFailure(error)
            signInFailures += 1
        }
    }
    private func finishSavedCloudSignIn() async throws {
        let access: String
        if cloudTokenNeedsRefresh() || SecureStore.read("cloudAccessToken") == nil {
            access = try await refreshCloudToken()
        } else {
            access = SecureStore.read("cloudAccessToken") ?? ""
        }
        guard let refresh = SecureStore.read("cloudRefreshToken") else { return }
        try await finishSignIn(["access_token": access, "refresh_token": refresh,
                                "access_token_expires_at": SecureStore.read("cloudAccessExpiresAt") ?? ""])
    }
    private func finishSignIn(_ tokens: JSONObject) async throws {
        let cloudToken = string(tokens["access_token"])
        let refreshToken = string(tokens["refresh_token"])
        guard !cloudToken.isEmpty, !refreshToken.isEmpty else {
            throw RelayAPIError(message: "Cloud did not grant persistent access. Start sign-in again.")
        }
        invalidateCloudRefresh()
        // The device code is single-use. Keep the Cloud credential before any
        // workspace lookup or Relaycast registration that might need a retry.
        SecureStore.write(refreshToken, for: "cloudRefreshToken")
        SecureStore.write(cloudToken, for: "cloudAccessToken")
        SecureStore.write(string(tokens["access_token_expires_at"]), for: "cloudAccessExpiresAt")
        guard SecureStore.read("cloudRefreshToken") == refreshToken else {
            throw RelayAPIError(message: "Could not save Cloud access on this device")
        }
        let identity = try await cloudCall("/auth/whoami", token: cloudToken)
        let id = string(object(identity["currentWorkspace"])["id"])
        guard let uuid = UUID(uuidString: id) else { throw RelayAPIError(message: "No Agent Relay workspace is selected") }
        let resolution = try await cloudCall("/workspaces/\(uuid.uuidString.lowercased())/resolve", token: cloudToken)
        let resolved = try WorkspaceKeyResolution(resolution: resolution, defaultBaseURL: defaultRelaycast)
        let key = resolved.key
        // Cloud owns this person's messaging identity. The desktop and web
        // clients use the same credential endpoint, so all three surfaces see
        // the same Relaycast conversations and message history.
        let messagingIdentity = try await cloudPersonCredential(workspaceID: id)
        guard messagingIdentity.baseURL == resolved.baseURL else {
            throw RelayAPIError(message: "Cloud returned a different Relaycast server for this workspace")
        }
        let previousKey = workspaceKey
        let previousURL = SecureStore.read("relaycastURL")
        let previousToken = agentToken
        let previousMobileName = UserDefaults.standard.string(forKey: "relay.mobileName")
        if previousKey != key {
            summaries = [:]
            goals = [:]
            resetRowSummaries()
            sessionSummaries = [:]
            sessionRevisions = [:]
            sessionSources = [:]
            sessionsMissingFromHistory = []
        }
        let accountUser = object(identity["user"])
        do {
            if let oldToken = previousToken, oldToken != messagingIdentity.token,
               let oldBase = previousURL ?? (previousKey == nil ? nil : defaultRelaycast),
               let pushToken = RelayPushState.shared.token,
               let oldURL = URL(string: oldBase.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
                    + "/v1/mobile/push-device") {
                _ = try? await call(oldURL, token: oldToken, method: "DELETE",
                                    body: ["device_token": pushToken, "environment": pushEnvironment])
            }
            SecureStore.write(key, for: "workspaceKey")
            SecureStore.write(messagingIdentity.baseURL, for: "relaycastURL")
            SecureStore.write(messagingIdentity.token, for: "agentToken")
            guard workspaceKey == key, agentToken == messagingIdentity.token,
                  relaycast == messagingIdentity.baseURL else {
                throw RelayAPIError(message: "Could not save the messaging credential")
            }
        } catch {
            if let previousKey { SecureStore.write(previousKey, for: "workspaceKey") }
            else { SecureStore.remove("workspaceKey") }
            if let previousURL { SecureStore.write(previousURL, for: "relaycastURL") }
            else { SecureStore.remove("relaycastURL") }
            if let previousToken { SecureStore.write(previousToken, for: "agentToken") }
            else { SecureStore.remove("agentToken") }
            if let previousMobileName { UserDefaults.standard.set(previousMobileName, forKey: "relay.mobileName") }
            else { UserDefaults.standard.removeObject(forKey: "relay.mobileName") }
            throw error
        }
        workspaceKeyRecovery.reset()
        if previousToken != messagingIdentity.token || previousURL != messagingIdentity.baseURL {
            resetPeople()
            registeredPushToken = nil
            advanceSessionGeneration()
            // Row loads from the old session come back nil; abandon them so
            // they are not stored as attempts and the rows load again now.
            resetRowSummaries()
        }
        account = string(accountUser["name"]).isEmpty ? string(accountUser["email"]) : string(accountUser["name"])
        workspace = resolved.workspaceName.isEmpty ? "Workspace" : resolved.workspaceName
        UserDefaults.standard.set(account, forKey: "relay.accountName")
        UserDefaults.standard.set(workspace, forKey: "relay.workspaceName")
        UserDefaults.standard.set(messagingIdentity.name, forKey: "relay.mobileName")
        UserDefaults.standard.removeObject(forKey: "relay.mobileIdentity")
        UserDefaults.standard.set("cloud", forKey: "relay.messagingIdentitySource")
        if previousKey != key || previousMobileName != messagingIdentity.name {
            // Clear the installation-specific agent's local view before
            // loading this person's shared conversations.
            refreshGeneration += 1
            conversationRefreshGeneration += 1
            conversationMetadataWorkerID = nil
            conversationAgentMetadata = [:]
            handoffDismissals = MobileHandoffDismissals()
            conversationRoster = ConversationRoster()
            rosterLoaded = false
            conversationAgentRetryAfter = [:]
            latestConversationThreads = []
            conversations = []
            channels = []
            joinedChannels = []
            channelSenders = [:]
            channelSenderRetryAfter = [:]
            recentJoins = [:]
            messages = []
            loadedHistoryConversation = nil
            unreadConversationIDs = []
            hiddenConversationIDs = []
            conversationIDs = [:]
            threadMisses.reset()
            latestMessageIDs = [:]
            localMessageNotices = LocalMessageNotices()
            latestIncomingMessageIDs = [:]
            conversationsInitialized = false
            selectedConversation = nil
            deliveredMessageIDs = []
        }
        UserDefaults.standard.set(id, forKey: "relay.workspaceID")
        cloudWorkspaceID = id
        summaryCredentialReady = SecureStore.read("cloudRefreshToken") != nil
        paired = true
        await refresh()
    }
    func signOut() async {
        guard !signingOut else { return }
        signingOut = true
        invalidateCloudRefresh()
        workspaceKeyRecovery.reset()
        resetPeople()
        // Invalidate work before the first suspension point so an in-flight
        // refresh cannot repopulate signed-out state while cleanup awaits.
        refreshGeneration += 1
        advanceSessionGeneration()
        conversationRefreshGeneration += 1
        conversationMetadataWorkerID = nil
        threadMisses.reset()
        if let token = RelayPushState.shared.token {
            _ = try? await api("/mobile/push-device", agent: true, method: "DELETE",
                               body: ["device_token": token, "environment": pushEnvironment])
        }
        registeredPushToken = nil
        await closeTerminal()
        UserDefaults.standard.removeObject(forKey: hiddenConversationsKey)
        for key in ["workspaceKey", "agentToken", "relaycastURL", "cloudAccessToken", "cloudRefreshToken", "cloudAccessExpiresAt",
                    "cloudPendingDeviceCode", "cloudPendingExpiresAt", "cloudPendingUserCode", "cloudPendingURL"] {
            SecureStore.remove(key)
        }
        UserDefaults.standard.removeObject(forKey: "relay.accountName")
        UserDefaults.standard.removeObject(forKey: "relay.workspaceName")
        UserDefaults.standard.removeObject(forKey: "relay.workspaceID")
        UserDefaults.standard.removeObject(forKey: "relay.mobileName")
        UserDefaults.standard.removeObject(forKey: "relay.mobileIdentity")
        UserDefaults.standard.removeObject(forKey: "relay.messagingIdentitySource")
        cloudWorkspaceID = nil
        workspace = "Workspace"
        account = "Agent Relay"
        paired = false
        connected = false
        signingOut = false
        relayAgents = []
        rosterLoaded = false
        fleetAgents = []
        spawnedSessions = [:]
        spawnNodes = []
        forkNodes = []
        conversations = []
        channels = []
        joinedChannels = []
        channelSenders = [:]
        channelSenderRetryAfter = [:]
        recentJoins = [:]
        unreadConversationIDs = []
        hiddenConversationIDs = []
        messages = []
        loadedHistoryConversation = nil
        selectedConversation = nil
        latestMessageIDs = [:]
        localMessageNotices = LocalMessageNotices()
        latestIncomingMessageIDs = [:]
        conversationsInitialized = false
        deliveredMessageIDs = []
        summaries = [:]
        goals = [:]
        resetRowSummaries()
        resetSessionEvents()
        historyCheckedConversation = nil
        sessionSummaries = [:]
        sessionRevisions = [:]
        sessionSources = [:]
        sessionsMissingFromHistory = []
        summaryCredentialReady = false
        conversationAgentMetadata = [:]
        handoffDismissals = MobileHandoffDismissals()
        conversationRoster = ConversationRoster()
        conversationAgentRetryAfter = [:]
        latestConversationThreads = []
    }
    private var pushEnvironment: String {
        #if DEBUG
        return "sandbox"
        #else
        return "production"
        #endif
    }
    func registerPushToken(_ token: String) async {
        guard paired, !signingOut, !token.isEmpty, registeredPushToken != token else { return }
        do {
            _ = try await api("/mobile/push-device", agent: true, method: "PUT",
                              body: ["device_token": token, "environment": pushEnvironment])
            registeredPushToken = token
        } catch {
            NSLog("Agent Relay push token upload failed: %@", error.localizedDescription)
        }
    }

    /// The view reports every scene phase, so requests left in flight when
    /// iOS suspended the app are not reported when it comes back.
    func sceneChanged(foreground: Bool) {
        connectionReports.sceneChanged(foreground: foreground)
    }
    /// Counts a failed automatic read unless it is stale, and returns whether
    /// it did. An automatic read never raises the alert: the next read retries
    /// it (after any wait the server asked for), a failed presence refresh
    /// shows as Offline, and a failed history read shows the open
    /// conversation's inline `refreshNotice`. A channel's history is not
    /// re-read by the periodic refresh, so its notice promises no retry.
    @discardableResult
    private func reportConnectionFailure(_ failure: Error, from ticket: ConnectionErrorReports.Ticket,
                                         channel: Bool = false) -> Bool {
        switch connectionReports.decision(for: failure, from: ticket) {
        case .show:
            if ticket.operation == .history {
                refreshNotice = channel ? UserFacingError.staleChannelMessages : UserFacingError.staleMessages
            }
            return true
        case .drop(let reason):
            NSLog("%@", ConnectionErrorReports.diagnostic(ticket.operation, reason, failure))
            return false
        }
    }

    /// Shows a failure the person caused (a send, a sign-in, a download) in
    /// the alert, in the app's words; a cancellation shows nothing.
    func reportFailure(_ failure: Error) {
        if let message = UserFacingError.message(for: failure) { error = message }
    }

    func refresh() async {
        guard paired, !signingOut else { return }
        refreshGeneration += 1
        let generation = refreshGeneration
        let ticket = connectionReports.begin(.presence)
        // DM history and presence reconcile independently: a failed node or
        // roster request must not stop messages arriving in an open chat.
        async let messageRefresh: Void = pollMessages(fromRefresh: generation)
        async let channelRefresh: Void = pollChannels(fromRefresh: generation)
        // Photos are optional: an unavailable Cloud endpoint must never block
        // the Relaycast roster, conversation history or messaging.
        peopleRefreshTask?.cancel()
        peopleRefreshTask = Task { await refreshPeople() }
        do {
            async let rosterData = api("/agents?status=all")
            async let nodeData = api("/nodes")
            let roster = try objects(await rosterData)
            guard generation == refreshGeneration, paired else { return }
            applyRoster(roster)
            rosterLoaded = true
            StartupTrace.firstRosterShown()
            let nodes = try objects(await nodeData)
            guard generation == refreshGeneration, paired else { return }
            let availableNodes = nodes.compactMap { node -> RelaySpawnNode? in
                guard string(node["status"]) == "online", node["live"] as? Bool != false,
                      node["handlers_live"] as? Bool != false,
                      !(node["tags"] as? [String] ?? []).contains("direct") else { return nil }
                let name = string(node["name"])
                let harnesses = Set(objects(node["capabilities"]).compactMap { capability -> String? in
                    let value = string(capability["name"])
                    guard value.hasPrefix("spawn:"), value.count > 6 else { return nil }
                    return String(value.dropFirst(6))
                })
                guard !name.isEmpty, !harnesses.isEmpty else { return nil }
                let fork = object(objects(node["capabilities"]).first { string($0["name"]) == SessionFork.capability }?["metadata"])
                return RelaySpawnNode(name: name, harnesses: harnesses.sorted(),
                                      forkHarnesses: fork["harnesses"] as? [String] ?? [],
                                      machine: string(fork["machine"]),
                                      recentFolders: SpawnFolderSuggestions.advertisedFolders(in: objects(node["capabilities"])))
            }.sorted { ($0.name.lowercased(), $0.name) < ($1.name.lowercased(), $1.name) }
            var machineKeys: Set<String> = []
            spawnNodes = availableNodes.filter { machineKeys.insert(MobileMachineFilter.key($0.name)).inserted }
            forkNodes = availableNodes.filter { !$0.forkHarnesses.isEmpty }
            let sessionIDs = Dictionary(relayAgents.map { ($0.name, $0.sessionID) }, uniquingKeysWith: { first, _ in first })
            var live: [RelayAgent] = []
            var reporting: [(name: String, capabilities: [JSONObject])] = []
            for node in nodes where string(node["status"]) == "online" &&
                node["live"] as? Bool != false && node["handlers_live"] as? Bool != false &&
                !(node["tags"] as? [String] ?? []).contains("direct") {
                let nodeName = string(node["name"])
                reporting.append((name: nodeName, capabilities: objects(node["capabilities"])))
                for capability in objects(node["capabilities"]) where string(capability["name"]) == "relay:live-agents:v1" {
                    for name in object(capability["metadata"])["names"] as? [String] ?? [] {
                        live.append(RelayAgent(fleet: ["name": name, "node": nodeName,
                                                       "session_id": sessionIDs[name] ?? "", "workerLiveness": "live"]))
                    }
                }
            }
            fleetAgents = live
            spawnedSessions = SessionFork.spawned(nodes: reporting)
            connected = true
            error = nil
            connectionReports.succeeded(ticket)
        } catch {
            guard generation == refreshGeneration, paired else { return }
            rosterLoaded = true
            // A failure from before the app was suspended says nothing about
            // the connection now; the foreground refresh decides that.
            if reportConnectionFailure(error, from: ticket), !UserFacingError.isBackoff(error) { connected = false }
        }
        await messageRefresh
        await channelRefresh
    }
    private func applyRoster(_ roster: [JSONObject]) {
        let registrations = roster.map(RelayAgent.init(relay:))
        conversationRoster.replace(registrations, excluding: messagingHandle)
        relayAgents = conversationRoster.listedAgents
        // Current registrations replace their cached metadata. Historical
        // lookups stay cached to avoid refetching every old DM peer each poll;
        // conversationRoster alone decides whether an agent is still present.
        for agent in relayAgents {
            conversationAgentMetadata[agent.name] = agent
            conversationAgentRetryAfter.removeValue(forKey: agent.name)
        }
        // A dismissed failure comes back once its agent publishes anything else.
        let published = Dictionary(conversationAgentMetadata.values.compactMap { agent in
            agent.mobileHandoff.map { (MobileHandoffDismissals.identity(name: agent.name, sessionID: agent.sessionID), $0) }
        }, uniquingKeysWith: { first, _ in first })
        let retained = handoffDismissals.retaining(published: published)
        if retained != handoffDismissals { handoffDismissals = retained }
        if conversationsInitialized {
            updateConversations(latestConversationThreads, metadata: conversationAgentMetadata)
        }
    }

    private func resetPeople() {
        peopleRefreshTask?.cancel()
        peopleRefreshTask = nil
        workspacePeople.reset()
    }

    private func refreshPeople() async {
        guard !Task.isCancelled, paired, !signingOut, let workspaceID = cloudWorkspaceID else { return }
        let generation = sessionGeneration
        let request = workspacePeople.begin(workspaceID: workspaceID, sessionGeneration: generation)
        let response: JSONObject
        do {
            #if DEBUG
            if let fixture = RelayUITestFixture.peopleResponse() {
                if RelayUITestFixture.delaysPeople {
                    try await Task.sleep(for: .seconds(10))
                }
                response = fixture
            } else {
                response = try await loadPeople(workspaceID: workspaceID)
            }
            #else
            response = try await loadPeople(workspaceID: workspaceID)
            #endif
        } catch {
            // Includes older Cloud servers, removed membership and expired
            // credentials. Keep initials usable and retry on the next refresh.
            response = [:]
        }
        guard !Task.isCancelled, paired, !signingOut,
              cloudWorkspaceID == workspaceID, sessionGeneration == generation else { return }
        workspacePeople.finish(response, for: request)
    }

    private func loadPeople(workspaceID: String) async throws -> JSONObject {
        guard UUID(uuidString: workspaceID) != nil, hasSavedCloudCredential else { return [:] }
        return try await authorizedCloudCall("/workspaces/\(workspaceID)/messaging/people")
    }

    func personProfile(for agent: RelayAgent) -> WorkspacePerson? {
        guard paired, !signingOut else { return nil }
        return workspacePeople.person(handle: agent.name, isHuman: agent.isHuman,
                                      workspaceID: cloudWorkspaceID, sessionGeneration: sessionGeneration)
    }

    func displayName(for agent: RelayAgent) -> String {
        if agent.source == .channel { return agent.address }
        guard agent.isHuman else { return agent.name }
        let name = personProfile(for: agent)?.displayName ?? ""
        if !name.isEmpty { return name }
        return agent.displayName.isEmpty ? agent.name : agent.displayName
    }

    func pollMessages(fromRefresh refresh: Int? = nil) async {
        guard paired, !signingOut, !Task.isCancelled,
              refresh == nil || refresh == refreshGeneration else { return }
        await reloadConversations(fromRefresh: refresh, afterCarryOver: false)
    }
    /// Reads the conversation list and the server's read state together.
    /// `afterCarryOver` is the one re-read after carrying the old device
    /// cursor over: it applies the inbox as it is and starts no carry-over.
    private func reloadConversations(fromRefresh refresh: Int?, afterCarryOver: Bool) async {
        guard paired, !signingOut, !Task.isCancelled,
              refresh == nil || refresh == refreshGeneration else { return }
        resetReadStateIfSessionChanged()
        conversationRefreshGeneration += 1
        let generation = conversationRefreshGeneration
        let session = sessionGeneration
        let ticket = readState.inboxRequested()
        let inboxStarted = Date()
        async let inboxData: Any? = try? api("/inbox", agent: true)
        async let threadData: Any? = try? api("/dm/conversations", agent: true)
        let inbox = await inboxData as? JSONObject
        let threads = await threadData.map { objects($0) }
        // A transient poll failure should not interrupt an open conversation.
        guard generation == conversationRefreshGeneration, session == sessionGeneration,
              paired, !signingOut else { return }
        let defaults = UserDefaults.standard
        let cursorKeys = [unreadSeenKey, unreadBaselineKey, unreadStoragePrefix + ".carryOverFailures"]
        let oldCursor = ConversationReadState.oldSeenCursor(defaults.dictionary(forKey: cursorKeys[0]),
                                                            hasBaseline: defaults.object(forKey: cursorKeys[1]) != nil)
        var answer: [String: String]?
        var carryOver: [(conversationID: String, messageID: String)] = []
        if let unread = inbox.map({ ConversationReadState.unread(inInbox: $0, mine: messagingHandle) }) {
            switch ConversationReadState.inboxStep(unread: unread, oldCursor: oldCursor,
                                                   carryOverRunning: carryingOverSeen, afterCarryOver: afterCarryOver) {
            case .apply(let forgetCursor):
                answer = unread
                if forgetCursor { cursorKeys.forEach(defaults.removeObject(forKey:)) }
            case .keepLast: break
            case .carryOver(let plan): carryOver = plan
            }
        }
        if let answer { readState.apply(inbox: answer, ticket: ticket) } else { readState.inboxUnanswered() }
        if let threads {
            latestConversationThreads = threads
            updateConversations(threads, metadata: conversationAgentMetadata)
            Task { await enrichConversationMetadata() }
        } else if answer != nil, conversationsInitialized {
            updateConversations(latestConversationThreads, metadata: conversationAgentMetadata)
        }
        if answer != nil { await removeReadNotifications(deliveredBefore: inboxStarted) }
        if !carryOver.isEmpty {
            carryingOverSeen = true
            var outcomes: [ConversationReadState.Outcome] = []
            for step in carryOver {
                outcomes.append(await postRead(step.messageID))
                guard session == sessionGeneration else { return }
            }
            carryingOverSeen = false
            let next = ConversationReadState.cursorAfterCarryOver(seen: oldCursor ?? [:], plan: carryOver, outcomes: outcomes,
                                                                  failedReloads: defaults.integer(forKey: cursorKeys[2]))
            if let seen = next.seen {
                defaults.set(seen, forKey: cursorKeys[0])
                defaults.set(next.failedReloads, forKey: cursorKeys[2])
            } else {
                cursorKeys.forEach(defaults.removeObject(forKey:))
            }
            await reloadConversations(fromRefresh: nil, afterCarryOver: true)
            return
        }
        if attachedID == nil, let open = selectedConversation,
           let agent = conversations.first(where: { $0.id == open }) ?? relayAgents.first(where: { $0.id == open }),
           let conversationID = conversationIDs[agent.name] {
            await markOpenConversationRead(agent, conversationID: conversationID)
        }
    }
    /// Lists workspace channels, marking the ones this person has joined.
    /// A failure keeps the last list, like a failed DM poll.
    func pollChannels(fromRefresh refresh: Int? = nil) async {
        guard paired, !signingOut, !Task.isCancelled,
              refresh == nil || refresh == refreshGeneration else { return }
        let session = sessionGeneration
        let handle = messagingHandle
        do {
            async let listData = api("/channels")
            async let joinedData = channelMemberships(of: handle)
            let list = try objects(await listData)
            // Without the person's own record, keep the memberships last seen.
            let listed = (try? await joinedData) ?? joinedChannels
            guard session == sessionGeneration, paired, !signingOut,
                  refresh == nil || refresh == refreshGeneration else { return }
            let now = Date()
            recentJoins = recentJoins.filter { !listed.contains($0.key) && now.timeIntervalSince($0.value) < 2 * 60 }
            let joined = listed.union(recentJoins.keys)
            joinedChannels = joined
            channels = RelayChannels.list(list, joined: joined)
        } catch {
            // A transient failure should not empty the Channels page.
        }
    }
    private func channelMemberships(of handle: String) async throws -> Set<String> {
        guard !handle.isEmpty else { return [] }
        return RelayChannels.joinedNames(object(try await api("/agents/\(RelayChannels.pathComponent(handle))")))
    }
    @discardableResult
    func joinChannel(_ channel: RelayAgent) async -> Bool {
        guard channel.source == .channel else { return false }
        do {
            try await join(channel.name, session: sessionGeneration)
            await pollChannels()
            return true
        } catch is CancellationError {
            return false
        } catch { reportFailure(error); return false }
    }
    /// Joining is idempotent on Relaycast. A join that returns after the
    /// person signed out or switched accounts is dropped.
    private func join(_ name: String, session: Int) async throws {
        try checkSession(session)
        do {
            _ = try await api("/channels/\(RelayChannels.pathComponent(name))/join", agent: true, method: "POST")
        } catch let failure as RelayAPIError where failure.status == 409 {
            // Already a member, as the desktop's Relaycast client also treats it.
        }
        try checkSession(session)
        joinedChannels.insert(name)
        recentJoins[name] = Date()
    }
    /// Who sent a channel message, so it shows their photo or agent artwork.
    func sender(of message: RelayMessage) -> RelayAgent {
        RelayChannels.sender(named: message.sender,
                             in: relayAgents + Array(conversationAgentMetadata.values) + Array(channelSenders.values))
    }
    private func loadChannelSenders(in messages: [RelayMessage]) async {
        let session = sessionGeneration
        let known = Set(relayAgents.map(\.name)).union(conversationAgentMetadata.keys).union(channelSenders.keys)
        let now = Date()
        let missing = Set(messages.filter { !$0.mine && !$0.sender.isEmpty }.map(\.sender))
            .subtracting(known).filter { (channelSenderRetryAfter[$0] ?? .distantPast) <= now }
        for name in missing.sorted().prefix(10) {
            // Retry later, like DM peers, until a lookup succeeds.
            channelSenderRetryAfter[name] = now.addingTimeInterval(5 * 60)
            guard let agent = await loadConversationAgent(named: name), session == sessionGeneration else { continue }
            channelSenders[name] = agent
            channelSenderRetryAfter.removeValue(forKey: name)
        }
    }
    private func conversationPeerNames(in threads: [JSONObject]) -> [String] {
        let mine = UserDefaults.standard.string(forKey: "relay.mobileName") ?? ""
        return Array(Set(threads.compactMap { thread in
            let peer = objects(thread["participants"]).first { string($0["agent_name"]) != mine }
            let name = string(peer?["agent_name"])
            return name.isEmpty ? nil : name
        })).sorted()
    }
    private func loadConversationAgent(named name: String) async -> RelayAgent? {
        let allowed = CharacterSet.urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))
        let encoded = name.addingPercentEncoding(withAllowedCharacters: allowed) ?? name
        guard let data = try? await api("/agents/\(encoded)"),
              !object(data).isEmpty else { return nil }
        return RelayAgent(relay: object(data))
    }
    private func enrichConversationMetadata() async {
        guard conversationMetadataWorkerID == nil, let workspaceID = cloudWorkspaceID else { return }
        let workerID = UUID()
        conversationMetadataWorkerID = workerID
        defer {
            if conversationMetadataWorkerID == workerID {
                conversationMetadataWorkerID = nil
            }
        }
        var attempted: Set<String> = []
        while conversationMetadataWorkerID == workerID && cloudWorkspaceID == workspaceID {
            let now = Date()
            let missing = conversationPeerNames(in: latestConversationThreads).filter {
                conversationAgentMetadata[$0] == nil && !attempted.contains($0) &&
                    (conversationAgentRetryAfter[$0] ?? .distantPast) <= now
            }
            guard !missing.isEmpty else { return }
            await withTaskGroup(of: (String, RelayAgent?).self) { group in
                var next = 0
                let concurrency = min(4, missing.count)
                for _ in 0..<concurrency {
                    let name = missing[next]
                    next += 1
                    group.addTask {
                        let agent = await self.loadConversationAgent(named: name)
                        return (name, agent)
                    }
                }
                while let (name, agent) = await group.next() {
                    attempted.insert(name)
                    guard conversationMetadataWorkerID == workerID,
                          cloudWorkspaceID == workspaceID else {
                        group.cancelAll()
                        continue
                    }
                    if let agent {
                        // A roster refresh may have supplied newer metadata
                        // while this historical lookup was in flight.
                        if conversationAgentMetadata[name] == nil {
                            conversationAgentMetadata[name] = agent
                            updateConversations(latestConversationThreads, metadata: conversationAgentMetadata)
                        }
                        conversationAgentRetryAfter.removeValue(forKey: name)
                    } else {
                        conversationAgentRetryAfter[name] = Date().addingTimeInterval(5 * 60)
                    }
                    if next < missing.count {
                        let nextName = missing[next]
                        next += 1
                        group.addTask {
                            let agent = await self.loadConversationAgent(named: nextName)
                            return (nextName, agent)
                        }
                    }
                }
            }
        }
    }
    private func updateConversations(_ threads: [JSONObject], metadata: [String: RelayAgent]) {
        let mine = UserDefaults.standard.string(forKey: "relay.mobileName") ?? ""
        // Relaycast's unread_count includes this agent's own messages; the
        // server's read state (`/inbox`) says which conversations are unread.
        let defaults = UserDefaults.standard
        var ids: [String: String] = [:]
        var latest: [String: String] = [:]
        var latestIncoming: [String: String] = [:]
        var unread: Set<String> = []
        var updated: [RelayAgent] = []
        for thread in threads {
            let conversationID = string(thread["id"])
            let participants = objects(thread["participants"])
            let peer = participants.first { string($0["agent_name"]) != mine }
            let name = string(peer?["agent_name"])
            guard !name.isEmpty else { continue }
            let isCurrentPeer = conversationRoster.includes(name, metadata: metadata[name])
            ids[name] = conversationID
            let last = object(thread["last_message"])
            let messageID = string(last["id"])
            if !messageID.isEmpty { latest[conversationID] = messageID }
            let myID = string(participants.first { string($0["agent_name"]) == mine }?["agent_id"])
            let senderID = string(last["agent_id"])
            let senderName = string(last["agent_name"])
            let incoming = !messageID.isEmpty &&
                (!senderID.isEmpty && !myID.isEmpty ? senderID != myID :
                 !senderName.isEmpty && senderName != mine)
            if incoming { latestIncoming[conversationID] = messageID }
            if readState.unreadConversationIDs.contains(conversationID) { unread.insert("conversation:\(name)") }
            if localMessageNotices.shouldPost(conversationID: conversationID, messageID: messageID,
                                              isCurrentPeer: isCurrentPeer,
                                              apnsToken: RelayPushState.shared.token,
                                              initialized: conversationsInitialized,
                                              fromMe: string(last["agent_id"]) == myID,
                                              serverSays: readState.notice(conversationID: conversationID)) {
                let content = UNMutableNotificationContent()
                // A person shows by name, as in the app; agents by their name.
                let person = metadata[name]
                content.title = person?.isHuman == true && !(person?.displayName.isEmpty ?? true)
                    ? person?.displayName ?? name : name
                content.body = "New message in Agent Relay"
                content.sound = .default
                content.userInfo = ["conversation_id": conversationID]
                notificationCenter.post(UNNotificationRequest(identifier: messageID,
                    content: content, trigger: nil))
            }
            var enriched = thread
            enriched["status"] = metadata[name]?.status ?? ""
            enriched["machine"] = metadata[name]?.machine ?? ""
            enriched["session_id"] = metadata[name]?.sessionID ?? ""
            enriched["objective"] = metadata[name]?.objective ?? ""
            enriched["session_title"] = metadata[name]?.sessionTitle ?? ""
            enriched["mobile_handoff_enabled"] = metadata[name]?.mobileHandoffEnabled ?? false
            enriched["mobile_native_continuation_enabled"] = metadata[name]?.mobileNativeContinuationEnabled ?? false
            enriched["mobile_native_continuation_approvals"] = metadata[name]?.mobileNativeCommandFileApprovals == true ? "command_file" : "desktop"
            enriched["cli"] = metadata[name]?.cli ?? ""
            enriched["desktop_registration"] = metadata[name]?.isDesktopRegistration ?? false
            if let handoff = metadata[name]?.mobileHandoff {
                enriched["handoff_metadata"] = ["mobile_handoff_state": handoff.state.rawValue,
                                                "mobile_handoff_error": handoff.detail]
            }
            enriched["is_human"] = metadata[name]?.isHuman ?? false
            enriched["display_name"] = metadata[name]?.displayName ?? ""
            if isCurrentPeer { updated.append(RelayAgent(conversation: enriched)) }
        }
        // A hidden conversation comes back when any newer message arrives, so
        // a reply is never lost behind the swipe, even when this device only
        // sees the person's own later message from another device.
        var hidden = HiddenConversations(defaults.dictionary(forKey: hiddenConversationsKey) as? [String: String] ?? [:])
        if hidden.reveal(newerThan: latest) { defaults.set(hidden.entries, forKey: hiddenConversationsKey) }
        let visibleIDs = Set(updated.map(\.id))
        let hiddenIDs = Set(ids.filter { hidden.contains($0.value) }.map { "conversation:\($0.key)" }).intersection(visibleIDs)
        conversationIDs = ids
        latestMessageIDs = latest
        latestIncomingMessageIDs = latestIncoming
        conversations = updated
        hiddenConversationIDs = hiddenIDs
        unreadConversationIDs = unread.intersection(visibleIDs).subtracting(hiddenIDs)
        conversationsInitialized = true
    }
    /// The read state belongs to the session it was read for.
    private func resetReadStateIfSessionChanged() {
        guard readStateSession != sessionGeneration else { return }
        readState = ConversationReadState()
        readStateSession = sessionGeneration
        carryingOverSeen = false
    }
    /// Marks the open direct conversation read up to its newest incoming
    /// message, once, while the app is in the foreground. A failed post is
    /// tried again by the next history load.
    private func markOpenConversationRead(_ agent: RelayAgent, conversationID: String) async {
        resetReadStateIfSessionChanged()
        guard ConversationReadState.mayMarkRead(isDirectConversation: agent.source != .channel,
                                                isOpen: selectedConversation == agent.id,
                                                historyLoaded: loadedHistoryConversation == agent.id,
                                                appActive: isAppActive()) else { return }
        let page = messages.map { (id: $0.hasServerID ? $0.id : "", mine: $0.mine) }
        guard let messageID = ConversationReadState.messageToMarkRead(
                page: page, latestIncomingID: latestIncomingMessageIDs[conversationID]),
              readState.beginRead(conversationID: conversationID, messageID: messageID) else { return }
        unreadConversationIDs.remove("conversation:\(agent.name)")
        let session = sessionGeneration
        let postStarted = Date()
        let outcome = await postRead(messageID)
        guard session == sessionGeneration else { return }
        readState.finishRead(conversationID: conversationID, messageID: messageID, outcome: outcome)
        if outcome == .success { await removeReadNotifications(deliveredBefore: postStarted) }
    }
    private func postRead(_ messageID: String) async -> ConversationReadState.Outcome {
        let allowed = CharacterSet.urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))
        let encoded = messageID.addingPercentEncoding(withAllowedCharacters: allowed) ?? messageID
        do {
            _ = try await api("/messages/\(encoded)/read", agent: true, method: "POST")
            return .success
        } catch let failure as RelayAPIError {
            return ConversationReadState.outcome(forFailureStatus: failure.status)
        } catch {
            return .failed
        }
    }
    /// Clears delivered notifications of direct conversations that are read,
    /// delivered before the read that decided it started.
    private func removeReadNotifications(deliveredBefore cutoff: Date) async {
        let direct = Set(conversationIDs.values)
        let unread = readState.unreadConversationIDs
        let read = ConversationReadState.deliveredNotificationsToRemove(
            await notificationCenter.delivered(), before: cutoff, directConversationIDs: direct, unread: unread)
        if !read.isEmpty { notificationCenter.remove(read) }
    }
    /// Hides a conversation from the Messages list on this device only.
    func hideConversation(_ agent: RelayAgent) {
        setConversation(agent, hidden: true)
    }
    func unhideConversation(_ agent: RelayAgent) {
        setConversation(agent, hidden: false)
    }
    private func setConversation(_ agent: RelayAgent, hidden isHidden: Bool) {
        guard agent.source == .conversation, let id = conversationIDs[agent.name], !id.isEmpty else { return }
        let defaults = UserDefaults.standard
        var hidden = HiddenConversations(defaults.dictionary(forKey: hiddenConversationsKey) as? [String: String] ?? [:])
        if isHidden { hidden.hide(id, latestMessageID: latestMessageIDs[id]) } else { hidden.unhide(id) }
        defaults.set(hidden.entries, forKey: hiddenConversationsKey)
        updateConversations(latestConversationThreads, metadata: conversationAgentMetadata)
    }
    func agent(forConversationID id: String) -> RelayAgent? {
        conversations.first { conversationIDs[$0.name] == id }
    }
    func searchRoster(_ query: String, scope: String) async -> [RelayAgent] {
        let lower = query.lowercased()
        let agents = scope == "active" ? relayAgents.filter { $0.isReachable() } : relayAgents
        return agents.filter { lower.isEmpty || $0.name.lowercased().contains(lower) || $0.machine.lowercased().contains(lower) }
    }
    func searchSummaries(_ query: String, cursor: String? = nil) async throws -> SummarySearchPage {
        #if DEBUG
        if RelayUITestFixture.summarySearchIsOffline { throw URLError(.notConnectedToInternet) }
        if RelayUITestFixture.summarySearchFails {
            throw RelayAPIError(message: "The summary index could not be reached.")
        }
        if RelayUITestFixture.delaysSummarySearch { try await Task.sleep(for: .seconds(2)) }
        if let response = RelayUITestFixture.summarySearchResponse(query, cursor: cursor) {
            return SummarySearchPage(response)
        }
        #endif
        let normalized = SummarySearchQuery.normalize(query)
        guard let workspaceID = cloudWorkspaceID, hasSavedCloudCredential else {
            throw RelayAPIError(message: "Sign in again to search session summaries.")
        }
        guard let path = SummarySearchRequest.path(workspaceID: workspaceID, query: normalized, cursor: cursor) else {
            throw RelayAPIError(message: "Search must be between 2 and 500 characters.")
        }
        let response = try await authorizedCloudCall(path)
        return SummarySearchPage(response)
    }
    var canLoadSummaries: Bool {
        summaryCredentialReady && cloudWorkspaceID != nil
    }
    var hasSavedCloudCredential: Bool {
        SecureStore.read("cloudRefreshToken") != nil
    }
    private func sessionPath(_ sessionID: String) throws -> String {
        guard let workspaceID = cloudWorkspaceID,
              UUID(uuidString: workspaceID) != nil,
              !sessionID.isEmpty, sessionID.count <= 512 else {
            throw RelayAPIError(message: "Sign in again to view session summaries")
        }
        let allowed = CharacterSet.urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))
        let encoded = sessionID.addingPercentEncoding(withAllowedCharacters: allowed) ?? sessionID
        return "/workspaces/\(workspaceID)/reflex/sessions/\(encoded)"
    }
    private func summaryPath(_ sessionID: String) throws -> String {
        try sessionPath(sessionID) + "/analysis"
    }
    func loadGoal(for agent: RelayAgent) async {
        let id = agent.sessionID
        guard !id.isEmpty, canLoadSummaries, goals[id] == nil, !loadingGoalIDs.contains(id) else { return }
        loadingGoalIDs.insert(id)
        defer { loadingGoalIDs.remove(id) }
        do {
            let response = try await authorizedCloudCall(try sessionPath(id) + "/events")
            let data = try JSONSerialization.data(withJSONObject: response)
            goals[id] = try SessionGoal.decode(data, sessionID: id) ?? ""
        } catch { /* The generated summary and published objective still work without History events. */ }
    }
    func loadSummary(for agent: RelayAgent) async {
        let id = agent.sessionID
        guard !id.isEmpty, canLoadSummaries, !loadingSummaryIDs.contains(id) else { return }
        loadingSummaryIDs.insert(id)
        defer { loadingSummaryIDs.remove(id) }
        let previous = summaries[id]
        if previous == nil { summaries[id] = SessionOverview(status: .loading) }
        do {
            let data = try await authorizedCloudCall(summaryPath(id))
            let raw = try JSONSerialization.data(withJSONObject: data)
            summaries[id] = try SessionOverview.decode(raw, sessionID: id)
            sessionsMissingFromHistory.remove(id)
        } catch let failure as RelayAPIError
                    where AgentOverviewState.isMissingFromHistory(status: failure.status, code: failure.code) {
            // Not an error to show in red: the session was never shared with
            // History. A brief from an earlier load is kept.
            summaries[id] = previous?.hasBrief == true ? previous : nil
            sessionsMissingFromHistory.insert(id)
        } catch {
            summaries[id] = SessionOverview(status: .failed, title: previous?.title ?? "",
                                            description: previous?.description ?? "", error: UserFacingError.message(for: error))
        }
    }
    private var canLoadRowSummaries: Bool {
        #if DEBUG
        if RelayUITestFixture.rowSummariesEnabled { return true }
        #endif
        return canLoadSummaries && !signingOut
    }
    /// A list row showing `agent` came on screen: its session's brief is
    /// loaded if it is missing or old (`AgentRowSummaryLoader`).
    /// A person's or channel's row shows no session lines, so it loads none.
    func rowAppeared(_ agent: RelayAgent) {
        let showsSession = !agent.isHuman && agent.source != .channel
        rowSummaryLoader.rowAppeared(agent.id, sessionID: showsSession ? agent.sessionID : "")
    }
    func rowDisappeared(_ agent: RelayAgent) {
        rowSummaryLoader.rowDisappeared(agent.id)
    }
    /// Loads what the visible rows still need, and again what has aged out.
    func refreshRowSummaries() {
        rowSummaryLoader.refresh()
    }
    private func resetRowSummaries() {
        rowSummaryLoader.reset()
        rowSummaries = [:]
    }
    /// One session's brief for its row, from the analysis the agent overview
    /// reads. Nil when the load failed: the row keeps what it had and the
    /// failure is not shown. A session Cloud History has no copy of has no
    /// brief, which is an answer, not a failure.
    private func fetchRowSummary(_ id: String) async -> AgentRowSummary? {
        #if DEBUG
        if RelayUITestFixture.enabled {
            return RelayUITestFixture.rowSummaryResponse(sessionID: id).flatMap { Self.rowSummary(from: $0, sessionID: id) }
        }
        #endif
        if let loaded = summaries[id], loaded.hasBrief {
            return AgentRowSummary(title: loaded.title, line: loaded.description)
        }
        if sessionsMissingFromHistory.contains(id) { return AgentRowSummary() }
        let workspaceID = cloudWorkspaceID
        let generation = sessionGeneration
        do {
            let response = try await authorizedCloudCall(summaryPath(id))
            guard cloudWorkspaceID == workspaceID, sessionGeneration == generation else { return nil }
            return Self.rowSummary(from: response, sessionID: id)
        } catch let failure as RelayAPIError
                    where AgentOverviewState.isMissingFromHistory(status: failure.status, code: failure.code) {
            return AgentRowSummary()
        } catch {
            return nil
        }
    }
    static func rowSummary(from response: JSONObject, sessionID: String) -> AgentRowSummary? {
        guard let raw = try? JSONSerialization.data(withJSONObject: response),
              let overview = try? SessionOverview.decode(raw, sessionID: sessionID) else { return nil }
        return AgentRowSummary(title: overview.title, line: overview.description)
    }
    /// What the list row for `agent` shows.
    func rowModel(for agent: RelayAgent, now: Date = Date()) -> AgentRowModel {
        let id = agent.sessionID
        // Only a fleet record states a model.
        let model = agent.model.isEmpty
            ? fleetAgents.first { $0.name == agent.name && !$0.model.isEmpty }?.model ?? ""
            : agent.model
        let loaded = AgentRowModel.Loaded(
            displayName: displayName(for: agent),
            cli: displayCLI(for: agent),
            model: model,
            overview: id.isEmpty ? nil : summaries[id],
            rowSummary: id.isEmpty ? nil : rowSummaries[id],
            recordedSummary: id.isEmpty ? nil : sessionSummaryState(for: id),
            goal: id.isEmpty ? "" : goals[id] ?? "",
            unread: agent.source == .conversation && unreadConversationIDs.contains(agent.id))
        return AgentRowModel(agent: agent, loaded: loaded, now: now)
    }
    /// Keeps what the summary list reported for a session: its revision and
    /// its canonical stored source.
    func noteSummarySearchResult(_ result: SummarySearchResult) {
        let id = result.sessionID
        guard !id.isEmpty else { return }
        if let revision = result.revision, !revision.isEmpty { sessionRevisions[id] = revision }
        sessionSources[id] = SessionSummaryRequest.usableSource(result.source)
    }
    /// The hosted summary to show; nil (section hidden) while it loads or
    /// when the session has no source.
    func sessionSummaryState(for sessionID: String) -> SessionSummaryState? {
        sessionSources[sessionID] == nil ? nil : sessionSummaries[sessionID]?.state
    }
    /// Loads the session's hosted summary unless the cached copy is still
    /// current. Asks for the long summary and falls back to the short one.
    /// Without a source from the summary list nothing is fetched and the
    /// section stays hidden.
    func loadSessionSummary(for agent: RelayAgent, force: Bool = false) async {
        let id = agent.sessionID
        guard !id.isEmpty, canLoadSummaries, let workspaceID = cloudWorkspaceID,
              let source = sessionSources[id] else { return }
        let revision = sessionRevisions[id]
        if !force, sessionSummaries[id]?.isFresh(revision: revision) == true { return }
        // A load already running for the session is awaited, not turned
        // away, and it stores its result even if this caller is cancelled.
        await sessionSummaryLoads.run(id) { [weak self] in
            guard let self else { return }
            let state = await self.fetchSessionSummary(id, workspaceID: workspaceID, source: source)
            guard self.cloudWorkspaceID == workspaceID else { return }
            self.sessionSummaries[id] = SessionSummaryCacheEntry(state: state, revision: revision, fetchedAt: Date())
        }
    }
    private func fetchSessionSummary(_ id: String, workspaceID: String, source: String) async -> SessionSummaryState {
        var kind = SessionSummary.Kind.long
        var backoffSeconds = 1
        while true {
            guard let path = SessionSummaryRequest.path(sessionID: id, workspaceID: workspaceID,
                                                        source: source, kind: kind) else { return .hidden }
            do {
                return SessionSummaryState(try SessionSummary(try await authorizedCloudCall(path)))
            } catch let failure as RelayAPIError {
                guard let state = SessionSummaryState.afterFailure(status: failure.status, code: failure.code,
                                                                   requested: kind) else {
                    kind = .short
                    continue
                }
                if SessionSummaryState.retriesAutomatically(status: failure.status), backoffSeconds <= 4 {
                    try? await Task.sleep(for: .seconds(backoffSeconds))
                    if Task.isCancelled { return state }
                    backoffSeconds *= 2
                    continue
                }
                return state
            } catch {
                return .unavailable
            }
        }
    }
    func requestSummary(for agent: RelayAgent) async {
        let id = agent.sessionID
        guard !id.isEmpty, canLoadSummaries, summaries[id]?.pending != true else { return }
        do {
            _ = try await authorizedCloudCall(summaryPath(id), method: "POST")
            summaries[id] = SessionOverview(status: .queued, title: summaries[id]?.title ?? "",
                                            description: summaries[id]?.description ?? "")
            await loadSummary(for: agent)
        } catch let failure as RelayAPIError where failure.status == 429 {
            summaries[id] = SessionOverview(status: .failed, title: summaries[id]?.title ?? "",
                                            description: summaries[id]?.description ?? "",
                                            error: "The daily summary limit has been reached. Try again tomorrow.")
        } catch let failure as RelayAPIError
                    where AgentOverviewState.isMissingFromHistory(status: failure.status, code: failure.code) {
            if summaries[id]?.hasBrief != true { summaries[id] = nil }
            sessionsMissingFromHistory.insert(id)
        } catch {
            summaries[id] = SessionOverview(status: .failed, title: summaries[id]?.title ?? "",
                                            description: summaries[id]?.description ?? "", error: UserFacingError.message(for: error))
        }
    }
    // MARK: Session events

    /// Whether the chat can show `agent`'s stored session: it has a session
    /// id and Cloud access is approved.
    func sessionEventsAvailable(for agent: RelayAgent) -> Bool {
        guard !agent.sessionID.isEmpty, agent.source != .channel, !agent.isHuman else { return false }
        if sessionEventFetch != nil { return true }
        #if DEBUG
        if SessionEventsFixture.covers(agent.sessionID) { return true }
        #endif
        return canLoadSummaries
    }

    /// Whether the first read of `agent`'s session finished, well or not.
    func sessionEventsSettled(for agent: RelayAgent) -> Bool {
        guard let store = sessionEvents[agent.sessionID] else { return false }
        return store.loaded || store.failures > 0
    }

    /// How long the open chat waits before reading the newest page again,
    /// or nil for no timer. `chatActivityAt`: the chat's newest message,
    /// sent or received.
    func sessionEventPollDelay(for agent: RelayAgent, chatActivityAt: Date? = nil, now: Date = Date()) -> TimeInterval? {
        let store = sessionEvents[agent.sessionID]
        return SessionEventPolling.interval(newestEventAt: store?.newest?.ts, chatActivityAt: chatActivityAt, now: now,
                                            failures: store?.failures ?? 0, unavailable: store?.unavailable ?? false,
                                            unknownNotImplemented: store?.unknownNotImplemented ?? 0)
    }

    /// How long to wait before the next newest-page read of `agent`'s
    /// session keeps reads `SessionEventPolling.minimumSpacing` apart; 0 when
    /// it can read now.
    func sessionEventReadWait(for agent: RelayAgent, now: Date = Date()) -> TimeInterval {
        guard let last = sessionEventLastRead[agent.sessionID] else { return 0 }
        return max(0, SessionEventPolling.minimumSpacing - now.timeIntervalSince(last))
    }

    /// Reads the session's newest page and merges it. One read runs per
    /// session; a call while one runs waits for it instead of starting
    /// another. A failure keeps what is shown. Nothing once the server said
    /// the session is unavailable.
    func refreshSessionEvents(for agent: RelayAgent) async {
        await readSessionEvents(agent.sessionID, available: sessionEventsAvailable(for: agent), older: false) { _ in
            SessionEventsRequest.newest
        }
    }

    /// Reads the next older page, or the newest page again after the server
    /// dropped the cursor. Nothing once the start was reached or the
    /// session is unavailable.
    func loadOlderSessionEvents(for agent: RelayAgent) async {
        await readSessionEvents(agent.sessionID, available: sessionEventsAvailable(for: agent), older: true) { store in
            store.olderRequest
        }
    }

    private func readSessionEvents(_ id: String, available: Bool, older: Bool,
                                   request: @escaping (SessionEventStore) -> SessionEventsRequest.Spec?) async {
        guard available, !id.isEmpty else { return }
        // Wait for a read already running for this session: one in flight.
        while let running = sessionEventReads[id] {
            await running.task.value
            if Task.isCancelled { return }
        }
        // The server's answer was final: no read until the store is dropped.
        if sessionEvents[id]?.unavailable == true { return }
        let generation = sessionEventsGeneration
        let workspaceID = cloudWorkspaceID
        let cacheWorkspace = workspaceID ?? ""
        let token = UUID()
        // Not a child of the caller: a chat closed mid-read still stores what
        // it read. It clears its own in-flight entry as its last step, so a
        // read waiting on it never sees a finished one.
        let read = Task { @MainActor [weak self] in
            guard let self else { return }
            defer { if self.sessionEventReads[id]?.token == token { self.sessionEventReads[id] = nil } }
            if self.sessionEvents[id] == nil {
                // The cached copy shows first; the network read merges into it.
                let restored = await self.sessionEventSnapshots.load(workspaceID: cacheWorkspace, sessionID: id)
                guard self.sessionEventsGeneration == generation else { return }
                if self.sessionEvents[id] == nil {
                    self.sessionEvents[id] = restored.flatMap { $0.sessionID == id ? SessionEventStore(snapshot: $0) : nil }
                        ?? SessionEventStore(sessionID: id)
                }
            }
            guard let store = self.sessionEvents[id], !store.unavailable, let spec = request(store) else { return }
            if !older { self.sessionEventLastRead[id] = Date() }
            do {
                let page = try await self.fetchSessionEventPage(id, workspaceID: workspaceID, spec: spec)
                guard self.sessionEventsGeneration == generation else { return }
                guard var updated = self.sessionEvents[id] else { return }
                let changed = updated.apply(page, for: spec)
                // An unchanged poll publishes nothing, so the open chat is not re-rendered.
                if updated != self.sessionEvents[id] { self.sessionEvents[id] = updated }
                if changed { self.sessionEventSnapshots.save(updated.snapshot, workspaceID: cacheWorkspace) }
            } catch {
                guard self.sessionEventsGeneration == generation else { return }
                let failure = error as? RelayAPIError
                self.sessionEvents[id]?.failed(status: failure?.status, code: failure?.code ?? "",
                                               reason: failure?.reason ?? "", older: older)
                if SessionEventsRequest.accessRevoked(status: failure?.status) {
                    // This account may no longer read it: off the screen and the disk.
                    self.sessionEvents[id]?.forgetEvents()
                    self.sessionEventSnapshots.remove(workspaceID: cacheWorkspace, sessionID: id)
                } else if SessionEventsRequest.sessionGone(status: failure?.status) {
                    // Gone from the server: forget the cached copy, keep what is shown.
                    self.sessionEventSnapshots.remove(workspaceID: cacheWorkspace, sessionID: id)
                }
            }
        }
        sessionEventReads[id] = SessionEventRead(token: token, task: read)
        await read.value
    }

    private func fetchSessionEventPage(_ id: String, workspaceID: String?,
                                       spec: SessionEventsRequest.Spec) async throws -> SessionEventPage {
        if let sessionEventFetch { return try await sessionEventFetch(id, spec) }
        #if DEBUG
        if SessionEventsFixture.covers(id) {
            if SessionEventsFixture.delaysFirstPage, sessionEvents[id]?.loaded != true {
                try await Task.sleep(for: .seconds(2))
            }
            guard let data = SessionEventsFixture.response(sessionID: id, spec: spec) else {
                throw RelayAPIError(message: "No fixture events")
            }
            return try SessionEventPage.decode(data, sessionID: id)
        }
        #endif
        guard let workspaceID, let path = SessionEventsRequest.path(workspaceID: workspaceID, sessionID: id, spec: spec) else {
            throw RelayAPIError(message: "Sign in again to view this session")
        }
        let response = try await authorizedCloudCall(path)
        return try SessionEventPage.decode(try JSONSerialization.data(withJSONObject: response), sessionID: id)
    }

    /// Starts a new identity session: work from the previous one is
    /// invalidated and its session events, in memory and on disk, forgotten,
    /// even when the workspace id stays the same.
    func advanceSessionGeneration() {
        sessionGeneration += 1
        resetSessionEvents()
    }

    /// Forgets every session's events, in memory and on disk, and any read in flight.
    private func resetSessionEvents() {
        sessionEventsGeneration += 1
        sessionEventReads.values.forEach { $0.task.cancel() }
        sessionEventReads = [:]
        sessionEventLastRead = [:]
        sessionEvents = [:]
        sessionEventSnapshots.removeAll()
    }

    func selectConversation(_ agent: RelayAgent?) {
        let id = agent?.id
        guard selectedConversation != id else { return }
        selectedConversation = id
        messages = []
        loadedHistoryConversation = nil
        historyCheckedConversation = nil
        refreshNotice = nil
    }
    /// Whether `messages` holds the history of `agent`'s conversation, read
    /// since it was opened. Until then, an empty or partial list says nothing
    /// about which messages already exist.
    func historyLoaded(for agent: RelayAgent) -> Bool {
        selectedConversation == agent.id && loadedHistoryConversation == agent.id
    }
    /// A failure shows as the inline `refreshNotice`, never the alert.
    /// Push-driven reloads pass `reportsErrors: false` so a flaky
    /// connection shows nothing the person didn't ask for.
    func loadHistory(for agent: RelayAgent, reportsErrors: Bool = true) async {
        guard selectedConversation == agent.id else { return }
        defer { if selectedConversation == agent.id { historyCheckedConversation = agent.id } }
        let ticket = connectionReports.begin(.history)
        do {
            let path: String
            var conversationID: String?
            if agent.source == .channel {
                path = "/channels/\(RelayChannels.pathComponent(agent.name))/messages?limit=100"
            } else {
                guard let id = conversationIDs[agent.name], !id.isEmpty else {
                    if selectedConversation == agent.id {
                        messages = []
                        loadedHistoryConversation = nil
                    }
                    return
                }
                conversationID = id
                path = "/dm/\(id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id)/messages?limit=100"
            }
            let result = try await api(path, agent: true)
            guard selectedConversation == agent.id else { return }
            messages = objects(result).reversed().enumerated().map { RelayMessage($0.element, index: $0.offset) }
            loadedHistoryConversation = agent.id
            if agent.source == .channel { await loadChannelSenders(in: messages) }
            error = nil
            refreshNotice = nil
            connectionReports.succeeded(ticket)
            if let id = conversationID { await markOpenConversationRead(agent, conversationID: id) }
        } catch {
            if reportsErrors, selectedConversation == agent.id {
                reportConnectionFailure(error, from: ticket, channel: agent.source == .channel)
            }
        }
        await refreshDelivery(for: agent)
    }
    /// What a read of an agent's DM history found.
    enum DirectHistory {
        /// The thread's messages, oldest first.
        case messages([RelayMessage])
        /// The agent has no DM thread with this device yet.
        case noThread
        /// The thread list or the messages could not be read.
        case failed
    }

    /// The DM history with an agent, oldest first, read without changing the
    /// open conversation, unread state or alerts. Voice calls read replies
    /// with it so opening another conversation cannot redirect them. The
    /// thread is looked up in the relay's conversation list when the inbox
    /// has not filled the cache yet (a call to an agent never messaged from
    /// this device, or a cold start), and remembered once found, so "no
    /// thread" is only ever said after the relay was asked.
    func directHistory(with agent: RelayAgent, startingCall: Bool = false, session started: Int? = nil) async -> DirectHistory {
        // The call's own session, captured when it started, or this one before any await: a sign-out
        // or workspace switch since then ends the lookup rather than letting it read or touch the new
        // session for the old call.
        let session = started ?? sessionGeneration
        let current = { [weak self] in
            guard let self else { return false }
            return VoiceDirectThread.stillCurrent(started: session, now: self.sessionGeneration, paired: self.paired, signingOut: self.signingOut)
        }
        // A call from an earlier session touches nothing here: no cache, no miss, no request.
        guard current() else { return .failed }
        let cached = conversationIDs[agent.name]
        var threads: [JSONObject]?
        if cached?.isEmpty != false {
            // A call that starts asks the relay afresh; between checks the last answer holds.
            if startingCall { threadMisses.forget(agent.name) }
            switch threadMisses.hold(agent.name) {
            case .none?: return .noThread
            case .unresolved?: return .failed
            case nil: break
            }
            threads = try? objects(await api("/dm/conversations", agent: true))
            guard current() else { return .failed }
        }
        // Read after the list: a credential refresh during the read may have renamed this device.
        let mine = messagingHandle
        let id: String
        switch VoiceDirectThread.resolve(cached: cached, threads: threads, peer: agent.name, mine: mine) {
        case .failed:
            // A list that was read but could not be resolved holds too, so the read is not repeated every check.
            if threads != nil { threadMisses.record(unresolved: agent.name) }
            return .failed
        case .noThread:
            threadMisses.record(miss: agent.name)
            return .noThread
        case .id(let found):
            conversationIDs[agent.name] = found
            threadMisses.found(agent.name)
            id = found
        }
        let path = "/dm/\(id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? id)/messages?limit=100"
        guard let result = try? await api(path, agent: true), current() else { return .failed }
        return .messages(objects(result).reversed().enumerated().map { RelayMessage($0.element, index: $0.offset) })
    }
    /// Checks whether the latest outgoing message in the open conversation has
    /// reached its recipient. Returns true once it has.
    @discardableResult
    func refreshDelivery(for agent: RelayAgent) async -> Bool {
        guard selectedConversation == agent.id,
              let message = messages.last(where: \.mine), message.hasServerID else { return false }
        if deliveredMessageIDs.contains(message.id) { return true }
        let id = message.id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? message.id
        guard let result = try? await api("/messages/\(id)/readers", agent: true) else { return false }
        guard MessageDelivery.delivered(readers: objects(result), sender: message.sender) else { return false }
        deliveredMessageIDs.insert(message.id)
        return true
    }
    /// Only server metadata can describe a handoff; viewing has no side effect.
    func handoffNotice(for agent: RelayAgent) -> MobileHandoffNotice? {
        (conversationAgentMetadata[agent.name] ?? agent).mobileHandoff
    }
    /// The notice to show: nil once the person dismissed this exact failure.
    func visibleHandoffNotice(for agent: RelayAgent) -> MobileHandoffNotice? {
        let current = conversationAgentMetadata[agent.name] ?? agent
        guard let notice = current.mobileHandoff else { return nil }
        let identity = MobileHandoffDismissals.identity(name: current.name, sessionID: current.sessionID)
        return handoffDismissals.hides(notice, for: identity) ? nil : notice
    }
    /// Hides the current failed notice; it never resends anything.
    func dismissHandoffNotice(for agent: RelayAgent) {
        let current = conversationAgentMetadata[agent.name] ?? agent
        guard let notice = current.mobileHandoff else { return }
        handoffDismissals.dismiss(notice, for: MobileHandoffDismissals.identity(name: current.name, sessionID: current.sessionID))
    }
    /// Called when a send to the agent starts; it never resends anything.
    func forgetHandoffDismissal(for agent: RelayAgent) {
        let current = conversationAgentMetadata[agent.name] ?? agent
        handoffDismissals.forget(MobileHandoffDismissals.identity(name: current.name, sessionID: current.sessionID))
    }
    func nativeContinuationNotice(for agent: RelayAgent) -> String? {
        (conversationAgentMetadata[agent.name] ?? agent).nativeContinuationNotice
    }
    func deliveryLabel(for message: RelayMessage, to agent: RelayAgent) -> String {
        let current = conversationAgentMetadata[agent.name] ?? agent
        return current.deliveryLabel(delivered: deliveredMessageIDs.contains(message.id))
    }
    func send(_ text: String, to agent: RelayAgent) async -> Bool {
        if case .accepted = await send(text, attachments: [], to: agent) { return true }
        return false
    }
    /// POSTs one message once. `idempotencyKey` names the person's send: a
    /// deliberate retry passes the same key, so Relaycast can recognise a
    /// message it already applied. Nothing here sends it again on its own.
    /// Returns as soon as Relaycast answers; history and delivery are then
    /// refreshed in the background, so a slow refresh cannot hold the
    /// composer's bubble in "Sending…".
    func send(_ text: String, attachments: [RelayAttachment], to agent: RelayAgent,
              idempotencyKey: String = UUID().uuidString) async -> RelaySendOutcome {
        // Every send reaches here (composer, failed-bubble retry, voice), so a
        // new attempt shows its handoff failure even when the detail repeats.
        forgetHandoffDismissal(for: agent)
        return await RelaySendFlow.run(post: {
            if agent.source == .channel {
                // Posting needs no membership, but only members are sent the
                // channel's later messages, so the first post joins it.
                let session = sessionGeneration
                if !joinedChannels.contains(agent.name) { try await join(agent.name, session: session) }
                try checkSession(session)
                var body: JSONObject = ["text": text]
                if !attachments.isEmpty { body["attachments"] = attachments.map(\.id) }
                return try await api("/channels/\(RelayChannels.pathComponent(agent.name))/messages",
                                     agent: true, method: "POST", body: body, idempotencyKey: idempotencyKey)
            }
            var body: JSONObject = ["to": agent.name, "text": text]
            if !attachments.isEmpty { body["attachments"] = attachments.map(\.id) }
            let response = try await api("/dm", agent: true, method: "POST", body: body, idempotencyKey: idempotencyKey)
            // The first message starts the thread: a voice call may look it up again now.
            threadMisses.sent(to: agent.name)
            return response
        }, onFailure: { error in
            self.reportFailure(error)
            // The POST may still have been applied; history decides, never a re-send.
            Task { if self.selectedConversation == agent.id { await self.loadHistory(for: agent, reportsErrors: false) } }
        }, afterAcceptance: { [weak self] in
            guard let self else { return }
            #if DEBUG
            MobileHandoffFixture.trace("post accepted; refreshing")
            #endif
            await self.refresh()
            #if DEBUG
            MobileHandoffFixture.trace("refresh finished; loading history")
            #endif
            if self.selectedConversation == agent.id { await self.loadHistory(for: agent) }
            #if DEBUG
            MobileHandoffFixture.trace("history finished; send completed")
            #endif
            // Delivery usually follows within seconds; check briefly, then
            // leave it to the regular history refresh.
            for _ in 0..<10 where self.selectedConversation == agent.id {
                if await self.refreshDelivery(for: agent) { break }
                try? await Task.sleep(for: .seconds(3))
            }
        })
    }
    private func checkSession(_ generation: Int) throws {
        guard generation == sessionGeneration, paired, !signingOut else { throw CancellationError() }
    }
    func uploadAttachment(from url: URL) async throws -> RelayAttachment {
        let generation = sessionGeneration
        let hasScopedAccess = url.startAccessingSecurityScopedResource()
        defer { if hasScopedAccess { url.stopAccessingSecurityScopedResource() } }

        let values = try url.resourceValues(forKeys: [.fileSizeKey, .nameKey])
        let fileSize = values.fileSize ?? ((try? FileManager.default.attributesOfItem(atPath: url.path)[.size]) as? NSNumber)?.intValue ?? 0
        guard fileSize > 0 else { throw RelayAPIError(message: "This file is empty and can’t be attached") }
        let filename = values.name ?? url.lastPathComponent
        let contentType = UTType(filenameExtension: url.pathExtension)?.preferredMIMEType ?? "application/octet-stream"
        let reservation = object(try await api("/files/upload", agent: true, method: "POST", body: [
            "filename": filename,
            "content_type": contentType,
            "size_bytes": fileSize
        ]))
        try checkSession(generation)
        let fileID = string(reservation["id"])
        guard let uploadURL = URL(string: string(reservation["upload_url"])),
              uploadURL.scheme?.lowercased() == "https", !fileID.isEmpty else {
            throw RelayAPIError(message: "Relaycast did not return a valid upload URL")
        }

        var request = URLRequest(url: uploadURL)
        request.httpMethod = "PUT"
        request.timeoutInterval = 180
        request.setValue(contentType, forHTTPHeaderField: "Content-Type")
        let (_, response) = try await URLSession.shared.upload(for: request, fromFile: url)
        guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
            let status = (response as? HTTPURLResponse)?.statusCode ?? 0
            throw RelayAPIError(message: "The file could not be uploaded. Try again.", status: status)
        }
        try checkSession(generation)

        let allowed = CharacterSet.urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))
        let encodedID = fileID.addingPercentEncoding(withAllowedCharacters: allowed) ?? fileID
        _ = try await api("/files/\(encodedID)/complete", agent: true, method: "POST")
        try checkSession(generation)
        return RelayAttachment(fileID: fileID, filename: filename, contentType: contentType, sizeBytes: fileSize)
    }
    func downloadURL(for attachment: RelayAttachment) async throws -> URL {
        let generation = sessionGeneration
        let allowed = CharacterSet.urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))
        let encodedID = attachment.id.addingPercentEncoding(withAllowedCharacters: allowed) ?? attachment.id
        let result = object(try await api("/files/\(encodedID)", agent: true))
        try checkSession(generation)
        guard let url = URL(string: string(result["download_url"])),
              url.scheme?.lowercased() == "https" else {
            throw RelayAPIError(message: "A download link isn’t available for this file")
        }
        return url
    }
    /// Starts a worker on an online node and waits for the node's readiness
    /// receipt. An unconfirmed result means the request was accepted but its
    /// final status could not be observed yet; callers must not submit it again.
    func spawnAgent(on node: RelaySpawnNode, harness: String, name: String,
                    task: String, workingDirectory: String, projectName: String? = nil,
                    forkFrom: String? = nil) async throws -> SpawnOutcome {
        let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let task = task.trimmingCharacters(in: .whitespacesAndNewlines)
        guard node.harnesses.contains(harness),
              name.range(of: "^[a-z][a-z0-9-]{1,47}$", options: .regularExpression) != nil,
              !task.isEmpty || forkFrom != nil else {
            throw RelayAPIError(message: "Choose an agent type, enter a valid name, and describe its task")
        }
        let allowed = CharacterSet.urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))
        let encodedNode = node.name.addingPercentEncoding(withAllowedCharacters: allowed) ?? node.name
        var input: JSONObject = [
            "capability": "spawn:\(harness)", "cli": harness, "node": node.name,
            "target_node": node.name, "name": name, "task": task, "verify_ready": true
        ]
        let folder = workingDirectory.trimmingCharacters(in: .whitespacesAndNewlines)
        if !folder.isEmpty {
            guard folder.hasPrefix("/") else { throw RelayAPIError(message: "Working folder must be an absolute path") }
            input["worker_cwd"] = folder
        }
        if let projectName = projectName?.trimmingCharacters(in: .whitespacesAndNewlines), !projectName.isEmpty {
            input["project"] = projectName
        }
        if let forkFrom {
            guard node.forkHarnesses.contains(harness) else {
                throw RelayAPIError(message: "\(node.name) can't fork \(harness) sessions. Update Agent Relay on it, then try again.")
            }
            input["fork_from"] = forkFrom
        }
        let accepted: JSONObject
        do {
            accepted = object(try await api("/nodes/\(encodedNode)/actions/spawn/invoke",
                                            agent: true, method: "POST", body: ["input": input]))
        } catch let error as RelayAPIError {
            throw SpawnFailure.error(error.message, node: node.name, name: name, harness: harness,
                                     status: error.status, code: error.code, fromServer: error.fromServer)
        } catch {
            // A server rejection keeps its HTTP status above. Anything else
            // means the reply never arrived: a fork must not be sent again.
            guard forkFrom != nil else { throw error }
            throw SessionFork.unanswered(error)
        }
        let invocationID = string(accepted["invocation_id"])
        guard !invocationID.isEmpty else {
            if forkFrom != nil {
                throw RelayAPIError(
                    message: "The fork may have started, but no receipt came back. Check Agents before trying again.",
                    code: SessionFork.uncertainSubmission)
            }
            throw RelayAPIError(message: "The start request did not return a receipt. Check Agents before trying again.")
        }
        let encodedID = invocationID.addingPercentEncoding(withAllowedCharacters: allowed) ?? invocationID
        // Some nodes start the worker but never complete the receipt, so the
        // node's live-agent list also counts as proof that the agent is up.
        for attempt in 1...60 {
            try await Task.sleep(for: .seconds(2))
            if attempt % 5 == 0 {
                await refresh()
                if fleetAgents.contains(where: { $0.name == name && $0.node == node.name }) { return .running }
            }
            let receipt: JSONObject
            do {
                receipt = object(try await api("/actions/spawn/invocations/\(encodedID)", agent: true))
            } catch {
                // A dropped poll does not mean the start failed; keep waiting.
                continue
            }
            switch string(receipt["status"]) {
            case "completed":
                let output = object(receipt["output"])
                guard output["spawned"] as? Bool == true,
                      output["ready"] as? Bool != false else {
                    throw SpawnFailure.error(string(output["error"]), node: node.name, name: name, harness: harness)
                }
                await refresh()
                return .ready
            case "failed", "cancelled", "canceled":
                throw SpawnFailure.error(string(receipt["error"]), node: node.name, name: name, harness: harness)
            default: break
            }
        }
        await refresh()
        if fleetAgents.contains(where: { $0.name == name && $0.node == node.name }) { return .running }
        return .unconfirmed
    }
    /// Whether `agent` runs on its computer now, by the latest roster and node
    /// lists. Fork and Call are offered only for live agents.
    func isLive(_ agent: RelayAgent) -> Bool {
        SessionFork.isLive(agent, roster: relayAgents, fleet: fleetAgents)
    }
    func forkAvailability(for agent: RelayAgent) -> SessionFork.Availability {
        SessionFork.availability(for: agent, nodes: forkNodes, live: isLive(agent),
                                 roster: relayAgents, spawned: spawnedSessions)
    }
    /// The harness an agent's icon shows (`SessionFork.displayCLI`).
    func displayCLI(for agent: RelayAgent) -> String {
        SessionFork.displayCLI(for: agent, roster: relayAgents, fleet: fleetAgents, spawned: spawnedSessions)
    }
    /// Starts `name` on the agent's own computer as a copy of its session.
    /// A node that doesn't hold the agent sends the request to the next
    /// installation on that machine; any other failure ends the attempt.
    func forkAgent(_ source: RelayAgent, name: String, task: String) async throws -> (SpawnOutcome, RelaySpawnNode) {
        guard case let .available(harness, nodes) = forkAvailability(for: source) else {
            throw RelayAPIError(message: "This session can't be forked right now. Refresh and try again.")
        }
        var lastError: Error?
        for node in nodes {
            do {
                let outcome = try await spawnAgent(on: node, harness: harness, name: name, task: task,
                                                   workingDirectory: "", forkFrom: source.name)
                return (outcome, node)
            } catch where SessionFork.triesNextNode(after: error) {
                lastError = error
            }
        }
        throw lastError ?? RelayAPIError(message: "No computer could fork \(source.name).")
    }
    /// Stops a broker-owned worker through the node that reported it as live.
    /// A request without a final receipt is left unconfirmed so the caller
    /// does not claim success or automatically submit a duplicate release.
    func releaseAgent(_ agent: RelayAgent) async -> Bool {
        guard agent.source == .drive, !agent.node.isEmpty, !agent.name.isEmpty else {
            error = "Only a live agent with a known node can be released here"
            return false
        }
        do {
            let allowed = CharacterSet.urlPathAllowed.subtracting(CharacterSet(charactersIn: "/"))
            let node = agent.node.addingPercentEncoding(withAllowedCharacters: allowed) ?? agent.node
            let accepted = object(try await api("/nodes/\(node)/actions/release/invoke", agent: true,
                                                method: "POST", body: ["input": ["name": agent.name]]))
            let invocationID = string(accepted["invocation_id"])
            guard !invocationID.isEmpty else {
                throw RelayAPIError(message: "Release request had no receipt. Check Agents before trying again.")
            }
            let encodedID = invocationID.addingPercentEncoding(withAllowedCharacters: allowed) ?? invocationID
            for _ in 0..<30 {
                try await Task.sleep(for: .seconds(2))
                let receipt: JSONObject
                do {
                    receipt = object(try await api("/actions/release/invocations/\(encodedID)", agent: true))
                } catch {
                    throw RelayAPIError(message: "Release was requested, but its result is not available yet. Check Agents before trying again.")
                }
                switch string(receipt["status"]) {
                case "completed":
                    let output = object(receipt["output"])
                    guard output["released"] as? Bool == true else {
                        throw RelayAPIError(message: string(output["error"]), fromServer: true, summary: "The agent did not stop")
                    }
                    await refresh()
                    return true
                case "failed", "cancelled", "canceled":
                    throw RelayAPIError(message: string(receipt["error"]), fromServer: true, summary: "The agent did not stop")
                default: break
                }
            }
            throw RelayAPIError(message: "Release was requested, but its result is not available yet. Check Agents before trying again.")
        } catch {
            reportFailure(error)
            return false
        }
    }
    func attach(_ agent: RelayAgent, mode: String) async {
        guard agent.attachable else { error = "This agent is not available for attach"; return }
        do {
            await closeTerminal()
            let node = agent.node.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? agent.node
            let result = try await api("/nodes/\(node)/terminal/sessions", method: "POST", body: ["agent": agent.name, "mode": mode])
            let data = object(result)
            let id = string(data["session_id"])
            guard var components = URLComponents(string: string(data["terminal_url"])),
                  ["wss", "https"].contains(components.scheme ?? ""),
                  components.host != nil, !id.isEmpty else {
                throw RelayAPIError(message: "Relaycast did not return a terminal session")
            }
            // Relaycast tickets use an HTTPS URL; URLSession requires the WebSocket scheme.
            if components.scheme == "https" { components.scheme = "wss" }
            guard let url = components.url else { throw RelayAPIError(message: "Invalid terminal URL") }
            attachedID = id
            attachedMode = mode
            terminalEnded = false
            terminalBacklog = ""
            screenHistory = []
            lastScreenCapture = .distantPast
            terminalSessionID = id
            let socket = URLSession.shared.webSocketTask(with: url)
            terminalSocket = socket
            socket.resume()
            terminalTask = Task { await streamTerminal(socket, id: id) }
        } catch { reportFailure(error) }
    }
    private func streamTerminal(_ socket: URLSessionWebSocketTask, id: String) async {
        do {
            while !Task.isCancelled && attachedID == id {
                let incoming = try await socket.receive()
                let raw: Data
                switch incoming {
                case .string(let text): raw = Data(text.utf8)
                case .data(let data): raw = data
                @unknown default: continue
                }
                guard let frame = (try? JSONSerialization.jsonObject(with: raw)) as? JSONObject,
                      string(frame["session_id"]) == id else { continue }
                switch string(frame["type"]) {
                case "terminal.ready", "terminal.snapshot":
                    let data = Data(base64Encoded: string(frame["screen"])) ?? Data()
                    if let output = String(data: data, encoding: .utf8), !output.isEmpty {
                        terminalBacklog = output
                        onTerminalOutput?(output)
                    }
                case "terminal.output":
                    let output = string(frame["chunk"])
                    terminalBacklog = String((terminalBacklog + output).suffix(200_000))
                    onTerminalOutput?(output)
                case "terminal.closed":
                    terminalEnded = true
                    return
                default: break
                }
            }
        } catch {
            if !Task.isCancelled && attachedID == id {
                terminalEnded = true
                reportFailure(error)
            }
        }
    }
    private func socketSend(_ frame: JSONObject) async {
        guard let socket = terminalSocket,
              let data = try? JSONSerialization.data(withJSONObject: frame),
              let text = String(data: data, encoding: .utf8) else { return }
        do { try await socket.send(.string(text)) }
        catch { reportFailure(error) }
    }
    func terminalInput(_ text: String, raw: Bool = false) async {
        guard let id = terminalSessionID, attachedMode == "drive" else { return }
        let value = raw ? text : text + "\n"
        await socketSend(["type": "terminal.input", "session_id": id, "data_base64": Data(value.utf8).base64EncodedString()])
    }
    func resizeTerminal(cols: Int, rows: Int) async {
        guard let id = terminalSessionID else { return }
        await socketSend(["type": "terminal.resize", "session_id": id, "cols": cols, "rows": rows])
    }
    func closeTerminal() async {
        terminalTask?.cancel()
        terminalTask = nil
        if let id = terminalSessionID {
            await socketSend(["type": "terminal.close", "session_id": id])
        }
        terminalSocket?.cancel(with: .normalClosure, reason: nil)
        terminalSocket = nil
        terminalSessionID = nil
        attachedID = nil
        attachedMode = nil
        onTerminalOutput = nil
        onTerminalScroll = nil
    }
}

extension SessionFork {
    /// The agents a node spawned and the harness each runs, from its
    /// `release` capability (`metadata.live.agents[]`: `name`, `harness`).
    /// A spawned agent's own roster record states no harness.
    static func spawned(node: String, capabilities: [JSONObject]) -> [String: Spawned] {
        var spawned: [String: Spawned] = [:]
        for capability in capabilities where string(capability["name"]) == "release" {
            for entry in objects(object(object(capability["metadata"])["live"])["agents"]) {
                let name = string(entry["name"]), harness = string(entry["harness"])
                guard !name.isEmpty, !harness.isEmpty, spawned[name] == nil else { continue }
                spawned[name] = Spawned(node: node, harness: harness)
            }
        }
        return spawned
    }

    /// Every node's spawned agents by name, in node order. A name that more
    /// than one node reports keeps every node, since one entry may be stale.
    static func spawned(nodes: [(name: String, capabilities: [JSONObject])]) -> [String: [Spawned]] {
        var all: [String: [Spawned]] = [:]
        for node in nodes {
            for (name, entry) in spawned(node: node.name, capabilities: node.capabilities) {
                all[name, default: []].append(entry)
            }
        }
        return all
    }

    /// The harness an agent's icon shows: the statement the fork decides from
    /// (`statedHarness(for:roster:spawned:)`), in the same order, then a
    /// name or persona guess from its records.
    static func displayCLI(for agent: RelayAgent, roster: [RelayAgent], fleet: [RelayAgent],
                           spawned: [String: [Spawned]]) -> String {
        let stated = cliInName(statedHarness(for: agent, roster: roster, spawned: spawned))
        if !stated.isEmpty { return stated }
        let known = [agent] + roster.filter { $0.name == agent.name } + fleet.filter { $0.name == agent.name }
        return known.first { !$0.cli.isEmpty }?.cli ?? ""
    }
}
