import AppKit
import Foundation
import Observation
import RelayDesktopCore
import RelaySessionOverview

/// The relay as the sessions page, the menu panel and the message composer
/// show it: this Mac's sessions and agents, the workspace roster and direct
/// messages. Every list is the core's; this keeps the last answer of each
/// and what the person is switching.
@MainActor
@Observable
final class RelayModel {
    private(set) var sessionRowsRevision: UInt64 = 0
    private(set) var sessionSearchMetadataRevision: UInt64 = 0

    private(set) var sessions: [LocalSession] = [] {
        didSet {
            guard oldValue != sessions else { return }
            sessionRowsRevision &+= 1
            armActivityCheck(reportingChange: false)
            model?.migrateHiddenSessionIDs(for: sessions)
            updateSessionLists()
            model?.updateRelayProblem()
            if model?.client.offers(CoreMethod.uploaderSessionsQuery) == true,
               Self.queryPresentation(oldValue) != Self.queryPresentation(sessions) {
                model?.localSessionPresentationChanged()
            }
        }
    }
    /// Query-affecting local state, excluding timestamps and push heartbeats.
    /// A frozen catalog page must admit a newly discovered relay-only row.
    private struct QueryPresentation: Equatable {
        let title: String
        let live: Bool
        let running: Bool?
        let directory: String?
        let project: String?
        let host: String?
        let name: String?
        let alias: String?
        let address: String?
    }
    private static func queryPresentation(_ sessions: [LocalSession]) -> [String: QueryPresentation] {
        Dictionary(sessions.map { session in
            (session.id, QueryPresentation(title: session.title, live: session.live, running: session.running,
                directory: session.directory, project: session.project, host: session.host,
                name: session.registration?.name, alias: session.registration?.alias, address: session.registration?.address))
        }, uniquingKeysWith: { _, latest in latest })
    }

    private(set) var agents: [LocalAgent] = []
    /// Agents that left the relay because their session was archived.
    private(set) var archivedAgents: [ArchivedAgent] = [] {
        didSet { if oldValue != archivedAgents { sessionSearchMetadataRevision &+= 1; model?.sessionPresentationChanged() } }
    }
    private(set) var roster: [RosterAgent] = [] {
        didSet {
            if oldValue != roster { sessionSearchMetadataRevision &+= 1 }
            if Set(oldValue.filter { $0.liveness == .idle }.map(\.name)) != Set(roster.filter { $0.liveness == .idle }.map(\.name)) { model?.sessionPresentationChanged() }
        }
    }
    /// Bumped when the core reconnects, so open title editors preload again.
    private(set) var overviewReloads = 0
    private(set) var isRefreshing = false
    /// Why the last change to a session failed, by target key.
    private(set) var errors: [String: String] = [:]
    var receiveMessagesError: String?
    /// Why taking a spawned agent over failed, by `LocalSession.id`, until it
    /// is tried again.
    var takeOverErrors: [String: String] = [:]
    /// Sessions whose owned spawn is being ended, and the last failure for
    /// each. Both outlive the transcript sidebar while the person switches.
    private(set) var endingSpawnedSessions: Set<String> = []
    private(set) var endSpawnedErrors: [String: String] = [:]
    /// The switch state asked for, by target key, while the relay catches up.
    private(set) var wanted: [String: Bool] = [:] {
        didSet { if oldValue != wanted { sessionSearchMetadataRevision &+= 1; model?.sessionPresentationChanged() } }
    }
    private(set) var busyTargets: Set<String> = [] {
        didSet {
            guard oldValue != busyTargets else { return }
            sessionSearchMetadataRevision &+= 1
            model?.sessionPresentationChanged()
            for key in oldValue.subtracting(busyTargets) {
                settleWaiters.removeValue(forKey: key)?.forEach { $0.resume() }
            }
        }
    }
    /// Who waits for a session's relay change to settle, by target key.
    /// The pending check of session activity, and the activity it last saw
    /// (see `armActivityCheck`).
    @ObservationIgnored private var activityCheck: Task<Void, Never>?
    @ObservationIgnored private var activitySeen: Int?

    /// Judges session activity again when it changes by time alone. A Devin
    /// Cloud session is Active only within a day of its last activity, so its
    /// Status, `is:active` and the menu's counts change with no new data.
    /// The check runs at the first such change, and at least every
    /// `activityRecheckMs` (a sleep or a clock set forward can carry the time
    /// past one); when activity did change it invalidates the cached rows,
    /// statuses and menu counts and reads the core's page again, as GTK's
    /// `relay/activity_clock.rs` does. A data change rebuilds those anyway.
    private func armActivityCheck(reportingChange: Bool) {
        activityCheck?.cancel()
        let now = currentTimeMs()
        let seen = sessions.activityRevision(atMs: now)
        if reportingChange, activitySeen != seen {
            sessionSearchMetadataRevision &+= 1
            model?.sessionPresentationChanged()
        }
        activitySeen = seen
        let wait = UInt64(max(0, sessions.nextActivityCheck(afterMs: now) &- now)) &* 1_000_000
        activityCheck = Task { [weak self] in
            do { try await Task.sleep(nanoseconds: wait) } catch { return }
            self?.armActivityCheck(reportingChange: true)
        }
    }

    @ObservationIgnored private var settleWaiters: [String: [CheckedContinuation<Void, Never>]] = [:]
    /// A message sent from the composer that its conversation does not list
    /// yet: shown under the history until a read lists it. Once the core
    /// took it, `sentId` is the id a history read confirms it by.
    struct PendingMessage {
        let echo: DirectMessage
        var sent = false
        var sentId: String?
    }
    /// By endpoint, oldest first.
    private(set) var pendingMessages: [String: [PendingMessage]] = [:]
    private(set) var isConnectingWorkspace = false
    private(set) var workspaceConnectError: String?
    /// The conversation with one endpoint, while the composer shows it.
    private(set) var conversation: (endpoint: String, history: History)?
    private(set) var conversationError: String?
    private(set) var isLoadingConversation = false
    /// The roster's record of the composer's recipient, while the composer
    /// is open.
    private(set) var recipient: (endpoint: String, recipient: Recipient)?
    /// The composer's last send: where it went, its message id, and the
    /// last state Relaycast reported for it.
    private(set) var lastSend: (endpoint: String, messageId: String?, state: MessageState?)?

    @ObservationIgnored private let client: CoreClient
    @ObservationIgnored weak var model: AppModel?
    @ObservationIgnored private var conversationReads = 0
    @ObservationIgnored private var recipientReads = 0
    /// The recipient the open composer shows; `nil` while it is closed.
    @ObservationIgnored private var composerEndpoint: String?
    /// Reads the open composer's recipient and conversation: once To
    /// settles, then again every 10 s while the window shows it.
    @ObservationIgnored private var composerRefresh: Task<Void, Never>?
    @ObservationIgnored private var windowVisible = false
    /// After a read of the conversation with `endpoint` failed: the failures
    /// so far, when the next read may run, and whether the last failure was
    /// its recipient's look-up alone. Until then nothing but the person
    /// reads it.
    @ObservationIgnored private var readBackoff: (endpoint: String, backoff: ReadBackoff, until: Date, recipient: Bool)?
    private(set) var titleOverviews: [String: SessionOverview] = [:]
    private(set) var titleCapabilities: [String: SessionTitleCapability] = [:]
    private(set) var titleEdits = SessionTitleEdits() { didSet { sessionRowsRevision &+= 1; model?.sessionPresentationChanged() } }
    private(set) var titleErrors: [String: String] = [:]
    @ObservationIgnored private(set) var titleWriteRevision: UInt64 = 0
    @ObservationIgnored private var registrationRevision: UInt64 = 0
    @ObservationIgnored private var titlePreloadQueue: [LocalSessionRow] = []
    @ObservationIgnored private var activeTitlePreloads: Set<String> = []
    @ObservationIgnored private var titlePreloadedAt: [String: Date] = [:]
    @ObservationIgnored private var titlePreloadGeneration: UInt64 = 0
    /// Renewed when the account changes: an answer about the previous
    /// account's roster or messages is dropped.
    @ObservationIgnored private var account = UUID()

    init(client: CoreClient) {
        self.client = client
    }

    // MARK: Snapshot

    var workspace: WorkspaceStatus? { model?.relayWorkspace }
    var storeError: String? { model?.agentsStatus?.storeError }
    var integrationError: String? { model?.agentsStatus?.integrationError }
    var deliveryError: String? { model?.agentsStatus?.deliveryError }

    // MARK: Derived lists
    //
    // Kept with the lists they come from, so a view reads them ready-made
    // and re-renders only when they change.

    /// Live sessions on the relay: the status bar's count.
    private(set) var liveOnRelayCount = 0

    private func updateSessionLists() {
        let live = sessions.filter { $0.live && $0.registration != nil }.count
        if liveOnRelayCount != live { liveOnRelayCount = live }
    }

    func agent(for session: LocalSession) -> LocalAgent? {
        agents.first { $0.target == session.target }
    }

    /// The push channel's state comes from the snapshot, which the core
    /// publishes on every change; the session list carries only the
    /// registration.
    func presence(of session: LocalSession) -> RelayPresence {
        // A busy session with no explicit request is switching to the other
        // state; one registered provisionally is on its way on.
        let wanted = wanted[session.id]
            ?? (busyTargets.contains(session.id) ? (session.registration == nil || session.registration?.isProvisional == true) : nil)
        let push = session.registration.flatMap { model?.push[$0.name] }
        return RelayPresence.of(onRelay: session.registration != nil, wanted: wanted, isLive: session.live,
                                pushLive: push.map(\.isLive))
    }

    /// Off the relay needs a running session and a workspace; on it can
    /// always be switched off.
    func canToggle(_ session: LocalSession, presence: RelayPresence) -> Bool {
        (model?.hiddenSessionIDs.contains(session.uploadKey ?? session.id) != true)
            && !SpawnHandover.canEnd(session)
            && (presence.isOn || presence.isChanging || (session.live && workspace != nil))
    }

    /// Why pressing an off switch cannot put `session` on the relay because
    /// Agent Relay does not see it running, with what to do next; nil when
    /// the switch works or another reason applies. The switch stays
    /// pressable then and shows this inline, so it is never silently dead
    /// (relay#2239).
    func notRunningReason(_ session: LocalSession, presence: RelayPresence) -> String? {
        guard model?.hiddenSessionIDs.contains(session.uploadKey ?? session.id) != true,
              !SpawnHandover.canEnd(session), !presence.isOn, !presence.isChanging,
              !session.live, workspace != nil else { return nil }
        return Self.notRunningMessage(for: session.target.kind)
    }

    /// The relay error a session's row and sidebar show. The not-running
    /// reason a press saved shows only while the session is still not seen
    /// running (relay#2239).
    func relayError(for session: LocalSession) -> String? {
        Self.shownError(errors[session.id], kind: session.target.kind,
                        stillNotRunning: notRunningReason(session, presence: presence(of: session)) != nil)
    }

    static func shownError(_ error: String?, kind: SessionKind, stillNotRunning: Bool) -> String? {
        guard let error else { return nil }
        return error == notRunningMessage(for: kind) && !stillNotRunning ? nil : error
    }

    static func notRunningMessage(for kind: SessionKind) -> String {
        kind == .codex
            ? "Agent Relay can't see this Codex chat running. Open it in Codex and try again."
            : "Agent Relay can't see this session running. Open it and try again."
    }

    /// A press on a session's Messages switch: flips it, or, when the session
    /// is not seen running, says so on its row and looks for it again.
    func pressRelay(for session: LocalSession) {
        let current = presence(of: session)
        guard !canToggle(session, presence: current),
              let reason = notRunningReason(session, presence: current) else {
            requestRelay(!current.isOn, for: session)
            return
        }
        errors[session.id] = reason
        Task { await refresh(.sessions) }
    }

    func switchHelp(_ session: LocalSession, presence: RelayPresence) -> String {
        if SpawnHandover.canEnd(session) { return "Use End Agent to stop this spawned agent" }
        if presence.isOn { return "Take off the relay" }
        if !session.live { return "Start this session to put it on the relay" }
        if workspace == nil { return "Connect a relay workspace first" }
        return session.uploadKey == nil
            ? "Put on the relay to message it by name"
            : "Put on the relay to message it by name. Sessions on the relay are uploaded."
    }

    // MARK: Refresh

    func refreshAll() async {
        isRefreshing = true
        defer { isRefreshing = false }
        async let sessions: Void = refresh(.sessions)
        async let agents: Void = refresh(.agents)
        async let roster: Void = refresh(.roster)
        _ = await (sessions, agents, roster)
    }

    /// The core invalidated `what`: re-fetches it, and a session or reply
    /// changing can change whether the open composer's recipient runs
    /// and what became of its message.
    func invalidated(_ what: Listing) async {
        await refresh(what)
        switch what {
        case .sessions, .roster, .messages:
            // Not while a failed read's backoff runs: the read waits for it.
            // A look-up that fails starts (or lengthens) one.
            if let endpoint = composerEndpoint, !backingOff(endpoint) {
                let lookUp = await readRecipient(endpoint)
                settleRead(endpoint, conversation: .dropped, recipient: lookUp)
            }
        default:
            break
        }
    }

    /// Re-fetches one collection.
    func refresh(_ what: Listing) async {
        let revision = model?.uploadConnectionRevision
        switch what {
        case .sessions:
            try? await loadSessions()
        case .agents:
            if let result = try? await client.agentsList() {
                guard revision == model?.uploadConnectionRevision else { return }
                if agents != result.agents { agents = result.agents }
                if archivedAgents != result.archived { archivedAgents = result.archived }
            }
        case .roster:
            let current = account
            do {
                let result = try await client.rosterList(RosterParams(includeIdle: false))
                guard current == account, revision == model?.uploadConnectionRevision else { return }
                if roster != result.agents { roster = result.agents }
            } catch let error as CoreError where error.kind == ErrorKind.notSignedIn.rawValue {
                guard current == account, revision == model?.uploadConnectionRevision else { return }
                if !roster.isEmpty { roster = [] }
            } catch {
                // Any other failure keeps the last roster.
            }
        case .messages:
            if let endpoint = conversation?.endpoint, !backingOff(endpoint) { await loadConversation(endpoint) }
        default:
            break
        }
    }

    /// Reads this Mac's sessions with their registrations.
    func loadSessions() async throws {
        let revision = model?.uploadConnectionRevision
        let titleRevision = titleWriteRevision
        let registrationReadRevision = registrationRevision
        let result = try await client.sessionsList()
        guard revision == model?.uploadConnectionRevision, titleRevision == titleWriteRevision,
              registrationReadRevision == registrationRevision else { return }
        if sessions != result.sessions { sessions = result.sessions }
        reconcileTitleEdits()
        settle()
        model?.reconcileHiddenSessions()
    }

    /// The sessions again, showing the panel's and page's progress.
    func refreshSessions() async {
        isRefreshing = true
        defer { isRefreshing = false }
        try? await loadSessions()
    }

    /// Retire reads from the old core without discarding the visible relay.
    func connectionInterrupted() {
        conversationReads += 1
        recipientReads += 1
        isLoadingConversation = false
        // A send the retired core never answered is not shown as on its way.
        pendingMessages = [:]
        retireTitlePreloads()
        titleOverviews = titleOverviews.filter { $0.value.status != .loading }
    }

    /// The replacement core has answered its first snapshot: open title
    /// editors read their suggestions again, since a read may have expired
    /// during a longer outage.
    func connectionRecovered() {
        retireTitlePreloads()
        overviewReloads += 1
    }

    private func retireTitlePreloads() {
        titlePreloadGeneration &+= 1
        titlePreloadQueue = []
        activeTitlePreloads = []
        titlePreloadedAt = [:]
        titleCapabilities = [:]
    }

    func accountChanged() {
        account = UUID()
        // A refusal in the previous workspace says nothing about this one.
        errors = [:]
        titlePreloadGeneration &+= 1
        activeTitlePreloads = []
        titleWriteRevision &+= 1
        titleEdits = SessionTitleEdits()
        titleErrors = [:]
        titleOverviews = [:]
        titleCapabilities = [:]
        titlePreloadedAt = [:]
        titlePreloadQueue = []
        conversationReads += 1
        conversation = nil
        conversationError = nil
        isLoadingConversation = false
        recipientReads += 1
        recipient = nil
        lastSend = nil
        pendingMessages = [:]
        readBackoff = nil
        roster = []
        Task { await refresh(.roster) }
    }

    func displayedTitle(for row: LocalSessionRow) -> String {
        titleEdits.title(for: row.id) ?? row.title
    }

    func titleState(for row: LocalSessionRow) -> SessionTitlePresentation {
        let overview = titleOverviews[row.id]
        let generated = overview?.title.trimmingCharacters(in: .whitespacesAndNewlines)
        return SessionTitlePresentation(title: displayedTitle(for: row),
            supported: titleCapabilities[row.id]?.supported,
            reason: titleCapabilities[row.id]?.reason,
            generatedTitle: generated?.isEmpty == false ? generated : nil,
            saving: titleEdits.isSaving(row.id), error: titleErrors[row.id],
            preloadRevision: overviewReloads)
    }

    /// Opening a session preloads its saved suggestion and native capability.
    /// A bounded queue and one-minute cache also cover quick session switching.
    func preloadTitle(_ row: LocalSessionRow) {
        guard !activeTitlePreloads.contains(row.id),
              !titlePreloadQueue.contains(where: { $0.id == row.id }),
              titlePreloadedAt[row.id].map({ Date().timeIntervalSince($0) >= 60 }) ?? true else { return }
        if titlePreloadQueue.count >= 64 { titlePreloadQueue.removeFirst() }
        titlePreloadQueue.append(row)
        drainTitlePreloads()
    }

    private func drainTitlePreloads() {
        while activeTitlePreloads.count < 4, !titlePreloadQueue.isEmpty {
            let row = titlePreloadQueue.removeFirst()
            activeTitlePreloads.insert(row.id)
            let current = account
            let revision = model?.uploadConnectionRevision
            let generation = titlePreloadGeneration
            Task {
                defer {
                    // Retired reads must not remove a replacement core's active request.
                    if generation == titlePreloadGeneration {
                        activeTitlePreloads.remove(row.id)
                        drainTitlePreloads()
                    }
                }
                do {
                    let capability = try await client.sessionsTitleCapability(SessionTitleIdentity(sessionId: row.sessionID, source: row.source))
                    guard generation == titlePreloadGeneration, current == account, revision == model?.uploadConnectionRevision else { return }
                    titleCapabilities[row.id] = capability
                } catch {
                    guard generation == titlePreloadGeneration, current == account, revision == model?.uploadConnectionRevision else { return }
                    titleCapabilities[row.id] = SessionTitleCapability(reason: error.localizedDescription, supported: false)
                }
                if row.upload?.status == .uploaded {
                    if titleOverviews[row.id] == nil { titleOverviews[row.id] = SessionOverview(status: .loading) }
                    do {
                        let response = try await client.sessionsOverviewGet(SessionOverviewParams(sessionId: row.sessionID))
                        let overview = try SessionOverview.decode(JSONEncoder().encode(response), sessionID: row.sessionID, source: row.source)
                        guard generation == titlePreloadGeneration, current == account, revision == model?.uploadConnectionRevision else { return }
                        titleOverviews[row.id] = overview
                    } catch {
                        guard generation == titlePreloadGeneration, current == account, revision == model?.uploadConnectionRevision else { return }
                        let previous = titleOverviews[row.id]
                        titleOverviews[row.id] = SessionOverview(status: .failed, title: previous?.title ?? "",
                            description: previous?.description ?? "", error: error.localizedDescription)
                    }
                }
                guard generation == titlePreloadGeneration, current == account, revision == model?.uploadConnectionRevision else { return }
                titlePreloadedAt[row.id] = Date()
            }
        }
    }

    func setSessionTitle(_ rawTitle: String, for row: LocalSessionRow) async {
        let title = rawTitle.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !title.isEmpty, !titleEdits.isSaving(row.id), titleCapabilities[row.id]?.supported == true else { return }
        guard title != titleState(for: row).title else { return }
        let current = account
        let token = titleEdits.begin(row.id, title: title)
        titleErrors[row.id] = nil
        titleWriteRevision &+= 1
        do {
            let result = try await client.sessionsTitleSet(SessionTitleSetParams(sessionId: row.sessionID, source: row.source, title: title))
            guard current == account else { return }
            titleWriteRevision &+= 1
            titleEdits.succeed(row.id, token: token, title: result.title)
            scheduleTitleExpiry(row)
            await refreshSessions()
            await model?.refreshSessions()
            reconcileTitleEdits()
        } catch {
            guard current == account else { return }
            titleWriteRevision &+= 1
            titleEdits.fail(row.id, token: token)
            scheduleTitleExpiry(row)
            titleErrors[row.id] = error.localizedDescription
        }
    }

    private func scheduleTitleExpiry(_ row: LocalSessionRow) {
        guard let expiration = titleEdits.expiration(for: row.id) else { return }
        let current = account
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(max(0, expiration.date.timeIntervalSinceNow)))
            guard let self, current == self.account else { return }
            var edits = self.titleEdits
            guard edits.expire(row.id, token: expiration.token) else { return }
            self.titleEdits = edits
            await self.refreshSessions()
            await self.model?.refreshSessions()
        }
    }

    func reconcileTitleEdits() {
        let rows = LocalSessionRow.mergeUnsorted(uploads: model?.sessions ?? [], sessions: sessions)
        var updated = titleEdits
        for row in rows { updated.observe(row.id, title: row.title) }
        if updated != titleEdits { titleEdits = updated }
    }

    /// Forgets a wanted state the relay has reached.
    private func settle() {
        for (key, on) in wanted where !busyTargets.contains(key) {
            if sessions.first(where: { $0.id == key }).map({ ($0.registration != nil) == on }) ?? true {
                wanted[key] = nil
            }
        }
    }

    // MARK: Switching

    /// Returns once no change to `key`'s relay state is running.
    func settled(_ key: String) async {
        guard busyTargets.contains(key) else { return }
        await withCheckedContinuation { settleWaiters[key, default: []].append($0) }
    }

    /// Puts a session on the relay or takes it off. The switch shows the new
    /// state at once; one change per session runs at a time, and the last
    /// state asked for wins.
    func requestRelay(_ on: Bool, for session: LocalSession) {
        guard !on || model?.hiddenSessionIDs.contains(session.uploadKey ?? session.id) != true else { return }
        wanted[session.id] = on
        errors[session.id] = nil
        receiveMessagesError = nil
        guard !busyTargets.contains(session.id) else { return }
        busyTargets.insert(session.id)
        Task { await drive(session.id, target: session.target) }
    }

    func register(_ session: LocalSession, as name: String) async {
        let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: "@")).lowercased()
        guard !name.isEmpty else { return }
        guard model?.hiddenSessionIDs.contains(session.uploadKey ?? session.id) != true else { return }
        guard !busyTargets.contains(session.id) else { return }
        busyTargets.insert(session.id)
        errors[session.id] = nil
        defer { busyTargets.remove(session.id); settle() }
        // The row reads as on the relay under `name` from the click; the
        // core's listing brings the real registration, and a failure takes
        // the provisional one away again.
        if let index = sessions.firstIndex(where: { $0.id == session.id }), sessions[index].registration == nil {
            sessions[index] = sessions[index].withRegistration(.provisional(name: name))
        }
        do {
            let registered = try await client.agentsRegister(RegisterParams(name: name, target: session.target))
            registrationRevision &+= 1
            // Preserve the successful mutation if the follow-up list fails,
            // so a queued off request still knows which agent to unregister.
            if let index = sessions.firstIndex(where: { $0.id == session.id }) {
                let current = sessions[index]
                let previous = registered.alreadyRegistered ? current.registration : nil
                let registration = SessionRegistration(addedBySession: previous?.addedBySession ?? false, address: registered.address, alias: previous?.alias, inboxStall: previous?.inboxStall, name: registered.name, push: previous?.push, spawn: previous?.spawn)
                sessions[index] = LocalSession(directory: current.directory, holder: current.holder, host: current.host, live: current.live, project: current.project, registration: registration, running: current.running, suggestedName: current.suggestedName, target: current.target, title: current.title, updatedAtMs: current.updatedAtMs)
            }
            try await loadSessions()
            await refresh(.agents)
        } catch {
            errors[session.id] = error.localizedDescription
            if let index = sessions.firstIndex(where: { $0.id == session.id }),
               let registration = sessions[index].registration, registration.isProvisional, registration.name == name {
                sessions[index] = sessions[index].withRegistration(nil)
            }
        }
        // A hide/off request made during explicit name registration must win
        // before callers observe this target as settled.
        if wanted[session.id] != nil { await drive(session.id, target: session.target) }
    }

    private func drive(_ key: String, target: Target) async {
        defer {
            busyTargets.remove(key)
            settle()
        }
        while let on = wanted[key] {
            let current = sessions.first { $0.id == key }
            let isOn = current?.registration != nil
            if on == isOn { break }
            do {
                if on {
                    let registered = try await client.agentsRegister(RegisterParams(target: target))
                    registrationRevision &+= 1
                    if let index = sessions.firstIndex(where: { $0.id == key }) {
                        let current = sessions[index]
                        let previous = registered.alreadyRegistered ? current.registration : nil
                        let registration = SessionRegistration(addedBySession: previous?.addedBySession ?? false, address: registered.address, alias: previous?.alias, inboxStall: previous?.inboxStall, name: registered.name, push: previous?.push, spawn: previous?.spawn)
                        sessions[index] = LocalSession(directory: current.directory, holder: current.holder, host: current.host, live: current.live, project: current.project, registration: registration, running: current.running, suggestedName: current.suggestedName, target: current.target, title: current.title, updatedAtMs: current.updatedAtMs)
                    }
                } else if let name = current?.registration?.name {
                    try await client.agentsUnregister(NameParams(name: name))
                    registrationRevision &+= 1
                    if let index = sessions.firstIndex(where: { $0.id == key }) {
                        let current = sessions[index]
                        sessions[index] = LocalSession(directory: current.directory, holder: current.holder, host: current.host, live: current.live, project: current.project, registration: nil, running: current.running, suggestedName: current.suggestedName, target: current.target, title: current.title, updatedAtMs: current.updatedAtMs)
                    }
                }
                // The switch settles on what the core lists; a list that
                // cannot be read ends the change rather than repeating it.
                try await loadSessions()
                if wanted[key] == on && (sessions.first(where: { $0.id == key })?.registration != nil) != on {
                    errors[key] = "Receive messages did not change. Refresh the session and try again."
                    receiveMessagesError = errors[key]
                    wanted[key] = nil
                    break
                }
                errors[key] = nil
                await refresh(.agents)
            } catch {
                errors[key] = error.localizedDescription
                receiveMessagesError = error.localizedDescription
                if let next = wanted[key], next != on { continue }
                wanted[key] = nil
                break
            }
        }
    }

    // MARK: Workspace

    /// Fetches the account workspace's relay key with the Agent Relay sign-in.
    func connectWorkspace() async {
        guard let workspaceID = model?.workspaceID, !isConnectingWorkspace else { return }
        isConnectingWorkspace = true
        workspaceConnectError = nil
        model?.connectingWorkspace = true
        defer {
            isConnectingWorkspace = false
            model?.connectingWorkspace = false
        }
        do {
            try await client.accountConnectWorkspace(ConnectWorkspaceParams(workspaceId: workspaceID))
        } catch {
            workspaceConnectError = error.localizedDescription
        }
    }

    func cancelConnectWorkspace() {
        Task { try? await client.accountSignInCancel() }
    }

    // MARK: Agent tools

    private func act(_ change: () async throws -> Void) async -> String? {
        do {
            try await change()
            await refresh(.agents)
            await refresh(.sessions)
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    enum RenameOutcome: Equatable {
        /// What the rename did, in words from the core's answer: the alias it released.
        case renamed(String)
        case failed(String)
    }

    /// The agent is shown under its new name from the click; the core's
    /// listing confirms it, and a failure shows the old name again.
    func rename(_ agent: LocalAgent, to name: String) async -> RenameOutcome {
        showAlias(name, of: agent)
        var released: String?
        if let error = await act({ released = try await client.agentsRename(RenameParams(name: agent.name, newName: name)).released }) {
            // Only where the name asked for is still what shows: a listing
            // read meanwhile is the core's word.
            showAlias(agent.alias, of: agent, where: name)
            return .failed(error)
        }
        return .renamed(AgentNameText.renamed(name: agent.name, previousAlias: released, requested: name))
    }

    /// Shows `agent` (and its session) under `alias`; with `current`, only
    /// where that alias shows now.
    private func showAlias(_ alias: String?, of agent: LocalAgent, where current: String? = nil) {
        agents = agents.map { listed in
            guard listed.name == agent.name, current == nil || listed.alias == current else { return listed }
            return listed.withAlias(alias)
        }
        sessions = sessions.map { session in
            guard let registration = session.registration, registration.name == agent.name,
                  current == nil || registration.alias == current else { return session }
            return session.withRegistration(registration.withAlias(alias))
        }
    }

    // MARK: Spawned agents

    /// Hands a spawned agent's session to the person (`agents.take_over`):
    /// returns once its headless process has stopped.
    func takeOver(_ name: String, interrupt: Bool) async throws -> TakenOver {
        let taken = try await client.agentsTakeOver(TakeOverParams(interrupt: interrupt, name: name))
        async let sessions: Void = refresh(.sessions)
        async let agents: Void = refresh(.agents)
        _ = await (sessions, agents)
        return taken
    }

    /// Stops a session Agent Relay spawned and still owns, then removes its
    /// relay identity. The core confirms the harness ended before removal.
    func endSpawned(_ name: String, sessionID: String) async throws {
        guard endingSpawnedSessions.insert(sessionID).inserted else { return }
        endSpawnedErrors[sessionID] = nil
        defer { endingSpawnedSessions.remove(sessionID) }
        do {
            _ = try await client.agentsEnd(NameParams(name: name))
            registrationRevision &+= 1
            async let sessions: Void = refresh(.sessions)
            async let agents: Void = refresh(.agents)
            _ = await (sessions, agents)
        } catch {
            endSpawnedErrors[sessionID] = error.localizedDescription
            throw error
        }
    }

    // MARK: Messages

    /// `@name` or `agent@machine`, as the composer takes it.
    static func endpoint(_ raw: String) -> String {
        let value = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        return value.hasPrefix("@") ? String(value.dropFirst()) : value
    }

    /// Reads the conversation with `raw`. A failure starts (or lengthens)
    /// its backoff and a success ends it.
    func loadConversation(_ raw: String) async {
        let endpoint = Self.endpoint(raw)
        let read = await readConversation(endpoint)
        settleRead(endpoint, conversation: read, recipient: .dropped)
    }

    /// What became of one read of the history or of the recipient.
    private enum ReadOutcome {
        case read
        case failed(String)
        /// Not read, or it cannot tell: a newer read (or none at all)
        /// answers for it.
        case dropped

        var succeeded: Bool {
            if case .read = self { return true }
            return false
        }
    }

    private func readConversation(_ endpoint: String) async -> ReadOutcome {
        guard !endpoint.isEmpty else { return .dropped }
        if conversation?.endpoint != endpoint { conversation = nil }
        // The newest read wins: an older one finishing later is dropped.
        conversationReads += 1
        let read = conversationReads
        isLoadingConversation = true
        defer { if read == conversationReads { isLoadingConversation = false } }
        do {
            let history = try await client.messagesHistory(HistoryParams(endpoint: endpoint))
            guard read == conversationReads else { return .dropped }
            conversation = (endpoint, history)
            conversationError = nil
            retirePendingMessages(endpoint, listed: history)
            return .read
        } catch {
            guard read == conversationReads else { return .dropped }
            conversationError = error.localizedDescription
            return .failed(error.localizedDescription)
        }
    }

    /// Whether a failed read's backoff for `endpoint` still runs: until it
    /// ends only the person's own actions read the conversation.
    private func backingOff(_ endpoint: String) -> Bool {
        backoffRemaining(endpoint) != nil
    }

    /// How long the backoff for `endpoint` still runs, if it does.
    private func backoffRemaining(_ endpoint: String) -> TimeInterval? {
        guard let readBackoff, readBackoff.endpoint == endpoint else { return nil }
        let remaining = readBackoff.until.timeIntervalSinceNow
        return remaining > 0 ? remaining : nil
    }

    /// One read of `endpoint` answered: a failure of either the
    /// conversation or its recipient waits a backoff before the next, and
    /// says so. A success goes back to the normal cadence only when it read
    /// what failed last: a history read says nothing about a look-up of
    /// the recipient that failed.
    private func settleRead(_ endpoint: String, conversation read: ReadOutcome, recipient lookUp: ReadOutcome) {
        let failed: (message: String, recipient: Bool)
        switch (read, lookUp) {
        case (.failed(let message), _): failed = (message, false)
        case (_, .failed(let message)): failed = (message, true)
        default:
            if let readBackoff, readBackoff.endpoint == endpoint,
               readBackoff.recipient ? lookUp.succeeded : read.succeeded {
                self.readBackoff = nil
            }
            return
        }
        var backoff = readBackoff.flatMap { $0.endpoint == endpoint ? $0.backoff : nil } ?? ReadBackoff()
        // The core's errors name no Retry-After, so the doubling applies.
        let delay = backoff.failed()
        readBackoff = (endpoint, backoff, Date().addingTimeInterval(delay), failed.recipient)
        conversationError = failed.recipient
            ? ReadBackoff.recipientFailureText(failed.message, retryingIn: delay)
            : ReadBackoff.failureText(failed.message, retryingIn: delay)
    }

    /// `to` for a name, `address` for an exact address.
    private static func recipientParams(_ endpoint: String) -> (address: String?, to: String?) {
        endpoint.contains("@") ? (endpoint, nil) : (nil, endpoint)
    }

    /// The open composer shows `raw` as its recipient (it opened, or To
    /// changed). Anything about another
    /// recipient (its record, a read in flight, its last send and that
    /// send's follow-up) is retired.
    func composerShows(_ raw: String) {
        let endpoint = Self.endpoint(raw)
        guard endpoint != composerEndpoint else { return }
        composerEndpoint = endpoint
        recipientReads += 1
        if recipient?.endpoint != endpoint { recipient = nil }
        if lastSend?.endpoint != endpoint { lastSend = nil }
        // The person moved to another recipient: it is read once To
        // settles, and a backoff from an earlier one says nothing about it.
        readBackoff = nil
        conversationError = nil
        refreshComposer(settle: true)
    }

    /// The person asked to read the conversation with `raw` again: its
    /// recipient and history are read at once, backoff or not. A failure
    /// lengthens the backoff; a success ends it.
    func refreshConversation(_ raw: String) async {
        let endpoint = Self.endpoint(raw)
        let lookUp = await readRecipient(endpoint)
        let read = await readConversation(endpoint)
        settleRead(endpoint, conversation: read, recipient: lookUp)
    }

    /// `messages`, `sessions` and `roster` invalidations read the
    /// recipient again; this also catches what the core does not announce,
    /// such as a message handed over or a remote agent going offline. Typing
    /// changes To on every key, so a new recipient is read once it settles.
    /// After a failed read the next waits for its backoff to end instead
    /// of 10 s, checking at least every 10 s whether the person's own read
    /// ended it sooner.
    private func refreshComposer(settle: Bool) {
        composerRefresh?.cancel()
        composerRefresh = nil
        guard let endpoint = composerEndpoint, !endpoint.isEmpty, windowVisible else { return }
        composerRefresh = Task { [weak self] in
            if settle {
                do { try await Task.sleep(nanoseconds: 300_000_000) } catch { return }
            }
            while let self, !Task.isCancelled, endpoint == self.composerEndpoint {
                if let wait = self.backoffRemaining(endpoint) {
                    do { try await Task.sleep(nanoseconds: RelayModel.nanoseconds(min(wait, ReadBackoff.cadence))) } catch { return }
                    continue
                }
                let lookUp = await self.readRecipient(endpoint)
                let read = await self.readConversation(endpoint)
                self.settleRead(endpoint, conversation: read, recipient: lookUp)
                let wait = min(self.backoffRemaining(endpoint) ?? ReadBackoff.cadence, ReadBackoff.cadence)
                do { try await Task.sleep(nanoseconds: RelayModel.nanoseconds(wait)) } catch { return }
            }
        }
    }

    private nonisolated static func nanoseconds(_ interval: TimeInterval) -> UInt64 {
        UInt64(max(0, interval) * 1_000_000_000)
    }

    /// The main window came on screen or left it: the open composer is
    /// read again while it shows, and not at all while it does not.
    func windowVisibilityChanged(_ visible: Bool) {
        guard windowVisible != visible else { return }
        windowVisible = visible
        refreshComposer(settle: false)
    }

    /// The composer opened showing `raw`: it starts clean and follows that
    /// recipient until it closes.
    func openComposer(_ raw: String) {
        closeComposer()
        composerShows(raw)
    }

    /// Reads the open composer's recipient `raw` and, for its last send
    /// there, what Relaycast holds for the message. Only a read: a recipient
    /// the composer does not show (or a closed composer) is not read.
    func lookUpRecipient(_ raw: String) async {
        await readRecipient(Self.endpoint(raw))
    }

    /// Reads `endpoint` while it is the composer's recipient; an answer that
    /// arrives after the composer moved on is dropped. A read that cannot
    /// tell keeps the last state Relaycast reported. Only a look-up that
    /// failed for this recipient, on a core that offers it, while it is
    /// still the one asked for, comes back failed, for the backoff.
    @discardableResult
    private func readRecipient(_ endpoint: String) async -> ReadOutcome {
        guard endpoint == composerEndpoint else { return .dropped }
        recipientReads += 1
        let read = recipientReads
        guard !endpoint.isEmpty else {
            recipient = nil
            return .dropped
        }
        let (address, to) = Self.recipientParams(endpoint)
        let messageId = lastSend.flatMap { $0.endpoint == endpoint ? $0.messageId : nil }
        let current = account
        let result: Recipient
        do {
            result = try await client.rosterGet(RecipientParams(address: address, messageId: messageId, to: to))
        } catch is CancellationError {
            return .dropped
        } catch {
            // As on Linux: a core without roster.get cannot tell, which is no
            // failure to back off from. Asked after the call, which waited
            // for the core's hello: before it, nothing is known as offered.
            guard client.offers(CoreMethod.rosterGet),
                  (error as? CoreError)?.kind != ErrorKind.methodNotFound.rawValue else { return .dropped }
            guard read == recipientReads, current == account, endpoint == composerEndpoint else { return .dropped }
            return .failed(error.localizedDescription)
        }
        guard read == recipientReads, current == account, endpoint == composerEndpoint else { return .dropped }
        recipient = (endpoint, result)
        if let state = result.message, let sent = lastSend, sent.endpoint == endpoint, sent.messageId == messageId {
            lastSend = (endpoint, messageId, state)
        }
        return .read
    }

    /// The recipient's record when it is the composer's `endpoint`.
    func recipient(at raw: String) -> Recipient? {
        let endpoint = Self.endpoint(raw)
        return recipient.flatMap { $0.endpoint == endpoint ? $0.recipient : nil }
    }

    /// What the composer says about its last send to `endpoint`.
    func deliveryNote(at raw: String) -> (text: String, tone: DeliveryTone)? {
        let endpoint = Self.endpoint(raw)
        guard let lastSend, lastSend.endpoint == endpoint else { return nil }
        return MessageDelivery.note(lastSend.state, recipient: recipient(at: endpoint))
    }

    /// The composer moved on from its last send (a new message is being
    /// written); the note gives way to the recipient's state.
    func forgetLastSend() {
        lastSend = nil
    }

    /// The composer closed: nothing is followed for it any more.
    func closeComposer() {
        composerRefresh?.cancel()
        composerRefresh = nil
        composerEndpoint = nil
        readBackoff = nil
        recipientReads += 1
        recipient = nil
        lastSend = nil
    }

    /// The conversation with `raw` as the composer shows it: the history
    /// read last, then the messages sent since that it does not list yet.
    func shownHistory(at raw: String) -> History? {
        let endpoint = Self.endpoint(raw)
        guard let conversation, conversation.endpoint == endpoint else { return nil }
        let pending = pendingMessages[endpoint] ?? []
        guard !pending.isEmpty else { return conversation.history }
        return History(identity: conversation.history.identity, messages: conversation.history.messages + pending.map(\.echo))
    }

    /// A history read of `endpoint` succeeded: the messages it lists, and
    /// those the core took without an id to confirm them by, are no longer
    /// shown as on their way.
    private func retirePendingMessages(_ endpoint: String, listed history: History) {
        let ids = Set(history.messages.map(\.id))
        pendingMessages[endpoint]?.removeAll { $0.sent && $0.sentId.map(ids.contains) ?? true }
    }

    /// Sends as the signed-in person; the answer's delivery state is kept
    /// for the composer. The message shows in the conversation at once, as
    /// on its way, until the history lists it. Returns why the send failed.
    func sendMessage(to raw: String, text: String) async -> String? {
        let endpoint = Self.endpoint(raw)
        let body = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !body.isEmpty else { return "Type a message first." }
        let (address, to) = Self.recipientParams(endpoint)
        let params = SendParams(address: address, text: body, to: to)
        let echo = PendingMessages.echo(body)
        pendingMessages[endpoint, default: []].append(PendingMessage(echo: echo))
        // The echo goes at once when the send fails, and once a history
        // read lists the message when it succeeds.
        var taken = false
        defer { if !taken { pendingMessages[endpoint]?.removeAll { $0.echo.id == echo.id } } }
        do {
            let sent = try await client.messagesSend(params)
            taken = true
            if let index = pendingMessages[endpoint]?.firstIndex(where: { $0.echo.id == echo.id }) {
                pendingMessages[endpoint]?[index].sent = true
                pendingMessages[endpoint]?[index].sentId = sent.messageId
            }
            guard endpoint == composerEndpoint else { return nil }
            recipientReads += 1
            recipient = (endpoint, sent.recipient)
            lastSend = (endpoint, sent.messageId, sent.recipient.message)
            // A failed read's backoff holds the follow-up reads too; the
            // message shows as on its way until the next read lists it.
            if !backingOff(endpoint) { await loadConversation(endpoint) }
            // Relaycast hands a message to a running agent's host within
            // moments; read its state again once it has had the chance.
            Task { [weak self] in
                try? await Task.sleep(nanoseconds: 2_000_000_000)
                // Only while the composer still shows this send: a To changed
                // since retired it, and its read never displaces the new one.
                guard let self, sent.messageId != nil, self.lastSend?.endpoint == endpoint,
                      self.lastSend?.messageId == sent.messageId, !self.backingOff(endpoint) else { return }
                let lookUp = await self.readRecipient(endpoint)
                self.settleRead(endpoint, conversation: .dropped, recipient: lookUp)
            }
            return nil
        } catch let error as CoreError where error.kind == ErrorKind.notFound.rawValue {
            if endpoint == composerEndpoint {
                recipientReads += 1
                recipient = (endpoint, Recipient(agent: nil, known: true))
                lastSend = nil
            }
            return MessageDelivery.note(.noSuchAgent, recipient: nil).text
        } catch {
            return error.localizedDescription
        }
    }
}
