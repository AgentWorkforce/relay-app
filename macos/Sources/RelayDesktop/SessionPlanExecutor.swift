import AppKit
import Observation
import RelayDesktopCore

/// Dispatching a URL or a launch request is not proof that a provider selected
/// the requested session. Only the exact-terminal adapter reports selection.
enum SessionPlanOutcome: Equatable {
    case focused
    case dispatched(String)
    case failed(String)
}

@MainActor @Observable final class SessionPlanExecutor {
    static let shared = SessionPlanExecutor()

    @MainActor struct Operations {
        var isCurrent: @MainActor (SessionLocation) -> Bool
        var authorize: @MainActor (String) async -> String?
        var focus: @MainActor (SessionLocation) async -> String?
        var show: @MainActor (SessionLocation) -> Bool
        var openLink: @MainActor (URL) -> Bool
        var resume: @MainActor (SessionResumeCommand, SessionLocation?) async -> String?

        static var native: Self {
            Self(isCurrent: SessionLocationLauncher.processIsCurrent,
                 authorize: SessionLocationLauncher.authorize,
                 focus: SessionLocationLauncher.focus,
                 show: { showApplication($0) }, openLink: { NSWorkspace.shared.open($0) }, resume: { command, location in
                     guard SessionTerminalAdapter.validEnvironment(command) else {
                         return "This provider’s captured environment is unsupported. No session was started."
                     }
                     if URL(fileURLWithPath: command.executable).lastPathComponent == "claude", command.args.first == "--desktop" {
                         return await SessionDesktopLauncher.launch(SessionTerminalAdapter.shellCommand(command))
                     }
                     let bundle = location?.bundleId ?? SessionTerminalAdapter.terminal.rawValue
                     guard let adapter = SessionTerminalAdapter(rawValue: bundle) else {
                         return "Resuming in \(location?.application ?? bundle) is unavailable. Relay has not started this session in another terminal."
                     }
                     return await adapter.resume(command)
                 })
        }
    }

    static func showApplication(_ target: SessionLocation,
                                lookup: (pid_t) -> NSRunningApplication? = { NSRunningApplication(processIdentifier: $0) }) -> Bool {
        guard let hostPid = target.hostPid, let pid = Int32(exactly: hostPid),
              let application = lookup(pid), application.bundleIdentifier == target.bundleId else { return false }
        return application.activate(options: [])
    }

    private var active: Set<String> = []
    private(set) var pendingResumes: Set<String> = []
    private(set) var resumeStorageError: String?
    static let pendingResumeMessage = "A resume request was already sent. The provider has not confirmed it opened. Check the earlier launch before trying again."
    private let operations: Operations
    private let store: any SessionPendingResumeStorage
    init(store: (any SessionPendingResumeStorage)? = nil, operations: Operations? = nil) {
        self.operations = operations ?? .native
        self.store = store ?? SessionPendingResumeStore()
        do { pendingResumes = try self.store.load() }
        catch { recordStorageError(error) }
    }

    @discardableResult private func recordStorageError(_ error: Error) -> String {
        let message = "Couldn’t read or save pending session launches. Resume is blocked. Check access to \(store.locationDescription), then restart Agent Relay. If the file is damaged, close or stop all earlier launches before restoring it. \(error.localizedDescription)"
        resumeStorageError = message
        return message
    }

    func observe(key: String, plan: SessionOpenPlan) {
        let hasOwner: Bool
        switch plan.action {
        case .focus, .showApplication, .openLink: hasOwner = plan.location != nil
        case .choose: hasOwner = !plan.choices.isEmpty
        default: hasOwner = false
        }
        if hasOwner, resumeStorageError == nil {
            do { pendingResumes = try store.remove(key) }
            catch { recordStorageError(error) }
        }
    }

    /// The user must first close or stop the earlier launch. A timeout or a
    /// terminal's successful creation cannot establish that no launch remains.
    @discardableResult func resolvePendingResume(key: String) -> Bool {
        guard !active.contains(key), resumeStorageError == nil else { return false }
        do {
            let latest = try store.load()
            pendingResumes = latest
            guard latest.contains(key) else { return false }
            pendingResumes = try store.remove(key)
            return true
        } catch {
            recordStorageError(error)
            return false
        }
    }

    static func sameTarget(_ left: SessionLocation?, _ right: SessionLocation?) -> Bool {
        guard let left, let right else { return left == nil && right == nil }
        return left.pid == right.pid && left.processStart == right.processStart
            && left.bundleId == right.bundleId && left.tty == right.tty
            && left.bootId == right.bootId && left.hostPid == right.hostPid
            && left.hostProcessStart == right.hostProcessStart
            && left.paneId == right.paneId && left.windowId == right.windowId
            && left.runtime == right.runtime && left.environmentIdentity == right.environmentIdentity
            && left.providerEnvironment == right.providerEnvironment
            && left.providerEnvironmentKnown == right.providerEnvironmentKnown
    }

    func execute(key: String, plan: SessionOpenPlan,
                 refresh: () async throws -> SessionOpenPlan) async -> SessionPlanOutcome {
        guard active.insert(key).inserted else { return .failed("This session is already being opened.") }
        defer { active.remove(key) }
        if plan.action == .resume {
            if let resumeStorageError { return .failed(resumeStorageError) }
            do { pendingResumes = try store.load() }
            catch { return .failed(recordStorageError(error)) }
        }
        if pendingResumes.contains(key), plan.action == .resume {
            do { observe(key: key, plan: try await refresh()) }
            catch { return .failed(Self.pendingResumeMessage) }
            if let resumeStorageError { return .failed(resumeStorageError) }
            return .failed(pendingResumes.contains(key) ? Self.pendingResumeMessage
                : "This session’s location changed. Refresh and open its current location.")
        }
        if plan.action == .unavailable || plan.action == .choose {
            return .failed(plan.reason ?? "Choose the session’s current location.")
        }
        let bundle = plan.location?.bundleId ?? SessionTerminalAdapter.terminal.rawValue
        let terminalResume = plan.action == .resume && plan.command?.args.first != "--desktop"
        if plan.action == .focus || terminalResume {
            if let error = await operations.authorize(bundle) { return .failed(error) }
        }
        // Permission dialogs can outlive the process. Re-plan afterward; never
        // convert a stale focus into a resume, or resume beside a new holder.
        do {
            let fresh = try await refresh()
            observe(key: key, plan: fresh)
            guard fresh.action == plan.action, Self.sameTarget(fresh.location, plan.location),
                  fresh.url == plan.url, fresh.command == plan.command else {
                return .failed("This session’s location changed. Refresh and open its current location.")
            }
        } catch { return .failed("Couldn’t verify this session’s current location. \(error.localizedDescription)") }
        switch plan.action {
        case .focus, .showApplication:
            guard let location = plan.location, operations.isCurrent(location) else {
                return .failed("This session’s process has ended or changed. Refresh and try again.")
            }
            if plan.action == .focus {
                if let error = await operations.focus(location) { return .failed(error) }
                guard operations.isCurrent(location) else {
                    return .failed("The original terminal was selected, but this session ended or moved during navigation.")
                }
                do {
                    let confirmed = try await refresh()
                    guard confirmed.action == .focus, Self.sameTarget(confirmed.location, location) else {
                        return .failed("The original terminal was selected, but the provider no longer reports this session there.")
                    }
                } catch { return .failed("The terminal was selected, but this session’s ownership could not be confirmed.") }
                return .focused
            }
            guard operations.show(location) else { return .failed("Couldn’t show \(location.application).") }
            return .dispatched("Showed \(location.application). Exact pane selection is unavailable for this terminal.")
        case .openLink:
            if let location = plan.location, !operations.isCurrent(location) {
                return .failed("This session’s process has ended or changed. Refresh and try again.")
            }
            guard let value = plan.url, let url = URL(string: value), operations.openLink(url) else {
                return .failed("The original application could not open this session link. No alternative session was started.")
            }
            return .dispatched("Sent the exact session link to its application. The application does not confirm session selection.")
        case .resume:
            guard let command = plan.command else { return .failed("The core did not provide a resume command.") }
            // Retain this beyond the adapter callback: shell startup may still
            // be blocked before the provider takes ownership of the session.
            if let resumeStorageError { return .failed(resumeStorageError) }
            do {
                let reservation = try store.reserve(key)
                pendingResumes = reservation.pending
                guard reservation.inserted else { return .failed(Self.pendingResumeMessage) }
            } catch { return .failed(recordStorageError(error)) }
            if let error = await operations.resume(command, plan.location) { return .failed(error) }
            return .dispatched("Sent the resume request for this session. The provider has not confirmed it opened.")
        case .choose, .unavailable, .unknown:
            return .failed(plan.reason ?? "Session opening is unavailable with this core version.")
        }
    }
}
