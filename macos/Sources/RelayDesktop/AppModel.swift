import AppKit
import Foundation
import Observation
import OSLog
import RelayDesktopCore
import UserNotifications

/// The app's state: a client of the relay core that keeps only what the
/// windows and the menu bar need. The core owns sign-in, the uploader,
/// agents, delivery and every Cloud and Relaycast call; this model renders
/// its snapshot, re-fetches what it invalidates and sends it commands.
///
/// ```
/// launch ─register LaunchAgent─▶ connect ─hello/subscribe─▶ status snapshots + events
///   account.state: signed_out ─sign in─▶ pending_approval ─▶ preparing ─▶ signed_in
/// ```
@MainActor
@Observable
final class AppModel {
    // Source revisions make query-only cache reads constant time. Published
    // setters notify before mutation; didSet advances these before the next read.
    private(set) var sessionRowsRevision: UInt64 = 0
    private(set) var sessionSearchMetadataRevision: UInt64 = 0
    let sessionSearchSource = SessionSearchSourceCache()

    enum Phase: Equatable {
        /// Before the core's first snapshot.
        case launching
        /// Signed out: the sign-in window is the app.
        case signedOut
        /// A sign-in is starting or connecting the workspace.
        case signingIn
        /// Waiting for the browser to return to this Mac.
        case waitingForBrowser
        /// Signed in; local sessions are being prepared for upload.
        case preparingSessions
        /// Signed in with the uploader connected.
        case attached
    }

    static let logger = Logger(subsystem: AppInfo.subsystem, category: "model")

    // MARK: Published state

    private(set) var isCoreConnected = false {
        didSet { if oldValue && !isCoreConnected { tools.reset() } }
    }
    private(set) var isCheckingAccountHealth = false
    private(set) var accountHealthError: String?
    /// The core's last snapshot. Views read the slices below, which change
    /// only when their part of the snapshot does; a new revision alone
    /// re-renders only Diagnostics.
    private(set) var core: Status? {
        didSet {
            guard let core else { return }
            if account != core.account { account = core.account }
            let workspace = core.workspace.isFound ? core.workspace : nil
            if relayWorkspace != workspace { relayWorkspace = workspace }
            if push != core.push { push = core.push }
            if agentsStatus != core.agents { agentsStatus = core.agents }
            if spawn != core.spawn { spawn = core.spawn }
            if coreVersion != core.daemon.version { coreVersion = core.daemon.version }
            updatePhase()
        }
    }
    /// The account as the last snapshot shows it.
    private(set) var account: AccountStatus = .signedOut {
        didSet { sessionSearchMetadataRevision &+= 1 }
    }
    /// The relay workspace, once the core has found one.
    private(set) var relayWorkspace: WorkspaceStatus? {
        didSet { sessionSearchMetadataRevision &+= 1 }
    }
    /// Push channel per registered agent name.
    private(set) var push: [String: PushState] = [:] {
        didSet { sessionSearchMetadataRevision &+= 1 }
    }
    private(set) var agentsStatus: AgentsStatus? { didSet { updateRelayProblem() } }
    private(set) var spawn: SpawnStatus?
    private(set) var coreVersion: String?
    /// Where the app is: launching, signing in or attached. Kept with the
    /// snapshot and the sign-in flags it is read from.
    private(set) var phase: Phase = .launching {
        didSet { updateDerivedStatus() }
    }
    private(set) var signInLink: String?
    /// The core could not be started or reached.
    private(set) var coreProblem: String? { didSet { updateRelayProblem() } }
    /// macOS holds the core until the person allows it in Login Items.
    private(set) var coreNeedsApproval = false
    private(set) var isRecoveringCore = false
    private(set) var lastError: String? { didSet { updateRelayProblem() } }
    /// The core still runs another build after every restart attempt. Only
    /// a hello sets or clears it: status refreshes that clear `lastError`
    /// must not hide it while the old core is still connected.
    private(set) var staleCoreWarning: String? { didSet { updateRelayProblem() } }
    /// Why the last connection link failed, for Diagnostics; cleared when a
    /// later link is parsed.
    private(set) var linkError: String?
    private(set) var signInError: String?
    /// From the sign-in click until the core answers or its snapshot shows
    /// the attempt; no second one starts meanwhile.
    private(set) var startingSignIn = false
    /// Retained for retries that need an acknowledgement or a workspace.
    private var signInProvider: SignInProvider = .google
    /// A self-host site's login configuration: its logo, organisation name
    /// and sign-in methods, asked for once while signed out. The GTK app
    /// keeps it in `login_config`.
    private(set) var loginConfig: LoginConfig?
    @ObservationIgnored private var loginConfigAsked = false
    /// The workspace a connection link asked to sign in to, kept until
    /// sign-in starts or is cancelled. The GTK app keeps the link
    /// in `pending_connect`.
    @ObservationIgnored private var connectionWorkspace: String?
    /// From Log out until the core has answered and its snapshot no longer
    /// shows the account.
    private(set) var signingOut = false

    private(set) var status: UploaderStatus? {
        didSet {
            if oldValue?.sharingMode != status?.sharingMode || oldValue?.uploadPolicyRevision != status?.uploadPolicyRevision { hiddenConfirmed = [] }
            // SessionUploadState reads only the gate; delivery counters
            // reach the footer without rebuilding every search row.
            updateDerivedStatus()
        }
    }
    private(set) var sessions: [UploadSession] = [] {
        didSet { if oldValue != sessions { sessionRowsRevision &+= 1 } }
    }
    private(set) var previewSessionIDs: Set<String> = [] {
        didSet { if oldValue != previewSessionIDs { sessionSearchMetadataRevision &+= 1 } }
    }
    private(set) var sessionPreparationStarted: Date?
    private(set) var isLoadingPreview = false
    /// Why the titles found on this Mac could not be read while history prepares.
    private(set) var previewError: String?
    private(set) var lastSessionsRefresh: Date?
    private(set) var isLoadingSessions = false
    private(set) var sessionsError: String? { didSet { updateRelayProblem() } }
    private(set) var sessionActionError: String?
    /// Why compacting the journal or retrying stopped uploads failed.
    private(set) var uploadActionError: String?
    private(set) var workDescription: String?
    private(set) var isWorking = false
    /// The upload policy asked for while `uploader.sharing.set` answers.
    private(set) var sharingModeWanted = PendingValue<SharingMode>()
    /// Paused or resumed, as asked for, while the uploader answers.
    private(set) var pausedWanted = PendingValue<Bool>()
    /// The stage action asked for (retry or compact) until its answer.
    private(set) var pendingUploadAction: UploadStage.Action?
    /// A stage action the core never received because it could not be
    /// reached, and whether it is still to be sent again on its own once
    /// the core answers (only the person's own click is).
    @ObservationIgnored private var unsentUploadAction: (action: UploadStage.Action, replay: Bool)?
    @ObservationIgnored private var drivingSharingMode = false
    @ObservationIgnored private var drivingPause = false
    private(set) var requestingUploads: Set<String> = [] {
        didSet { if oldValue != requestingUploads { sessionSearchMetadataRevision &+= 1; sessionQueryChanged() } }
    }
    private(set) var stoppingUploads: Set<String> = [] {
        didSet { if oldValue != stoppingUploads { sessionSearchMetadataRevision &+= 1; sessionQueryChanged() } }
    }
    private(set) var uploadErrors: [String: String] = [:]
    @ObservationIgnored private var uploadTasks: [String: Task<Bool, Never>] = [:]
    private(set) var sessionLimit = AppModel.sessionPage
    var sessionUploadFilter: SessionUploadFilter = .all
    var selectedLocalSession: LocalSessionRow? {
        didSet {
            if selectedLocalSession != nil { showsSessionSearch = false }
            if oldValue?.id != selectedLocalSession?.id { showsSessionDetails = false }
        }
    }
    var showsSessionDetails = false
    var sessionSearch = "" { didSet { if oldValue != sessionSearch { sessionQueryChanged(); scheduleSearchTelemetry(sessionSearch, surface: .sessions) } } }
    private(set) var queryArchivedSessionKeys: Set<String>? { didSet { if oldValue != queryArchivedSessionKeys { sessionSearchMetadataRevision &+= 1 } } }
    private(set) var queriedLocalSessions: [LocalSession]? { didSet { if oldValue != queriedLocalSessions { sessionRowsRevision &+= 1 } } }
    private(set) var sessionNextCursor: String?
    private(set) var sessionFacets: SessionFacetsPage? { didSet { if oldValue != sessionFacets { sessionRowsRevision &+= 1 } } }
    @ObservationIgnored private var sessionFacetsTask: Task<Void, Never>?
    private(set) var sessionCursor: String?
    @ObservationIgnored private var sessionQueryTask: Task<Void, Never>?
    @ObservationIgnored private var hiddenConfirmed: Set<String> = []
    @ObservationIgnored private var hiddenRetryTask: Task<Void, Never>?
    @ObservationIgnored private var hiddenRetryAttempt = 0
    @ObservationIgnored private var sessionQueryRevision = 0
    private(set) var sessionVerificationPending = false
    @ObservationIgnored private var sessionKnownRevision: Int?
    func sessionPresentationChanged() { sessionQueryChanged() }
    func localSessionPresentationChanged() {
        // Let an in-flight first page publish, then coalesce one current read.
        // Completed pages reset so a new live row is not hidden off-page.
        if queriedLocalSessions == nil { Task { await refreshSessions() } }
        else { sessionQueryChanged() }
    }
    private func sessionQueryChanged() {
        sessionQueryRevision += 1
        sessionKnownRevision = nil
        sessionCursor = nil
        sessionNextCursor = nil
        sessionQueryTask?.cancel()
        guard phase == .attached else { return }
        sessionQueryTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 80_000_000)
            guard !Task.isCancelled, let self else { return }
            await self.refreshSessions()
        }
    }
    func firstSessionPage() async { sessionQueryChanged(); sessionQueryTask?.cancel(); await refreshSessions() }

    var showsSessionSearch = false
    var sessionSort = LocalSessionSort() { didSet { if oldValue != sessionSort { sessionQueryChanged() } } }
    var savedSessionViews = SavedSessionViews() {
        didSet { defaults.set(savedSessionViews.data, forKey: SavedSessionViews.preferenceKey) }
    }
    var sessionViewConfiguration: SessionViewConfiguration {
        SessionViewConfiguration(search: sessionSearch, grouping: sessionGrouping, sort: sessionSort,
                                 columns: sessionColumns, showHidden: showHiddenSessions)
    }
    var currentSavedSessionView: SavedSessionView? {
        let configuration = sessionViewConfiguration
        return savedSessionViews.views.first { $0.configuration == configuration }
    }
    func applySessionView(_ view: SavedSessionView) {
        let configuration = view.configuration
        sessionSearch = configuration.search
        sessionGrouping = configuration.grouping
        sessionSort = configuration.sort.column == .reasoning && client.offers(CoreMethod.uploaderSessionsQuery) ? LocalSessionSort() : configuration.sort
        sessionColumns = SessionColumnLayout(data: configuration.columns)
        showHiddenSessions = configuration.showHidden
        selectedLocalSession = nil
        showsSessionSearch = false
        page = .sessions
    }
    var sessionGrouping: LocalSessionGrouping = .none
    private(set) var hiddenSessionIDs: Set<String> = [] {
        didSet { defaults.set(Array(hiddenSessionIDs).sorted(), forKey: "hiddenSessions.v1"); if oldValue != hiddenSessionIDs { sessionQueryChanged() } }
    }
    private(set) var hidingSessionIDs: Set<String> = []
    var showHiddenSessions = false {
        didSet { defaults.set(showHiddenSessions, forKey: "showHiddenSessions.v1"); if oldValue != showHiddenSessions { sessionQueryChanged() } }
    }

    var sessionColumns = SessionColumnLayout() {
        didSet { defaults.set(sessionColumns.data, forKey: SessionColumnLayout.preferenceKey) }
    }
    /// Sessions offers to put Capy threads on the relay.
    private(set) var showsCapySetup = false
    /// Whether Capy is on this Mac; asked at launch and whenever the app
    /// becomes active, so installing Capy later shows the card.
    @ObservationIgnored private let detectCapy: () -> Bool
    /// Set up in Capy copied its prompt.
    private(set) var capyPromptCopied = false
    private(set) var cloudSetupStarted: Set<String> = []
    /// Not now was chosen on the Devin card.
    private(set) var devinSetupDismissed = false
    /// Why Turn On on the Devin card failed; the core's sentence.
    private(set) var devinSetupError: String?
    /// Sessions offers Agent Relay's hooks to Devin while the core reports
    /// them off.
    var showsDevinSetup: Bool {
        DevinSetup.shows(hooks: settings?.devinDirectDelivery, dismissed: devinSetupDismissed)
    }

    var page: DesktopPage = .sessions {
        didSet {
            if page != .sessions { showsSessionSearch = false }
            if oldValue != page {
                recordPageView()
                if (oldValue == .settings(.diagnostics)) != (page == .settings(.diagnostics)) { readRequestLogWhileShown(restart: true) }
            }
        }
    }
    @ObservationIgnored private var lastTelemetrySurface: TelemetrySurface?
    @ObservationIgnored private var pendingMessageComposerOpens = 0
    @ObservationIgnored private var searchTelemetryTasks: [TelemetrySurface: Task<Void, Never>] = [:]
    /// Whether the message composer is open; `agentrelay://` message links open it.
    var isComposingMessage = false {
        didSet {
            guard oldValue != isComposingMessage else { return }
            if isComposingMessage {
                if hasRecordedLaunch { record(.messageComposer, .started, surface: .messages) }
                else { pendingMessageComposerOpens += 1 }
            }
            recordPageView()
        }
    }
    /// The message composer's target; not secret.
    var messageEndpoint = ""

    /// The core's settings as Settings shows them: the last ones the core
    /// reported, with every change still on its way written over them.
    private(set) var settings: SettingsValues?
    /// The settings as the core last reported them.
    @ObservationIgnored private var confirmedSettings: SettingsValues? { didSet { showSettings() } }
    /// Changes sent and not yet answered, oldest first, each under the
    /// token its answer carries.
    private var pendingSettings: [(token: UInt64, change: SettingsChange)] = [] { didSet { showSettings() } }
    @ObservationIgnored private var settingsWriteToken: UInt64 = 0
    /// Why the last change to a setting failed, by its JSON key, until the
    /// setting is changed again.
    private(set) var settingErrors: [String: String] = [:]
    /// Counts the core's Settings invalidations, so rows that read their own
    /// status (Agent Relay tools) read it again, as Linux does.
    private(set) var settingsInvalidations = 0
    var launchAtLogin = false
    private(set) var loginItemNeedsApproval = false
    /// Quitting leaves the core running unless the person turned this off;
    /// then quitting unregisters its LaunchAgent until the app opens again.
    private(set) var keepsAgentsConnectedAfterQuit: Bool

    let relay: RelayModel
    let claudeDelivery: ClaudeDeliveryModel

    static let sessionPage = 500
    /// The core binary the LaunchAgent was last registered with. Read off
    /// the main actor too (`ProductTakeover.uncarriedDefaults`).
    nonisolated static let registeredCoreKey = "registeredCoreBinary"
    static let keepsAgentsConnectedKey = "keepAgentsConnectedAfterQuit"

    // MARK: Private state

    private(set) var requestEntries: [RequestEntry] = []
    private(set) var requestFailure: RequestEntry?
    var selectedRequest: RequestEntry?
    private(set) var requestLogError: String?
    @ObservationIgnored private var seenRequestFailures: Set<String> = []
    @ObservationIgnored private var readingRequests = false
    private(set) var relaycastUpgrade: RelaycastUpgrade.Reason?
    var showRelaycastUpgrade: Bool { relaycastUpgrade != nil }
    @ObservationIgnored private var lastRelaycastUpgradeAt: Date?
    @ObservationIgnored private var requestIdentityRevision: UInt64 = 0
    @ObservationIgnored private var requestIdentityStartedAtMs: Int = 0

    func dismissRelaycastUpgrade() { relaycastUpgrade = nil }

    func openRelaycastBilling() {
        guard let url = RelaycastUpgrade.billingURL(siteURL: siteURL, reason: relaycastUpgrade ?? .planLimit) else { return }
        NSWorkspace.shared.open(url)
        dismissRelaycastUpgrade()
    }

    /// Manage plan / Upgrade plan: the plan's own link, else Cloud Billing.
    func openPlan() {
        guard let url = WorkspacePlanPresentation.manageURL(workspacePlan, siteURL: siteURL) else { return }
        NSWorkspace.shared.open(url)
    }

    /// Opens the dashboard's Workspace settings, where an account with no
    /// workspace creates one before signing in again.
    func openWorkspaceSettings() {
        guard let url = WorkspaceSetup.dashboardURL(siteURL: siteURL) else { return }
        NSWorkspace.shared.open(url)
    }

    /// Connections in the signed-in site's Agent Relay Cloud, where Amp connects.
    var ampConnectionsURL: URL? { AmpConnections.url(siteURL: siteURL) }

    /// Opens Connections in Agent Relay Cloud, where Amp connects.
    func openAmpConnections() {
        guard let url = ampConnectionsURL else { return }
        NSWorkspace.shared.open(url)
    }

    /// Opens a new Capy thread with the setup prompt, and copies it as a fallback.
    func setUpCapy() {
        setUpCloudProvider(.capy)
    }

    func setUpCloudProvider(_ setup: CloudProviderSetup) {
        if let prompt = setup.prompt {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(prompt, forType: .string)
        }
        cloudSetupStarted.insert(setup.rawValue)
        if setup == .capy { capyPromptCopied = true }
        if !setup.copyOnly { openCloudProviderApp(setup) }
    }

    func openCloudProviderApp(_ setup: CloudProviderSetup) {
        if setup == .museAssistant,
           let app = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.meta.endo") {
            NSWorkspace.shared.openApplication(at: app, configuration: NSWorkspace.OpenConfiguration()) { _, error in
                if error != nil { DispatchQueue.main.async { NSWorkspace.shared.open(setup.webURL) } }
            }
            return
        }
        Self.openCloudProvider(setup,
            appAvailable: { NSWorkspace.shared.urlForApplication(toOpen: $0) != nil },
            open: { NSWorkspace.shared.open($0) })
    }

    @discardableResult
    static func openCloudProvider(_ setup: CloudProviderSetup, appAvailable: (URL) -> Bool,
                                  open: (URL) -> Bool) -> Bool {
        if let appURL = setup.appURL, appAvailable(appURL), open(appURL) { return true }
        return open(setup.webURL)
    }

    /// Looks for Capy again.
    func refreshCapySetup() {
        let shows = CapySetup.shows(installed: detectCapy(), dismissed: defaults.bool(forKey: CapySetup.dismissedKey))
        if shows != showsCapySetup { showsCapySetup = shows }
    }

    /// Not now: hides the Capy card for good.
    func dismissCapySetup() {
        defaults.set(true, forKey: CapySetup.dismissedKey)
        showsCapySetup = false
    }

    /// Turn On on the Devin card: adds the hooks. The card goes once the
    /// core reports them on; a failure shows the core's sentence under it.
    func turnOnDevin() async {
        devinSetupError = await setSetting(SettingsChange(devinDirectDelivery: true))
    }

    /// Not now: hides the Devin card for good.
    func dismissDevinSetup() {
        defaults.set(true, forKey: DevinSetup.dismissedKey)
        devinSetupDismissed = true
    }

    /// Reads the request log: Diagnostics shows it, so its entries are
    /// published only while that pane is on screen; a new failure and a
    /// plan-limit notice are noticed on any page.
    func refreshRequestLog() async {
        guard !readingRequests else { return }
        let identityRevision = requestIdentityRevision
        readingRequests = true
        defer { readingRequests = false }
        do {
            let result = try await client.diagnosticsRequests()
            if page == .settings(.diagnostics), requestEntries != result.entries { requestEntries = result.entries }
            if requestLogError != nil { requestLogError = nil }
            let failed = result.entries.filter(\.failed)
            let now = Date()
            let recent = failed.filter {
                $0.startedAtMs >= requestIdentityStartedAtMs &&
                abs(now.timeIntervalSince1970 - Double($0.startedAtMs) / 1000) < 300
            }
            if identityRevision == requestIdentityRevision,
               let reason = RelaycastUpgrade.reason(for: recent, plan: core?.workspace.workspacePlan,
                                                    unseen: { !seenRequestFailures.contains($0.id) }),
               lastRelaycastUpgradeAt.map({ now.timeIntervalSince($0) >= 900 }) ?? true {
                relaycastUpgrade = reason
                lastRelaycastUpgradeAt = now
            }
            if let newest = failed.first(where: { !seenRequestFailures.contains($0.id) }) {
                requestFailure = newest
            }
            seenRequestFailures.formUnion(failed.map(\.id))
            if seenRequestFailures.count > 1000 { seenRequestFailures = Set(failed.map(\.id)) }
        } catch {
            requestLogError = "Request log unavailable. The background service may need to be updated or restarted."
        }
    }

    func openFailedRequest() {
        guard let entry = requestFailure else { return }
        selectedRequest = entry
        requestFailure = nil
        page = .settings(.diagnostics)
    }

    let client: CoreClient
    let tools: ToolsModel
    private let service: CoreService
    private let loginItem: any LoginItemManaging
    /// The core version this bundle carries; a running core of another
    /// version is asked to stop so launchd starts this one.
    private let bundledCoreVersion: String?
    /// How long launchd takes to stop an unregistered core (its exit timeout is 5 s).
    private let unregisterSettle: UInt64
    /// Restarts asked of a core of another version, by version, not by pid:
    /// bad bundle metadata must not kill every replacement of an otherwise
    /// bridge-compatible core. The first core of another version is stopped
    /// at once. A manual install can leave launchd starting the old build
    /// again, so a replacement still on it is used, and rechecked after a
    /// growing delay (`coreVersionRecheckDelay`) until the version has had
    /// `coreVersionRestartLimit` restarts; then the person is told.
    @ObservationIgnored private(set) var coreVersionRestarts: [String: Int] = [:]
    static let coreVersionRestartLimit = 3
    /// The first recheck's delay; each later one waits a multiple of it.
    private let coreVersionRecheckDelay: UInt64
    /// The pending recheck of a replacement core still on another version.
    @ObservationIgnored private var coreVersionRecheck: Task<Void, Never>?
    /// What the person is told when the core still runs another build after
    /// every restart attempt.
    static let staleCoreMessagePrefix = "The background service is still running Agent Relay "
    static func staleCoreMessage(running: String, bundled: String) -> String {
        staleCoreMessagePrefix + "\(running), not \(bundled). Quit Agent Relay and open it again from Applications; if this stays, restart your computer."
    }
    /// Cores asked to stop, by pid. One serves until it exits, so a
    /// connection to it waits for launchd's replacement instead.
    @ObservationIgnored private(set) var retiredCorePIDs: Set<Int> = []
    /// Registrations made again for a core launchd refuses.
    @ObservationIgnored private var refusalRepairs = 0
    static let refusalRepairLimit = 8
    private let configuredSite: String
    private let manageSystemServices: Bool
    private let defaults: UserDefaults
    @ObservationIgnored private var connectTask: Task<Void, Never>?
    @ObservationIgnored private var pollTask: Task<Void, Never>?
    @ObservationIgnored private var initialRefreshTask: Task<Void, Never>?
    @ObservationIgnored private var startupWatchdog: Task<Void, Never>?
    private let startupTimeout: UInt64
    @ObservationIgnored private var openedSignInLinks: Set<String> = []
    private let openSignInBrowser: ((URL) -> Bool)?
    @ObservationIgnored private var openWindowAction: ((String) -> Void)?
    @ObservationIgnored private var wantsMainWindow = false
    /// The main window is on screen: ordered in and not covered.
    @ObservationIgnored private(set) var windowOpen = false
    /// The menu bar panel is on screen.
    @ObservationIgnored private(set) var panelOpen = false
    /// How many times the menu panel read its summary. A closed panel reads
    /// nothing, whatever else changes.
    @ObservationIgnored private(set) var menuSummaryReads = 0
    /// How many times the summary was counted: once per change to what it
    /// counts, however often it is read.
    @ObservationIgnored private(set) var menuSummaryBuilds = 0
    @ObservationIgnored private var menuSummaryCache: (key: MenuSummaryKey, summary: MenuSummary)?
    /// The request log's reader: runs while a window or the panel shows.
    @ObservationIgnored private var requestLogTask: Task<Void, Never>?
    /// The Sessions table's results: searched, sorted and grouped off the
    /// main thread, while the table is on screen.
    let sessionResults = SessionSearchProjection()
    @ObservationIgnored private var sessionResultsFeed: ModelReaction<SessionResultsInput>?
    @ObservationIgnored private var appliedSessionResultsInput: SessionResultsInput?
    @ObservationIgnored private var sessionTableShown = false
    @ObservationIgnored private var hasBootstrapped = false
    @ObservationIgnored private var migratedSettings = false
    /// Bumped whenever the uploader's connection changes, so answers to
    /// requests made before are dropped.
    @ObservationIgnored private var connectionRevision = 0
    /// The reads a new connection makes, one after another. A sign-out
    /// between them ends it, so no read starts for the signed-out account.
    private(set) var connectionRefresh: Task<Void, Never>?
    @ObservationIgnored private var previousPhase: Phase = .launching { didSet { updatePhase() } }
    /// A workspace connection is running; its browser approval does not
    /// sign anyone out.
    var connectingWorkspace = false { didSet { updatePhase() } }
    /// A sign-in renewing the session the core calls Cloud with while this
    /// Mac stays connected; its browser approval does not sign anyone out.
    private(set) var renewingSignIn = false { didSet { updatePhase() } }
    /// The core answered that renewal with its approval running, so it ends
    /// at the first snapshot without one, even when a lagging event stream
    /// skipped the `pending_approval` snapshot itself.
    @ObservationIgnored private var renewalAnswered = false

    init(client: CoreClient = CoreClient(socketPath: AppInfo.coreSocket),
         service: CoreService = LaunchAgentService(),
         bundledCoreVersion: String? = AppInfo.bundledCoreVersion,
         unregisterSettle: UInt64 = 6_000_000_000,
         startupTimeout: UInt64 = 20_000_000_000,
         coreVersionRecheckDelay: UInt64 = 30_000_000_000,
         siteURL: String = AppInfo.defaultSiteURL,
         manageSystemServices: Bool = true,
         defaults: UserDefaults = .standard,
         detectCapy: @escaping () -> Bool = { false },
         loginItem: any LoginItemManaging = SystemLoginItem(),
         openSignInBrowser: ((URL) -> Bool)? = nil) {
        self.client = client
        if let openSignInBrowser {
            self.openSignInBrowser = openSignInBrowser
        } else if manageSystemServices {
            self.openSignInBrowser = { NSWorkspace.shared.open($0) }
        } else {
            self.openSignInBrowser = nil
        }
        self.tools = ToolsModel(socketPath: client.socketPath)
        self.service = service
        self.loginItem = loginItem
        self.bundledCoreVersion = bundledCoreVersion
        self.unregisterSettle = unregisterSettle
        self.coreVersionRecheckDelay = coreVersionRecheckDelay
        self.startupTimeout = startupTimeout
        configuredSite = siteURL
        self.manageSystemServices = manageSystemServices
        self.defaults = defaults
        self.keepsAgentsConnectedAfterQuit = defaults.object(forKey: Self.keepsAgentsConnectedKey) as? Bool ?? true
        self.sessionColumns = SessionColumnLayout(data: defaults.data(forKey: SessionColumnLayout.preferenceKey))
        self.hiddenSessionIDs = Set((defaults.stringArray(forKey: "hiddenSessions.v1") ?? []).map(LocalSessionRow.currentHiddenID))
        self.showHiddenSessions = defaults.bool(forKey: "showHiddenSessions.v1")
        self.detectCapy = detectCapy
        self.showsCapySetup = CapySetup.shows(installed: detectCapy(),
                                              dismissed: defaults.bool(forKey: CapySetup.dismissedKey))
        self.devinSetupDismissed = defaults.bool(forKey: DevinSetup.dismissedKey)
        relay = RelayModel(client: client)
        claudeDelivery = ClaudeDeliveryModel(client: client)
        if manageSystemServices {
            AppUpdater.shared.recordTelemetry = { [weak self] event in
                guard let self else { return }
                _ = try? await self.client.telemetryRecord(event)
            }
            AppUpdater.shared.feedHeaders = { [weak self] site in
                guard let self, let account = self.accountID, let workspace = self.workspaceID else { return nil }
                let params = FeedHeadersParams(
                    accountId: account, profileId: AppInfo.profileID, siteUrl: site, workspaceId: workspace)
                return try? await self.client.accountFeedHeaders(params).headers
            }
        }
        relay.model = self
        savedSessionViews = SavedSessionViews(data: defaults.data(forKey: SavedSessionViews.preferenceKey))
        if let view = savedSessionViews.defaultView { applySessionView(view) }
        sessionResultsFeed = ModelReaction(read: { [unowned self] in sessionResultsInput }, apply: { [unowned self] in scheduleSessionResults($0) })
    }

    // MARK: Derived state

    private func updatePhase() {
        let current = computedPhase
        if phase != current { phase = current }
    }

    /// Connected, but the core holds no Cloud session to act as the person:
    /// Cloud refused it even after a refresh, or the CLI's sign-in it
    /// borrowed expired. Cloud actions wait until the person signs in again.
    var needsSignIn: Bool { phase == .attached && account.needsSignIn == true }

    private var computedPhase: Phase {
        guard let core else { return .launching }
        let account = core.account
        switch account.state {
        case .signedIn: return .attached
        case .preparing:
            return account.phase == "preparing_local_sessions" ? .preparingSessions : .signingIn
        case .pendingApproval:
            return (connectingWorkspace || renewingSignIn) && previousPhase == .attached ? .attached : .waitingForBrowser
        case .signedOut, .denied, .expired, .error:
            return .signedOut
        }
    }

    var accountID: String? { account.person?.id }
    var accountEmail: String? { account.person?.email }
    var accountName: String? { account.person?.name }
    var hasMultipleWorkspaces: Bool { (account.workspaceCount ?? 0) > 1 }
    var workspaceAvatarURL: String? { account.workspaceAvatarUrl }
    var accountAvatarURL: String? { account.person?.avatarUrl }
    var workspaceID: String? { account.workspaceId }
    var workspaceName: String {
        if let name = account.workspaceName?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty { return name }
        return "Workspace"
    }
    var siteURL: String { account.siteUrl ?? configuredSite }
    /// The sign-in screen's "Development" badge: the site of a core for
    /// another site than production, nil for production. The core's site
    /// once it reported one, else this build's own: the app and the core
    /// are normally one build, but the core's word wins where it has
    /// spoken. Linux and Windows decide the same way (`models::development_badge`).
    var developmentBadge: String? { Self.developmentBadge(reported: account.siteUrl, builtIn: configuredSite) }

    /// `developmentBadge` for an app built for `builtIn` whose core
    /// reported `reported`.
    static func developmentBadge(reported: String?, builtIn: String) -> String? {
        let site = reported ?? builtIn
        return site == AppInfo.productionSiteURL ? nil : site
    }
    /// The Agent Relay plan Cloud reports for the workspace; nil until read
    /// and when the organization has none.
    var workspacePlan: WorkspacePlan? { core?.workspace.plan }
    var workspaceUsage: WorkspaceUsage? { core?.workspace.usage }
    /// The full indexed history, independent of loaded rows, filters and hidden sessions.
    private(set) var localSessionCountText = "Counting sessions…"
    /// The uploader's status as the window and panel show it.
    private(set) var statusViewModel: StatusViewModel? { didSet { updateRelayProblem() } }
    /// What a session's upload state reads from the status.
    private(set) var uploadGate = UploaderGate(status: nil) {
        didSet { sessionSearchMetadataRevision &+= 1 }
    }
    /// The upload policy and install directory the status reports; nil
    /// without a status.
    private(set) var uploaderSharingMode: SharingMode?
    private(set) var uploaderDirectory: String?
    /// What stops messages or uploads right now, for the banners and the
    /// menu bar icon.
    private(set) var relayProblem: String?

    /// Keeps what the window shows of the status and phase; each value is
    /// assigned only when it changed, so a status whose counters alone
    /// moved re-renders only what shows them.
    private func updateDerivedStatus() {
        let text: String
        if let status, !status.upgrading, phase != .preparingSessions {
            let count = status.sessions.total
            text = "\(count) \(count == 1 ? "session" : "sessions") on this machine"
        } else {
            text = "Counting sessions…"
        }
        if localSessionCountText != text { localSessionCountText = text }
        let shown = status.map(StatusViewModel.init(status:))
        if statusViewModel != shown { statusViewModel = shown }
        let gate = UploaderGate(status: status)
        if uploadGate != gate { uploadGate = gate }
        if uploaderSharingMode != status?.sharingMode { uploaderSharingMode = status?.sharingMode }
        if uploaderDirectory != status?.directory { uploaderDirectory = status?.directory }
    }

    func updateRelayProblem() {
        let problem = computedRelayProblem
        if relayProblem != problem { relayProblem = problem }
    }

    private var computedRelayProblem: String? {
        if let error = coreProblem ?? staleCoreWarning ?? sessionsError ?? lastError { return error }
        if let status = statusViewModel, case .needsAttention(let message) = status.state { return message }
        if let error = agentsStatus?.storeError ?? agentsStatus?.integrationError { return error }
        if relay.sessions.contains(where: { $0.registration != nil }), let error = agentsStatus?.deliveryError {
            return "Can't reach the relay: \(error)"
        }
        return nil
    }

    var installDirectory: String? { uploaderDirectory }
    var captureProgress: CaptureProgress? { account.progress }
    var bridgeVersion: Int? { core?.daemon.bridgeVersion }
    /// Paused as the person last asked, else as the uploader reports.
    var isPaused: Bool { pausedWanted.shown(uploadGate.paused) }
    var canLogOut: Bool { (phase == .attached || phase == .preparingSessions) && !signingOut }
    var analyticsEnabled: Bool { settings?.analyticsEnabled ?? true }
    /// The upload policy as the person last chose it, else as the uploader
    /// reports it.
    var sharingMode: SharingMode { sharingModeWanted.shown(uploaderSharingMode ?? .selected) }
    var isChangingSharingMode: Bool { sharingModeWanted.isPending }
    /// The upload policy the workspace's operator enforces, if any; the
    /// picker is removed.
    var enforcedSharingMode: SharingMode? {
        OrganizationPolicy.sharingMode(in: status?.enforcedSettings)
    }
    /// The operator runs uploads; the person may not pause or resume them.
    var isUploadControlEnforced: Bool {
        OrganizationPolicy.controlsUploads(in: status?.enforcedSettings)
    }
    /// The person may delete their sessions from Cloud; the operator can
    /// withdraw it, which removes every delete control.
    var canDeleteFromCloud: Bool {
        OrganizationPolicy.allowsDelete(in: status?.enforcedSettings)
    }
    /// Uploads wait until the operator's policy can be read; the person
    /// cannot change the upload policy meanwhile.
    var areUploadsWithheld: Bool { status?.uploadsWithheld == true }
    var isSessionTableReadOnly: Bool { sharingMode == .all || phase != .attached }
    var isShowingSessionPreview: Bool { phase == .preparingSessions || !previewSessionIDs.isEmpty }

    var isPreparingSessionList: Bool {
        phase == .preparingSessions || captureProgress?.isCapturing == true || !previewSessionIDs.isEmpty
            || (lastSessionsRefresh == nil && isLoadingSessions)
    }


    var sessionUploadSummary: SessionUploadSummary? {
        statusViewModel.map { SessionUploadSummary(status: $0, sessions: sessions, requesting: requestingUploads) }
    }

    func uploadState(for session: UploadSession) -> SessionUploadState {
        if previewSessionIDs.contains(session.id) { return .discovering }
        if requestingUploads.contains(session.id) { return .preparing }
        return SessionUploadState(session: session, gate: uploadGate)
    }

    func sessionCount(for filter: SessionUploadFilter) -> Int {
        sessions.filter { filter.matches(uploadState(for: $0)) }.count
    }

    func canSelectSession(_ row: UploadSession) -> Bool {
        !isSessionTableReadOnly && !previewSessionIDs.contains(row.id)
    }

    // MARK: Window plumbing

    /// SwiftUI hands the app its `openWindow` action from a live view.
    func registerOpenWindow(_ action: @escaping (String) -> Void) {
        openWindowAction = action
        if wantsMainWindow { showMainWindow() }
    }

    func showSettingsWindow(_ pane: SettingsPane = .general) {
        // Change the covered page first so dismissing the composer records only Settings.
        page = .settings(pane.destination)
        isComposingMessage = false
        showMainWindow()
    }

    func showMainWindow() {
        guard manageSystemServices else { openWindowAction?(AppInfo.mainWindowID); return }
        NSApp.activate(ignoringOtherApps: true)
        if let openWindowAction {
            wantsMainWindow = false
            openWindowAction(AppInfo.mainWindowID)
            return
        }
        wantsMainWindow = true
        for window in NSApp.windows where window.identifier?.rawValue.contains(AppInfo.mainWindowID) == true {
            wantsMainWindow = false
            window.makeKeyAndOrderFront(nil)
            return
        }
    }

    func showSessionUploads() {
        isComposingMessage = false
        page = .sessions
        showMainWindow()
    }

    /// Uploaded sessions open in Cloud; others open their row in Session Uploads.
    func openSession(_ session: UploadSession) {
        if let url = SessionNavigation.cloudURL(siteURL: siteURL, session: session) {
            NSWorkspace.shared.open(url)
            return
        }
        sessionUploadFilter = .all
        sessionSearch = session.sessionId
        showSessionUploads()
    }

    func panelDidAppear() {
        guard !panelOpen else { return }
        panelOpen = true
        refreshRelayOnReveal()
        readRequestLogWhileShown()
        Task { await refreshVisibleState() }
    }

    func panelDidDisappear() {
        guard panelOpen else { return }
        panelOpen = false
        readRequestLogWhileShown()
    }

    /// The main window came on screen or went off it (closed, minimized,
    /// hidden or covered). Everything that runs for it runs only while it
    /// is shown.
    func windowVisibilityChanged(_ visible: Bool) {
        let revealed = visible && !windowOpen
        guard visible != windowOpen else { return }
        windowOpen = visible
        if revealed { refreshRelayOnReveal() }
        readRequestLogWhileShown()
        relay.windowVisibilityChanged(visible)
    }

    // MARK: Request log

    /// The core announces no change to the request log, so it is read while
    /// someone could see what it says: every 2 s while Diagnostics shows,
    /// and every 30 s on any other page or in the panel, so a failed request
    /// or a plan limit still shows. Nothing reads it while the app is out
    /// of sight.
    private func readRequestLogWhileShown(restart: Bool = false) {
        let shown = windowOpen || panelOpen
        guard shown else {
            requestLogTask?.cancel()
            requestLogTask = nil
            return
        }
        guard restart || requestLogTask == nil else { return }
        requestLogTask?.cancel()
        requestLogTask = Task { [weak self] in
            while let self, !Task.isCancelled {
                await self.refreshRequestLog()
                let interval: UInt64 = self.page == .settings(.diagnostics) && self.windowOpen ? 2_000_000_000 : 30_000_000_000
                do { try await Task.sleep(nanoseconds: interval) } catch { return }
            }
        }
    }

    // MARK: Menu summary

    /// The menu panel's four counts. They are counted again only after
    /// something they count changed: a session, its upload or relay state,
    /// or which sessions are hidden. Reading them registers the panel for
    /// exactly those changes.
    func menuSummary() -> MenuSummary {
        menuSummaryReads += 1
        let key = MenuSummaryKey(rows: sessionRowsRevision, metadata: sessionSearchMetadataRevision,
                                 relayRows: relay.sessionRowsRevision, relayMetadata: relay.sessionSearchMetadataRevision,
                                 hidden: hiddenSessionIDs, showHidden: showHiddenSessions)
        if let cached = menuSummaryCache, cached.key == key { return cached.summary }
        let summary = MenuSummary(counts: MenuSummaryFilter.counts(menuSessionRows(relay: relay)) { $0.upload.map(uploadState) })
        menuSummaryBuilds += 1
        menuSummaryCache = (key, summary)
        return summary
    }

    // MARK: Session table

    /// What the Sessions table's results are made from.
    struct SessionResultsInput: Equatable {
        /// Lists and the state of their rows, by revision.
        var rows: UInt64
        var metadata: UInt64
        var relayRows: UInt64
        var relayMetadata: UInt64
        var search: String
        /// How the results are arranged: a change shows at once.
        var sort: LocalSessionSort
        var grouping: LocalSessionGrouping
        var hidden: Set<String>
        var showHidden: Bool

        func arrangementDiffers(from other: Self) -> Bool {
            sort != other.sort || grouping != other.grouping || hidden != other.hidden || showHidden != other.showHidden
        }
    }

    private var sessionResultsInput: SessionResultsInput {
        SessionResultsInput(rows: sessionRowsRevision, metadata: sessionSearchMetadataRevision,
                            relayRows: relay.sessionRowsRevision, relayMetadata: relay.sessionSearchMetadataRevision,
                            search: sessionSearch, sort: sessionSort, grouping: sessionGrouping,
                            hidden: hiddenSessionIDs, showHidden: showHiddenSessions)
    }

    /// The Sessions table came on screen or left it, its window included.
    func sessionTableVisibilityChanged(_ shown: Bool) {
        guard sessionTableShown != shown else { return }
        sessionTableShown = shown
        feedSessionResults()
    }

    private func feedSessionResults() {
        if sessionTableShown {
            sessionResultsFeed?.start()
        } else {
            sessionResultsFeed?.stop()
            sessionResults.cancel()
            appliedSessionResultsInput = nil
        }
    }

    /// Typing and list changes settle for 80 ms; a new sort, grouping or
    /// hidden set shows at once.
    private func scheduleSessionResults(_ input: SessionResultsInput) {
        let previous = appliedSessionResultsInput
        guard previous != input else { return }
        appliedSessionResultsInput = input
        let immediate = previous.map { input.arrangementDiffers(from: $0) } ?? true
        sessionResults.schedule(delay: immediate ? 0 : 80_000_000) { [unowned self] in
            SessionSearchSnapshot(source: sessionSearchSource.snapshot(model: self, relay: relay), query: sessionSearch,
                                  sort: sessionSort, grouping: sessionGrouping, hiddenSessionIDs: hiddenSessionIDs,
                                  showHiddenSessions: showHiddenSessions)
        }
    }

    /// A reveal is fresh discovery demand, independent of history/status
    /// polling. The core returns its cache promptly and coalesces provider
    /// work; later invalidations bring new rows into an already open page.
    private func refreshRelayOnReveal() {
        Task { await relay.refresh(.sessions) }
    }

    func quit() { NSApp.terminate(nil) }

    // MARK: Launch

    /// Moves what earlier builds kept, registers the core's LaunchAgent and
    /// connects to it.
    func bootstrapIfNeeded() async {
        if hasBootstrapped { return }
        hasBootstrapped = true
        if manageSystemServices {
            loginItem.removeIfUnavailable()
            launchAtLogin = loginItem.isEnabled
            loginItemNeedsApproval = loginItem.needsApproval
            do {
                coreRegistrationPending = true
                defer { coreRegistrationPending = false }
                try await service.setStartsAtLoginAsync(launchAtLogin || loginItemNeedsApproval)
            } catch {
                coreProblem = "Agent Relay could not change the background service login setting: \(error.localizedDescription)"
            }
            // What earlier builds of the production app kept belongs to the
            // production app, not to a build for another site.
            if AppInfo.bundleIdentifier == AppInfo.productionBundleIdentifier {
                do {
                    try LegacyMigration.moveConversations(
                        from: LegacyMigration.legacyConversations(supportDirectory: AppInfo.applicationSupport),
                        to: AppInfo.dataDirectory.appendingPathComponent("Conversations")
                    )
                } catch {
                    Self.logger.error("could not move Skip conversations: \(error.localizedDescription, privacy: .public)")
                }
            }
        }
        watchStartup()
        await ensureService()
        // A copy opened from a DMG must not attach to the installed core:
        // version reconciliation could stop that core even without registering.
        guard service.installationProblem == nil else {
            startupWatchdog?.cancel()
            return
        }
        connectTask = Task { [weak self] in await self?.connectLoop() }
        pollTask = Task { [weak self] in await self?.pollLoop() }
    }

    /// Quitting stops the core when the person turned off keeping agents
    /// connected, so a signed-in person is asked first.
    var asksBeforeQuitting: Bool {
        !keepsAgentsConnectedAfterQuit && (phase == .attached || phase == .preparingSessions)
    }

    /// Quitting the app leaves the core running, launchd's, unless the
    /// person chose otherwise: then the core stops until the app opens again.
    func shutdown() {
        if !keepsAgentsConnectedAfterQuit {
            // Before restoring a pending registration, which `stopping` skips.
            stopBackgroundService()
        }
        isCoreConnected = false
        searchTelemetryTasks.values.forEach { $0.cancel() }
        searchTelemetryTasks.removeAll()
        connectTask?.cancel()
        initialRefreshTask?.cancel()
        hiddenRetryTask?.cancel()
        startupWatchdog?.cancel()
        coreVersionRecheck?.cancel()
        pollTask?.cancel()
        client.reset()
        // applicationWillTerminate is synchronous: finish a pending registration
        // now rather than relying on the cancelled repair task to resume.
        do {
            try finishCoreRegistration()
        } catch {
            Self.logger.error("could not restore the core before quitting: \(error.localizedDescription, privacy: .public)")
        }
    }

    @ObservationIgnored private var coreRegistrationPending = false

    private func reregisterCore() async throws {
        coreRegistrationPending = true
        do {
            if try await service.reloadForUpdateAsync() {
                coreRegistrationPending = false
                defaults.set(service.binaryIdentity, forKey: Self.registeredCoreKey)
                return
            }
        } catch {
            coreRegistrationPending = false
            throw error
        }
        coreRegistrationPending = false
        try await service.unregisterAsync()
        coreRegistrationPending = true
        try? await Task.sleep(nanoseconds: unregisterSettle)
        // Cancellation ends the wait, but must not leave launchd unregistered.
        try finishCoreRegistration()
    }

    private func finishCoreRegistration() throws {
        guard coreRegistrationPending else { return }
        coreRegistrationPending = false
        guard !stopping else { return }
        try service.register()
        defaults.set(service.binaryIdentity, forKey: Self.registeredCoreKey)
    }

    /// Registers the core's LaunchAgent when it is not registered, and says
    /// when macOS waits for the person to allow it.
    func ensureService() async {
        guard !stopping, !Task.isCancelled, !coreRegistrationPending else { return }
        if let installationProblem = service.installationProblem {
            coreNeedsApproval = false
            coreProblem = installationProblem
            return
        }
        let identity = service.binaryIdentity
        do {
            switch service.status {
            case .notRegistered:
                coreRegistrationPending = true
                defer { coreRegistrationPending = false }
                try await service.registerAsync()
                defaults.set(identity, forKey: Self.registeredCoreKey)
            case .enabled where identity != nil && defaults.string(forKey: Self.registeredCoreKey) != identity:
                // launchd admits only the binary the agent was registered
                // with. The old core must have stopped before the new
                // registration, or launchd keeps the old one's record.
                try await reregisterCore()
            case .enabled, .requiresApproval, .notFound:
                break
            }
        } catch {
            Self.logger.error("could not register the core: \(error.localizedDescription, privacy: .public)")
            coreProblem = "The background service could not start. Try again, or check Agent Relay in System Settings › General › Login Items."
        }
        coreNeedsApproval = service.status == .requiresApproval
        if coreNeedsApproval {
            coreProblem = "Allow Agent Relay in System Settings › General › Login Items so it can keep your agents connected."
        } else if service.status == .notFound {
            coreProblem = "This build has no background service installed. Open a complete Agent Relay app, or start the core manually when developing with Swift."
        }
    }

    /// launchd admits the core against a code requirement Background Task
    /// Management records for the agent. A Developer ID or Apple Development
    /// signature names a team, which an update keeps. An ad-hoc signature
    /// has none, so the requirement pins the replaced binary's hash and launchd
    /// refuses the new core ("Launch Constraint Violation"). BTM keeps the
    /// old requirement for a while after the bundle changes; a registration
    /// made once it lets go records the new hash. So while an enabled core
    /// does not answer, it is registered again, a bounded number of times.
    private func recoverBrokenBundleProgram() async throws -> Bool {
        guard !coreRegistrationPending else { return false }
        coreRegistrationPending = true
        defer { coreRegistrationPending = false }
        let recovered = try await service.recoverSpawnFailureAsync()
        coreNeedsApproval = service.status == .requiresApproval
        if coreNeedsApproval {
            coreProblem = "Allow Agent Relay in System Settings › General › Login Items so it can keep your agents connected."
        }
        guard recovered else { return false }
        Self.logger.notice("using an absolute-path LaunchAgent after macOS lost the core bundle path")
        defaults.set(service.binaryIdentity, forKey: Self.registeredCoreKey)
        return true
    }

    private func recoverRefusedCore() async {
        guard !isRecoveringCore, !stopping, !coreRegistrationPending else { return }
        isRecoveringCore = true
        defer { isRecoveringCore = false }
        let wasEnabled = service.status == .enabled
        let identity = service.binaryIdentity
        let neededUpdate = identity != nil && defaults.string(forKey: Self.registeredCoreKey) != identity
        await ensureService()
        // ensureService may just have submitted the missing job or replaced
        // an older binary. Give that new core a chance to answer instead of
        // immediately booting it out again in this same recovery pass.
        guard wasEnabled, !neededUpdate else { return }
        guard !stopping, !Task.isCancelled, service.installationProblem == nil,
              service.status == .enabled,
              refusalRepairs < Self.refusalRepairLimit else { return }
        refusalRepairs += 1
        Self.logger.notice("registering the core again: it does not answer")
        do {
            if try await recoverBrokenBundleProgram() { return }
            try await reregisterCore()
        } catch {
            Self.logger.error("could not register the core: \(error.localizedDescription, privacy: .public)")
            if (error as NSError).domain == "com.agentrelay.desktop.launch-agent" {
                coreProblem = "macOS could not start Agent Relay’s backup background service. Try again, or check the app’s log for details."
            } else {
                coreProblem = "The background service could not restart. Check Agent Relay in System Settings › General › Login Items, then try again."
            }
        }
        coreNeedsApproval = service.status == .requiresApproval
        if coreNeedsApproval {
            coreProblem = "Allow Agent Relay in System Settings › General › Login Items so it can keep your agents connected."
        }
    }

    private func watchStartup() {
        startupWatchdog?.cancel()
        startupWatchdog = Task { [weak self, startupTimeout] in
            do { try await Task.sleep(nanoseconds: startupTimeout) } catch { return }
            guard let self, !isCoreConnected, coreProblem == nil else { return }
            coreProblem = StartupPresentation.timeoutMessage
        }
    }

    /// Explicit recovery also works after the automatic repair budget is used.
    func retryBackgroundService() async {
        guard !isRecoveringCore, !stopping else { return }
        refusalRepairs = 0
        coreProblem = nil
        watchStartup()
        await recoverRefusedCore()
    }

    func openLoginItemsSettings() { service.openSystemSettings() }

    var backgroundServiceStatus: CoreServiceStatus { service.status }
    var coreNeedsInstallation: Bool { service.installationProblem != nil }

    func stopBackgroundService() {
        guard service.installationProblem == nil else { return }
        stopping = true
        connectTask?.cancel()
        initialRefreshTask?.cancel()
        startupWatchdog?.cancel()
        pollTask?.cancel()
        client.reset()
        do {
            try service.unregister()
        } catch {
            Self.logger.error("could not stop the core: \(error.localizedDescription, privacy: .public)")
        }
    }

    /// The person stopped the core; nothing registers it again this launch.
    @ObservationIgnored private var stopping = false

    /// After a growing delay, asks the core `pid`, still on `version`, to
    /// stop again so launchd can start the build now on disk. The event
    /// connection closing, or any new connection, cancels it first. The stop
    /// is asked only of a core that still answers as `pid` on `version`:
    /// after `pid` exits launchd may already run the current build, which
    /// must not be stopped.
    private func scheduleCoreVersionRecheck(pid: Int, version: String, attempt: Int) {
        let delay = coreVersionRecheckDelay * UInt64(attempt)
        coreVersionRecheck = Task { [weak self] in
            try? await Task.sleep(nanoseconds: delay)
            guard let self, !Task.isCancelled, !self.stopping else { return }
            // Retired first, so a reconnect to the exiting core waits for
            // launchd's replacement. When no stop was asked (another core
            // answered, the connection or its hello failed, or the task was
            // cancelled) the core keeps serving, so it is usable again and a
            // later recheck can still stop it.
            self.retiredCorePIDs.insert(pid)
            guard await self.client.shutdownDaemon(ifPID: pid, version: version) else {
                self.retiredCorePIDs.remove(pid)
                return
            }
            self.coreVersionRestarts[version] = attempt + 1
            Self.logger.notice("restarted a core of another version again (attempt \(attempt + 1, privacy: .public))")
            self.client.reset()
        }
    }

    private func cancelCoreVersionRecheck() {
        coreVersionRecheck?.cancel()
        coreVersionRecheck = nil
    }

    private func connectLoop() async {
        var attempts = 0
        while !Task.isCancelled {
            do {
                let (hello, events) = try await client.subscribe(topics: [.status, .activity, .invalidated, .notify, .signIn])
                if retiredCorePIDs.contains(hello.pid) {
                    throw CoreError.unreachable("The background service is restarting.")
                }
                cancelCoreVersionRecheck()
                if let bundled = bundledCoreVersion, hello.daemonVersion != bundled {
                    let restarts = coreVersionRestarts[hello.daemonVersion, default: 0]
                    if restarts == 0 {
                        // An app update replaced the binary under a running core.
                        coreVersionRestarts[hello.daemonVersion] = 1
                        Self.logger.notice("restarting a core of another version")
                        retiredCorePIDs.insert(hello.pid)
                        client.reset()
                        await client.shutdownDaemon()
                        continue
                    }
                    // launchd's replacement still runs another build. It is
                    // used, never stopped on connect: bad bundle metadata
                    // must not kill every replacement. A later recheck asks
                    // it to stop again, and past the bound the person is told.
                    if restarts < Self.coreVersionRestartLimit {
                        scheduleCoreVersionRecheck(pid: hello.pid, version: hello.daemonVersion, attempt: restarts)
                    } else {
                        staleCoreWarning = Self.staleCoreMessage(running: hello.daemonVersion, bundled: bundled)
                    }
                } else {
                    staleCoreWarning = nil
                }
                attempts = 0
                refusalRepairs = 0
                coreProblem = nil
                coreNeedsApproval = false
                isCoreConnected = true
                // Reads interrupted by the previous core may have surfaced a
                // temporary transport error. The new snapshot and refreshes
                // replace their results; keep action errors, whose outcome is
                // uncertain until the person checks it.
                if lastError == CoreError.reconnecting.message || lastError == CoreError.closed.message {
                    lastError = nil
                }
                if sessionsError == CoreError.reconnecting.message || sessionsError == CoreError.closed.message {
                    sessionsError = nil
                }
                if previewError == CoreError.reconnecting.message || previewError == CoreError.closed.message {
                    previewError = nil
                }
                coreAnswered()
                startupWatchdog?.cancel()
                // subscribe queues the initial snapshot before returning.
                // Apply it before any settings or session discovery work.
                var iterator = events.makeAsyncIterator()
                guard let first = await iterator.next() else { throw CoreError.closed }
                let wasAttached = phase == .attached
                handle(first)
                // An unchanged snapshot from the same core is not applied.
                replayPendingLinks()
                if wasAttached { relay.connectionRecovered() }
                // Entering an account starts its own guarded refresh in apply.
                let refreshAccount = wasAttached || phase != .attached
                initialRefreshTask?.cancel()
                initialRefreshTask = Task { await connected(refreshAccount: refreshAccount) }
                while let event = await iterator.next() { handle(event) }
                initialRefreshTask?.cancel()
                // The core a pending recheck was for may be gone; the next
                // connection decides again.
                cancelCoreVersionRecheck()
                Self.logger.notice("the core connection closed")
            } catch let error as CoreError where error.kind == "bridge_version_mismatch" {
                Self.logger.notice("stopping a core of another bridge version")
                await client.shutdownDaemon()
            } catch {
                cancelCoreVersionRecheck()
                attempts += 1
                var recoveredBrokenBundle = false
                // A broken BTM bundle path is already visible in launchd's
                // first failed spawn. Repair it before the normal 15-attempt
                // re-registration cycle or startup timeout.
                if attempts <= 6 && service.installationProblem == nil {
                    do {
                        if try await recoverBrokenBundleProgram() {
                            attempts = 0
                            recoveredBrokenBundle = true
                        }
                    } catch {
                        Self.logger.error("could not start the backup core: \(error.localizedDescription, privacy: .public)")
                        coreProblem = "macOS could not start Agent Relay’s backup background service. Try again, or check the app’s log for details."
                    }
                }
                if !recoveredBrokenBundle && attempts % 15 == 0 {
                    await recoverRefusedCore()
                }
                if attempts >= 15 && !coreNeedsApproval && coreProblem == nil {
                    coreProblem = "Agent Relay could not connect to its background service. Try restarting the service to continue to sign-in."
                }
            }
            if isCoreConnected {
                // Answers from the retired core must not put its errors or
                // stale data back into the freshly connected account view.
                invalidateUploadConnection()
                connectionRefresh?.cancel()
                isWorking = false
                workDescription = nil
                sharingModeWanted.clear()
                pausedWanted.clear()
                pendingUploadAction = nil
                pendingSettings = []
                startingSignIn = false
                relay.connectionInterrupted()
                claudeDelivery.connectionInterrupted()
            }
            isCoreConnected = false
            initialRefreshTask?.cancel()
            client.reset()
            // launchd may still be starting the core: back off to 2 s.
            try? await Task.sleep(nanoseconds: UInt64(min(2.0, 0.1 * pow(1.5, Double(attempts))) * 1_000_000_000))
        }
    }

    /// Everything a fresh connection needs: settings, the one-time
    /// settings migration, and every collection shown.
    private func connected(refreshAccount: Bool) async {
        await loadSettings()
        guard !Task.isCancelled else { return }
        if !migratedSettings {
            await migrateSettings()
        }
        if !hasRecordedLaunch {
            hasRecordedLaunch = true
            record(.appLaunch, .started)
            for _ in 0..<pendingMessageComposerOpens {
                record(.messageComposer, .started, surface: .messages)
            }
            pendingMessageComposerOpens = 0
            recordPageView()
        }
        guard !Task.isCancelled else { return }
        await refreshAll(includeAccount: refreshAccount)
    }

    @ObservationIgnored private var hasRecordedLaunch = false

    /// Opt-outs and flags earlier builds kept in UserDefaults reach the core
    /// once; the old keys are then forgotten.
    private func migrateSettings() async {
        if let pending = LegacyMigration.settings(in: defaults) {
            do {
                confirmedSettings = try await client.settingsSet(pending)
            } catch {
                Self.logger.error("could not carry settings over to the core")
                return
            }
        }
        LegacyMigration.forgetDefaults(in: defaults)
        migratedSettings = true
    }

    private func refreshAll(includeAccount: Bool = true) async {
        async let uploader: Void = includeAccount ? refreshUploader() : ()
        async let relay: Void = self.relay.refreshAll()
        async let claudeDelivery: Void = self.claudeDelivery.refresh()
        _ = await (uploader, relay, claudeDelivery)
    }

    // MARK: Events

    func handle(_ event: CoreEvent) {
        switch event {
        case .status(let snapshot):
            guard snapshot.revision > (core?.revision ?? 0) || snapshot.daemon.pid != core?.daemon.pid else { return }
            apply(snapshot)
        case .signIn(let state):
            if signInLink != state.link { signInLink = state.link }
            openSignInLinkIfNew(state.link)
        case .invalidated(let invalidation):
            invalidated(invalidation.what)
        case .notify(let notification):
            show(notification)
        case .activity(let line):
            Self.logger.info("core: \(line.message, privacy: .public)")
        case .shutdown:
            break
        }
    }

    private func apply(_ snapshot: Status) {
        // Links held while the core was away, or for one more try, go with
        // every snapshot, not only the first: the account view keeps its
        // snapshot across a reconnect.
        defer { replayPendingLinks() }
        let before = phase
        let beforeAccount = core?.account
        if beforeAccount?.authCheckedAt != snapshot.account.authCheckedAt { accountHealthError = nil }
        core = snapshot
        let after = phase
        recordPageView()
        if beforeAccount?.workspaceId != snapshot.account.workspaceId || beforeAccount?.person?.id != snapshot.account.person?.id || (snapshot.account.state == .signedOut && beforeAccount?.state != .signedOut) {
            relaycastUpgrade = nil
            lastRelaycastUpgradeAt = nil
            requestIdentityRevision &+= 1
            requestIdentityStartedAtMs = Int(Date().timeIntervalSince1970 * 1000)
        }
        previousPhase = after
        if snapshot.account.state.isFailure {
            if signInError != snapshot.account.error { signInError = snapshot.account.error }
        } else if snapshot.account.state != .signedOut, signInError != nil {
            signInError = nil
        }
        if snapshot.account.state != .pendingApproval {
            if signInLink != nil { signInLink = nil }
        }
        if renewingSignIn && (beforeAccount?.state == .pendingApproval || renewalAnswered)
            && snapshot.account.state != .pendingApproval {
            renewingSignIn = false
            renewalAnswered = false
        }
        // The snapshot shows where the attempt stands; its state guards
        // from here.
        if startingSignIn && after != .signedOut { startingSignIn = false }
        if after == .signedOut { loadLoginConfig() }
        if signingOut && after == .signedOut { signingOut = false }
        if after == .preparingSessions && before != .preparingSessions {
            sessionPreparationStarted = Date()
            Task { await refreshSessionPreview() }
        }
        if (before == .signingIn || before == .waitingForBrowser) && (after == .preparingSessions || after == .attached) {
            page = .sessions
            showMainWindow()
        }
        if after == .attached && before != .attached {
            invalidateUploadConnection()
            connectionRefresh = Task { await refreshUploader() }
        }
        if after == .signedOut && before != .signedOut && before != .launching {
            resetUploader()
            showMainWindow()
        }
        if beforeAccount?.workspaceId != snapshot.account.workspaceId || beforeAccount?.person?.id != snapshot.account.person?.id {
            relay.accountChanged()
        }
    }

    private func invalidated(_ what: Listing) {
        switch what {
        case .uploader: Task { await refreshUploader() }
        // An agent changed a setting (`PATCH /settings`).
        case .settings:
            settingsInvalidations += 1
            Task { await loadSettings() }
        case .activity:
            Task {
                await relay.invalidated(what)
                await claudeDelivery.refresh()
            }
        default: Task { await relay.invalidated(what) }
        }
    }

    private func openSignInLinkIfNew(_ link: String?) {
        guard let link, let url = URL(string: link), openedSignInLinks.insert(link).inserted else { return }
        // Replies and events can straddle an older preparing snapshot. Keep
        // the URL guard across those snapshots, as the GTK app does.
        guard let openSignInBrowser else { return }
        record(.browserApproval, .started)
        let opened = openSignInBrowser(url)
        record(.browserApproval, opened ? .succeeded : .failed, failure: opened ? nil : .browserOpen)
    }

    // MARK: Notifications

    private var notificationsAvailable: Bool { manageSystemServices && Bundle.main.bundleIdentifier != nil }

    /// Permission is asked when the first notification arrives, so macOS
    /// prompts with something to show; later calls answer from the
    /// person's saved choice without prompting.
    private func show(_ notification: CoreNotification) {
        guard notificationsAvailable else { return }
        let content = UNMutableNotificationContent()
        content.title = notification.title
        content.body = notification.body
        if let link = notification.link { content.userInfo = ["link": link] }
        let request = UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil)
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { granted, _ in
            guard granted else { return }
            UNUserNotificationCenter.current().add(request)
        }
    }

    // MARK: Polling

    /// The uploader's status and sessions arrive as `invalidated` events;
    /// this reads them again every 60 s only as a safety net.
    private func pollLoop() async {
        while !Task.isCancelled {
            try? await Task.sleep(nanoseconds: Self.uploaderSafetyNet)
            guard core != nil else { continue }
            await refreshVisibleState()
        }
    }

    /// How often the uploader is read without an event, in nanoseconds.
    static let uploaderSafetyNet: UInt64 = 60_000_000_000

    func showLocalSessions(search: String? = nil) {
        if let search { sessionSearch = search }
        selectedLocalSession = nil
        page = .sessions
    }

    func refreshVisibleState() async {
        if phase == .preparingSessions {
            await refreshSessionPreview()
            return
        }
        async let status: Void = refreshStatus()
        if windowOpen || panelOpen { await refreshSessions() }
        await status
    }

    // MARK: Uploader

    /// Pending hide work belongs to one connection. Retire its markers with
    /// its replies so persisted intent can be retried on the new connection.
    private func invalidateUploadConnection() {
        connectionRevision += 1
        sessionFacetsTask?.cancel()
        sessionFacetsTask = nil
        hiddenConfirmed = []
        hidingSessionIDs = []
        hiddenRetryTask?.cancel()
        hiddenRetryTask = nil
        hiddenRetryAttempt = 0
    }

    private func resetUploader() {
        invalidateUploadConnection()
        status = nil
        sessions = []
        previewSessionIDs = []
        previewError = nil
        sessionPreparationStarted = nil
        lastSessionsRefresh = nil
        sessionsError = nil
        sessionActionError = nil
        uploadActionError = nil
        unsentUploadAction = nil
        requestingUploads = []
        stoppingUploads = []
        sharingModeWanted.clear()
        pausedWanted.clear()
        pendingUploadAction = nil
        uploadErrors = [:]
        uploadTasks.values.forEach { $0.cancel() }
        uploadTasks = [:]
        sessionLimit = Self.sessionPage
        queriedLocalSessions = nil
        queryArchivedSessionKeys = nil
        sessionFacets = nil
        sessionKnownRevision = nil
        sessionVerificationPending = false
        sessionCursor = nil
        sessionNextCursor = nil
        sessionQueryRevision += 1
        selectedLocalSession = nil
        showsSessionSearch = false
        sessionSearch = ""
        sessionUploadFilter = .all
        isComposingMessage = false
        isWorking = false
        workDescription = nil
    }

    private func refreshUploader() async {
        async let status: Void = refreshStatus()
        async let sessions: Void = refreshSessions()
        _ = await (status, sessions)
    }

    func checkAccountHealth() async {
        guard isCoreConnected, !isCheckingAccountHealth else { return }
        isCheckingAccountHealth = true
        accountHealthError = nil
        let revision = connectionRevision
        defer { isCheckingAccountHealth = false }
        do {
            _ = try await client.accountStatus()
            let snapshot = try await client.statusGet()
            guard revision == connectionRevision else { return }
            apply(snapshot)
        } catch {
            guard revision == connectionRevision else { return }
            accountHealthError = error.localizedDescription
        }
    }

    func refreshStatus() async {
        guard phase == .attached || phase == .preparingSessions else { return }
        let revision = connectionRevision
        do {
            let response = try await client.uploaderStatus()
            guard revision == connectionRevision else { return }
            if status != response {
                let policyChanged = status?.sharingMode != response.sharingMode || status?.uploadPolicyRevision != response.uploadPolicyRevision
                status = response
                // The visible page excludes hidden identities. Only saved
                // policy changes revoke their independent hide confirmations.
                if policyChanged { reconcileHiddenSessions() }
            }
            if lastError != nil { lastError = nil }
            coreAnswered()
        } catch let error as CoreError where error.kind == ErrorKind.notSignedIn.rawValue {
            guard revision == connectionRevision else { return }
            if status != nil { status = nil }
        } catch {
            guard revision == connectionRevision else { return }
            if lastError != error.localizedDescription { lastError = error.localizedDescription }
        }
    }

    /// Bumped by every change to which sessions upload: a listing read
    /// before one is stale and is read again.
    @ObservationIgnored private var selectionRevision = 0
    @ObservationIgnored private var sessionsReadAgain = false

    func refreshSessions() async {
        guard phase == .attached else { return }
        guard !isLoadingSessions else {
            sessionsReadAgain = true
            return
        }
        if client.offers(CoreMethod.uploaderSessionsQuery), sessionSort.column == .reasoning { sessionSort = LocalSessionSort() }
        let revision = connectionRevision
        let selection = selectionRevision
        let titleRevision = relay.titleWriteRevision
        let queryRevision = sessionQueryRevision
        isLoadingSessions = true
        defer {
            isLoadingSessions = false
            if sessionsReadAgain {
                sessionsReadAgain = false
                Task { await refreshSessions() }
            }
        }
        do {
            let response: UploadSessions
            var queriedPage: SessionQueryPage?
            if client.offers(CoreMethod.uploaderSessionsQuery) {
                let page = try await client.uploaderSessionsQuery(SessionQueryParams(
                    ascending: sessionSort.ascending, cursor: sessionCursor,
                    hidden: showHiddenSessions ? [] : Array(hiddenSessionIDs).sorted(), knownRevision: sessionKnownRevision, limit: 200,
                    presentation: SessionQueryPresentation(
                        idleNames: relay.roster.filter { $0.liveness == .idle }.map(\.name).sorted(),
                        preparing: Array(requestingUploads).sorted(),
                        relayStates: Dictionary(relay.sessions.map { ($0.uploadKey ?? $0.id, relay.presence(of: $0).isOn) }, uniquingKeysWith: { _, last in last }),
                        uploadStates: Dictionary(uniqueKeysWithValues: requestingUploads.union(stoppingUploads).map { ($0, !stoppingUploads.contains($0)) })),
                    query: sessionSearch, sort: Self.querySort(sessionSort.column)))
                guard queryRevision == sessionQueryRevision else { sessionsReadAgain = true; return }
                guard revision == connectionRevision, selection == selectionRevision, titleRevision == relay.titleWriteRevision else { sessionsReadAgain = true; return }
                sessionKnownRevision = page.verificationPending ? nil : page.revision
                if sessionVerificationPending != page.verificationPending { sessionVerificationPending = page.verificationPending }
                if page.unchanged {
                    // Nothing on screen changes: publish nothing.
                    sessionsRefreshed()
                    reconcileHiddenSessions()
                    refreshSessionFacets()
                    return
                }
                queriedPage = page
                response = UploadSessions(sessions: page.sessions, sharingMode: sharingMode)
            } else {
                response = try await client.uploaderSessionsList(LimitParams(limit: sessionLimit))
            }
            guard revision == connectionRevision else { return }
            guard selection == selectionRevision, titleRevision == relay.titleWriteRevision else {
                sessionsReadAgain = true
                return
            }
            if let page = queriedPage {
                if queriedLocalSessions != page.localSessions { queriedLocalSessions = page.localSessions }
                if sessionNextCursor != page.nextCursor { sessionNextCursor = page.nextCursor }
                let archived = page.archivedKeys.map { Set($0) }
                if queryArchivedSessionKeys != archived { queryArchivedSessionKeys = archived }
            } else if queryArchivedSessionKeys != nil { queryArchivedSessionKeys = nil }
            if sessions != response.sessions { sessions = response.sessions }
            refreshSessionFacets()
            relay.reconcileTitleEdits()
            reconcileHiddenSessions()
            if !previewSessionIDs.isEmpty { previewSessionIDs = [] }
            if sessionPreparationStarted != nil { sessionPreparationStarted = nil }
            sessionsRefreshed()
        } catch {
            guard revision == connectionRevision, queryRevision == sessionQueryRevision else { return }
            if let core = error as? CoreError, core.kind == ErrorKind.busy.rawValue, sessionCursor != nil {
                sessionCursor = nil
                sessionsReadAgain = true
                return
            }
            if sessionsError != error.localizedDescription { sessionsError = error.localizedDescription }
        }
    }

    /// Only whether the list has been read since the last reset is shown,
    /// so a repeated read publishes nothing.
    private func sessionsRefreshed() {
        if lastSessionsRefresh == nil { lastSessionsRefresh = Date() }
        if sessionsError != nil { sessionsError = nil }
    }

    /// Titles found on this Mac while history prepares, before the uploader
    /// can list them.
    private func refreshSessionPreview() async {
        guard phase == .preparingSessions, lastSessionsRefresh == nil, !isLoadingPreview else { return }
        let revision = connectionRevision
        let titleRevision = relay.titleWriteRevision
        isLoadingPreview = true
        defer { isLoadingPreview = false }
        let response: UploadPreview
        do {
            response = try await client.uploaderSessionsPreview(PreviewParams(refresh: true))
        } catch {
            if revision == connectionRevision { previewError = error.localizedDescription }
            return
        }
        guard revision == connectionRevision, titleRevision == relay.titleWriteRevision, lastSessionsRefresh == nil else { return }
        previewError = nil
        guard !response.sessions.isEmpty else { return }
        sessions = response.sessions
        previewSessionIDs = Set(response.sessions.map(\.id))
    }

    private func refreshSessionFacets() {
        guard client.offers(CoreMethod.uploaderSessionsFacets), sessionFacetsTask == nil else { return }
        let generation = connectionRevision
        sessionFacetsTask = Task { [weak self] in
            guard let self else { return }
            defer { if generation == self.connectionRevision { self.sessionFacetsTask = nil } }
            var cursor: String?, revision: Int?
            var harnesses: [String] = [], projects: [String] = []
            var restarts = 0
            do {
                while true {
                    let page: SessionFacetsPage
                    do { page = try await self.client.uploaderSessionsFacets(SessionFacetParams(cursor: cursor, limit: 200)) }
                    catch let error as CoreError where error.kind == ErrorKind.busy.rawValue && cursor != nil && restarts == 0 {
                        cursor = nil; revision = nil; harnesses = []; projects = []; restarts += 1
                        continue
                    }
                    guard generation == self.connectionRevision, !Task.isCancelled else { return }
                    if cursor == nil, page.revision == self.sessionFacets?.revision { return }
                    if let revision, revision != page.revision { return }
                    revision = page.revision
                    harnesses += page.harnesses
                    projects += page.projects
                    cursor = page.nextCursor
                    if cursor == nil { break }
                }
                if let revision { self.sessionFacets = SessionFacetsPage(harnesses: harnesses, projects: projects, revision: revision) }
            } catch { /* Keep the last complete facets; the next refresh retries. */ }
        }
    }

    private static func querySort(_ column: LocalSessionSortColumn) -> String {
        switch column {
        case .session: return "session"
        case .project: return "project"
        case .status: return "status"
        case .updated: return "updated"
        case .relay: return "relay"
        case .upload: return "upload"
        case .harness: return "harness"
        case .model: return "model"
        case .reasoning: return "reasoning"
        case .created: return "created"
        case .directory: return "directory"
        case .sessionID: return "sessionID"
        }
    }

    var canLoadMoreSessions: Bool {
        phase == .attached && !isLoadingSessions && previewSessionIDs.isEmpty
            && (client.offers(CoreMethod.uploaderSessionsQuery) ? sessionNextCursor != nil : sessions.count >= sessionLimit && sessionLimit < 10_000)
    }

    func loadMoreSessions() async {
        guard canLoadMoreSessions else { return }
        if client.offers(CoreMethod.uploaderSessionsQuery) { sessionCursor = sessionNextCursor; sessionKnownRevision = nil }
        else { sessionLimit = min(10_000, sessionLimit + Self.sessionPage) }
        await refreshSessions()
    }

    /// Runs one uploader action at a time; refuses, rather than skips
    /// silently, when it cannot run now.
    private func perform(_ description: String, _ work: () async throws -> Void) async -> Error? {
        guard phase == .attached else {
            return CoreError(kind: ErrorKind.notSignedIn.rawValue, message: "Sign in to Agent Relay to upload sessions.")
        }
        guard !isWorking else {
            return CoreError(kind: "busy", message: "Another upload change is still running. Try again in a moment.", retryable: true)
        }
        let revision = connectionRevision
        isWorking = true
        workDescription = description
        defer {
            if revision == connectionRevision {
                isWorking = false
                workDescription = nil
            }
        }
        do {
            try await work()
            return nil
        } catch {
            return error
        }
    }

    /// The button stays usable while a change runs: a later click asks for
    /// the newer state, and the last one asked for wins.
    var canTogglePause: Bool { phase == .attached && uploaderSharingMode != nil && !isUploadControlEnforced }

    /// Pauses or resumes at once; the uploader is asked until it reports the
    /// state last asked for, and the latest request's failure reverts it.
    func setPaused(_ paused: Bool) async {
        guard paused != isPaused, !isUploadControlEnforced else { return }
        pausedWanted.begin(paused)
        guard !drivingPause else { return }
        drivingPause = true
        defer { drivingPause = false }
        let revision = connectionRevision
        // Each request is sent once; the loop goes on only for a newer one.
        while let wanted = pausedWanted.wanted {
            let token = pausedWanted.token
            let failure = await perform(wanted ? "Pausing uploads…" : "Resuming uploads…") {
                if wanted { try await client.uploaderPause() } else { try await client.uploaderResume() }
                await refreshStatus()
            }
            guard revision == connectionRevision else { return }
            if let failure {
                // A later click still gets its turn; the final choice's
                // failure falls back to the state the uploader reports.
                if pausedWanted.finish(token) { lastError = failure.localizedDescription }
            } else {
                pausedWanted.finish(token)
            }
        }
    }

    /// Retries uploads a problem stopped; queued records are kept. The stage
    /// shows the retry under way until the uploader's status says more.
    func retryUploads() async {
        await sendUploadAction(.retry, automatic: false)
    }

    private func runUploadAction(_ action: UploadStage.Action, _ description: String, automatic: Bool = false,
                                 _ work: () async throws -> Void) async {
        guard pendingUploadAction == nil else { return }
        uploadActionError = nil
        unsentUploadAction = nil
        pendingUploadAction = action
        let revision = connectionRevision
        let failure = await perform(description) {
            try await work()
            await refreshStatus()
        }
        guard revision == connectionRevision else { return }
        pendingUploadAction = nil
        if let failure {
            uploadActionError = failure.localizedDescription
            // Never sent: sent again once the core answers, at most once
            // per click. Both actions are idempotent.
            if let core = failure as? CoreError, CoreError.neverSent(core.message) {
                unsentUploadAction = (action, !automatic)
            }
        }
    }

    /// The core answered a call. A failure that only said it could not be
    /// reached is no longer true, so it clears without anyone dismissing it,
    /// and a retry or compact that was never sent is sent now. Failures from
    /// calls that reached the core keep their message until the next action.
    private func coreAnswered() {
        if let error = uploadActionError, CoreError.neverSent(error) { uploadActionError = nil }
        if let error = sessionActionError, CoreError.neverSent(error) { sessionActionError = nil }
        if uploadErrors.values.contains(where: CoreError.neverSent) {
            uploadErrors = uploadErrors.filter { !CoreError.neverSent($0.value) }
        }
        // Another change is under way: the next answer sends it instead.
        guard let unsent = unsentUploadAction, !isWorking, pendingUploadAction == nil else { return }
        unsentUploadAction = nil
        guard unsent.replay else { return }
        Task { await sendUploadAction(unsent.action, automatic: true) }
    }

    private func sendUploadAction(_ action: UploadStage.Action, automatic: Bool) async {
        switch action {
        case .retry:
            await runUploadAction(.retry, "Retrying uploads…", automatic: automatic) { try await client.uploaderRetry() }
        case .compact:
            await runUploadAction(.compact, "Compacting the upload journal…", automatic: automatic) { try await client.uploaderCompact() }
        }
    }

    /// Asks Agent Relay Cloud again to record this computer's spawn node as
    /// the person's (`spawn.claim.retry`); the status shows the answer.
    func retrySpawnClaim() async {
        do {
            try await client.spawnClaimRetry()
        } catch {
            lastError = error.localizedDescription
        }
    }

    /// Removes consumed records from a full upload journal.
    func compactJournal() async {
        await sendUploadAction(.compact, automatic: false)
    }

    @discardableResult
    func includeSessions(keys: [String]) async -> Bool {
        hiddenConfirmed.subtract(keys)
        return await changeSessionUploads(keys: keys.filter { !hiddenSessionIDs.contains($0) }, included: true)
    }

    /// Retains the web-history id beside a hidden Claude cloud target id.
    func migrateHiddenSessionIDs(for sessions: [LocalSession]) {
        let additions = hiddenSessionIDs.compactMap { LocalSessionRow.additionalHiddenID($0, sessions: sessions) }
            .filter { !hiddenSessionIDs.contains($0) }
        if !additions.isEmpty { hiddenSessionIDs.formUnion(additions) }
    }

    /// Reconcile persisted hidden sessions when fresh core state arrives,
    /// including sessions discovered by the uploader after they were hidden.
    func reconcileHiddenSessions() {
        hiddenConfirmed = hiddenConfirmed.filter { key in
            !sessions.contains { $0.id == key && $0.included }
                && LocalSessionRow.relaySession(for: key, in: relay.sessions)?.registration == nil
        }
        let keys = hiddenSessionIDs.subtracting(hiddenConfirmed).subtracting(hidingSessionIDs)
        guard !keys.isEmpty else { return }
        Task { await setSessionsHidden(keys: keys.filter { hiddenSessionIDs.contains($0) }, hidden: true) }
    }

    private func retryHiddenSessions() {
        guard hiddenRetryTask == nil, phase == .attached, isCoreConnected else { return }
        let revision = connectionRevision
        let delay = UInt64(1 << min(hiddenRetryAttempt, 5)) * 1_000_000_000
        hiddenRetryAttempt = min(hiddenRetryAttempt + 1, 5)
        hiddenRetryTask = Task { [weak self] in
            do { try await Task.sleep(nanoseconds: delay) } catch { return }
            guard let self, revision == self.connectionRevision else { return }
            self.hiddenRetryTask = nil
            self.reconcileHiddenSessions()
        }
    }

    /// Hiding stops sharing without deleting an existing cloud copy. Unhiding
    /// restores visibility only; sharing must be explicitly enabled again.
    func setSessionsHidden(keys: [String], hidden: Bool) async {
        let keys = Set(keys).subtracting(hidingSessionIDs)
        if !hidden {
            hiddenSessionIDs = hiddenSessionIDs.filter { stored in !keys.contains { LocalSessionRow.sameHiddenSession(stored, $0) } }
            hiddenConfirmed = hiddenConfirmed.filter { stored in !keys.contains { LocalSessionRow.sameHiddenSession(stored, $0) } }
            return
        }
        if !keys.isSubset(of: hiddenSessionIDs) { hiddenSessionIDs.formUnion(keys); hiddenRetryAttempt = 0 }
        hidingSessionIDs.formUnion(keys)
        let revision = connectionRevision
        for key in keys {
            defer { if revision == connectionRevision { hidingSessionIDs.remove(key) } }
            if let session = LocalSessionRow.relaySession(for: key, in: relay.sessions) {
                relay.requestRelay(false, for: session)
                await relay.settled(session.id)
                guard revision == connectionRevision else { continue }
                if let error = relay.errors[session.id] {
                    hiddenSessionIDs = hiddenSessionIDs.filter { !LocalSessionRow.sameHiddenSession($0, key) }
                    uploadErrors[key] = "Could not hide session: " + error
                    continue
                }
            }
            // Finish any include already in flight before excluding. Hidden
            // sessions cannot start another include through the UI.
            if let task = uploadTasks[key] { _ = await task.value }
            guard revision == connectionRevision, !Task.isCancelled else { continue }
            if key.hasPrefix("claude-cloud:") { hiddenConfirmed.insert(key); continue }
            guard phase == .attached && isCoreConnected else { uploadErrors[key] = "Waiting to stop uploads…"; continue }
            do {
                let policyRevision = status?.uploadPolicyRevision
                try await client.uploaderSessionsExclude(SessionKeysParams(sessionKeys: [key]))
                guard revision == connectionRevision else { continue }
                // A newer policy may have enabled this identity after the
                // exclusion committed but before its reply arrived.
                guard policyRevision == status?.uploadPolicyRevision else {
                    uploadErrors[key] = "Waiting to stop uploads…"
                    retryHiddenSessions()
                    continue
                }
                hiddenConfirmed.insert(key)
                hiddenRetryAttempt = 0
                selectionRevision += 1
                sessions = sessions.map { $0.id == key ? $0.withInclusion(false) : $0 }
                uploadErrors[key] = nil
            } catch let error as CoreError where error.kind == ErrorKind.notSignedIn.rawValue || error.kind == ErrorKind.busy.rawValue || error.kind == CoreError.closed.kind {
                guard revision == connectionRevision else { continue }
                uploadErrors[key] = "Waiting to stop uploads…"
                retryHiddenSessions()
            } catch {
                guard revision == connectionRevision else { continue }
                hiddenSessionIDs = hiddenSessionIDs.filter { !LocalSessionRow.sameHiddenSession($0, key) }
                uploadErrors[key] = "Could not hide session: " + error.localizedDescription
            }
        }
    }

    var uploadConnectionRevision: Int { connectionRevision }

    func excludeSessions(keys: [String], confirmedAt revision: Int? = nil) async {
        if let revision, revision != connectionRevision { return }
        guard canDeleteFromCloud else { return }
        _ = await changeSessionUploads(keys: keys, included: false)
    }

    /// Keeps a session waiting in the organization's upload window from
    /// uploading. Local only, through the exclusion store; the core refuses
    /// once the window has ended.
    func keepBack(_ session: UploadSession) async {
        guard phase == .attached else { return }
        let revision = connectionRevision
        uploadErrors[session.id] = nil
        do {
            _ = try await client.uploaderSessionsExclude(SessionKeysParams(sessionKeys: [session.id]))
            guard revision == connectionRevision else { return }
            selectionRevision += 1
            sessions = sessions.map { $0.id == session.id ? $0.withInclusion(false) : $0 }
        } catch {
            guard revision == connectionRevision else { return }
            uploadErrors[session.id] = error.localizedDescription
        }
        await refreshUploader()
    }

    /// Each session has its own serialized request loop. The visible intent is
    /// updated before awaiting the core, and a second click replaces that intent.
    private func changeSessionUploads(keys: [String], included: Bool) async -> Bool {
        guard !keys.isEmpty else { return false }
        guard phase == .attached else {
            sessionActionError = "Sign in to Agent Relay to upload sessions."
            return false
        }
        guard !isSessionTableReadOnly, keys.allSatisfy({ !previewSessionIDs.contains($0) }) else { return false }
        let revision = connectionRevision
        sessionActionError = nil
        let tasks = Set(keys).map { key -> Task<Bool, Never> in
            uploadErrors[key] = nil
            if included {
                requestingUploads.insert(key)
                stoppingUploads.remove(key)
            } else {
                stoppingUploads.insert(key)
                requestingUploads.remove(key)
            }
            selectionRevision += 1
            if let task = uploadTasks[key] { return task }
            let task = Task { await driveUpload(key, revision: revision) }
            uploadTasks[key] = task
            return task
        }
        var succeeded = true
        for task in tasks { if !(await task.value) { succeeded = false } }
        if revision == connectionRevision { await refreshUploader() }
        return succeeded
    }

    private func driveUpload(_ key: String, revision: Int) async -> Bool {
        defer {
            if revision == connectionRevision {
                uploadTasks[key] = nil
                requestingUploads.remove(key)
                stoppingUploads.remove(key)
            }
        }
        var reconcile = false
        while revision == connectionRevision && !Task.isCancelled {
            let on = requestingUploads.contains(key)
            guard on || stoppingUploads.contains(key) else { return true }
            // Upload exclusion does not prove the Cloud copy was deleted.
            if on && !reconcile && sessions.first(where: { $0.id == key })?.included == true { return true }
            do {
                let params = SessionKeysParams(sessionKeys: [key])
                if on { try await client.uploaderSessionsInclude(params) }
                else { try await client.uploaderSessionsDelete(params) }
                guard revision == connectionRevision else { return false }
                selectionRevision += 1
                sessions = sessions.map { $0.id == key ? $0.withInclusion(on) : $0 }
                if requestingUploads.contains(key) == on { return true }
            } catch {
                guard revision == connectionRevision else { return false }
                // A later click still gets its turn; a failed final choice rolls
                // back to the last confirmed value and explains the error in-row.
                if requestingUploads.contains(key) != on { reconcile = true; continue }
                uploadErrors[key] = error.localizedDescription
                return false
            }
        }
        return false
    }

    /// The chosen policy shows at once; the uploader is asked until it
    /// reports the policy last chosen, and the latest request's failure
    /// reverts to the one it reports.
    func setSharingMode(_ mode: SharingMode) async {
        guard mode != sharingMode else { return }
        sessionActionError = nil
        sharingModeWanted.begin(mode)
        guard !drivingSharingMode else { return }
        drivingSharingMode = true
        defer { drivingSharingMode = false }
        let revision = connectionRevision
        // Each request is sent once; the loop goes on only for a newer one.
        while let wanted = sharingModeWanted.wanted {
            let token = sharingModeWanted.token
            let failure = await perform("Updating upload selection…") {
                try await client.uploaderSharingSet(SharingParams(mode: wanted))
                selectionRevision += 1
            }
            guard revision == connectionRevision else { return }
            if let failure {
                if sharingModeWanted.finish(token) { sessionActionError = failure.localizedDescription }
            } else {
                await refreshUploader()
                guard revision == connectionRevision else { return }
                sharingModeWanted.finish(token)
            }
        }
    }

    func openLogFile() {
        guard let directory = installDirectory else { return }
        record(.openLogs, .started)
        let log = URL(fileURLWithPath: directory).appendingPathComponent("collector.log")
        if FileManager.default.fileExists(atPath: log.path) {
            NSWorkspace.shared.open(log)
        } else {
            NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: directory)])
        }
    }

    // MARK: Sign-in

    /// Opens the browser account chooser, sharing only the sessions chosen in
    /// My sessions; Settings › Session sharing offers automatic uploads.
    /// Without a `workspace`, a connection link's still waiting is used.
    func signIn(acknowledgeUninspectedSchedules: Bool = false, workspace: String? = nil, provider: SignInProvider? = nil) {
        guard !startingSignIn, phase == .signedOut else { return }
        if let provider { signInProvider = provider }
        signInError = nil
        lastError = nil
        startingSignIn = true
        let workspace = workspace ?? connectionWorkspace
        let chosen = provider
        Task {
            // The site's own sign-in page names no provider; only the
            // site's Microsoft sign-in, when it has one and it was clicked.
            var provider: SignInProvider? = signInProvider
            if let config = siteLoginConfig {
                provider = chosen == .microsoft && config.microsoftName != nil ? .microsoft : nil
            }
            let params = SignInRequest(acknowledgeUninspectedSchedules: acknowledgeUninspectedSchedules, flow: .browser, provider: provider,
                                       sharing: .selected, workspace: workspace)
            do {
                let state = try await client.accountSignIn(params)
                connectionWorkspace = nil
                // The core started no attempt: nothing is left to wait for.
                // Otherwise its snapshot shows where the attempt stands.
                if state.state == .signedOut || state.state.isFailure { startingSignIn = false }
                signInLink = state.link
                openSignInLinkIfNew(state.link)
            } catch {
                startingSignIn = false
                signInError = error.localizedDescription
            }
        }
    }

    /// The site's login configuration, when it published one; the sign-in
    /// screen then opens the site's own page in the browser.
    var siteLoginConfig: LoginConfig? { loginConfig?.available == true ? loginConfig : nil }

    /// Asks the core once; a failed call asks again on the next snapshot
    /// that shows this Mac signed out.
    private func loadLoginConfig() {
        guard !loginConfigAsked, client.offers(CoreMethod.accountLoginConfig) else { return }
        loginConfigAsked = true
        Task {
            do {
                let config = try await client.accountLoginConfig()
                if config.available { loginConfig = config }
            } catch {
                loginConfigAsked = false
            }
        }
    }

    func openApprovalPageAgain() {
        if let signInLink { openedSignInLinks.remove(signInLink) }
        openSignInLinkIfNew(signInLink)
    }

    /// Signs in again in the browser while this Mac stays connected: the
    /// core keeps a new session for Cloud calls such as session summaries,
    /// and uploads carry on.
    func renewSignIn() {
        guard phase == .attached, !renewingSignIn else { return }
        renewingSignIn = true
        renewalAnswered = false
        Task {
            let params = SignInRequest(acknowledgeUninspectedSchedules: false, flow: .browser,
                                       sharing: sharingMode, workspace: nil)
            do {
                let state = try await client.accountSignIn(params)
                if state.state == .pendingApproval {
                    // A snapshot may already have ended this renewal.
                    guard renewingSignIn else { return }
                    renewalAnswered = true
                    signInLink = state.link
                    openSignInLinkIfNew(state.link)
                } else {
                    // The core already holds a usable session.
                    renewingSignIn = false
                }
            } catch {
                renewingSignIn = false
                signInError = error.localizedDescription
            }
        }
    }

    func selectWorkspace(_ workspaceID: String) {
        signInError = nil
        Task {
            do { try await client.accountSelectWorkspace(ConnectWorkspaceParams(workspaceId: workspaceID)) }
            catch { signInError = error.localizedDescription }
        }
    }

    func cancelSignIn() {
        connectionWorkspace = nil
        Task { try? await client.accountSignInCancel() }
    }

    /// Logs out: the window shows the account leaving at once, until the
    /// core has answered and its snapshot no longer shows the account.
    func disconnect() async {
        guard !signingOut else { return }
        isComposingMessage = false
        signingOut = true
        do {
            try await client.accountSignOut()
            // Signed out, the window waits for the snapshot without the
            // account, which arrives on the event stream and may trail
            // this answer.
            if phase == .signedOut { signingOut = false }
        } catch {
            signingOut = false
            signInError = "Could not finish disconnecting. Uploads may still be running. Sign in again to retry cleanup. " + error.localizedDescription
        }
    }

    /// `agentrelay://` links: a message endpoint opens the composer; a
    /// connection link signs in to its workspace.
    /// Links that arrived while the core was not connected, or before its
    /// first snapshot, in order; each with how many times it was already
    /// tried again after the core could not be reached.
    @ObservationIgnored private var pendingLinks: [(url: URL, retries: Int)] = []
    /// How many times a link is tried again after the core could not be
    /// reached for it, as on Linux and Windows.
    static let linkRetries = 1
    /// How long a link held for its retry waits while the core stays
    /// attached, as on Linux and Windows (`LINK_RETRY_DELAY`). Nothing else
    /// may replay it then: a snapshot comes only when something changes and
    /// a reconnect only when the event stream drops, so a timed-out parse
    /// would otherwise never be tried again or answered. Long enough for a
    /// core that was briefly busy to settle.
    static let linkRetryDelay: UInt64 = 2_000_000_000

    /// While the core is away the window comes up, so the person sees the
    /// app starting or reconnecting; the same link already held is not held
    /// twice, as on Linux and Windows.
    func openConnection(_ url: URL) {
        if core == nil || !isCoreConnected { showMainWindow() }
        guard !pendingLinks.contains(where: { $0.url == url }) else { return }
        parseConnection(url, retries: 0)
    }

    private func replayPendingLinks() {
        guard !pendingLinks.isEmpty else { return }
        let links = pendingLinks
        pendingLinks = []
        for link in links { parseConnection(link.url, retries: link.retries) }
    }

    private func parseConnection(_ url: URL, retries: Int) {
        guard core != nil, isCoreConnected else {
            // A cold launch from a link, or one opened while the core is
            // away: it is parsed once the core answers and the account's
            // phase is known, or dropped at the startup deadline.
            holdWhileAway(url, retries: retries)
            return
        }
        Task {
            do {
                let link = try await client.linksParse(LinkParams(url: url.absoluteString))
                linkError = nil
                switch link {
                case .message(_, let endpoint):
                    messageEndpoint = endpoint
                    // Open the composer before changing its covered page.
                    isComposingMessage = true
                    page = .sessions
                    lastError = nil
                case .connect(_, _, let workspace):
                    if phase == .signingIn || phase == .waitingForBrowser || phase == .preparingSessions {
                        lastError = "Finish or cancel the current operation before opening another connection link."
                        break
                    }
                    if let current = workspaceID, canLogOut, current.lowercased() != workspace.lowercased() {
                        record(.connectionLink, .failed, failure: .workspaceConflict)
                        lastError = "This app is connected to another account or workspace. Log out in Account before connecting a different workspace."
                        page = .settings(.account)
                    } else {
                        record(.connectionLink, .succeeded)
                        lastError = nil
                        if phase == .signedOut {
                            connectionWorkspace = workspace
                            signIn(workspace: workspace)
                        }
                    }
                case .unknown:
                    linkFailed(Self.notALinkRefusal)
                    rejectConnectionLink(Self.notALinkRefusal, failure: .invalidConnection)
                }
            } catch {
                // The core says why; a link for another site names it. Only
                // its refusal is one; a core that could not be reached gets
                // the link once more, then says it is still starting.
                let cause = Self.linkRefusal(error.localizedDescription)
                linkFailed(cause)
                let kind = (error as? CoreError)?.kind ?? ""
                switch kind {
                case ErrorKind.invalidParams.rawValue:
                    rejectConnectionLink(cause, failure: .invalidConnection)
                case "core_unreachable", "core_timeout":
                    if retries < Self.linkRetries {
                        holdForRetry(url, retries: retries + 1)
                        return
                    }
                    rejectConnectionLink(Self.linkStillStarting, failure: kind == "core_timeout" ? .timedOut : .unexpected)
                default:
                    rejectConnectionLink(cause, failure: .unexpected)
                }
            }
            showMainWindow()
        }
    }

    /// Holds `url` for its retry. With the core connected, it is tried
    /// again after `linkRetryDelay`; a snapshot or reconnect that comes
    /// first replays it instead, and the timer then finds it gone. A link
    /// held while the core is away waits for the reconnect, until the
    /// deadline.
    private func holdForRetry(_ url: URL, retries: Int) {
        guard isCoreConnected else {
            holdWhileAway(url, retries: retries)
            return
        }
        pendingLinks.append((url, retries))
        Task { [weak self] in
            try? await Task.sleep(nanoseconds: AppModel.linkRetryDelay)
            guard let self,
                  let index = pendingLinks.firstIndex(where: { $0.url == url && $0.retries == retries })
            else { return }
            pendingLinks.remove(at: index)
            parseConnection(url, retries: retries)
        }
    }

    /// Holds `url` until the core is connected and has sent a snapshot, as
    /// long as startup waits for the core (`startupTimeout`, as
    /// `state.startup_timeout` on Linux and Windows): a core that stays away
    /// would otherwise leave the link held for good and the person with no
    /// answer. A snapshot or reconnect that replays it first leaves the
    /// deadline nothing to drop.
    private func holdWhileAway(_ url: URL, retries: Int) {
        pendingLinks.append((url, retries))
        Task { [weak self, startupTimeout] in
            try? await Task.sleep(nanoseconds: startupTimeout)
            guard let self,
                  let index = pendingLinks.firstIndex(where: { $0.url == url && $0.retries == retries })
            else { return }
            pendingLinks.remove(at: index)
            linkFailed(Self.linkHoldTimedOut(startupTimeout))
            rejectConnectionLink(Self.linkStillStarting, failure: .timedOut)
            showMainWindow()
        }
    }

    /// Why a link held while the core was away was dropped after `timeout`
    /// nanoseconds, for Diagnostics, word for word as on Linux and Windows
    /// (`models::link_hold_timed_out`).
    static func linkHoldTimedOut(_ timeout: UInt64) -> String {
        "Agent Relay's background service was not connected within \(timeout / 1_000_000_000) s."
    }

    /// The core's own sentence for a link it does not open, which a `kind`
    /// this build does not know amounts to.
    static let notALinkRefusal = "That is not an Agent Relay link this app opens."

    /// What the person reads when a link could not be parsed because the
    /// core was starting, reconnecting or did not answer, after the link
    /// was held and tried again once, word for word as on Linux and Windows
    /// (`models::LINK_STILL_STARTING`).
    static let linkStillStarting = "Agent Relay is still starting. Open the link again in a moment."

    /// Keeps why a link was not opened, for Diagnostics, and logs it.
    private func linkFailed(_ cause: String) {
        Self.logger.error("could not open a connection link: \(cause, privacy: .public)")
        linkError = cause
    }

    /// Records that a link was not opened and says `shown`.
    private func rejectConnectionLink(_ shown: String, failure: TelemetryFailure) {
        record(.connectionLink, .failed, failure: failure)
        lastError = shown
    }

    /// What the person reads when the core refuses a link, word for word
    /// as on Linux and Windows (`models::link_refusal`). For a connect link
    /// for another site (`That link connects to <site>, but this app is
    /// built for <built in>.`) the link's site is whatever the link
    /// carried, so it is not repeated: its host alone, bounded, names which
    /// app opens the link, beside the site this app is built for. Any
    /// other reason (not a link, the core not up) is shown as it is.
    static func linkRefusal(_ reason: String) -> String {
        let prefix = "That link connects to "
        let separator = ", but this app is built for "
        guard reason.hasPrefix(prefix), let split = reason.range(of: separator) else { return reason }
        let site = String(reason[reason.index(reason.startIndex, offsetBy: prefix.count)..<split.lowerBound])
        var builtIn = String(reason[split.upperBound...])
        while builtIn.hasSuffix(".") { builtIn.removeLast() }
        let app: String
        if let host = URLComponents(string: site)?.host?.lowercased(), !host.isEmpty {
            app = "the Agent Relay app for \(boundedHost(host))"
        } else {
            app = "that site's Agent Relay app"
        }
        return "That link is for another Agent Relay site. Open it with \(app), or ask for a link for this app's site (\(builtIn))."
    }

    /// `host`, cut to a length a dialog can show.
    private static func boundedHost(_ host: String) -> String {
        let most = 64
        return host.count <= most ? host : String(host.prefix(most)) + "…"
    }

    // MARK: Settings

    func loadSettings() async {
        let revision = connectionRevision
        if let loaded = try? await client.settingsGet(), revision == connectionRevision { confirmedSettings = loaded }
    }

    private func showSettings() {
        let shown = shownSettings
        if settings != shown { settings = shown }
    }

    private var shownSettings: SettingsValues? {
        pendingSettings.reduce(confirmedSettings) { $0?.applying($1.change) }
    }

    /// Whether a change to the setting under `field` (its JSON key) is still
    /// on its way: the row shows the value asked for, with a marker.
    func isSettingPending(_ field: String) -> Bool {
        pendingSettings.contains { $0.change.fields.contains(field) }
    }

    /// Sets one of the core's settings. The change shows at once; the core's
    /// answer replaces it, and a failure reverts it and is returned as the
    /// core's sentence (also kept in `settingErrors` under the setting).
    @discardableResult
    func setSetting(_ change: SettingsChange) async -> String? {
        settingsWriteToken &+= 1
        let token = settingsWriteToken
        let revision = connectionRevision
        for field in change.fields { settingErrors[field] = nil }
        pendingSettings.append((token, change))
        do {
            let saved = try await client.settingsSet(change)
            guard revision == connectionRevision else { return nil }
            pendingSettings.removeAll { $0.token == token }
            // The latest change's answer is the core's last word. An older
            // answer may predate a newer change still on its way, so only
            // its own change joins the confirmed values.
            confirmedSettings = token == settingsWriteToken ? saved : confirmedSettings?.applying(change)
            return nil
        } catch {
            guard revision == connectionRevision else { return nil }
            pendingSettings.removeAll { $0.token == token }
            for field in change.fields { settingErrors[field] = error.localizedDescription }
            return error.localizedDescription
        }
    }

    /// Whether the person's OpenClaw agents can use Agent Relay's tools
    /// (`openclaw.relay_tools.status`); a failed read says why, as the core
    /// worded it.
    func openClawToolsStatus() async -> OpenClawToolsStatus {
        do {
            return try await client.openclawRelayToolsStatus()
        } catch {
            return OpenClawToolsStatus(detail: error.localizedDescription, state: .unavailable)
        }
    }

    /// Whether Settings lists caller requests: only while signed in.
    var canSeeCallerRequests: Bool { phase == .attached || phase == .preparingSessions }

    /// The callers this Mac knows (`spawn_callers.list`), or the core's
    /// error, whose message names its class.
    func callerRequests() async -> Result<[CallerSummary], CoreError> {
        do {
            return .success(try await client.spawnCallersList(CallerListParams()).callers)
        } catch let error as CoreError {
            return .failure(error)
        } catch {
            return .failure(CoreError(kind: "internal", message: error.localizedDescription))
        }
    }

    /// Approves or denies a waiting caller (`spawn_callers.decide`).
    func decideCallerRequest(_ callerId: String, approved: Bool) async -> Result<Void, CoreError> {
        do {
            _ = try await client.spawnCallersDecide(CallerDecideParams(approved: approved, callerId: callerId))
            return .success(())
        } catch let error as CoreError {
            return .failure(error)
        } catch {
            return .failure(CoreError(kind: "internal", message: error.localizedDescription))
        }
    }

    /// Connect (or Open Again): the core starts OpenClaw's sign-in and the
    /// consent page opens in the browser, as the sign-in approval page does.
    /// Returns the new state, or the core's sentence on failure.
    func connectOpenClawTools() async -> Result<OpenClawToolsState, CoreError> {
        do {
            let connected = try await client.openclawRelayToolsConnect()
            if manageSystemServices, let link = connected.authorizationUrl,
               let url = URL(string: link), url.scheme == "https" || url.scheme == "http" {
                NSWorkspace.shared.open(url)
            }
            return .success(connected.state)
        } catch let error as CoreError {
            return .failure(error)
        } catch {
            return .failure(CoreError(kind: "internal", message: error.localizedDescription))
        }
    }

    /// OpenClaw announces no finished sign-in: while its consent page waits
    /// in the browser, the status is read every 2 s. Returns when it stops
    /// waiting or the row that asked goes away.
    func followOpenClawTools(read: () async -> Void, waiting: () -> Bool) async {
        await read()
        while waiting(), !Task.isCancelled {
            do { try await Task.sleep(nanoseconds: OpenClawToolsCopy.pollNanoseconds) } catch { return }
            await read()
        }
    }

    func setAnalyticsEnabled(_ enabled: Bool) {
        Task { await setSetting(SettingsChange(analyticsEnabled: enabled)) }
    }

    func setKeepsAgentsConnectedAfterQuit(_ enabled: Bool) {
        keepsAgentsConnectedAfterQuit = enabled
        defaults.set(enabled, forKey: Self.keepsAgentsConnectedKey)
    }

    func setLaunchAtLogin(_ enabled: Bool) {
        lastError = nil
        let previous = loginItem.isEnabled || loginItem.needsApproval
        var changed = loginItem.setEnabled(enabled)
        if changed {
            do {
                try service.setStartsAtLogin(enabled)
                coreNeedsApproval = service.status == .requiresApproval
                lastError = nil
            } catch {
                _ = loginItem.setEnabled(previous)
                changed = false
                lastError = "Agent Relay could not change the background service login setting: \(error.localizedDescription)"
            }
        }
        launchAtLogin = loginItem.isEnabled
        loginItemNeedsApproval = loginItem.needsApproval
        record(.launchAtLogin, changed ? .succeeded : .failed, failure: changed ? nil : .systemSetting)
        if !changed && lastError == nil { lastError = "Agent Relay could not change the login setting." }
    }

    /// Search/filter usage after 500 ms of inactivity; no query text leaves the UI.
    private func scheduleSearchTelemetry(_ query: String, surface: TelemetrySurface) {
        searchTelemetryTasks.removeValue(forKey: surface)?.cancel()
        guard hasRecordedLaunch, isCoreConnected, !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        searchTelemetryTasks[surface] = Task { [weak self] in
            do { try await Task.sleep(nanoseconds: 500_000_000) } catch { return }
            guard !Task.isCancelled, let self else { return }
            self.searchTelemetryTasks.removeValue(forKey: surface)
            guard self.isCoreConnected, !self.isComposingMessage,
                  self.page.telemetrySurface == surface else { return }
            self.record(.search, .succeeded, surface: surface)
        }
    }

    private func recordPageView() {
        // Legacy diagnostics opt-out must migrate before the first usage event.
        guard hasRecordedLaunch else { return }
        guard phase == .attached || phase == .preparingSessions else {
            lastTelemetrySurface = nil
            searchTelemetryTasks.values.forEach { $0.cancel() }
            searchTelemetryTasks.removeAll()
            return
        }
        let surface: TelemetrySurface = isComposingMessage ? .messages : page.telemetrySurface
        guard surface != lastTelemetrySurface else { return }
        // A destination query may be applied immediately before navigation.
        for (pendingSurface, task) in searchTelemetryTasks where pendingSurface != surface {
            task.cancel()
        }
        searchTelemetryTasks = searchTelemetryTasks.filter { $0.key == surface }
        lastTelemetrySurface = surface
        record(.pageView, .succeeded, surface: surface)
        if case .settings = page, !isComposingMessage {
            record(.settings, .started, surface: surface)
        }
    }

    /// Records an operation only the UI performs; the core records its own.
    func record(_ operation: TelemetryOperation, _ outcome: TelemetryOutcome, failure: TelemetryFailure? = nil, surface: TelemetrySurface? = nil) {
        let event = TelemetryEvent(failure: failure, operation: operation, outcome: outcome, surface: surface)
        Task { try? await client.telemetryRecord(event) }
    }
}
