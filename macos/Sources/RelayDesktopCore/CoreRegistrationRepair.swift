import Foundation

/// What `launchctl print gui/<uid>/<label>` says about one job.
public struct LaunchdJob: Equatable, Sendable {
    public var loaded: Bool
    public var pid: Int?
    public var jobState: String?
    public var lastExitCode: Int?
    /// The CFBundleVersion launchd recorded when the job was submitted. After
    /// the app bundle is replaced it still names the old build.
    public var parentBundleVersion: String?

    public init(loaded: Bool, pid: Int? = nil, jobState: String? = nil, lastExitCode: Int? = nil,
                parentBundleVersion: String? = nil) {
        self.loaded = loaded
        self.pid = pid
        self.jobState = jobState
        self.lastExitCode = lastExitCode
        self.parentBundleVersion = parentBundleVersion
    }

    public static let missing = LaunchdJob(loaded: false)

    public var isRunning: Bool { loaded && pid != nil }
    public var spawnFailed: Bool { jobState == "spawn failed" || lastExitCode == 78 }

    public static func parse(success: Bool, output: String) -> LaunchdJob {
        guard success else { return .missing }
        var job = LaunchdJob(loaded: true)
        // Only the job's own top-level keys: nested dictionaries (endpoints,
        // coalitions) repeat names like `state` one tab deeper.
        for raw in output.split(separator: "\n") {
            guard raw.hasPrefix("\t"), !raw.hasPrefix("\t\t") else { continue }
            let line = raw.trimmingCharacters(in: .whitespaces)
            func value(_ key: String) -> String? {
                line.hasPrefix(key + " = ") ? String(line.dropFirst(key.count + 3)) : nil
            }
            if let pid = value("pid").flatMap(Int.init) { job.pid = pid }
            if let state = value("job state") { job.jobState = state }
            if let code = value("last exit code") {
                job.lastExitCode = Int(code.prefix { $0.isNumber || $0 == "-" })
            }
            if let version = value("parent bundle version") { job.parentBundleVersion = version }
        }
        return job
    }
}

/// The self-repair ladder for a core that does not answer. Each outage walks
/// it one rung per call, so the person never has to open Terminal:
///
/// 1. A persisted absolute-path fallback whose job is missing or broken is
///    dropped (deleting the app keeps its defaults, so a reinstall inherits
///    it), and SMAppService is tried again.
/// 2. An SMAppService job that macOS reports enabled but launchd never
///    loaded is registered again, which reloads it.
/// 3. A job that spawn-fails, has no process, or still names a replaced
///    bundle's version is unregistered and registered again. `register()`
///    alone is a no-op for an enabled job, and launchd keeps spawning the
///    old record (exit 78, EX_CONFIG).
/// 4. Only then the absolute-path LaunchAgent, as the last resort.
///
/// With startup at login off (a fresh install's default) the core runs as a
/// session job the app bootstraps itself. A session job launchd keeps loaded
/// but never runs ("Enabled", no process) is reloaded once; when that does not
/// help, the app starts the core itself, outside launchd, for this session.
/// Startup at login stays off and no login job is left behind. The core
/// refuses a second instance on its socket, so a late session job cannot
/// double it.
public enum CoreRegistrationRepair {
    public enum Action: String, Equatable, Sendable {
        /// The core runs; the connection just has not caught up.
        case none
        /// The person must allow Agent Relay in Login Items.
        case waitForApproval
        /// The bundle cannot keep a service (outside Applications).
        case moveToApplications
        case register
        case resetRegistration
        case leaveFallback
        case useFallback
        /// Startup at login is off: the session job is the only job.
        case reloadSessionJob
        /// The session job does not run even after a reload: the app starts
        /// the core itself for this session.
        case runCoreDirectly
        /// Every rung was tried this outage.
        case exhausted
    }

    public struct Observation: Equatable, Sendable {
        public var usesFallback: Bool
        public var startsAtLogin: Bool
        public var installationProblem: Bool
        public var registration: CoreServiceStatus
        public var bundledJob: LaunchdJob
        public var fallbackJob: LaunchdJob
        public var appBundleVersion: String?
        /// A core the app started itself is still running.
        public var directCoreRunning: Bool

        public init(usesFallback: Bool, startsAtLogin: Bool = true, installationProblem: Bool = false,
                    registration: CoreServiceStatus, bundledJob: LaunchdJob, fallbackJob: LaunchdJob = .missing,
                    appBundleVersion: String?, directCoreRunning: Bool = false) {
            self.directCoreRunning = directCoreRunning
            self.usesFallback = usesFallback
            self.startsAtLogin = startsAtLogin
            self.installationProblem = installationProblem
            self.registration = registration
            self.bundledJob = bundledJob
            self.fallbackJob = fallbackJob
            self.appBundleVersion = appBundleVersion
        }

        var bundledJobIsStale: Bool {
            guard let recorded = bundledJob.parentBundleVersion, let current = appBundleVersion else { return false }
            return recorded != current
        }
    }

    /// The rungs one outage may use. Past it, the person is told what to do.
    public static let stepLimit = 4

    /// `step` counts repairs already tried during this outage, from 0.
    public static func next(_ observation: Observation, step: Int) -> Action {
        if observation.installationProblem { return .moveToApplications }
        if observation.registration == .requiresApproval { return .waitForApproval }
        guard step < stepLimit else { return .exhausted }
        if !observation.startsAtLogin {
            if observation.fallbackJob.isRunning || observation.directCoreRunning { return .none }
            return step == 0 ? .reloadSessionJob : .runCoreDirectly
        }
        if observation.usesFallback {
            if observation.fallbackJob.isRunning { return .none }
            // Early rungs: the persisted choice is stale. Retry SMAppService.
            // The last rung: SMAppService already failed this outage, so the
            // fallback is the one remaining way; submit it once more.
            return step < stepLimit - 1 ? .leaveFallback : .useFallback
        }
        let job = observation.bundledJob
        if job.isRunning && !observation.bundledJobIsStale { return .none }
        switch step {
        case 0:
            switch observation.registration {
            case .enabled where !job.loaded: return .register
            case .notRegistered, .notFound: return .register
            default: return .resetRegistration
            }
        case 1:
            return .resetRegistration
        default:
            // register() and a full reset both left no running core.
            return .useFallback
        }
    }
}

/// Bounds an OS call that has no timeout of its own (SMAppService register,
/// unregister and status talk to smd and can block indefinitely when Background
/// Task Management is wedged). The work runs off the main actor; when it does
/// not finish in time the caller moves on and the late result is dropped.
public enum BoundedCall {
    public struct TimedOut: LocalizedError, Equatable {
        public let operation: String
        public init(operation: String) { self.operation = operation }
        public var errorDescription: String? { "macOS did not answer \(operation) in time." }
    }

    public static func run<T>(_ operation: String, timeout: TimeInterval,
                                        queue: DispatchQueue = .global(qos: .userInitiated),
                                        _ work: @escaping () throws -> T) async throws -> T {
        let gate = Gate<T>()
        return try await withCheckedThrowingContinuation { continuation in
            gate.install(continuation)
            queue.async { gate.finish(Result { try work() }) }
            DispatchQueue.global().asyncAfter(deadline: .now() + timeout) {
                gate.finish(.failure(TimedOut(operation: operation)))
            }
        }
    }

    private final class Gate<T>: @unchecked Sendable {
        private let lock = NSLock()
        private var continuation: CheckedContinuation<T, Error>?
        private var pending: Result<T, Error>?
        private var done = false

        func install(_ continuation: CheckedContinuation<T, Error>) {
            lock.lock()
            if let pending { lock.unlock(); continuation.resume(with: pending); return }
            self.continuation = continuation
            lock.unlock()
        }

        func finish(_ result: Result<T, Error>) {
            lock.lock()
            guard !done else { lock.unlock(); return }
            done = true
            guard let continuation else { pending = result; lock.unlock(); return }
            self.continuation = nil
            lock.unlock()
            continuation.resume(with: result)
        }
    }
}
