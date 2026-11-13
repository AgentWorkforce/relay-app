import Foundation
import ServiceManagement
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

final class LaunchAgentServiceTests: XCTestCase {
    func testStartupOffLoadsOnlyASessionJobAndOnRestoresTheLoginJob() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let session = fixture.home.appendingPathComponent(
            "Library/Application Support/AgentRelay/SessionServices/\(FakeSystem.label).session.plist")

        try fixture.service.setStartsAtLogin(false)
        XCTAssertEqual(fixture.os.bundledStatus, .notRegistered)
        XCTAssertEqual(fixture.service.status, .enabled, "manual opening must still run uploads")
        XCTAssertTrue(FileManager.default.fileExists(atPath: session.path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.fallback.path))
        let events = fixture.os.events
        try fixture.service.setStartsAtLogin(false)
        XCTAssertEqual(fixture.os.events, events, "reapplying the startup choice must not restart uploads")

        try fixture.service.setStartsAtLogin(true)
        XCTAssertEqual(fixture.os.bundledStatus, .enabled)
        XCTAssertEqual(fixture.service.status, .enabled)
        XCTAssertFalse(FileManager.default.fileExists(atPath: session.path))
    }

    func testStartupOffRemovesThePersistentFallbackAndSurvivesRelaunch() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        try fixture.seedOldFallback()
        try fixture.service.setStartsAtLogin(false)
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.fallback.path))
        XCTAssertEqual(fixture.os.bundledStatus, .notRegistered)
        XCTAssertEqual(fixture.service.status, .enabled)
        // Logout discards the explicitly bootstrapped job. Nothing in the
        // LaunchAgents folder remains for launchd to load at next login.
        fixture.os.loadedProgram = nil
        XCTAssertEqual(fixture.service.status, .notRegistered)
        try fixture.service.register()
        XCTAssertEqual(fixture.service.status, .enabled)
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.fallback.path))
        try fixture.service.setStartsAtLogin(true)
        XCTAssertTrue(FileManager.default.fileExists(atPath: fixture.fallback.path))
    }

    func testSessionJobRespectsRevokedBackgroundServiceApproval() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        fixture.os.legacyStatus = .requiresApproval
        try fixture.service.setStartsAtLogin(false)
        XCTAssertEqual(fixture.service.status, .requiresApproval)
        XCTAssertNil(fixture.os.loadedProgram)
        XCTAssertFalse(fixture.os.events.contains("bootstrap"))
    }

    func testFailedSessionRegistrationRestoresThePreviousLoginPolicy() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        fixture.os.bootstrapFailures = 1
        XCTAssertThrowsError(try fixture.service.setStartsAtLogin(false))
        XCTAssertEqual(fixture.os.bundledStatus, .enabled)
        XCTAssertEqual(fixture.service.status, .enabled)
        XCTAssertNil(fixture.os.loadedProgram)
        XCTAssertTrue(fixture.os.events.contains("unregisterBundled"))
    }

    func testMovedFallbackReloadsTheLoadedProgramAndPreservesBundleAssociation() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        fixture.selectFallback()
        fixture.os.loadedProgram = "/old/Applications/Agent Relay.app/Contents/Helpers/Agent Relay Helper"
        XCTAssertEqual(fixture.service.status, .notRegistered)

        try fixture.service.register()

        XCTAssertEqual(fixture.os.loadedProgram, fixture.helper.path)
        XCTAssertEqual(fixture.service.status, .enabled)
        XCTAssertEqual(fixture.os.events, ["bootout", "bootstrap", "unregisterBundled"])
        let plist = try XCTUnwrap(PropertyListSerialization.propertyList(
            from: Data(contentsOf: fixture.fallback), format: nil) as? [String: Any])
        XCTAssertEqual(plist["AssociatedBundleIdentifiers"] as? [String], ["com.agentrelay.desktop"])
        XCTAssertNil(plist["ProcessType"], "Fallback must use the bundled agent's default classification")
        XCTAssertEqual(plist["Program"] as? String, fixture.helper.path)
        XCTAssertEqual(plist["ProgramArguments"] as? [String], ["Agent Relay Helper", "serve"])
        XCTAssertEqual(plist["RunAtLoad"] as? Bool, true)
        XCTAssertEqual(plist["KeepAlive"] as? Bool, true)
        XCTAssertEqual(plist["ThrottleInterval"] as? Int, 10)
        XCTAssertEqual(fixture.appliedPolicyRevision, LaunchAgentService.fallbackPolicyRevision)
        // The marker is the bundle id the core's label is made of.
        XCTAssertEqual(plist[ProductTakeover.markerKey] as? String, AppInfo.bundleIdentifier)
        XCTAssertEqual(ProductTakeover.product(ofLaunchAgentAt: fixture.fallback), AppInfo.bundleIdentifier,
                       "another Agent Relay product recognises this fallback by its marker")
    }

    func testAnUnmarkedBundledJobGivesTheFallbackNoMarker() throws {
        // A development build's bundled LaunchAgent carries no marker, so
        // the fallback written from it carries none either.
        let fixture = try Fixture(marked: false)
        defer { fixture.remove() }
        fixture.selectFallback()

        try fixture.service.register()

        let plist = try XCTUnwrap(PropertyListSerialization.propertyList(
            from: Data(contentsOf: fixture.fallback), format: nil) as? [String: Any])
        XCTAssertEqual(plist["Program"] as? String, fixture.helper.path)
        XCTAssertNil(plist[ProductTakeover.markerKey])
        XCTAssertNil(ProductTakeover.product(ofLaunchAgentAt: fixture.fallback),
                     "no other Agent Relay product recognises an unmarked fallback")
    }

    @MainActor
    func testNormalEnsureServiceMigratesAnExistingFallbackOnceWithoutBinaryIdentity() async throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        try fixture.seedOldFallback()
        XCTAssertNil(fixture.service.binaryIdentity)
        XCTAssertEqual(fixture.service.status, .notRegistered)
        let model = AppModel(client: CoreClient(socketPath: fixture.home.appendingPathComponent("unused.sock").path),
                             service: fixture.service, bundledCoreVersion: nil, unregisterSettle: 0,
                             manageSystemServices: false, defaults: fixture.defaults)
        defer { model.shutdown() }

        await model.ensureService()

        XCTAssertEqual(fixture.os.events, ["bootout", "bootstrap", "unregisterBundled"])
        XCTAssertNil(fixture.os.loadedProcessType)
        XCTAssertEqual(fixture.appliedPolicyRevision, LaunchAgentService.fallbackPolicyRevision)
        XCTAssertEqual(fixture.service.status, .enabled)
        await model.ensureService()
        XCTAssertFalse(try fixture.service.recoverSpawnFailure())
        XCTAssertEqual(fixture.os.events, ["bootout", "bootstrap", "unregisterBundled"])
    }

    func testSameProgramRecoveryReloadsAnOldPolicyOnce() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        try fixture.seedOldFallback()
        fixture.defaults.set(0, forKey: fixture.policyKey)

        XCTAssertTrue(try fixture.service.recoverSpawnFailure())

        XCTAssertEqual(fixture.os.events, ["bootout", "bootstrap", "unregisterBundled"])
        XCTAssertNil(fixture.os.loadedProcessType)
        XCTAssertEqual(fixture.service.status, .enabled)
        XCTAssertFalse(try fixture.service.recoverSpawnFailure())
        XCTAssertEqual(fixture.os.events, ["bootout", "bootstrap", "unregisterBundled"])
    }

    func testFailedPolicyBootstrapDoesNotMistakeRewrittenPlistForAppliedPolicy() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        try fixture.seedOldFallback()
        fixture.os.bootstrapFailures = 1

        XCTAssertThrowsError(try fixture.service.recoverSpawnFailure())

        XCTAssertNil(fixture.appliedPolicyRevision)
        let plist = try XCTUnwrap(PropertyListSerialization.propertyList(
            from: Data(contentsOf: fixture.fallback), format: nil) as? [String: Any])
        XCTAssertNil(plist["ProcessType"], "Writing the new definition alone cannot complete migration")
        XCTAssertEqual(fixture.service.status, .notRegistered)
        XCTAssertEqual(fixture.os.events, ["bootout", "bootstrap"])
        XCTAssertTrue(try fixture.service.recoverSpawnFailure())
        XCTAssertEqual(fixture.os.events, ["bootout", "bootstrap", "bootstrap", "unregisterBundled"])
        XCTAssertEqual(fixture.appliedPolicyRevision, LaunchAgentService.fallbackPolicyRevision)
    }

    func testFailedPolicyBootoutKeepsMigrationPendingAndOldJobIdentifiable() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        try fixture.seedOldFallback()
        fixture.os.bootoutFailures = 1

        XCTAssertThrowsError(try fixture.service.recoverSpawnFailure())

        XCTAssertNil(fixture.appliedPolicyRevision)
        XCTAssertEqual(fixture.os.loadedProgram, fixture.helper.path)
        XCTAssertEqual(fixture.os.loadedProcessType, "Background")
        XCTAssertEqual(fixture.service.status, .notRegistered)
        XCTAssertEqual(fixture.os.events, ["bootout"])
        XCTAssertTrue(try fixture.service.recoverSpawnFailure())
        XCTAssertEqual(fixture.os.events, ["bootout", "bootout", "bootstrap", "unregisterBundled"])
        XCTAssertEqual(fixture.appliedPolicyRevision, LaunchAgentService.fallbackPolicyRevision)
    }

    func testSuccessfulUnregisterClearsAppliedFallbackPolicy() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        fixture.selectFallback()
        try fixture.service.register()
        XCTAssertEqual(fixture.appliedPolicyRevision, LaunchAgentService.fallbackPolicyRevision)
        XCTAssertNil(fixture.os.loadedProcessType, "A new fallback uses the bundled default classification")

        try fixture.service.unregister()

        XCTAssertNil(fixture.appliedPolicyRevision)
        XCTAssertEqual(fixture.service.status, .notRegistered)
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.fallback.path))
    }

    func testExplicitRegistrationWaitsForOldJobToDisappearEvenAtTheSamePath() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        fixture.selectFallback()
        fixture.os.loadedProgram = fixture.helper.path
        fixture.os.lingeringPrintsOnBootout = 2

        try fixture.service.register()

        XCTAssertEqual(fixture.os.events, ["bootout", "bootstrap", "unregisterBundled"])
        XCTAssertEqual(fixture.os.pauses, 2)
        XCTAssertEqual(fixture.os.loadedProgram, fixture.helper.path)
    }

    func testLingeringOldJobFailsInsteadOfBeingMistakenForNewRegistration() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        fixture.selectFallback()
        fixture.os.loadedProgram = fixture.helper.path
        fixture.os.lingeringPrintsOnBootout = 1000

        XCTAssertThrowsError(try fixture.service.register())
        XCTAssertEqual(fixture.os.events, ["bootout"])
        XCTAssertEqual(fixture.os.pauses, 120)
        XCTAssertNil(fixture.appliedPolicyRevision)
        XCTAssertEqual(fixture.service.status, .notRegistered)
    }

    func testFallbackUpdateWaitsThroughLaunchdsFiveSecondExitGrace() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        fixture.selectFallback()
        fixture.os.loadedProgram = fixture.helper.path
        fixture.os.lingeringPrintsOnBootout = 100

        XCTAssertTrue(try fixture.service.reloadForUpdate())

        XCTAssertEqual(fixture.os.pauses, 100)
        XCTAssertEqual(fixture.os.events, ["bootout", "bootstrap", "unregisterBundled"])
        XCTAssertEqual(fixture.service.status, .enabled)
    }

    @MainActor
    func testAsyncFallbackUpdateYieldsWhileTheOldJobRetires() async throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        fixture.selectFallback()
        fixture.os.loadedProgram = fixture.helper.path
        fixture.os.lingeringPrintsOnBootout = 100
        var otherActorWorkRan = false
        let other = Task { @MainActor in otherActorWorkRan = true }

        let reloaded = try await fixture.service.reloadForUpdateAsync()

        XCTAssertTrue(reloaded)
        XCTAssertTrue(otherActorWorkRan, "Retiring launchd must not block the UI actor")
        XCTAssertEqual(fixture.os.pauses, 100)
        XCTAssertEqual(fixture.service.status, .enabled)
        await other.value
    }

    func testFailedBootstrapKeepsBundledJobEligibleForTheNextEarlyRetry() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        fixture.os.bootstrapFailures = 1

        XCTAssertThrowsError(try fixture.service.recoverSpawnFailure())
        XCTAssertEqual(fixture.os.bundledStatus, .enabled)
        XCTAssertTrue(fixture.defaults.bool(forKey: fixture.fallbackKey))
        XCTAssertEqual(fixture.service.status, .notRegistered)
        XCTAssertEqual(fixture.os.events, ["bootstrap"])

        XCTAssertTrue(try fixture.service.recoverSpawnFailure())
        XCTAssertEqual(fixture.os.events, ["bootstrap", "bootstrap", "unregisterBundled"])
        XCTAssertEqual(fixture.os.loadedProgram, fixture.helper.path)
        XCTAssertEqual(fixture.service.status, .enabled)
    }

    func testSelectedFallbackCanRetryBootstrapWithoutAnEnabledBundledJob() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        fixture.selectFallback()
        fixture.os.bundledStatus = .notRegistered
        fixture.os.bootstrapFailures = 1
        XCTAssertThrowsError(try fixture.service.recoverSpawnFailure())
        XCTAssertTrue(try fixture.service.recoverSpawnFailure())
        XCTAssertFalse(try fixture.service.recoverSpawnFailure())
        XCTAssertEqual(fixture.os.events, ["bootstrap", "bootstrap"])
    }

    func testStopAfterFailedBootstrapAlsoUnregistersTheBrokenOriginal() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        fixture.os.bootstrapFailures = 1
        XCTAssertThrowsError(try fixture.service.recoverSpawnFailure())

        try fixture.service.unregister()

        XCTAssertEqual(fixture.os.bundledStatus, .notRegistered)
        XCTAssertNil(fixture.os.loadedProgram)
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.fallback.path))
        XCTAssertEqual(fixture.os.events, ["bootstrap", "unregisterBundled"])
    }

    func testApprovalRequiredIsReportedEvenWithALoadedJobAndNeverBypassed() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        fixture.selectFallback()
        fixture.os.bundledStatus = .notRegistered
        fixture.os.loadedProgram = fixture.helper.path
        fixture.os.legacyStatus = .requiresApproval

        XCTAssertEqual(fixture.service.status, .requiresApproval)
        try fixture.service.register()
        XCTAssertFalse(try fixture.service.recoverSpawnFailure())
        XCTAssertTrue(fixture.os.events.isEmpty)
        XCTAssertNil(fixture.appliedPolicyRevision)
    }

    func testApprovalDiscoveredDuringBootstrapSurfacesWithoutRetriedRegistration() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        fixture.os.approvalDuringBootstrap = true

        XCTAssertTrue(try fixture.service.recoverSpawnFailure())
        XCTAssertEqual(fixture.service.status, .requiresApproval)
        XCTAssertFalse(try fixture.service.recoverSpawnFailure())
        XCTAssertEqual(fixture.os.events, ["bootstrap", "unregisterBundled"])
        XCTAssertNil(fixture.appliedPolicyRevision)
    }

    func testFallbackUpdatePreservesItsPlistAndRetriesTransientBootstrapFailure() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        fixture.selectFallback()
        fixture.os.bundledStatus = .notRegistered
        fixture.os.loadedProgram = fixture.helper.path
        fixture.os.bootstrapFailures = 1
        fixture.os.requirePersistentPlist = true

        XCTAssertTrue(try fixture.service.reloadForUpdate())

        XCTAssertEqual(fixture.os.events, ["bootout", "bootstrap", "bootstrap"])
        XCTAssertEqual(fixture.os.pauses, 1)
        XCTAssertEqual(fixture.service.status, .enabled)
        XCTAssertTrue(FileManager.default.fileExists(atPath: fixture.fallback.path))
        XCTAssertTrue(fixture.defaults.bool(forKey: fixture.fallbackKey))
    }

    func testFallbackUpdateFailureKeepsThePlistForAutomaticRecovery() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        fixture.selectFallback()
        fixture.os.bundledStatus = .notRegistered
        fixture.os.loadedProgram = fixture.helper.path
        fixture.os.bootstrapFailures = 4
        XCTAssertThrowsError(try fixture.service.reloadForUpdate())
        XCTAssertTrue(FileManager.default.fileExists(atPath: fixture.fallback.path))
        XCTAssertEqual(fixture.service.status, .notRegistered)
        XCTAssertTrue(try fixture.service.recoverSpawnFailure())
        XCTAssertEqual(fixture.service.status, .enabled)
    }

    func testFallbackUpdateRespectsRevokedApprovalAndBundledJobsKeepTheirExistingHandoff() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        XCTAssertFalse(try fixture.service.reloadForUpdate())
        fixture.selectFallback()
        fixture.os.loadedProgram = fixture.helper.path
        fixture.os.legacyStatus = .requiresApproval
        XCTAssertTrue(try fixture.service.reloadForUpdate())
        XCTAssertEqual(fixture.service.status, .requiresApproval)
        XCTAssertFalse(fixture.os.events.contains("bootout"))
        XCTAssertFalse(fixture.os.events.contains("bootstrap"))
    }

    private final class Fixture {
        let home: URL
        let helper: URL
        let fallback: URL
        let defaults: UserDefaults
        let suite = "launch-agent-test-\(UUID())"
        let os = FakeSystem()
        let service: LaunchAgentService
        var fallbackKey: String { "\(FakeSystem.label).usesLocalLaunchAgent" }
        var policyKey: String { "\(FakeSystem.label).fallbackPolicyRevision" }
        var appliedPolicyRevision: Int? { defaults.object(forKey: policyKey) as? Int }

        init(marked: Bool = true) throws {
            home = FileManager.default.temporaryDirectory.appendingPathComponent("launch-agent-\(UUID())")
            let bundle = home.appendingPathComponent("Applications/Agent Relay.app")
            helper = bundle.appendingPathComponent("Contents/Helpers/Agent Relay Helper")
            fallback = home.appendingPathComponent("Library/LaunchAgents/\(FakeSystem.label).local.plist")
            defaults = UserDefaults(suiteName: suite)!
            let plistURL = bundle.appendingPathComponent("Contents/Library/LaunchAgents/\(LaunchAgentService.plistName)")
            try FileManager.default.createDirectory(at: helper.deletingLastPathComponent(), withIntermediateDirectories: true)
            try Data("#!/bin/sh\nexit 0\n".utf8).write(to: helper)
            try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: helper.path)
            try FileManager.default.createDirectory(at: plistURL.deletingLastPathComponent(), withIntermediateDirectories: true)
            var plist: [String: Any] = ["BundleProgram": "Contents/Helpers/Agent Relay Helper",
                                        "ProgramArguments": ["Agent Relay Helper", "serve"]]
            if marked { plist[ProductTakeover.markerKey] = AppInfo.bundleIdentifier }
            try PropertyListSerialization.data(fromPropertyList: plist, format: .xml, options: 0).write(to: plistURL)
            os.plistPath = fallback.path
            service = LaunchAgentService(bundleURL: bundle, homeDirectory: home,
                                         bundleIdentifier: "com.agentrelay.desktop", defaults: defaults,
                                         system: os.operations)
        }

        func selectFallback() { defaults.set(true, forKey: fallbackKey) }
        func seedOldFallback() throws {
            selectFallback()
            try FileManager.default.createDirectory(at: fallback.deletingLastPathComponent(), withIntermediateDirectories: true)
            let plist: [String: Any] = ["Program": helper.path, "ProcessType": "Background"]
            try PropertyListSerialization.data(fromPropertyList: plist, format: .xml, options: 0).write(to: fallback)
            os.loadedProgram = helper.path
            os.loadedProcessType = "Background"
        }
        func remove() {
            defaults.removePersistentDomain(forName: suite)
            try? FileManager.default.removeItem(at: home)
        }
    }

    private final class FakeSystem {
        static var label: String { LaunchAgentService.plistName.replacingOccurrences(of: ".plist", with: "") }
        var bundledStatus: SMAppService.Status = .enabled
        var legacyStatus: SMAppService.Status = .enabled
        var loadedProgram: String?
        var loadedProcessType: String?
        var bootoutFailures = 0
        var bootstrapFailures = 0
        var approvalDuringBootstrap = false
        var lingeringPrintsOnBootout = 0
        var remainingPrints: Int?
        var pauses = 0
        var requirePersistentPlist = false
        var events: [String] = []
        var plistPath = ""

        var operations: LaunchAgentService.System {
            .init(bundledStatus: { self.bundledStatus }, registerBundled: { self.bundledStatus = .enabled },
                  unregisterBundled: {
                      self.events.append("unregisterBundled")
                      self.bundledStatus = .notRegistered
                  }, legacyStatus: { _ in self.legacyStatus }, launchctl: { self.run($0) },
                  pause: { _ in self.pauses += 1 },
                  pauseAsync: { _ in self.pauses += 1; await Task.yield() })
        }

        func run(_ arguments: [String]) -> (success: Bool, output: String) {
            switch arguments.first {
            case "print":
                guard arguments[1].hasSuffix(".local") || arguments[1].hasSuffix(".session") else {
                    return (bundledStatus == .enabled, "job state = spawn failed\nlast exit code = 78: EX_CONFIG\nprogram identifier = Contents/Helpers/Agent Relay Helper")
                }
                if let remaining = remainingPrints {
                    if remaining == 0 { loadedProgram = nil; loadedProcessType = nil; remainingPrints = nil }
                    else { remainingPrints = remaining - 1 }
                }
                return (loadedProgram != nil, loadedProgram.map { "job = {\n\tprogram = \($0)\n}" } ?? "not found")
            case "bootout":
                if requirePersistentPlist { XCTAssertTrue(FileManager.default.fileExists(atPath: plistPath)) }
                events.append("bootout")
                if bootoutFailures > 0 { bootoutFailures -= 1; return (false, "temporary bootout failure") }
                remainingPrints = lingeringPrintsOnBootout
                return (true, "")
            case "bootstrap":
                if requirePersistentPlist { XCTAssertTrue(FileManager.default.fileExists(atPath: arguments[2])) }
                events.append("bootstrap")
                if approvalDuringBootstrap { legacyStatus = .requiresApproval; return (false, "approval required") }
                if bootstrapFailures > 0 { bootstrapFailures -= 1; return (false, "temporary bootstrap failure") }
                guard loadedProgram == nil,
                      let data = try? Data(contentsOf: URL(fileURLWithPath: arguments[2])),
                      let plist = try? PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any],
                      let program = plist["Program"] as? String else { return (false, "old job still loaded or invalid plist") }
                loadedProgram = program
                loadedProcessType = plist["ProcessType"] as? String
                return (true, "")
            default:
                return (false, "unexpected command")
            }
        }
    }
}
