import Foundation
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

@MainActor
final class AppModelKnownSpawnFailureTests: XCTestCase {
    func testSuccessfulSpawnRecoveryReportsKnownFailureOnce() async throws {
        let captures = try await retryService(evidence: .brokenBundleProgram, recovery: .success(true))
        XCTAssertEqual(captures, [unavailable("known_spawn_failure")])
    }

    func testThrowingSpawnRecoveryReportsKnownFailureOnceAndRepairFailure() async throws {
        let captures = try await retryService(evidence: .brokenBundleProgram, recovery: .failure(RecoveryError()))
        XCTAssertEqual(captures, [unavailable("known_spawn_failure"), unavailable("repair_failed")])
    }

    func testThrowingSpawnRecoveryWithoutEvidenceReportsOnlyRepairFailure() async throws {
        let captures = try await retryService(evidence: nil, recovery: .failure(RecoveryError()))
        XCTAssertEqual(captures, [unavailable("repair_failed")])
    }

    private struct Capture: Decodable, Equatable {
        struct Properties: Decodable, Equatable {
            let reason: String
        }

        let event: String
        let properties: Properties
    }

    private struct RecoveryError: Error {}

    private func unavailable(_ reason: String) -> Capture {
        Capture(event: "desktop_core_unavailable", properties: .init(reason: reason))
    }

    private func retryService(
        evidence: CoreServiceDiagnostics.SpawnFailure?, recovery: Result<Bool, RecoveryError>,
        file: StaticString = #filePath, line: UInt = #line
    ) async throws -> [Capture] {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: false)
        defer { try? FileManager.default.removeItem(at: directory) }
        try Data(UUID().uuidString.utf8).write(to: directory.appendingPathComponent("installation-id"))

        let suite = "known-spawn-failure-\(UUID())"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }

        let project = try XCTUnwrap(CoreOutageTelemetry.Project(
            key: "phc_test", site: "https://agentrelay.com", bundleIdentifier: "com.agentrelay.desktop", isRelease: true
        ))
        var requests: [URLRequest] = []
        let telemetry = CoreOutageTelemetry(
            project: project, dataDirectory: directory, defaults: defaults,
            appVersion: "1.0.0", bundledCoreVersion: "1.0.0",
            osVersion: .init(majorVersion: 15, minorVersion: 0, patchVersion: 0),
            transport: { request in
                requests.append(request)
                return true
            }
        )
        let service = SpawnRecoveryService(evidence: evidence, recovery: recovery)
        let model = AppModel(client: CoreClient(socketPath: directory.appendingPathComponent("core.sock").path),
                             service: service, bundledCoreVersion: "1.0.0", unregisterSettle: 0,
                             manageSystemServices: false, defaults: defaults, outageTelemetry: telemetry)
        defer { model.shutdown() }

        await model.retryBackgroundService()

        XCTAssertEqual(service.recoveryCalls, 1, file: file, line: line)
        return try requests.map { request in
            let body = try XCTUnwrap(request.httpBody, file: file, line: line)
            return try JSONDecoder().decode(Capture.self, from: body)
        }
    }

    private final class SpawnRecoveryService: CoreService {
        let status: CoreServiceStatus = .enabled
        let binaryIdentity: String? = nil
        let spawnFailureEvidence: CoreServiceDiagnostics.SpawnFailure?
        private let recovery: Result<Bool, RecoveryError>
        private(set) var recoveryCalls = 0

        var diagnostics: CoreServiceDiagnostics {
            .init(smAppServiceStatus: status, spawnFailure: spawnFailureEvidence)
        }

        init(evidence: CoreServiceDiagnostics.SpawnFailure?, recovery: Result<Bool, RecoveryError>) {
            spawnFailureEvidence = evidence
            self.recovery = recovery
        }

        @MainActor
        func recoverSpawnFailureAsync() async throws -> Bool {
            recoveryCalls += 1
            return try recovery.get()
        }

        func setStartsAtLogin(_ enabled: Bool) throws {}
        func register() throws {}
        func unregister() throws {}
        func openSystemSettings() {}
    }
}
