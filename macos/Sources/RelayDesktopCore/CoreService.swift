import Foundation

/// Where the relay core runs: a launchd LaunchAgent the app registers
/// through SMAppService or, when macOS loses its bundle path, directly.
/// It runs at login with no window, keeps
/// delivering through app crashes and updates, and launchd restarts it
/// when it exits. Behind a protocol so tests never touch launchd.
public protocol CoreService: AnyObject {
    var status: CoreServiceStatus { get }
    @MainActor func statusAsync() async throws -> CoreServiceStatus
    var diagnostics: CoreServiceDiagnostics { get }
    var spawnFailureEvidence: CoreServiceDiagnostics.SpawnFailure? { get }
    func clearSpawnFailureEvidence()
    /// One rung of the self-repair ladder for a core that does not answer.
    @MainActor func selfRepairAsync(step: Int) async throws -> CoreRegistrationRepair.Action
    /// Why this copy of the app must be moved before registering its agent.
    var installationProblem: String? { get }
    /// Identifies the core binary this bundle carries. launchd admits only
    /// the binary the agent was registered with, so a new one (an update)
    /// is registered again.
    var binaryIdentity: String? { get }
    /// Controls whether registration survives logout. Off still permits a
    /// service for this login session when the person opens the app manually.
    func setStartsAtLogin(_ enabled: Bool) throws
    func register() throws
    func unregister() throws
    /// Replaces a loaded fallback job without deleting its persistent plist.
    /// Returns false when the normal unregister/register handoff is required.
    func reloadForUpdate() throws -> Bool
    /// Repairs the macOS BTM record that loses the bundle containing a
    /// BundleProgram. Other implementations have nothing to repair.
    func recoverSpawnFailure() throws -> Bool
    /// Opens System Settings › General › Login Items.
    @MainActor func setStartsAtLoginAsync(_ enabled: Bool) async throws
    @MainActor func registerAsync() async throws
    @MainActor func unregisterAsync() async throws
    @MainActor func reloadForUpdateAsync() async throws -> Bool
    @MainActor func recoverSpawnFailureAsync() async throws -> Bool
    func openSystemSettings()
}

public extension CoreService {
    @MainActor func statusAsync() async throws -> CoreServiceStatus { status }
    var diagnostics: CoreServiceDiagnostics { .init() }
    var spawnFailureEvidence: CoreServiceDiagnostics.SpawnFailure? { nil }
    func clearSpawnFailureEvidence() {}
    @MainActor func selfRepairAsync(step: Int) async throws -> CoreRegistrationRepair.Action { .none }
    @MainActor func setStartsAtLoginAsync(_ enabled: Bool) async throws { try setStartsAtLogin(enabled) }
    @MainActor func registerAsync() async throws { try register() }
    @MainActor func unregisterAsync() async throws { try unregister() }
    @MainActor func reloadForUpdateAsync() async throws -> Bool { try reloadForUpdate() }
    @MainActor func recoverSpawnFailureAsync() async throws -> Bool { try recoverSpawnFailure() }
    var installationProblem: String? { nil }
    func recoverSpawnFailure() throws -> Bool { false }
    func reloadForUpdate() throws -> Bool { false }
}

public enum CoreServiceStatus: String, Equatable, Sendable, Encodable {
    /// Registered and allowed to run.
    case enabled
    /// Registered, but the person must allow it in Login Items.
    case requiresApproval = "requires_approval"
    case notRegistered = "not_registered"
    /// This build carries no LaunchAgent (`swift run`): a core started by
    /// hand serves it.
    case notFound = "not_found"
}

public struct CoreServiceDiagnostics: Encodable, Sendable {
    public enum InstallState: String, Encodable, Sendable {
        case installed, outsideApplications = "outside_applications", noAgent = "no_agent", unknown
    }
    public enum BackupAgent: String, Encodable, Sendable { case bundled, local, session, unknown }
    public enum Quarantine: String, Encodable, Sendable { case present, absent, unknown }
    public enum SpawnFailure: Int, Encodable, Sendable { case brokenBundleProgram = 78 }

    public let smAppServiceStatus: CoreServiceStatus?
    public let installState: InstallState
    public let inApplications: Bool?
    public let translocated: Bool?
    public let quarantine: Quarantine
    public let backupAgent: BackupAgent
    public let spawnFailure: SpawnFailure?

    public init(smAppServiceStatus: CoreServiceStatus? = nil, installState: InstallState = .unknown,
                inApplications: Bool? = nil, translocated: Bool? = nil, quarantine: Quarantine = .unknown,
                backupAgent: BackupAgent = .unknown, spawnFailure: SpawnFailure? = nil) {
        self.smAppServiceStatus = smAppServiceStatus
        self.installState = installState
        self.inApplications = inApplications
        self.translocated = translocated
        self.quarantine = quarantine
        self.backupAgent = backupAgent
        self.spawnFailure = spawnFailure
    }
}
