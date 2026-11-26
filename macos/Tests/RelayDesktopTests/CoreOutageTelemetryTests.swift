import Darwin
import Foundation
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

@MainActor
final class CoreOutageTelemetryTests: XCTestCase {
    func testProjectRequiresReleaseConfigurationForTheProductionSiteAndBundle() {
        let key = "phc_test"
        let site = "https://agentrelay.com"
        let bundle = "com.agentrelay.desktop"
        XCTAssertNotNil(CoreOutageTelemetry.Project(key: key, site: site, bundleIdentifier: bundle, isRelease: true))
        let configurations: [(String?, String?, String?, Bool)] = [
            (key, site, bundle, false),
            (nil, site, bundle, true),
            (" ", site, bundle, true),
            ("phx_test", site, bundle, true),
            (key, nil, bundle, true),
            (key, "https://site.example.test", bundle, true),
            (key, "http://localhost:3100", bundle, true),
            (key, site, "com.agentrelay.desktop.dev", true)
        ]
        for configuration in configurations {
            XCTAssertNil(CoreOutageTelemetry.Project(key: configuration.0, site: configuration.1,
                                                     bundleIdentifier: configuration.2, isRelease: configuration.3))
        }
    }

    func testConsentAndIdentityAreReadAtCaptureTime() throws {
        let fixture = try Fixture(seedIdentity: false)
        defer { fixture.remove() }
        let telemetry = try fixture.telemetry()
        try fixture.write("preferences.json", "{\"analytics_enabled\":false}")
        fixture.failure(telemetry, .startupWatchdog)
        try fixture.write("preferences.json", "{\"analytics_enabled\":\"true\"}")
        fixture.failure(telemetry, .startupWatchdog)
        try fixture.write("preferences.json", "{\"analytics_enabled\":null}")
        fixture.failure(telemetry, .startupWatchdog)
        try fixture.write("preferences.json", #"{"analytics_enabled":true,"analytics_ena\u0062led":false}"#)
        fixture.failure(telemetry, .startupWatchdog)
        try fixture.write("preferences.json", "{\"analytics_enabled\":true}")
        fixture.defaults.set(false, forKey: LegacyMigration.analyticsEnabledKey)
        fixture.failure(telemetry, .startupWatchdog)
        fixture.defaults.removeObject(forKey: LegacyMigration.analyticsEnabledKey)
        fixture.failure(telemetry, .startupWatchdog, allowed: false)
        fixture.defaults.set("true", forKey: LegacyMigration.analyticsEnabledKey)
        fixture.failure(telemetry, .startupWatchdog)
        fixture.defaults.removeObject(forKey: LegacyMigration.analyticsEnabledKey)
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.identity.path))

        try fixture.write("installation-id", "not a UUID")
        fixture.failure(telemetry, .startupWatchdog)
        XCTAssertEqual(try String(contentsOf: fixture.identity, encoding: .utf8), "not a UUID")
        XCTAssertTrue(fixture.requests.isEmpty)

        try fixture.write("installation-id", Fixture.id + "\n")
        fixture.failure(telemetry, .startupWatchdog)
        try fixture.write("preferences.json", "{\"analytics_enabled\":false}")
        fixture.recovered(telemetry)
        XCTAssertEqual(fixture.events, ["desktop_core_unavailable"])
    }

    func testFirstLaunchPersistsIdentityBeforeCaptureAndReusesItForRecovery() throws {
        let fixture = try Fixture(seedIdentity: false)
        defer { fixture.remove() }
        try FileManager.default.removeItem(at: fixture.directory)
        let telemetry = try fixture.telemetry()
        fixture.recovered(telemetry)
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.directory.path))

        fixture.failure(telemetry, .startupWatchdog)
        let saved = try String(contentsOf: fixture.identity, encoding: .utf8)
        let id = saved.trimmingCharacters(in: .newlines)
        XCTAssertEqual(UUID(uuidString: id)?.uuidString.lowercased(), id)
        fixture.recovered(telemetry)
        let relaunched = try fixture.telemetry()
        fixture.failure(relaunched, .repeatedRefusal)
        XCTAssertEqual(fixture.events, ["desktop_core_unavailable", "desktop_core_recovered", "desktop_core_unavailable"])
        XCTAssertEqual(fixture.captures.compactMap { $0["distinct_id"] as? String }, Array(repeating: "desktop:" + id, count: 3))
        XCTAssertEqual(try String(contentsOf: fixture.identity, encoding: .utf8), saved)
    }

    func testFirstLaunchSkipsCaptureWhileCoreHoldsIdentityLockAndRetriesAfterRelease() throws {
        let fixture = try Fixture(seedIdentity: false)
        defer { fixture.remove() }
        let telemetry = try fixture.telemetry()
        let lockPath = fixture.directory.appendingPathComponent("installation-id.lock").path
        let coreLock = open(lockPath, O_RDWR | O_CREAT | O_CLOEXEC, mode_t(0o600))
        guard coreLock >= 0 else { return XCTFail("Could not open the synthetic core lock") }
        defer { close(coreLock) }
        guard flock(coreLock, LOCK_EX | LOCK_NB) == 0 else {
            return XCTFail("Could not acquire the synthetic core lock")
        }

        fixture.failure(telemetry, .startupWatchdog)
        XCTAssertTrue(fixture.requests.isEmpty)
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.identity.path))

        let saved = Fixture.id + "\n"
        try fixture.write("installation-id", saved)
        fixture.failure(telemetry, .startupWatchdog)
        XCTAssertTrue(fixture.requests.isEmpty)
        XCTAssertEqual(try String(contentsOf: fixture.identity, encoding: .utf8), saved)

        guard flock(coreLock, LOCK_UN) == 0 else { return XCTFail("Could not release the synthetic core lock") }
        fixture.failure(telemetry, .startupWatchdog)
        XCTAssertEqual(fixture.events, ["desktop_core_unavailable"])
        XCTAssertEqual(fixture.captures.first?["distinct_id"] as? String, "desktop:" + Fixture.id)
        XCTAssertEqual(try String(contentsOf: fixture.identity, encoding: .utf8), saved)
    }

    func testContentionKeepsAnOutageOpenAndRetriesRecoveryAfterRelease() async throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let telemetry = try fixture.telemetry()
        fixture.failure(telemetry, .startupWatchdog)
        let lock = try fixture.openIdentityLock()
        defer { close(lock) }

        XCTAssertEqual(flock(lock, LOCK_EX | LOCK_NB), 0)
        fixture.failure(telemetry, .repeatedRefusal)
        XCTAssertEqual(fixture.events, ["desktop_core_unavailable"])
        XCTAssertEqual(flock(lock, LOCK_UN), 0)
        fixture.failure(telemetry, .repeatedRefusal)
        XCTAssertEqual(fixture.reasons, ["startup_watchdog", "repeated_refusal"])

        XCTAssertEqual(flock(lock, LOCK_EX | LOCK_NB), 0)
        fixture.recovered(telemetry)
        fixture.recovered(telemetry)
        XCTAssertEqual(fixture.events.count, 2)
        XCTAssertEqual(flock(lock, LOCK_UN), 0)

        let recovered = await eventually { fixture.events.count == 3 }
        XCTAssertTrue(recovered)
        XCTAssertEqual(fixture.events.last, "desktop_core_recovered")
        XCTAssertEqual(fixture.captures.last?["distinct_id"] as? String, "desktop:" + Fixture.id)
    }

    func testRecoveryWaitingOnLockRechecksConsentAndIdentity() async throws {
        for transition in ["disk opt-out", "rotated identity", "requested opt-out"] {
            let fixture = try Fixture()
            defer { fixture.remove() }
            let telemetry = try fixture.telemetry()
            fixture.failure(telemetry, .startupWatchdog)
            let lock = try fixture.openIdentityLock()
            defer { close(lock) }

            XCTAssertEqual(flock(lock, LOCK_EX | LOCK_NB), 0)
            fixture.recovered(telemetry)
            switch transition {
            case "disk opt-out":
                try fixture.write("preferences.json", #"{"analytics_enabled":false}"#)
            case "rotated identity":
                try fixture.write("installation-id", "11111111-1111-4111-8111-111111111111")
            default:
                telemetry.optOutRequested()
            }
            XCTAssertEqual(flock(lock, LOCK_UN), 0)

            try await Task.sleep(for: .milliseconds(200))
            XCTAssertEqual(fixture.events, ["desktop_core_unavailable"], transition)
        }
    }

    func testConsentChangeBeforeTransportSuppressesCapture() throws {
        let fixture = try Fixture(seedIdentity: false)
        defer { fixture.remove() }
        let telemetry = try fixture.telemetry()
        func diagnosticsAfterOptOut() -> CoreServiceDiagnostics {
            do {
                try fixture.write("preferences.json", #"{"analytics_enabled":false}"#)
                try FileManager.default.removeItem(at: fixture.identity)
            } catch {
                XCTFail("Could not simulate the core opting out: \(error)")
            }
            return .init()
        }

        telemetry.failure(.startupWatchdog, service: diagnosticsAfterOptOut(), attempts: 1,
                          repairs: 0, lastError: nil, runningVersion: nil, allowed: true)
        fixture.recovered(telemetry)
        XCTAssertTrue(fixture.requests.isEmpty)
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.identity.path))
    }

    func testMissingIdentityDuringOutageDoesNotCreateAReplacementForRecovery() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let telemetry = try fixture.telemetry()
        fixture.failure(telemetry, .startupWatchdog)
        try FileManager.default.removeItem(at: fixture.identity)
        fixture.failure(telemetry, .repeatedRefusal)
        fixture.recovered(telemetry)
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.identity.path))
        XCTAssertEqual(fixture.events, ["desktop_core_unavailable"])
        fixture.failure(telemetry, .startupWatchdog)
        XCTAssertEqual(fixture.events.count, 2)
        XCTAssertNotEqual(fixture.captures.last?["distinct_id"] as? String, "desktop:" + Fixture.id)
    }

    func testUnsafeDiskStateSuppressesFirstLaunchCapture() throws {
        let fixture = try Fixture(seedIdentity: false)
        defer { fixture.remove() }
        let telemetry = try fixture.telemetry()
        let files = FileManager.default
        let preferences = fixture.directory.appendingPathComponent("preferences.json")
        let missing = fixture.directory.appendingPathComponent("missing")
        for path in [preferences, fixture.identity, fixture.directory.appendingPathComponent("installation-id.lock")] {
            try files.createSymbolicLink(at: path, withDestinationURL: missing)
            fixture.failure(telemetry, .startupWatchdog)
            XCTAssertTrue(fixture.requests.isEmpty)
            XCTAssertEqual(try files.destinationOfSymbolicLink(atPath: path.path), missing.path)
            try files.removeItem(at: path)
            // Earlier probes may have initialized the lock file.
            let lock = fixture.directory.appendingPathComponent("installation-id.lock")
            if files.fileExists(atPath: lock.path) { try files.removeItem(at: lock) }
        }
        XCTAssertFalse(files.fileExists(atPath: fixture.identity.path))
    }

    func testRotationAndPendingOptOutCannotLinkAnOldOutageToANewIdentity() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let telemetry = try fixture.telemetry()
        fixture.failure(telemetry, .startupWatchdog)
        try fixture.write("installation-id", "11111111-1111-4111-8111-111111111111")
        fixture.failure(telemetry, .repeatedRefusal)
        fixture.recovered(telemetry)
        fixture.failure(telemetry, .startupWatchdog)
        telemetry.optOutRequested()
        fixture.recovered(telemetry)
        XCTAssertEqual(fixture.events, ["desktop_core_unavailable", "desktop_core_unavailable"])
        XCTAssertEqual(fixture.captures.compactMap { $0["distinct_id"] as? String },
                       ["desktop:" + Fixture.id, "desktop:11111111-1111-4111-8111-111111111111"])
    }

    func testPayloadUsesOnlyClosedFieldsAndBoundsNumbers() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let telemetry = try fixture.telemetry(appVersion: "2026.11.26")
        let noisyError = NSError(domain: "path:/Users/example/private", code: 42, userInfo: [
            NSLocalizedDescriptionKey: "synthetic-user@example.test /Users/example/private synthetic-session"
        ])
        let service = CoreServiceDiagnostics(smAppServiceStatus: .notFound, installState: .outsideApplications,
                                             inApplications: false, translocated: true, quarantine: .present,
                                             backupAgent: .local, spawnFailure: .brokenBundleProgram)
        telemetry.failure(.knownSpawnFailure, service: service, attempts: Int.max, repairs: -10,
                          lastError: .init(noisyError), runningVersion: "1.0.0-private", allowed: true)
        let request = try XCTUnwrap(fixture.requests.first)
        XCTAssertEqual(request.url?.absoluteString, "https://i.agentrelay.com/i/v0/e/")
        XCTAssertEqual(request.httpMethod, "POST")
        XCTAssertNil(request.value(forHTTPHeaderField: "Authorization"))
        let capture = try XCTUnwrap(fixture.captures.first)
        XCTAssertEqual(Set(capture.keys), ["api_key", "event", "distinct_id", "properties"])
        XCTAssertEqual(capture["api_key"] as? String, "phc_test")
        XCTAssertEqual(capture["distinct_id"] as? String, "desktop:" + Fixture.id)
        let properties = try XCTUnwrap(capture["properties"] as? [String: Any])
        XCTAssertEqual(Set(properties.keys), [
            "schema_version", "app", "platform", "emitter", "$geoip_disable", "$process_person_profile",
            "reason", "app_version", "bundled_core_version", "os_version", "architecture", "service",
            "attempts", "repairs", "last_error"
        ])
        XCTAssertEqual(properties["$geoip_disable"] as? Bool, true)
        XCTAssertEqual(properties["$process_person_profile"] as? Bool, false)
        XCTAssertEqual(properties["app_version"] as? [Int], [2026, 11, 26])
        XCTAssertEqual(properties["os_version"] as? [Int], [15, 1, 0])
        XCTAssertEqual(properties["attempts"] as? Int, 255)
        XCTAssertEqual(properties["repairs"] as? Int, 0)
        let error = try XCTUnwrap(properties["last_error"] as? [String: String])
        XCTAssertEqual(error, ["domain": "other"])
        let evidence = try XCTUnwrap(properties["service"] as? [String: Any])
        XCTAssertEqual(evidence["sm_app_service_status"] as? String, "not_found")
        XCTAssertEqual(evidence["spawn_failure"] as? Int, 78)
        XCTAssertEqual(evidence["in_applications"] as? Bool, false)
        XCTAssertEqual(evidence["quarantine"] as? String, "present")
        let json = String(decoding: try XCTUnwrap(request.httpBody), as: UTF8.self)
        for secret in ["synthetic-user", "/Users/example", "synthetic-session", "private"] {
            XCTAssertFalse(json.contains(secret))
        }
        XCTAssertNil(CoreOutageTelemetry.Failure(NSError(domain: NSCocoaErrorDomain, code: Int.max)).code)
        XCTAssertEqual(CoreOutageTelemetry.Failure(CoreError.unreachable("synthetic", socketErrorCode: 61)).code, 61)
    }

    func testEachFailureReasonAndFirstHealthyRecoveryAreDeduplicatedPerOutage() throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let telemetry = try fixture.telemetry()
        fixture.recovered(telemetry)
        for _ in 0..<2 {
            for _ in 0..<3 {
                fixture.failure(telemetry, .registrationFailed)
                fixture.failure(telemetry, .repairFailed)
            }
            fixture.recovered(telemetry)
            fixture.recovered(telemetry)
        }
        XCTAssertEqual(fixture.reasons, ["registration_failed", "repair_failed", "registration_failed", "repair_failed"])
        XCTAssertEqual(fixture.events, [
            "desktop_core_unavailable", "desktop_core_unavailable", "desktop_core_recovered",
            "desktop_core_unavailable", "desktop_core_unavailable", "desktop_core_recovered"
        ])
    }

    func testAppReportsWatchdogAndRepeatedRefusalThenFirstHealthySubscription() async throws {
        let fixture = try Fixture(seedIdentity: false)
        defer { fixture.remove() }
        let service = FakeService(status: .enabled)
        fixture.defaults.set("build-1", forKey: AppModel.registeredCoreKey)
        let path = "/tmp/outage-\(UUID().uuidString.prefix(8)).sock"
        let model = try fixture.model(path: path, service: service, startupTimeout: 50_000_000, backoff: 0.003)
        defer { model.shutdown() }
        await model.bootstrapIfNeeded()
        let failed = await eventually {
            fixture.reasons.contains("startup_watchdog") && fixture.reasons.contains("repeated_refusal")
        }
        XCTAssertTrue(failed)
        let core = FakeCore(path: path)
        defer { core.stop() }
        let recovered = await eventually { model.isCoreConnected && fixture.events.contains("desktop_core_recovered") }
        XCTAssertTrue(recovered)
        XCTAssertEqual(fixture.reasons.filter { $0 == "repeated_refusal" }.count, 1)
        XCTAssertEqual(fixture.events.filter { $0 == "desktop_core_recovered" }.count, 1)
        let id = try String(contentsOf: fixture.identity, encoding: .utf8).trimmingCharacters(in: .newlines)
        XCTAssertTrue(fixture.captures.allSatisfy { $0["distinct_id"] as? String == "desktop:" + id })
        XCTAssertEqual(fixture.properties.last?["running_core_version"] as? [Int], [1, 0, 0])
    }

    func testAppReportsInstallAndRegistrationFailuresWithoutTheSocket() async throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        let cases: [(CoreServiceStatus, String?, NSError?, String)] = [
            (.enabled, "Synthetic move required", nil, "installation_required"),
            (.notFound, nil, nil, "service_not_found"),
            (.notRegistered, nil, NSError(domain: NSCocoaErrorDomain, code: 1), "registration_failed"),
            (.enabled, nil, NSError(domain: NSCocoaErrorDomain, code: 1), "repair_failed")
        ]
        for (status, installProblem, registrationError, reason) in cases {
            let service = FakeService(status: status)
            service.installationProblem = installProblem
            service.registrationError = registrationError
            fixture.defaults.set("build-1", forKey: AppModel.registeredCoreKey)
            let model = try fixture.model(path: "/tmp/outage-\(UUID().uuidString.prefix(8)).sock", service: service)
            if reason == "repair_failed" { await model.retryBackgroundService() }
            else { await model.bootstrapIfNeeded() }
            XCTAssertEqual(fixture.reasons.last, reason)
            model.shutdown()
        }
    }

    func testManualRestartStaysVisibleUntilFirstHealthySubscription() async throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        fixture.defaults.set("build-1", forKey: AppModel.registeredCoreKey)
        let service = FakeService(status: .enabled)
        let path = "/tmp/outage-\(UUID().uuidString.prefix(8)).sock"
        let model = try fixture.model(path: path, service: service, startupTimeout: 400_000_000, backoff: 0.05)
        defer { model.shutdown() }
        await model.bootstrapIfNeeded()
        let timedOut = await eventually { model.coreProblem != nil }
        XCTAssertTrue(timedOut)
        await model.retryBackgroundService()
        XCTAssertEqual(service.registered, 1)
        XCTAssertTrue(model.isRestartingCore)
        XCTAssertFalse(model.canRetryBackgroundService)
        let core = FakeCore(path: path)
        defer { core.stop() }
        let reconnected = await eventually { model.isCoreConnected }
        XCTAssertTrue(reconnected)
        XCTAssertFalse(model.isRestartingCore)
        XCTAssertNil(model.coreProblem)
    }

    func testManualRestartTimesOutAndReenablesRetry() async throws {
        let fixture = try Fixture()
        defer { fixture.remove() }
        fixture.defaults.set("build-1", forKey: AppModel.registeredCoreKey)
        let model = try fixture.model(path: "/tmp/outage-\(UUID().uuidString.prefix(8)).sock",
                                      service: FakeService(status: .enabled), startupTimeout: 150_000_000)
        defer { model.shutdown() }
        await model.bootstrapIfNeeded()
        await model.retryBackgroundService()
        XCTAssertTrue(model.isRestartingCore)
        let finished = await eventually { model.canRetryBackgroundService && model.coreProblem != nil }
        XCTAssertTrue(finished)
        XCTAssertFalse(model.isCoreConnected)
    }

    @MainActor
    private final class Fixture {
        static let id = "abcdef01-2345-4678-9abc-def012345678"
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("core-outage-\(UUID())")
        let suite = "core-outage-test-\(UUID())"
        let defaults: UserDefaults
        private(set) var requests: [URLRequest] = []

        var identity: URL { directory.appendingPathComponent("installation-id") }

        init(seedIdentity: Bool = true) throws {
            defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            if seedIdentity { try write("installation-id", Self.id.uppercased() + "\n") }
        }

        func remove() {
            defaults.removePersistentDomain(forName: suite)
            try? FileManager.default.removeItem(at: directory)
        }

        func write(_ name: String, _ text: String) throws {
            try Data(text.utf8).write(to: directory.appendingPathComponent(name), options: .atomic)
        }

        func telemetry(appVersion: String = "1.0.0") throws -> CoreOutageTelemetry {
            let project = try XCTUnwrap(CoreOutageTelemetry.Project(key: "phc_test", site: "https://agentrelay.com",
                                                                    bundleIdentifier: "com.agentrelay.desktop", isRelease: true))
            return CoreOutageTelemetry(project: project, dataDirectory: directory, defaults: defaults,
                                       appVersion: appVersion, bundledCoreVersion: "1.0.0",
                                       osVersion: .init(majorVersion: 15, minorVersion: 1, patchVersion: 0),
                                       architecture: .arm64, transport: { [weak self] request in
                                           guard let self else { return false }
                                           self.requests.append(request)
                                           return true
                                       })
        }

        func model(path: String, service: FakeService, startupTimeout: UInt64 = 20_000_000_000,
                   backoff: Double = 2) throws -> AppModel {
            AppModel(client: CoreClient(socketPath: path), service: service, bundledCoreVersion: "1.0.0",
                     unregisterSettle: 0, startupTimeout: startupTimeout, manageSystemServices: false,
                     defaults: defaults, outageTelemetry: try telemetry(), connectionBackoffLimit: backoff)
        }

        func openIdentityLock() throws -> Int32 {
            let path = directory.appendingPathComponent("installation-id.lock").path
            let lock = open(path, O_RDWR | O_CLOEXEC)
            guard lock >= 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
            return lock
        }

        func failure(_ telemetry: CoreOutageTelemetry, _ reason: CoreOutageTelemetry.Reason, allowed: Bool = true) {
            telemetry.failure(reason, service: .init(), attempts: 1, repairs: 0, lastError: nil,
                              runningVersion: nil, allowed: allowed)
        }

        func recovered(_ telemetry: CoreOutageTelemetry) {
            telemetry.recovered(service: .init(), attempts: 1, repairs: 0, lastError: nil,
                                runningVersion: "1.0.0", allowed: true)
        }

        var captures: [[String: Any]] {
            requests.compactMap { request in
                guard let body = request.httpBody else { return nil }
                return (try? JSONSerialization.jsonObject(with: body)) as? [String: Any]
            }
        }
        var events: [String] { captures.compactMap { $0["event"] as? String } }
        var properties: [[String: Any]] { captures.compactMap { $0["properties"] as? [String: Any] } }
        var reasons: [String] { properties.compactMap { $0["reason"] as? String } }
    }
}
