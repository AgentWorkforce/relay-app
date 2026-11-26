import Foundation
import ServiceManagement
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

/// The self-repair ladder, against a launchd model that behaves as an
/// isolated test bundle did on a real Mac (macOS 26.5.1): after the app bundle
/// is replaced, SMAppService still reports enabled while launchd's record keeps
/// the old `parent bundle version` and spawn-fails with exit 78; `register()`
/// on that enabled job is a no-op; unregister then register records the new
/// bundle and the helper runs.
final class CoreRegistrationRepairTests: XCTestCase {
    // MARK: launchctl print parsing

    /// Captured from `launchctl print` of the isolated test job, trimmed.
    static let spawnFailedPrint = """
        gui/501/dev.repro.raprobe.core = {
        \tactive count = 0
        \tpath = (submitted by smd.84657)
        \ttype = Submitted
        \tmanaged_by = com.apple.xpc.ServiceManagement
        \tstate = spawn scheduled
        \tprogram identifier = Contents/Helpers/RAProbe Helper (mode: 2)
        \tparent bundle identifier = dev.repro.raprobe
        \tparent bundle version = 200
        \truns = 3
        \tlast exit code = 78: EX_CONFIG
        \tresource coalition = {
        \t\tID = 189340
        \t\tstate = active
        \t}
        \tspawn type = daemon (3)
        \tjob state = spawn failed
        }
        """

    static func runningPrint(pid: Int, version: String) -> String {
        "gui/501/x = {\n\tstate = running\n\tparent bundle version = \(version)\n\tpid = \(pid)\n\tlast exit code = (never exited)\n\t\tstate = active\n\tjob state = running\n}"
    }

    func testParsesASpawnFailedJobPinnedToTheReplacedBundle() {
        let job = LaunchdJob.parse(success: true, output: Self.spawnFailedPrint)
        XCTAssertEqual(job, LaunchdJob(loaded: true, pid: nil, jobState: "spawn failed", lastExitCode: 78,
                                       parentBundleVersion: "200"))
        XCTAssertFalse(job.isRunning)
        XCTAssertTrue(job.spawnFailed)
    }

    func testParsesARunningJobAndAMissingOne() {
        let job = LaunchdJob.parse(success: true, output: Self.runningPrint(pid: 94201, version: "150"))
        XCTAssertEqual(job.pid, 94201)
        XCTAssertEqual(job.jobState, "running")
        XCTAssertNil(job.lastExitCode, "(never exited) is not an exit code")
        XCTAssertTrue(job.isRunning)
        XCTAssertEqual(LaunchdJob.parse(success: false, output: "Could not find service"), .missing)
    }

    // MARK: the ladder

    private func observe(fallback: Bool = false, startsAtLogin: Bool = true, registration: CoreServiceStatus = .enabled,
                         bundled: LaunchdJob = .missing, fallbackJob: LaunchdJob = .missing,
                         installationProblem: Bool = false) -> CoreRegistrationRepair.Observation {
        .init(usesFallback: fallback, startsAtLogin: startsAtLogin, installationProblem: installationProblem,
              registration: registration, bundledJob: bundled, fallbackJob: fallbackJob, appBundleVersion: "150")
    }

    private let spawnFailedOld = LaunchdJob(loaded: true, jobState: "spawn failed", lastExitCode: 78, parentBundleVersion: "200")
    private let running = LaunchdJob(loaded: true, pid: 7, jobState: "running", parentBundleVersion: "150")

    func testEnabledJobPinnedToAReplacedBundleIsResetThenFallsBack() {
        let o = observe(bundled: spawnFailedOld)
        XCTAssertEqual(CoreRegistrationRepair.next(o, step: 0), .resetRegistration)
        XCTAssertEqual(CoreRegistrationRepair.next(o, step: 1), .resetRegistration)
        XCTAssertEqual(CoreRegistrationRepair.next(o, step: 2), .useFallback)
        XCTAssertEqual(CoreRegistrationRepair.next(o, step: CoreRegistrationRepair.stepLimit), .exhausted)
    }

    func testEnabledButNeverLoadedIsRegisteredFirst() {
        XCTAssertEqual(CoreRegistrationRepair.next(observe(bundled: .missing), step: 0), .register)
        XCTAssertEqual(CoreRegistrationRepair.next(observe(bundled: .missing), step: 1), .resetRegistration)
    }

    func testNotRegisteredAfterRegisterEscalatesToResetAndFallback() {
        let o = observe(registration: .notRegistered)
        XCTAssertEqual(CoreRegistrationRepair.next(o, step: 0), .register)
        XCTAssertEqual(CoreRegistrationRepair.next(o, step: 1), .resetRegistration)
        XCTAssertEqual(CoreRegistrationRepair.next(o, step: 2), .useFallback)
    }

    /// A reinstalled bundle with another signature at the same path: SMAppService
    /// says not found while launchd keeps the old record spawn-failing.
    func testResignedReinstallRegistersThenResets() {
        let o = observe(registration: .notRegistered, bundled: spawnFailedOld)
        XCTAssertEqual(CoreRegistrationRepair.next(o, step: 0), .register)
        XCTAssertEqual(CoreRegistrationRepair.next(o, step: 1), .resetRegistration)
    }

    func testRunningCurrentJobNeedsNothing() {
        XCTAssertEqual(CoreRegistrationRepair.next(observe(bundled: running), step: 0), .none)
    }

    func testRunningJobOfAReplacedBundleIsStillReset() {
        let old = LaunchdJob(loaded: true, pid: 9, jobState: "running", parentBundleVersion: "200")
        XCTAssertEqual(CoreRegistrationRepair.next(observe(bundled: old), step: 0), .resetRegistration)
    }

    func testHealthyFallbackIsPreserved() {
        let o = observe(fallback: true, registration: .notRegistered, fallbackJob: running)
        for step in 0..<CoreRegistrationRepair.stepLimit {
            XCTAssertEqual(CoreRegistrationRepair.next(o, step: step), .none)
        }
    }

    func testStaleFallbackFlagWithNoJobRetriesSMAppServiceBeforeFallingBackAgain() {
        let o = observe(fallback: true, registration: .notRegistered, fallbackJob: .missing)
        XCTAssertEqual(CoreRegistrationRepair.next(o, step: 0), .leaveFallback)
        XCTAssertEqual(CoreRegistrationRepair.next(o, step: CoreRegistrationRepair.stepLimit - 1), .useFallback)
    }

    func testApprovalAndInstallationStatesAreNeverOverridden() {
        for step in 0...CoreRegistrationRepair.stepLimit {
            XCTAssertEqual(CoreRegistrationRepair.next(observe(registration: .requiresApproval, bundled: spawnFailedOld),
                                                       step: step), .waitForApproval)
            XCTAssertEqual(CoreRegistrationRepair.next(observe(fallback: true, registration: .requiresApproval), step: step),
                           .waitForApproval)
            XCTAssertEqual(CoreRegistrationRepair.next(observe(installationProblem: true), step: step), .moveToApplications)
        }
    }

    /// 11.19's default: a session job launchd keeps loaded but never runs.
    func testSessionJobThatNeverRunsIsReloadedThenMovedToTheLoginJob() {
        let stuck = LaunchdJob(loaded: true, jobState: "spawn failed", lastExitCode: 78)
        let o = observe(fallback: true, startsAtLogin: false, registration: .enabled, fallbackJob: stuck)
        XCTAssertEqual(CoreRegistrationRepair.next(o, step: 0), .reloadSessionJob)
        XCTAssertEqual(CoreRegistrationRepair.next(o, step: 1), .runCoreDirectly)
        XCTAssertEqual(CoreRegistrationRepair.next(o, step: 2), .runCoreDirectly, "a direct core that exited is started again")
        var started = o
        started.directCoreRunning = true
        XCTAssertEqual(CoreRegistrationRepair.next(started, step: 2), .none, "one direct core at a time")
        XCTAssertEqual(CoreRegistrationRepair.next(o, step: CoreRegistrationRepair.stepLimit), .exhausted)
    }

    func testSessionOnlyStartupReloadsItsSessionJob() {
        let o = observe(fallback: true, startsAtLogin: false, registration: .notRegistered)
        XCTAssertEqual(CoreRegistrationRepair.next(o, step: 0), .reloadSessionJob)
        XCTAssertEqual(CoreRegistrationRepair.next(observe(fallback: true, startsAtLogin: false, fallbackJob: running),
                                                   step: 0), .none)
    }

    // MARK: bounded OS calls

    func testBoundedCallTimesOutAHungCall() async {
        let started = Date()
        let hang = DispatchSemaphore(value: 0)
        do {
            _ = try await BoundedCall.run("register", timeout: 0.2) { hang.wait(); return 1 }
            XCTFail("a hung call must time out")
        } catch let error as BoundedCall.TimedOut {
            XCTAssertEqual(error.operation, "register")
        } catch { XCTFail("unexpected \(error)") }
        XCTAssertLessThan(Date().timeIntervalSince(started), 2)
        hang.signal()
    }

    func testBoundedCallReturnsAndRethrows() async throws {
        let value = try await BoundedCall.run("status", timeout: 2) { 42 }
        XCTAssertEqual(value, 42)
        struct Boom: Error {}
        let failing: () throws -> Int = { throw Boom() }
        do { _ = try await BoundedCall.run("x", timeout: 2, failing); XCTFail("must rethrow") } catch is Boom {}
    }

    // MARK: sequences through LaunchAgentService

    @MainActor
    func testBundleReplacementRepairsWithResetAndTheNewBundleRuns() async throws {
        let mac = try SimulatedMac()
        defer { mac.remove() }
        mac.launchd.register(version: "200")
        mac.launchd.replaceBundle()           // the app was deleted and reinstalled
        XCTAssertEqual(mac.service.status, .enabled, "SMAppService still says enabled")

        let action = try await mac.service.selfRepairAsync(step: 0)

        XCTAssertEqual(action, .resetRegistration)
        XCTAssertEqual(mac.launchd.bundled, .running(version: "150"))
        XCTAssertEqual(mac.launchd.calls.filter { $0 != "status" }, ["unregister", "register"])
    }

    @MainActor
    func testRegisterIsANoOpForAnEnabledJobSoOnlyTheResetHelps() async throws {
        let mac = try SimulatedMac()
        defer { mac.remove() }
        mac.launchd.register(version: "200")
        mac.launchd.replaceBundle()
        try mac.service.register()            // what earlier builds retried
        XCTAssertEqual(mac.launchd.bundled, .spawnFailed(version: "200"))
    }

    @MainActor
    func testEnabledWithNoLoadedJobIsRegistered() async throws {
        let mac = try SimulatedMac()
        defer { mac.remove() }
        mac.launchd.register(version: "150")
        mac.launchd.bundled = .missing        // booted out behind SMAppService's back
        let action1 = try await mac.service.selfRepairAsync(step: 0)
        XCTAssertEqual(action1, .register)
        XCTAssertEqual(mac.launchd.bundled, .running(version: "150"))
    }

    @MainActor
    func testRegisterThatLeavesNotRegisteredFallsBackToTheAbsolutePathAgent() async throws {
        let mac = try SimulatedMac()
        defer { mac.remove() }
        mac.launchd.registerHasNoEffect = true // Adam's second install
        var actions: [CoreRegistrationRepair.Action] = []
        for step in 0..<3 { actions.append(try await mac.service.selfRepairAsync(step: step)) }

        XCTAssertEqual(actions, [.register, .resetRegistration, .useFallback])
        XCTAssertEqual(mac.launchd.fallbackProgram, mac.helper.path)
        XCTAssertTrue(mac.defaults.bool(forKey: mac.fallbackKey))
        XCTAssertEqual(mac.service.status, .enabled)
        let action2 = try await mac.service.selfRepairAsync(step: 3)
        XCTAssertEqual(action2, .none, "the running fallback is kept")
    }

    @MainActor
    func testStaleFallbackFlagFromAnEarlierInstallIsDroppedForSMAppService() async throws {
        let mac = try SimulatedMac()
        defer { mac.remove() }
        // Deleting the app kept its defaults; the fallback job is gone.
        mac.defaults.set(true, forKey: mac.fallbackKey)
        mac.defaults.set(1, forKey: mac.policyKey)
        try FileManager.default.createDirectory(at: mac.fallback.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data("stale".utf8).write(to: mac.fallback)
        XCTAssertEqual(mac.service.status, .notRegistered)

        let action3 = try await mac.service.selfRepairAsync(step: 0)
        XCTAssertEqual(action3, .leaveFallback)

        XCTAssertFalse(mac.defaults.bool(forKey: mac.fallbackKey))
        XCTAssertNil(mac.defaults.object(forKey: mac.policyKey))
        XCTAssertFalse(FileManager.default.fileExists(atPath: mac.fallback.path))
        XCTAssertEqual(mac.launchd.bundled, .running(version: "150"))
        XCTAssertEqual(mac.service.status, .enabled)
    }

    @MainActor
    func testHealthyFallbackIsNotTouched() async throws {
        let mac = try SimulatedMac()
        defer { mac.remove() }
        mac.launchd.registerHasNoEffect = true
        for step in 0..<3 { _ = try await mac.service.selfRepairAsync(step: step) }
        let calls = mac.launchd.calls.count
        let action4 = try await mac.service.selfRepairAsync(step: 0)
        XCTAssertEqual(action4, .none)
        XCTAssertEqual(mac.launchd.calls.count, calls + 1, "only a status read")
    }

    /// The session branch reads the session job (`<label>.session`), so a
    /// healthy session core is left alone.
    @MainActor
    func testHealthySessionJobIsLeftAlone() async throws {
        let mac = try SimulatedMac()
        defer { mac.remove() }
        mac.defaults.set(false, forKey: mac.startsAtLoginKey)
        try await mac.service.registerAsync()
        XCTAssertNotNil(mac.launchd.fallbackProgram)
        let before = mac.launchd.calls
        let action = try await mac.service.selfRepairAsync(step: 0)
        XCTAssertEqual(action, .none)
        XCTAssertEqual(Array(mac.launchd.calls.dropFirst(before.count)), ["status"])
        XCTAssertTrue(mac.launchd.printed.contains { $0.hasSuffix(".session") })
    }

    /// 11.19's default: Start at Login off, and launchd keeps the session job
    /// loaded ("Enabled") without ever running it. The app reloads it, then
    /// starts the core itself, and Start at Login stays off with no login job.
    @MainActor
    func testStuckSessionJobRunsTheCoreDirectlyAndKeepsStartAtLoginOff() async throws {
        let mac = try SimulatedMac()
        defer { mac.remove() }
        mac.defaults.set(false, forKey: mac.startsAtLoginKey)
        mac.launchd.fallbackNeverRuns = true
        try await mac.service.registerAsync()
        XCTAssertEqual(mac.service.status, .enabled, "what 11.19's About window showed")

        let first = try await mac.service.selfRepairAsync(step: 0)
        let second = try await mac.service.selfRepairAsync(step: 1)
        let third = try await mac.service.selfRepairAsync(step: 2)

        XCTAssertEqual([first, second, third], [.reloadSessionJob, .runCoreDirectly, .none])
        XCTAssertEqual(mac.launchd.directStarts.count, 1, "exactly one core")
        XCTAssertEqual(mac.launchd.directStarts.first?.program, mac.helper.path)
        XCTAssertEqual(mac.launchd.directStarts.first?.arguments, ["serve"])
        XCTAssertEqual(mac.defaults.object(forKey: mac.startsAtLoginKey) as? Bool, false, "the saved setting stays off")
        XCTAssertEqual(mac.launchd.smStatus, .notRegistered, "no SMAppService login job")
        XCTAssertEqual(mac.launchd.bundled, .missing)
        XCTAssertFalse(mac.defaults.bool(forKey: mac.fallbackKey))
        XCTAssertFalse(FileManager.default.fileExists(atPath: mac.fallback.path), "no ~/Library/LaunchAgents job")
        XCTAssertNil(mac.launchd.fallbackProgram, "the stuck session job was booted out")
        XCTAssertFalse(mac.launchd.calls.contains("register"))
    }

    /// The direct core runs only the helper inside this app bundle, with the
    /// arguments the bundled agent gives, never a path the plist escapes to.
    @MainActor
    func testDirectCoreRefusesAProgramOutsideTheBundle() async throws {
        let mac = try SimulatedMac()
        defer { mac.remove() }
        mac.defaults.set(false, forKey: mac.startsAtLoginKey)
        mac.launchd.fallbackNeverRuns = true
        try await mac.service.registerAsync()
        _ = try await mac.service.selfRepairAsync(step: 0)
        try PropertyListSerialization.data(fromPropertyList: ["BundleProgram": "Contents/Helpers/../../../../bin/sh",
                                                              "ProgramArguments": ["sh", "-c", "true"]],
                                           format: .xml, options: 0).write(to: mac.agentPlist)
        do { _ = try await mac.service.selfRepairAsync(step: 1); XCTFail("must refuse") } catch {}
        XCTAssertTrue(mac.launchd.directStarts.isEmpty)
    }

    @MainActor
    func testDirectCoreRefusesAHelperSymlinkedOutsideTheBundle() async throws {
        let mac = try SimulatedMac()
        defer { mac.remove() }
        mac.defaults.set(false, forKey: mac.startsAtLoginKey)
        mac.launchd.fallbackNeverRuns = true
        try await mac.service.registerAsync()
        _ = try await mac.service.selfRepairAsync(step: 0)
        let outside = mac.home.appendingPathComponent("outside-helper")
        try Data("#!/bin/sh\nexit 0\n".utf8).write(to: outside)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: outside.path)
        try FileManager.default.removeItem(at: mac.helper)
        try FileManager.default.createSymbolicLink(at: mac.helper, withDestinationURL: outside)
        do { _ = try await mac.service.selfRepairAsync(step: 1); XCTFail("must refuse") } catch {}
        XCTAssertTrue(mac.launchd.directStarts.isEmpty)
    }

    @MainActor
    func testDirectCoreAcceptsAHelperLinkThatStaysInsideTheBundle() async throws {
        let mac = try SimulatedMac()
        defer { mac.remove() }
        mac.defaults.set(false, forKey: mac.startsAtLoginKey)
        mac.launchd.fallbackNeverRuns = true
        try await mac.service.registerAsync()
        _ = try await mac.service.selfRepairAsync(step: 0)
        let inside = mac.helper.deletingLastPathComponent().appendingPathComponent("real-helper")
        try FileManager.default.moveItem(at: mac.helper, to: inside)
        try FileManager.default.createSymbolicLink(at: mac.helper, withDestinationURL: inside)
        let action = try await mac.service.selfRepairAsync(step: 1)
        XCTAssertEqual(action, .runCoreDirectly)
        XCTAssertEqual(mac.launchd.directStarts.count, 1)
    }

    @MainActor
    func testStoppingTheServiceStopsADirectCore() async throws {
        let mac = try SimulatedMac()
        defer { mac.remove() }
        mac.defaults.set(false, forKey: mac.startsAtLoginKey)
        mac.launchd.fallbackNeverRuns = true
        try await mac.service.registerAsync()
        _ = try await mac.service.selfRepairAsync(step: 0)
        _ = try await mac.service.selfRepairAsync(step: 1)
        XCTAssertTrue(mac.launchd.directCoreRunning)
        try await mac.service.unregisterAsync()
        XCTAssertFalse(mac.launchd.directCoreRunning)
        XCTAssertEqual(mac.defaults.object(forKey: mac.startsAtLoginKey) as? Bool, false)
    }

    @MainActor
    func testApprovalRequiredIsLeftToThePerson() async throws {
        let mac = try SimulatedMac()
        defer { mac.remove() }
        mac.launchd.smStatus = .requiresApproval
        let action5 = try await mac.service.selfRepairAsync(step: 0)
        XCTAssertEqual(action5, .waitForApproval)
        XCTAssertEqual(mac.launchd.calls, ["status"])
    }

    @MainActor
    func testAHungSMAppServiceCallEndsTheRepair() async throws {
        let mac = try SimulatedMac(osCallTimeout: 0.2)
        defer { mac.remove() }
        mac.launchd.register(version: "200")
        mac.launchd.replaceBundle()
        let hang = DispatchSemaphore(value: 0)
        mac.launchd.hangUnregister = hang
        let started = Date()
        do { _ = try await mac.service.selfRepairAsync(step: 0); XCTFail("must time out") }
        catch is BoundedCall.TimedOut {}
        XCTAssertLessThan(Date().timeIntervalSince(started), 3)
        hang.signal()
    }
}

extension CoreRegistrationRepairTests {
    /// A timed-out unregister may still be running inside macOS. Until it
    /// returns, no other change starts, so it cannot undo a later recovery;
    /// once it has, the repair goes on and the core runs.
    @MainActor
    func testALateUnregisterCannotUndoALaterRecovery() async throws {
        let mac = try SimulatedMac(osCallTimeout: 0.2)
        defer { mac.remove() }
        mac.launchd.register(version: "200")
        mac.launchd.replaceBundle()
        let hang = DispatchSemaphore(value: 0)
        mac.launchd.hangUnregister = hang
        do { _ = try await mac.service.selfRepairAsync(step: 0); XCTFail("must time out") }
        catch is BoundedCall.TimedOut {}
        mac.launchd.hangUnregister = nil

        // While the late call is in flight, every change is refused.
        do { _ = try await mac.service.selfRepairAsync(step: 1); XCTFail("must refuse while busy") }
        catch { XCTAssertEqual((error as NSError).code, LaunchAgentService.MutationGate.busyCode) }
        XCTAssertThrowsError(try mac.service.register())
        XCTAssertFalse(mac.launchd.calls.contains("register"), "nothing registered under the late unregister")

        hang.signal()                        // macOS finally returns the old call
        let returned = await eventually(2) { mac.launchd.calls.contains("unregister") }
        XCTAssertTrue(returned)
        try await Task.sleep(nanoseconds: 50_000_000)

        let action = try await mac.service.selfRepairAsync(step: 1)
        XCTAssertEqual(action, .resetRegistration)
        XCTAssertEqual(mac.launchd.bundled, .running(version: "150"))
        let changes = mac.launchd.calls.filter { $0 != "status" }
        XCTAssertEqual(changes.last, "register", "the late unregister came before, never after, the recovery")
    }

    /// Approval withdrawn between reading the state and changing it: the rung
    /// changes nothing and leaves the approval to the person.
    @MainActor
    func testApprovalIsReadAgainBeforeAnyChange() async throws {
        let mac = try SimulatedMac()
        defer { mac.remove() }
        mac.launchd.register(version: "200")
        mac.launchd.replaceBundle()
        mac.launchd.approvalWithdrawnAfterReads = 1
        let action = try await mac.service.selfRepairAsync(step: 0)
        XCTAssertEqual(action, .waitForApproval)
        XCTAssertFalse(mac.launchd.calls.contains("unregister"))
        XCTAssertFalse(mac.launchd.calls.contains("register"))
    }

    @MainActor
    func testAJobAlreadyGoneIsNotADenial() async throws {
        let mac = try SimulatedMac()
        defer { mac.remove() }
        mac.launchd.register(version: "200")
        mac.launchd.replaceBundle()
        mac.launchd.unregisterError = NSError(domain: "SMAppServiceErrorDomain", code: Int(kSMErrorJobNotFound))
        let action = try await mac.service.selfRepairAsync(step: 0)
        XCTAssertEqual(action, .resetRegistration)
        XCTAssertTrue(mac.launchd.calls.contains("register"), "already gone: register goes on")
    }

    @MainActor
    func testADeniedUnregisterStopsTheRungWithoutRegistering() async throws {
        let mac = try SimulatedMac()
        defer { mac.remove() }
        mac.launchd.register(version: "200")
        mac.launchd.replaceBundle()
        mac.launchd.unregisterError = NSError(domain: "SMAppServiceErrorDomain", code: Int(kSMErrorLaunchDeniedByUser))
        do { _ = try await mac.service.selfRepairAsync(step: 0); XCTFail("a denial must not be read as done") } catch {
            XCTAssertEqual((error as NSError).code, Int(kSMErrorLaunchDeniedByUser))
        }
        XCTAssertFalse(mac.launchd.calls.contains("register"))
    }

    /// Several calls, each well inside its own timeout, still end the rung
    /// at its shared budget.
    @MainActor
    func testSlowCallsEndTheRungAtItsSharedBudget() async throws {
        let mac = try SimulatedMac(osCallTimeout: 5, repairBudget: 0.4)
        defer { mac.remove() }
        mac.launchd.register(version: "200")
        mac.launchd.replaceBundle()
        mac.launchd.callDelay = 0.15
        let started = Date()
        do { _ = try await mac.service.selfRepairAsync(step: 0); XCTFail("must run out of budget") }
        catch is BoundedCall.TimedOut {}
        XCTAssertLessThan(Date().timeIntervalSince(started), 1.0)
    }
}

extension CoreRegistrationRepairTests {
    @MainActor
    private func model(for mac: SimulatedMac) -> AppModel {
        AppModel(client: CoreClient(socketPath: "/tmp/stop-race-\(UUID().uuidString.prefix(8)).sock"),
                 service: mac.service, bundledCoreVersion: "1.0.0", unregisterSettle: 0,
                 startupTimeout: 60_000_000_000, manageSystemServices: false, defaults: mac.defaults)
    }

    /// Stop service while the direct rung waits on launchd, just before it
    /// would start the core: once launchd answers, the rung starts nothing.
    @MainActor
    func testStopDuringADirectRungStartsNoCore() async throws {
        let mac = try SimulatedMac()
        defer { mac.remove() }
        mac.defaults.set(false, forKey: mac.startsAtLoginKey)
        mac.launchd.fallbackNeverRuns = true
        try await mac.service.registerAsync()
        let model = model(for: mac)
        _ = await model.selfRepairCore()                 // step 0: reloads the session job
        // Step 1 reads the session job for its observation, then again to
        // boot it out before starting the core: hold that second read.
        mac.launchd.suspendSessionRead(after: 1)
        let rung = Task { await model.selfRepairCore() }
        let held = await eventually(2) { mac.launchd.isSuspended }
        XCTAssertTrue(held)

        model.stopBackgroundService()
        let changesAtStop = mac.launchd.calls.filter { $0 != "status" }.count
        mac.launchd.releaseSuspension()
        let submitted = await rung.value

        XCTAssertFalse(submitted)
        XCTAssertTrue(mac.launchd.directStarts.isEmpty, "no core started after Stop")
        XCTAssertFalse(mac.launchd.directCoreRunning)
        XCTAssertNil(mac.launchd.fallbackProgram, "no session job left loaded")
        XCTAssertEqual(mac.launchd.calls.filter { $0 != "status" }.count, changesAtStop, "no change after Stop")
        XCTAssertEqual(mac.launchd.smStatus, .notRegistered)
    }

    /// Stop service while a repair's registration is still in flight inside
    /// macOS: the stop waits for it and unregisters after it, and the
    /// resumed rung changes nothing more.
    @MainActor
    func testStopAfterAnInFlightRegistrationLeavesNothingRegistered() async throws {
        let mac = try SimulatedMac()
        defer { mac.remove() }
        mac.launchd.register(version: "200")
        mac.launchd.replaceBundle()
        let hang = DispatchSemaphore(value: 0)
        mac.launchd.hangRegister = hang
        let model = model(for: mac)
        let rung = Task { await model.selfRepairCore() }  // step 0: reset, its register hangs
        let inFlight = await eventually(2) { mac.launchd.registerStarted }
        XCTAssertTrue(inFlight)

        DispatchQueue.global().asyncAfter(deadline: .now() + 0.3) { hang.signal() }
        model.stopBackgroundService()
        _ = await rung.value
        try await Task.sleep(nanoseconds: 100_000_000)

        XCTAssertEqual(mac.launchd.smStatus, .notRegistered)
        XCTAssertEqual(mac.launchd.bundled, .missing)
        XCTAssertEqual(mac.launchd.calls.filter { $0 != "status" }.last, "unregister", "the stop came last")
        XCTAssertTrue(mac.launchd.directStarts.isEmpty)
    }

    /// The registration is still inside macOS after Stop's whole wait. Stop
    /// returns without blocking, keeps its intent, and the stop runs as soon
    /// as the registration returns: the service ends unregistered.
    @MainActor
    func testStopOutlastedByAnInFlightRegistrationStillEndsUnregistered() async throws {
        let mac = try SimulatedMac(osCallTimeout: 0.2)
        defer { mac.remove() }
        mac.launchd.register(version: "200")
        mac.launchd.replaceBundle()
        let hang = DispatchSemaphore(value: 0)
        mac.launchd.hangRegister = hang
        let model = model(for: mac)
        let rung = Task { await model.selfRepairCore() }
        let inFlight = await eventually(2) { mac.launchd.registerStarted }
        XCTAssertTrue(inFlight)

        let started = Date()
        model.stopBackgroundService()
        XCTAssertLessThan(Date().timeIntervalSince(started), 1, "Stop must not block on macOS")
        XCTAssertTrue(mac.service.hasPendingStop)
        _ = await rung.value

        hang.signal()                                     // only now does macOS return the registration
        let stopped = await eventually(2) { mac.launchd.smStatus == .notRegistered && !mac.service.hasPendingStop }

        XCTAssertTrue(stopped)
        XCTAssertEqual(mac.launchd.bundled, .missing)
        let changes = mac.launchd.calls.filter { $0 != "status" }
        XCTAssertEqual(Array(changes.suffix(2)), ["register", "unregister"], "the late registration, then the stop")
    }

    /// The ordering control: an explicit start after Stop replaces the stop
    /// still waiting, so the late registration is not undone.
    @MainActor
    func testANewerStartReplacesAStopStillWaiting() async throws {
        let mac = try SimulatedMac(osCallTimeout: 0.2)
        defer { mac.remove() }
        mac.launchd.register(version: "200")
        mac.launchd.replaceBundle()
        let hang = DispatchSemaphore(value: 0)
        mac.launchd.hangRegister = hang
        let model = model(for: mac)
        let rung = Task { await model.selfRepairCore() }
        let inFlight = await eventually(2) { mac.launchd.registerStarted }
        XCTAssertTrue(inFlight)
        model.stopBackgroundService()
        XCTAssertTrue(mac.service.hasPendingStop)
        _ = await rung.value

        XCTAssertThrowsError(try mac.service.register(), "still busy, but the start intent wins")
        XCTAssertFalse(mac.service.hasPendingStop)
        hang.signal()
        let registered = await eventually(2) { mac.launchd.calls.contains("register") }
        try await Task.sleep(nanoseconds: 200_000_000)

        XCTAssertTrue(registered)
        XCTAssertEqual(mac.launchd.smStatus, .enabled)
        XCTAssertEqual(mac.launchd.calls.filter { $0 != "status" }.last, "register", "no stop after the newer start")
    }

    /// A launchd read that never answers ends at the rung's shared deadline.
    @MainActor
    func testTheRungDeadlineCoversLaunchctlReads() async throws {
        let mac = try SimulatedMac(osCallTimeout: 5, repairBudget: 0.3)
        defer { mac.remove() }
        mac.defaults.set(false, forKey: mac.startsAtLoginKey)
        mac.launchd.fallbackNeverRuns = true
        try await mac.service.registerAsync()
        mac.launchd.suspendSessionRead(after: 0)          // the observation's read hangs
        let started = Date()
        do { _ = try await mac.service.selfRepairAsync(step: 0); XCTFail("must time out") }
        catch is BoundedCall.TimedOut {}
        XCTAssertLessThan(Date().timeIntervalSince(started), 1.5)
        mac.launchd.releaseSuspension()
    }
}

/// A launchd and SMAppService that behave as the real ones did.
private final class SimulatedLaunchd {
    enum Job: Equatable { case missing, running(version: String), spawnFailed(version: String) }
    var smStatus: SMAppService.Status = .notRegistered
    var bundled: Job = .missing
    var fallbackProgram: String?
    var currentVersion = "150"
    var registerHasNoEffect = false
    var fallbackNeverRuns = false
    var hangUnregister: DispatchSemaphore?
    var unregisterError: Error?
    var hangRegister: DispatchSemaphore?
    private(set) var registerStarted = false
    /// Suspends the Nth session-job read through the async launchctl, so a
    /// test can act while a rung waits on launchd.
    private var sessionReadsBeforeSuspend: Int?
    private var suspended: CheckedContinuation<Void, Never>?
    var isSuspended: Bool { suspended != nil }

    func suspendSessionRead(after reads: Int) { sessionReadsBeforeSuspend = reads }
    func releaseSuspension() { let held = suspended; suspended = nil; held?.resume() }

    @MainActor func launchctlAsync(_ arguments: [String]) async -> (success: Bool, output: String) {
        if arguments.first == "print", arguments[1].hasSuffix(".session"), let remaining = sessionReadsBeforeSuspend {
            if remaining == 0 {
                sessionReadsBeforeSuspend = nil
                await withCheckedContinuation { suspended = $0 }
            } else {
                sessionReadsBeforeSuspend = remaining - 1
            }
        }
        return launchctl(arguments)
    }
    /// Every status, register and unregister call takes this long.
    var callDelay: TimeInterval = 0
    /// After this many status reads, the person has withdrawn approval.
    var approvalWithdrawnAfterReads: Int?
    private var statusReads = 0
    var calls: [String] = []
    var printed: [String] = []
    var directStarts: [(program: String, arguments: [String])] = []
    var directCoreRunning = false
    private let lock = NSLock()

    func record(_ call: String) { lock.lock(); calls.append(call); lock.unlock() }

    func register(version: String) { smStatus = .enabled; bundled = .running(version: version) }

    /// The old helper exits; launchd's record still names the old bundle.
    func replaceBundle() {
        if case .running(let version) = bundled { bundled = .spawnFailed(version: version) }
    }

    var system: LaunchAgentService.System {
        .init(bundledStatus: {
                  self.record("status")
                  if self.callDelay > 0 { Thread.sleep(forTimeInterval: self.callDelay) }
                  self.statusReads += 1
                  if let after = self.approvalWithdrawnAfterReads, self.statusReads > after { self.smStatus = .requiresApproval }
                  return self.smStatus
              },
              registerBundled: {
                  self.registerStarted = true
                  self.hangRegister?.wait()
                  if self.callDelay > 0 { Thread.sleep(forTimeInterval: self.callDelay) }
                  self.record("register")
                  guard !self.registerHasNoEffect else { return }
                  // A no-op for a job macOS already calls enabled and launchd
                  // has loaded; an enabled job launchd lost is loaded again.
                  guard self.smStatus != .enabled || self.bundled == .missing else { return }
                  self.register(version: self.currentVersion)
              },
              unregisterBundled: {
                  self.hangUnregister?.wait()
                  if self.callDelay > 0 { Thread.sleep(forTimeInterval: self.callDelay) }
                  self.record("unregister")
                  if let error = self.unregisterError { throw error }
                  self.smStatus = .notRegistered
                  self.bundled = .missing
              },
              legacyStatus: { _ in .enabled },
              launchctl: { self.launchctl($0) },
              pause: { _ in },
              startCore: { program, arguments in
                  self.directStarts.append((program.path, arguments))
                  self.directCoreRunning = true
                  return .init(isRunning: { self.directCoreRunning }, stop: { self.directCoreRunning = false })
              },
              pauseAsync: { _ in await Task.yield() })
    }

    func launchctl(_ arguments: [String]) -> (success: Bool, output: String) {
        switch arguments.first {
        case "print":
            printed.append(arguments[1])
            if arguments[1].hasSuffix(".local") || arguments[1].hasSuffix(".session") {
                guard let program = fallbackProgram else { return (false, "Could not find service") }
                if fallbackNeverRuns {
                    return (true, "x = {\n\tprogram = \(program)\n\tlast exit code = 78: EX_CONFIG\n\tjob state = spawn failed\n}")
                }
                return (true, "x = {\n\tprogram = \(program)\n\tpid = 11\n\tjob state = running\n}")
            }
            switch bundled {
            case .missing: return (false, "Could not find service")
            case .running(let version): return (true, CoreRegistrationRepairTests.runningPrint(pid: 10, version: version))
            case .spawnFailed(let version):
                return (true, CoreRegistrationRepairTests.spawnFailedPrint
                    .replacingOccurrences(of: "parent bundle version = 200", with: "parent bundle version = \(version)"))
            }
        case "bootstrap":
            guard let data = try? Data(contentsOf: URL(fileURLWithPath: arguments[2])),
                  let plist = try? PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any]
            else { return (false, "invalid plist") }
            fallbackProgram = plist["Program"] as? String
            return (true, "")
        case "bootout":
            fallbackProgram = nil
            return (true, "")
        default:
            return (false, "unexpected")
        }
    }
}

private final class SimulatedMac {
    let home: URL
    let helper: URL
    let fallback: URL
    let suite = "core-repair-test-\(UUID())"
    let defaults: UserDefaults
    let launchd = SimulatedLaunchd()
    let service: LaunchAgentService
    var agentPlist: URL!
    var label: String { LaunchAgentService.plistName.replacingOccurrences(of: ".plist", with: "") }
    var fallbackKey: String { "\(label).usesLocalLaunchAgent" }
    var policyKey: String { "\(label).fallbackPolicyRevision" }
    var startsAtLoginKey: String { "\(label).startsAtLogin" }
    var session: URL {
        home.appendingPathComponent("Library/Application Support/AgentRelay/SessionServices/\(label).session.plist")
    }

    init(osCallTimeout: TimeInterval = 2, repairBudget: TimeInterval = 8) throws {
        home = FileManager.default.temporaryDirectory.appendingPathComponent("core-repair-\(UUID())")
        let bundle = home.appendingPathComponent("Applications/Agent Relay.app")
        helper = bundle.appendingPathComponent("Contents/Helpers/Agent Relay Helper")
        fallback = home.appendingPathComponent("Library/LaunchAgents/\(LaunchAgentService.plistName.replacingOccurrences(of: ".plist", with: "")).local.plist")
        defaults = UserDefaults(suiteName: suite)!
        let fm = FileManager.default
        try fm.createDirectory(at: helper.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data("#!/bin/sh\nexit 0\n".utf8).write(to: helper)
        try fm.setAttributes([.posixPermissions: 0o755], ofItemAtPath: helper.path)
        let agent = bundle.appendingPathComponent("Contents/Library/LaunchAgents/\(LaunchAgentService.plistName)")
        agentPlist = agent
        try fm.createDirectory(at: agent.deletingLastPathComponent(), withIntermediateDirectories: true)
        try PropertyListSerialization.data(fromPropertyList: ["BundleProgram": "Contents/Helpers/Agent Relay Helper",
                                                              "ProgramArguments": ["Agent Relay Helper", "serve"]],
                                           format: .xml, options: 0).write(to: agent)
        try PropertyListSerialization.data(fromPropertyList: ["CFBundleIdentifier": "com.agentrelay.desktop",
                                                              "CFBundleVersion": launchd.currentVersion,
                                                              "CFBundlePackageType": "APPL"],
                                           format: .xml, options: 0)
            .write(to: bundle.appendingPathComponent("Contents/Info.plist"))
        var system = launchd.system
        system.osCallTimeout = osCallTimeout
        system.repairBudget = repairBudget
        let launchd = launchd
        system.launchctlAsync = { arguments in await launchd.launchctlAsync(arguments) }
        service = LaunchAgentService(bundleURL: bundle, homeDirectory: home, bundleIdentifier: "com.agentrelay.desktop",
                                     defaults: defaults, system: system)
    }

    func remove() {
        defaults.removePersistentDomain(forName: suite)
        try? FileManager.default.removeItem(at: home)
    }
}
