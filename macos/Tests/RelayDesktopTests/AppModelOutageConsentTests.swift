import Foundation
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

@MainActor
final class AppModelOutageConsentTests: XCTestCase {
    func testOptOutDuringCoreOutageSuppressesFurtherFailureAndRecoveryCaptures() async throws {
        for optsOut in [false, true] {
            let fixture = try CoreOutageTelemetryTests.Fixture()
            defer { fixture.remove() }
            try fixture.write("preferences.json", "{\"analytics_enabled\":true}")
            let service = FakeService(status: .notFound)
            let path = "/tmp/outage-\(UUID().uuidString.prefix(8)).sock"
            let model = try fixture.model(path: path, service: service)
            defer { model.shutdown() }

            await model.ensureService()
            XCTAssertFalse(model.isCoreConnected)
            XCTAssertEqual(fixture.events, ["desktop_core_unavailable"])
            XCTAssertEqual(fixture.reasons, ["service_not_found"])

            if optsOut {
                // The core cannot persist the opt-out while its socket is absent.
                let error = await model.setSetting(SettingsChange(analyticsEnabled: false))
                XCTAssertNotNil(error)
            }

            // A distinct failure reason exercises a capture that deduplication would allow.
            let installationProblem = "Synthetic installation problem"
            service.installationProblem = installationProblem
            await model.ensureService()
            XCTAssertEqual(model.coreProblem, installationProblem)
            XCTAssertEqual(fixture.reasons, optsOut
                           ? ["service_not_found"]
                           : ["service_not_found", "installation_required"])

            service.installationProblem = nil
            service.status = .enabled
            fixture.defaults.set(service.binaryIdentity, forKey: AppModel.registeredCoreKey)
            let core = FakeCore(path: path)
            defer { core.stop() }
            await model.bootstrapIfNeeded()
            let connected = await eventually { model.isCoreConnected }
            XCTAssertTrue(connected)
            XCTAssertEqual(fixture.events, optsOut
                           ? ["desktop_core_unavailable"]
                           : ["desktop_core_unavailable", "desktop_core_unavailable", "desktop_core_recovered"])
            XCTAssertEqual(fixture.requests.count, optsOut ? 1 : 3)
        }
    }
}
