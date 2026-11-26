import Darwin
import Foundation
import OSLog
import RelayDesktopCore
import ServiceManagement

/// The core's LaunchAgent, `Contents/Library/LaunchAgents/<label>.plist`.
/// Its label is `<bundle id>.core`, as `build-app.sh` writes it, so each
/// build with its own bundle id (Agent Relay Dev, a build for another
/// site) has its own label, socket and data directory.
final class LaunchAgentService: CoreService {
    private static let logger = Logger(subsystem: AppInfo.subsystem, category: "launch-agent")
    static var plistName: String { "\(AppInfo.bundleIdentifier).core.plist" }

    /// OS calls are replaceable so recovery tests never register a real job.
    struct System {
        var bundledStatus: () -> SMAppService.Status
        var registerBundled: () throws -> Void
        var unregisterBundled: () throws -> Void
        var legacyStatus: (URL) -> SMAppService.Status
        var launchctl: ([String]) -> (success: Bool, output: String)
        var pause: (TimeInterval) -> Void
        var launchctlAsync: (@MainActor ([String]) async throws -> (success: Bool, output: String))? = nil
        /// The longest self-repair waits for one SMAppService call.
        var osCallTimeout: TimeInterval = 5
        /// The longest one self-repair rung may take, all of its calls together.
        var repairBudget: TimeInterval = 8
        /// Starts the helper outside launchd.
        var startCore: (URL, [String]) throws -> DirectCore = LaunchAgentService.startCoreProcess
        // A pending handoff must finish even when its caller is cancelled.
        // The timer suspends the actor without a blocking sleep or detached work.
        var pauseAsync: @MainActor (TimeInterval) async -> Void = { interval in
            await withCheckedContinuation { continuation in
                DispatchQueue.main.asyncAfter(deadline: .now() + interval) { continuation.resume() }
            }
        }

        static func live() -> Self {
            let service = SMAppService.agent(plistName: LaunchAgentService.plistName)
            return Self(bundledStatus: { service.status },
                        registerBundled: { try service.register() },
                        unregisterBundled: { try service.unregister() },
                        legacyStatus: { SMAppService.statusForLegacyPlist(at: $0) },
                        launchctl: LaunchAgentService.runLaunchctl,
                        pause: { Thread.sleep(forTimeInterval: $0) },
                        launchctlAsync: { arguments in
                            try await LaunchAgentService.runCommandAsync(LaunchAgentService.launchctlExecutable,
                                                                         arguments: arguments, timeout: 2)
                        })
        }
    }

    private let system: System
    private let repairUnregister: () throws -> Void
    private let repairRegister: () throws -> Void
    private let mutationGate: MutationGate
    /// A Stop is waiting for a change still in flight inside macOS.
    var hasPendingStop: Bool { mutationGate.hasPendingStop }
    private let bundleURL: URL
    private let homeDirectory: URL
    private let bundleIdentifier: String
    private let defaults: UserDefaults
    private(set) var spawnFailureEvidence: CoreServiceDiagnostics.SpawnFailure?

    init(bundleURL: URL = Bundle.main.bundleURL,
         homeDirectory: URL = FileManager.default.homeDirectoryForCurrentUser,
         bundleIdentifier: String = AppInfo.bundleIdentifier,
         defaults: UserDefaults = .standard, system: System? = nil) {
        self.bundleURL = bundleURL
        self.homeDirectory = homeDirectory
        self.bundleIdentifier = bundleIdentifier
        self.defaults = defaults
        // Every SMAppService change goes through one gate. A call the repair
        // stopped waiting for can still be running in macOS; until it has
        // returned, no other change starts, so a late unregister can never
        // undo a later registration.
        var gated = system ?? .live()
        let gate = MutationGate()
        mutationGate = gate
        let register = gated.registerBundled, unregister = gated.unregisterBundled
        gated.registerBundled = { try gate.start { try register() } }
        // Stopping waits for a change still in flight, so a registration a
        // repair started cannot land after the stop.
        let settle = gated.osCallTimeout
        gated.unregisterBundled = { try gate.stop(waitingUpTo: settle) { try unregister() } }
        // A repair rung never queues behind another change: it is refused.
        // Its register is not a newer start: it never cancels a pending stop.
        repairUnregister = { try gate.run { try unregister() } }
        repairRegister = { try gate.run { try register() } }
        self.system = gated
    }

    private var label: String { Self.plistName.replacingOccurrences(of: ".plist", with: "") }
    private var startsAtLoginKey: String { "\(label).startsAtLogin" }
    private var startsAtLogin: Bool { defaults.object(forKey: startsAtLoginKey) as? Bool ?? true }
    private var fallbackLabel: String { "\(label).\(startsAtLogin ? "local" : "session")" }
    private var fallbackKey: String { "\(label).usesLocalLaunchAgent" }
    private var fallbackPolicyKey: String { "\(startsAtLogin ? label : fallbackLabel).fallbackPolicyRevision" }
    static let fallbackPolicyRevision = 1
    private var hasCurrentFallbackPolicy: Bool {
        defaults.integer(forKey: fallbackPolicyKey) == Self.fallbackPolicyRevision
    }
    private var usesFallback: Bool { !startsAtLogin || defaults.bool(forKey: fallbackKey) }
    private var fallbackURL: URL {
        let directory = startsAtLogin ? "Library/LaunchAgents" : "Library/Application Support/AgentRelay/SessionServices"
        return homeDirectory.appendingPathComponent("\(directory)/\(fallbackLabel).plist")
    }

    var installationProblem: String? {
        guard !AppInfo.isDevelopment, bundlesPlist,
              !Self.isInstalledInApplications(bundleURL, homeDirectory: homeDirectory) else { return nil }
        return "Move Agent Relay to Applications, then open that copy. A copy opened from a disk image or temporary folder cannot keep its background service connected."
    }

    static func isInstalledInApplications(_ bundleURL: URL, homeDirectory: URL) -> Bool {
        let parent = bundleURL.standardizedFileURL.resolvingSymlinksInPath()
            .deletingLastPathComponent().path
        let applications = URL(fileURLWithPath: "/Applications", isDirectory: true)
            .resolvingSymlinksInPath().path
        let userApplications = homeDirectory.appendingPathComponent("Applications", isDirectory: true)
            .resolvingSymlinksInPath().path
        return parent == applications || parent == userApplications
    }

    var diagnostics: CoreServiceDiagnostics {
        let inApplications = Self.isInstalledInApplications(bundleURL, homeDirectory: homeDirectory)
        let path = bundleURL.standardizedFileURL.resolvingSymlinksInPath()
        let components = path.pathComponents
        let translocated = components.indices.dropLast(3).contains { index in
            components[index] == "AppTranslocation" && UUID(uuidString: components[index + 1]) != nil &&
                components[index + 2] == "d"
        }
        let quarantine: CoreServiceDiagnostics.Quarantine
        if getxattr(path.path, "com.apple.quarantine", nil, 0, 0, XATTR_NOFOLLOW) >= 0 {
            quarantine = .present
        } else {
            quarantine = errno == ENOATTR ? .absent : .unknown
        }
        return .init(smAppServiceStatus: Self.bundledRegistration(system.bundledStatus()),
                     installState: bundlesPlist ? (inApplications ? .installed : .outsideApplications) : .noAgent,
                     inApplications: inApplications, translocated: translocated,
                     quarantine: quarantine, backupAgent: usesFallback ? (startsAtLogin ? .local : .session) : .bundled,
                     spawnFailure: spawnFailureEvidence)
    }

    func clearSpawnFailureEvidence() { spawnFailureEvidence = nil }

    var status: CoreServiceStatus {
        if usesFallback {
            if system.legacyStatus(fallbackURL) == .requiresApproval { return .requiresApproval }
            let loaded = launchctl("print", "gui/\(getuid())/\(fallbackLabel)")
            return fallbackStatus(loaded)
        }
        return bundledStatus
    }

    @MainActor func statusAsync() async throws -> CoreServiceStatus {
        guard usesFallback else { return bundledStatus }
        if system.legacyStatus(fallbackURL) == .requiresApproval { return .requiresApproval }
        return fallbackStatus(try await launchctlAsync("print", fallbackTarget))
    }

    private var fallbackTarget: String { "gui/\(getuid())/\(fallbackLabel)" }

    private func fallbackStatus(_ loaded: (success: Bool, output: String)) -> CoreServiceStatus {
        guard loaded.success,
              let expected = try? fallbackPlist()["Program"] as? String,
              Self.loadedProgram(loaded.output) == expected,
              hasCurrentFallbackPolicy else { return .notRegistered }
        return .enabled
    }

    private var bundledStatus: CoreServiceStatus {
        let status = Self.bundledRegistration(system.bundledStatus()) ?? .notFound
        // A bundled plist can be registered even when launchd has no record yet.
        return status == .notFound && bundlesPlist ? .notRegistered : status
    }

    private static func bundledRegistration(_ status: SMAppService.Status) -> CoreServiceStatus? {
        switch status {
        case .enabled: return .enabled
        case .requiresApproval: return .requiresApproval
        case .notRegistered: return .notRegistered
        case .notFound: return .notFound
        @unknown default: return nil
        }
    }

    private var bundlesPlist: Bool {
        FileManager.default.fileExists(atPath: bundleURL
            .appendingPathComponent("Contents/Library/LaunchAgents/\(Self.plistName)").path)
    }

    /// This build: the bundled core's SHA-256, which `build-app.sh` records
    /// beside it, the app executable, and the bundle's absolute path. Moving
    /// the app changes the fallback LaunchAgent's Program even if its bytes
    /// are identical, so that move must re-register the loaded job.
    var binaryIdentity: String? {
        guard let url = Bundle.main.url(forResource: "agent-relay-probe", withExtension: "sha256"),
              let line = try? String(contentsOf: url, encoding: .utf8),
              let digest = line.split(separator: " ").first, !digest.isEmpty,
              let executable = Bundle.main.executableURL,
              let attributes = try? FileManager.default.attributesOfItem(atPath: executable.path),
              let modified = attributes[.modificationDate] as? Date else { return nil }
        return Self.binaryIdentity(digest: String(digest), modified: modified,
                                   bundleURL: bundleURL)
    }

    static func binaryIdentity(digest: String, modified: Date, bundleURL: URL) -> String {
        "\(digest)-\(modified.timeIntervalSince1970)-\(bundleURL.standardizedFileURL.path)"
    }

    func setStartsAtLogin(_ enabled: Bool) throws {
        guard bundlesPlist, startsAtLogin != enabled else { return }
        if let installationProblem { throw fallbackError(installationProblem) }
        let previous = startsAtLogin
        // Retire the old job before switching paths and labels. A session-only
        // job is loaded explicitly, never discovered by launchd at next login.
        if usesFallback || system.bundledStatus() == .enabled || system.bundledStatus() == .requiresApproval {
            try unregister()
        }
        defaults.set(enabled, forKey: startsAtLoginKey)
        do {
            try register()
        } catch {
            try? unregister()
            defaults.set(previous, forKey: startsAtLoginKey)
            try? register()
            throw error
        }
    }

    func register() throws {
        if let installationProblem { throw fallbackError(installationProblem) }
        guard usesFallback else { try system.registerBundled(); return }
        _ = try registerFallback(forceReload: true)
        retireBundledJob()
    }

    /// A fallback update is a reload, not a user-requested removal. Keeping the
    /// plist avoids withdrawing its BTM record between the two signed builds.
    func reloadForUpdate() throws -> Bool {
        guard usesFallback else { return false }
        if let installationProblem { throw fallbackError(installationProblem) }
        _ = try registerFallback(forceReload: true, bootstrapAttempts: 4)
        retireBundledJob()
        return true
    }

    @discardableResult
    private func registerFallback(forceReload: Bool, bootstrapAttempts: Int = 1) throws -> Bool {
        let preparation = try prepareFallback(forceReload: forceReload)
        guard preparation.submit else { return false }
        if preparation.bootout { try bootoutFallback() }
        return try bootstrapFallback(attempts: bootstrapAttempts)
    }

    private func prepareFallback(forceReload: Bool) throws -> (submit: Bool, bootout: Bool) {
        let plist = try writeFallbackPlist()
        guard system.legacyStatus(fallbackURL) != .requiresApproval else { return (false, false) }
        return fallbackPreparation(plist, loaded: launchctl("print", fallbackTarget), forceReload: forceReload)
    }

    private func writeFallbackPlist() throws -> [String: Any] {
        let plist = try fallbackPlist()
        let contents = try PropertyListSerialization.data(fromPropertyList: plist, format: .xml, options: 0)
        try FileManager.default.createDirectory(at: fallbackURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        try contents.write(to: fallbackURL, options: .atomic)
        return plist
    }

    private func fallbackPreparation(_ plist: [String: Any], loaded: (success: Bool, output: String),
                                     forceReload: Bool) -> (submit: Bool, bootout: Bool) {
        if loaded.success, !forceReload, hasCurrentFallbackPolicy,
           Self.loadedProgram(loaded.output) == plist["Program"] as? String { return (false, false) }
        return (true, loaded.success)
    }

    private func bootstrapFallback(attempts: Int) throws -> Bool {
        for attempt in 0..<attempts {
            let result = launchctl("bootstrap", "gui/\(getuid())", fallbackURL.path)
            if result.success {
                // A rewritten plist is not the loaded job. Only successful
                // submission completes migration, even with the same binary.
                defaults.set(Self.fallbackPolicyRevision, forKey: fallbackPolicyKey)
                return true
            }
            // Approval may become visible only after bootstrap. Never retry it
            // or replace a label another registration has already submitted.
            guard system.legacyStatus(fallbackURL) != .requiresApproval else { return false }
            guard attempt + 1 < attempts,
                  !launchctl("print", "gui/\(getuid())/\(fallbackLabel)").success else {
                throw fallbackError(result.output)
            }
            system.pause(0.25)
        }
        return false
    }

    private func bootoutFallback() throws {
        let result = launchctl("bootout", "gui/\(getuid())/\(fallbackLabel)")
        // bootout can return before the old label disappears. Never treat a
        // lingering job as the newly submitted one, even with the same path.
        // launchd gives an exiting job five seconds before forcing it down.
        // A one-second poll budget abandons the handoff while that job is
        // still retiring, leaving no replacement once it finally exits.
        let deadline = DispatchTime.now() + 6
        for _ in 0..<120 {
            if !launchctl("print", "gui/\(getuid())/\(fallbackLabel)").success { return }
            guard result.success else { throw fallbackError(result.output) }
            guard DispatchTime.now() < deadline else { break }
            system.pause(0.05)
        }
        throw fallbackError("The previous background service has not stopped yet. Try again.")
    }

    func unregister() throws {
        serviceEpoch += 1
        stopDirectCore()
        guard usesFallback else { try system.unregisterBundled(); return }
        if launchctl("print", "gui/\(getuid())/\(fallbackLabel)").success {
            try bootoutFallback()
        }
        try finishUnregisterFallback()
    }

    private func finishUnregisterFallback() throws {
        // A failed submission (or failed retirement) can leave the broken
        // original registered alongside the selected fallback. Stop owns both.
        if system.bundledStatus() == .enabled || system.bundledStatus() == .requiresApproval {
            try system.unregisterBundled()
        }
        if FileManager.default.fileExists(atPath: fallbackURL.path) {
            try FileManager.default.removeItem(at: fallbackURL)
        }
        defaults.removeObject(forKey: fallbackPolicyKey)
    }

    /// BTM can retain a relative agent URL with no parent bundle even though
    /// SMAppService says enabled. Re-registering repeats the same failed job.
    /// Only this exact launchd failure switches to a user LaunchAgent whose
    /// Program names the helper in the installed app by absolute path.
    func recoverSpawnFailure() throws -> Bool {
        guard installationProblem == nil else { return false }
        if usesFallback {
            let recovered = try registerFallback(forceReload: false)
            retireBundledJob()
            return recovered
        }
        guard system.bundledStatus() == .enabled else { return false }
        let result = launchctl("print", "gui/\(getuid())/\(label)")
        guard result.success, Self.hasBrokenBundleProgram(result.output) else { return false }
        spawnFailureEvidence = .brokenBundleProgram
        // Persist the selected recovery method, not a claim that it started.
        // status and the usesFallback branch above handle failed submissions
        // immediately, including after relaunch or an original job disappearing.
        defaults.set(true, forKey: fallbackKey)
        // Keep the broken job registered until the fallback is submitted.
        do { try registerFallback(forceReload: true) } catch {
            throw fallbackError("The backup background service could not start: \(error.localizedDescription)")
        }
        retireBundledJob()
        return true
    }


    // Startup and recovery use these actor-suspending variants. The synchronous
    // methods remain available for the final, synchronous app-quit handoff.
    @MainActor func registerAsync() async throws {
        if let installationProblem { throw fallbackError(installationProblem) }
        guard usesFallback else { try system.registerBundled(); return }
        _ = try await registerFallbackAsync(forceReload: true)
        retireBundledJob()
    }

    @MainActor func reloadForUpdateAsync() async throws -> Bool {
        guard usesFallback else { return false }
        if let installationProblem { throw fallbackError(installationProblem) }
        _ = try await registerFallbackAsync(forceReload: true, bootstrapAttempts: 4)
        retireBundledJob()
        return true
    }

    @MainActor private func registerFallbackAsync(forceReload: Bool, bootstrapAttempts: Int = 1) async throws -> Bool {
        let plist = try writeFallbackPlist()
        guard system.legacyStatus(fallbackURL) != .requiresApproval else { return false }
        let loaded = try await launchctlAsync("print", fallbackTarget)
        let preparation = fallbackPreparation(plist, loaded: loaded, forceReload: forceReload)
        guard preparation.submit else { return false }
        if preparation.bootout { try await bootoutFallbackAsync() }
        for attempt in 0..<bootstrapAttempts {
            let result = try await launchctlAsync("bootstrap", "gui/\(getuid())", fallbackURL.path)
            if result.success {
                defaults.set(Self.fallbackPolicyRevision, forKey: fallbackPolicyKey)
                return true
            }
            guard system.legacyStatus(fallbackURL) != .requiresApproval else { return false }
            guard attempt + 1 < bootstrapAttempts,
                  !(try await launchctlAsync("print", fallbackTarget)).success else {
                throw fallbackError(result.output)
            }
            await system.pauseAsync(0.25)
        }
        return false
    }

    @MainActor private func bootoutFallbackAsync() async throws {
        let result = try await launchctlAsync("bootout", fallbackTarget)
        let deadline = DispatchTime.now() + 6
        for _ in 0..<120 {
            if !(try await launchctlAsync("print", fallbackTarget)).success { return }
            guard result.success else { throw fallbackError(result.output) }
            guard DispatchTime.now() < deadline else { break }
            await system.pauseAsync(0.05)
        }
        throw fallbackError("The previous background service has not stopped yet. Try again.")
    }

    @MainActor func unregisterAsync() async throws {
        serviceEpoch += 1
        stopDirectCore()
        guard usesFallback else { try system.unregisterBundled(); return }
        if (try await launchctlAsync("print", fallbackTarget)).success {
            try await bootoutFallbackAsync()
        }
        try finishUnregisterFallback()
    }

    @MainActor func setStartsAtLoginAsync(_ enabled: Bool) async throws {
        guard bundlesPlist, startsAtLogin != enabled else { return }
        if let installationProblem { throw fallbackError(installationProblem) }
        let previous = startsAtLogin
        if usesFallback || system.bundledStatus() == .enabled || system.bundledStatus() == .requiresApproval {
            try await unregisterAsync()
        }
        defaults.set(enabled, forKey: startsAtLoginKey)
        do { try await registerAsync() }
        catch {
            try? await unregisterAsync()
            defaults.set(previous, forKey: startsAtLoginKey)
            try? await registerAsync()
            throw error
        }
    }

    @MainActor func recoverSpawnFailureAsync() async throws -> Bool {
        guard installationProblem == nil else { return false }
        if usesFallback {
            let recovered = try await registerFallbackAsync(forceReload: false)
            retireBundledJob()
            return recovered
        }
        guard system.bundledStatus() == .enabled else { return false }
        let result = try await launchctlAsync("print", "gui/\(getuid())/\(label)")
        guard result.success, Self.hasBrokenBundleProgram(result.output) else { return false }
        spawnFailureEvidence = .brokenBundleProgram
        defaults.set(true, forKey: fallbackKey)
        do { _ = try await registerFallbackAsync(forceReload: true) }
        catch {
            if (error as NSError).domain == Self.errorDomain { throw error }
            throw fallbackError("The backup background service could not start: \(error.localizedDescription)")
        }
        retireBundledJob()
        return true
    }

    private func retireBundledJob() {
        guard system.bundledStatus() == .enabled else { return }
        // The affected install proved both labels can coexist if BTM refuses
        // to remove the broken original job.
        do { try system.unregisterBundled() } catch {
            Self.logger.error("could not remove the broken SMAppService job: \(error.localizedDescription, privacy: .public)")
        }
    }

    static func hasBrokenBundleProgram(_ output: String) -> Bool {
        let lines = output.split(separator: "\n").map { $0.trimmingCharacters(in: .whitespaces) }
        return lines.contains("job state = spawn failed")
            && lines.contains { $0.range(of: #"^last exit code = 78(?::|\s|$)"#, options: .regularExpression) != nil }
            && lines.contains { $0.hasPrefix("program identifier = Contents/Helpers/") }
    }

    static func loadedProgram(_ output: String) -> String? {
        output.split(separator: "\n").lazy.map { $0.trimmingCharacters(in: .whitespaces) }
            .first { $0.hasPrefix("program = ") }.map { String($0.dropFirst("program = ".count)) }
    }

    private func fallbackPlist() throws -> [String: Any] {
        let bundled = bundleURL
            .appendingPathComponent("Contents/Library/LaunchAgents/\(Self.plistName)")
        let data = try Data(contentsOf: bundled)
        guard let source = try PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any],
              let program = source["BundleProgram"] as? String,
              program.hasPrefix("Contents/Helpers/"),
              let arguments = source["ProgramArguments"] as? [String],
              !arguments.isEmpty else { throw fallbackError("The bundled core LaunchAgent is invalid.") }
        let helper = bundleURL.appendingPathComponent(program).standardizedFileURL
        // Contained lexically and after symlinks resolve: a link inside
        // Contents/Helpers must not lead to a program outside the bundle.
        let resolvedBundle = bundleURL.standardizedFileURL.resolvingSymlinksInPath().path + "/"
        guard helper.path.hasPrefix(bundleURL.standardizedFileURL.path + "/"),
              helper.resolvingSymlinksInPath().path.hasPrefix(resolvedBundle),
              FileManager.default.isExecutableFile(atPath: helper.path) else {
            throw fallbackError("The bundled core helper is missing.")
        }
        var plist: [String: Any] = [
            "Label": fallbackLabel,
            "Program": helper.path,
            "ProgramArguments": arguments,
            "AssociatedBundleIdentifiers": [bundleIdentifier],
            "RunAtLoad": true,
            "KeepAlive": true,
            "ThrottleInterval": 10
        ]
        // The Agent Relay family marker build-app.sh gave the bundled job, so
        // another Agent Relay product recognises this one (ProductTakeover).
        if let product = source[ProductTakeover.markerKey] as? String { plist[ProductTakeover.markerKey] = product }
        return plist
    }

    private func launchctl(_ arguments: String...) -> (success: Bool, output: String) {
        system.launchctl(arguments)
    }

    @MainActor private func launchctlAsync(_ arguments: String...) async throws -> (success: Bool, output: String) {
        // During a repair every launchctl call is checked against a stop and
        // the rung's deadline. A read is also cut off at the deadline; a
        // bootstrap or bootout is never abandoned midway (each is bounded on
        // its own) and is not started once the rung is stale or out of time.
        if activeRepairEpoch != nil {
            try checkRepairCurrent()
            let remaining = try remainingRepairTime()
            let result: (success: Bool, output: String)
            if arguments.first == "print" {
                result = try await BoundedCall.runAsync("launchctl", timeout: remaining) { [self] in
                    try await runLaunchctl(arguments)
                }
            } else {
                result = try await runLaunchctl(arguments)
            }
            try checkRepairCurrent()
            return result
        }
        return try await runLaunchctl(arguments)
    }

    @MainActor private func runLaunchctl(_ arguments: [String]) async throws -> (success: Bool, output: String) {
        if let run = system.launchctlAsync { return try await run(arguments) }
        return system.launchctl(arguments)
    }


    static func runLaunchctl(_ arguments: [String]) -> (success: Bool, output: String) {
        do { return try commandQueue.sync { try runCommand(launchctlExecutable, arguments: arguments, timeout: 2) } }
        catch { return (false, error.localizedDescription) }
    }

    private static let launchctlExecutable = URL(fileURLWithPath: "/bin/launchctl")
    private static let commandQueue = DispatchQueue(label: "com.agentrelay.desktop.launchctl", qos: .userInitiated)

    static func runCommandAsync(_ executable: URL, arguments: [String],
                                timeout: TimeInterval) async throws -> (success: Bool, output: String) {
        try await withCheckedThrowingContinuation { continuation in
            commandQueue.async {
                continuation.resume(with: Result { try runCommand(executable, arguments: arguments, timeout: timeout) })
            }
        }
    }

    static let errorDomain = "com.agentrelay.desktop.launch-agent"

    private static func runCommand(_ executable: URL, arguments: [String], timeout: TimeInterval) throws -> (success: Bool, output: String) {
        let deadline = DispatchTime.now() + timeout
        let process = Process()
        process.executableURL = executable
        process.arguments = arguments
        let pipe = Pipe()
        let reader = pipe.fileHandleForReading
        let fd = reader.fileDescriptor
        guard fcntl(fd, F_SETFL, O_NONBLOCK) == 0 else { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = pipe
        process.standardError = pipe
        try process.run()
        defer {
            if process.isRunning { kill(process.processIdentifier, SIGKILL) }
            try? reader.close()
        }
        var output = Data()
        var buffer = [UInt8](repeating: 0, count: 4096)
        while DispatchTime.now() < deadline {
            // Sample exit before reading so a child that writes and exits
            // between the two observations still gets one final drain.
            let running = process.isRunning
            let count = buffer.withUnsafeMutableBytes { read(fd, $0.baseAddress, $0.count) }
            if count > 0 {
                guard output.count + count <= 64 * 1024 else {
                    throw NSError(domain: errorDomain, code: 3, userInfo: [NSLocalizedDescriptionKey: "launchctl returned too much output."])
                }
                output.append(contentsOf: buffer.prefix(count))
                continue
            }
            if count < 0, errno == EINTR { continue }
            if count < 0, errno != EAGAIN { throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno)) }
            if !running {
                return (process.terminationStatus == 0, String(decoding: output, as: UTF8.self))
            }
            Thread.sleep(forTimeInterval: 0.01)
        }
        throw NSError(domain: errorDomain, code: 2, userInfo: [NSLocalizedDescriptionKey: "launchctl did not finish in time."])
    }

    private func fallbackError(_ message: String) -> NSError {
        NSError(domain: Self.errorDomain, code: 1,
                userInfo: [NSLocalizedDescriptionKey: message])
    }

    // MARK: Self-repair

    /// Admits one SMAppService change at a time, across threads, for as long
    /// as macOS takes to return it.
    final class MutationGate: @unchecked Sendable {
        private let lock = NSLock()
        private var inFlight = false

        static let busyCode = 5
        var isBusy: Bool { lock.lock(); defer { lock.unlock() }; return inFlight }

        private let released = NSCondition()

        /// Runs `work` once no other change is in flight, waiting up to
        /// `wait` for one to return (0: refuse at once).
        func run<T>(waitingUpTo wait: TimeInterval = 0, _ work: () throws -> T) throws -> T {
            let deadline = Date().addingTimeInterval(wait)
            released.lock()
            while isBusy {
                guard released.wait(until: deadline) || !isBusy else { break }
            }
            lock.lock()
            guard !inFlight else {
                lock.unlock()
                released.unlock()
                throw NSError(domain: LaunchAgentService.errorDomain, code: Self.busyCode, userInfo: [
                    NSLocalizedDescriptionKey: "macOS is still finishing an earlier background service change. Try again."
                ])
            }
            inFlight = true
            lock.unlock()
            released.unlock()
            defer { finish() }
            return try work()
        }

        /// A stop that could not run because another change was still in
        /// flight. It runs as soon as that change returns, on the thread that
        /// made it, unless a newer start comes first.
        private var pendingStop: (() throws -> Void)?

        /// Stops now, or once the change in flight has returned: Stop never
        /// blocks the UI past `wait`, and is never dropped.
        func stop(waitingUpTo wait: TimeInterval, _ work: @escaping () throws -> Void) throws {
            do { try run(waitingUpTo: wait, work) } catch let error as NSError where error.code == Self.busyCode
                && error.domain == LaunchAgentService.errorDomain {
                lock.lock()
                if inFlight {
                    pendingStop = work
                    lock.unlock()
                    return
                }
                lock.unlock()
                try run(work)
            }
        }

        /// A newer start replaces a stop still waiting to run.
        func start<T>(_ work: () throws -> T) throws -> T {
            lock.lock(); pendingStop = nil; lock.unlock()
            return try run(work)
        }

        var hasPendingStop: Bool { lock.lock(); defer { lock.unlock() }; return pendingStop != nil }

        private func finish() {
            released.lock()
            lock.lock()
            let stop = pendingStop
            pendingStop = nil
            inFlight = stop != nil      // the stop keeps the gate: nothing slips in between
            lock.unlock()
            if let stop {
                released.unlock()
                do { try stop() } catch {
                    LaunchAgentService.logger.error("the delayed stop failed: \(error.localizedDescription, privacy: .public)")
                }
                released.lock()
                lock.lock(); inFlight = false; lock.unlock()
            }
            released.broadcast()
            released.unlock()
        }
    }

    /// When the current rung must have finished.
    private var repairDeadline: Date?
    /// Moves on whenever the service is stopped or unregistered. A rung that
    /// started under an earlier value changes nothing more once it resumes.
    private var serviceEpoch = 0
    private var activeRepairEpoch: Int?

    private func checkRepairCurrent() throws {
        guard let activeRepairEpoch else { return }
        if activeRepairEpoch != serviceEpoch || Task.isCancelled { throw CancellationError() }
    }

    private func remainingRepairTime() throws -> TimeInterval {
        guard let repairDeadline else { return system.osCallTimeout }
        let remaining = repairDeadline.timeIntervalSinceNow
        guard remaining > 0 else { throw BoundedCall.TimedOut(operation: "the background service repair") }
        return remaining
    }

    /// A core this app started itself after its session job would not run.
    struct DirectCore {
        var isRunning: () -> Bool
        var stop: () -> Void
    }
    private var directCore: DirectCore?

    /// Stopping the service also stops a core the app started itself.
    private func stopDirectCore() {
        directCore?.stop()
        directCore = nil
    }

    static func startCoreProcess(_ program: URL, arguments: [String]) throws -> DirectCore {
        let process = Process()
        process.executableURL = program
        process.arguments = arguments
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        try process.run()
        return DirectCore(isRunning: { process.isRunning }, stop: { if process.isRunning { process.terminate() } })
    }

    private var appBundleVersion: String? {
        Bundle(url: bundleURL)?.object(forInfoDictionaryKey: "CFBundleVersion") as? String
    }

    private func boundedOS<T>(_ operation: String, _ work: @escaping () throws -> T) async throws -> T {
        try checkRepairCurrent()
        let result = try await BoundedCall.run(operation, timeout: min(system.osCallTimeout, try remainingRepairTime()), work)
        try checkRepairCurrent()
        return result
    }

    /// Whether the person must allow the service, read again right before a
    /// rung changes anything: approval can change while the rung runs.
    @MainActor private func requiresApprovalNow() async throws -> Bool {
        let status = system.bundledStatus
        if try await boundedOS("the background service status", { status() }) == .requiresApproval { return true }
        guard usesFallback else { return false }
        let legacy = system.legacyStatus, url = fallbackURL
        return try await boundedOS("the backup service status") { legacy(url) } == .requiresApproval
    }

    private static func changesRegistration(_ action: CoreRegistrationRepair.Action) -> Bool {
        switch action {
        case .none, .waitForApproval, .moveToApplications, .exhausted: return false
        case .register, .resetRegistration, .leaveFallback, .useFallback, .reloadSessionJob, .runCoreDirectly: return true
        }
    }

    @MainActor func repairObservation() async throws -> CoreRegistrationRepair.Observation {
        let status = system.bundledStatus
        let registration = Self.bundledRegistration(try await boundedOS("the background service status") { status() })
        let legacy = system.legacyStatus
        let fallbackURL = fallbackURL
        let fallbackApproval = usesFallback
            ? try await boundedOS("the backup service status") { legacy(fallbackURL) } : .enabled
        let bundled = try await launchctlAsync("print", "gui/\(getuid())/\(label)")
        let fallback = try await launchctlAsync("print", fallbackTarget)
        var registrationState = registration ?? .notFound
        if registrationState == .notFound && bundlesPlist { registrationState = .notRegistered }
        if fallbackApproval == .requiresApproval { registrationState = .requiresApproval }
        return .init(usesFallback: usesFallback, startsAtLogin: startsAtLogin,
                     installationProblem: installationProblem != nil, registration: registrationState,
                     bundledJob: LaunchdJob.parse(success: bundled.success, output: bundled.output),
                     fallbackJob: LaunchdJob.parse(success: fallback.success, output: fallback.output),
                     appBundleVersion: appBundleVersion, directCoreRunning: directCore?.isRunning() == true)
    }

    /// One rung of `CoreRegistrationRepair` for a core that does not answer.
    /// Every OS call is bounded, so the caller's "Restarting service…" ends.
    @MainActor func selfRepairAsync(step: Int) async throws -> CoreRegistrationRepair.Action {
        let epoch = serviceEpoch
        activeRepairEpoch = epoch
        repairDeadline = Date().addingTimeInterval(system.repairBudget)
        defer {
            if activeRepairEpoch == epoch {
                activeRepairEpoch = nil
                repairDeadline = nil
            }
        }
        let observation = try await repairObservation()
        let action = CoreRegistrationRepair.next(observation, step: step)
        if Self.changesRegistration(action), try await requiresApprovalNow() { return .waitForApproval }
        Self.logger.notice("""
            core self-repair step \(step, privacy: .public): \(action.rawValue, privacy: .public) \
            (registration \(observation.registration.rawValue, privacy: .public), \
            fallback \(observation.usesFallback, privacy: .public), \
            job loaded \(observation.bundledJob.loaded, privacy: .public) \
            pid \(observation.bundledJob.pid ?? -1, privacy: .public) \
            exit \(observation.bundledJob.lastExitCode ?? -1, privacy: .public) \
            version \(observation.bundledJob.parentBundleVersion ?? "-", privacy: .public)/\(observation.appBundleVersion ?? "-", privacy: .public))
            """)
        switch action {
        case .none, .waitForApproval, .moveToApplications, .exhausted:
            break
        case .register:
            let register = repairRegister
            try await boundedOS("registering the background service") { try register() }
        case .resetRegistration:
            try await resetBundledRegistration()
        case .leaveFallback:
            // Deleting the app keeps its defaults, so a reinstalled build can
            // inherit a fallback choice whose job no longer exists.
            if (try await launchctlAsync("print", fallbackTarget)).success { try await bootoutFallbackAsync() }
            if FileManager.default.fileExists(atPath: fallbackURL.path) {
                try FileManager.default.removeItem(at: fallbackURL)
            }
            defaults.removeObject(forKey: fallbackKey)
            defaults.removeObject(forKey: fallbackPolicyKey)
            try await resetBundledRegistration()
        case .useFallback:
            defaults.set(true, forKey: fallbackKey)
            _ = try await registerFallbackAsync(forceReload: true)
            let status = system.bundledStatus, unregister = repairUnregister
            if try await boundedOS("the background service status", { status() }) == .enabled {
                do { try await boundedOS("removing the broken background service") { try unregister() } } catch {
                    Self.logger.error("could not remove the broken SMAppService job: \(error.localizedDescription, privacy: .public)")
                }
            }
        case .reloadSessionJob:
            _ = try await registerFallbackAsync(forceReload: true)
        case .runCoreDirectly:
            // Startup at login stays off: no login job is registered. The
            // session plist stays for the next launch to try again.
            if (try await launchctlAsync("print", fallbackTarget)).success { try await bootoutFallbackAsync() }
            let plist = try fallbackPlist()
            guard let program = plist["Program"] as? String,
                  let arguments = plist["ProgramArguments"] as? [String] else {
                throw fallbackError("The bundled core LaunchAgent is invalid.")
            }
            try checkRepairCurrent()
            directCore = try system.startCore(URL(fileURLWithPath: program), Array(arguments.dropFirst()))
        }
        return action
    }

    /// `register()` is a no-op for a job macOS already calls enabled, while
    /// launchd keeps spawning the replaced bundle's record. Only removing the
    /// registration and submitting it again records the current bundle.
    @MainActor private func resetBundledRegistration() async throws {
        let unregister = repairUnregister, register = repairRegister, status = system.bundledStatus
        // Only a registration macOS reports is removed. A job that is already
        // gone is not an error; any other refusal (a denial, a busy gate) is.
        if try await boundedOS("the background service status", { status() }) == .enabled {
            do { try await boundedOS("removing the background service") { try unregister() } }
            catch where Self.isAlreadyUnregistered(error) {
                Self.logger.notice("the background service was already unregistered")
            }
        }
        let target = "gui/\(getuid())/\(label)"
        // Bounded by the rung's deadline and, outside one, by a count.
        for _ in 0..<60 {
            _ = try remainingRepairTime()
            if !(try await launchctlAsync("print", target)).success { break }
            await system.pauseAsync(0.1)
        }
        try await boundedOS("registering the background service") { try register() }
    }

    static func isAlreadyUnregistered(_ error: Error) -> Bool {
        guard !(error is BoundedCall.TimedOut) else { return false }
        let error = error as NSError
        return error.domain != errorDomain
            && (error.code == Int(kSMErrorJobNotFound) || error.code == Int(kSMErrorJobPlistNotFound))
    }

    func openSystemSettings() { SMAppService.openSystemSettingsLoginItems() }
}
