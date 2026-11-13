import Foundation

/// Where the relay core runs: a launchd LaunchAgent the app registers
/// through SMAppService or, when macOS loses its bundle path, directly.
/// It runs at login with no window, keeps
/// delivering through app crashes and updates, and launchd restarts it
/// when it exits. Behind a protocol so tests never touch launchd.
public protocol CoreService: AnyObject {
    var status: CoreServiceStatus { get }
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
    @MainActor func setStartsAtLoginAsync(_ enabled: Bool) async throws { try setStartsAtLogin(enabled) }
    @MainActor func registerAsync() async throws { try register() }
    @MainActor func unregisterAsync() async throws { try unregister() }
    @MainActor func reloadForUpdateAsync() async throws -> Bool { try reloadForUpdate() }
    @MainActor func recoverSpawnFailureAsync() async throws -> Bool { try recoverSpawnFailure() }
    var installationProblem: String? { nil }
    func recoverSpawnFailure() throws -> Bool { false }
    func reloadForUpdate() throws -> Bool { false }
}

public enum CoreServiceStatus: Equatable, Sendable {
    /// Registered and allowed to run.
    case enabled
    /// Registered, but the person must allow it in Login Items.
    case requiresApproval
    case notRegistered
    /// This build carries no LaunchAgent (`swift run`): a core started by
    /// hand serves it.
    case notFound
}
