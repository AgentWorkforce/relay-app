import Foundation
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

/// The connection loop's use of the self-repair ladder (`AppModel`), with a
/// synthetic service and no core listening.
@MainActor
final class CoreSelfRepairModelTests: XCTestCase {
    private let suite = "core-self-repair-model-\(UUID())"
    private var defaults: UserDefaults!

    override func setUp() async throws {
        defaults = UserDefaults(suiteName: suite)
        defaults.set("build-1", forKey: AppModel.registeredCoreKey)
    }

    override func tearDown() async throws {
        defaults.removePersistentDomain(forName: suite)
    }

    private func model(_ service: FakeService) -> AppModel {
        AppModel(client: CoreClient(socketPath: "/tmp/self-repair-\(UUID().uuidString.prefix(8)).sock"),
                 service: service, bundledCoreVersion: "1.0.0", unregisterSettle: 0,
                 startupTimeout: 60_000_000_000, manageSystemServices: false, defaults: defaults,
                 connectionBackoffLimit: 0.003)
    }

    /// A first outage where every rung is tried and none brings the core
    /// back ends on the guidance, not the generic restart offer, without the
    /// person pressing Try again.
    func testAFullyFailingFirstOutageEndsOnTheExhaustedGuidance() async {
        let service = FakeService(status: .enabled)
        service.repairActions = [.register, .resetRegistration, .useFallback, .useFallback]
        let model = model(service)
        defer { model.shutdown() }
        await model.bootstrapIfNeeded()

        let guided = await eventually(10) { model.coreProblem == AppModel.selfRepairExhaustedMessage }

        XCTAssertTrue(guided, "coreProblem: \(model.coreProblem ?? "nil")")
        XCTAssertEqual(service.repairSteps, [0, 1, 2, 3], "each rung once, in order")
        XCTAssertNotEqual(model.coreProblem, AppModel.unreachableCoreMessage)
    }

    /// A core the ladder finds running is not counted as a tried rung, so the
    /// person still gets the ordinary restart offer.
    func testRungsThatFindNothingToRepairKeepTheRestartOffer() async {
        let service = FakeService(status: .enabled)
        let model = model(service)
        defer { model.shutdown() }
        await model.bootstrapIfNeeded()

        let offered = await eventually(10) { model.coreProblem == AppModel.unreachableCoreMessage }

        XCTAssertTrue(offered, "coreProblem: \(model.coreProblem ?? "nil")")
        XCTAssertFalse(service.repairSteps.isEmpty)
        XCTAssertTrue(service.repairSteps.allSatisfy { $0 == 0 }, "nothing tried, the ladder stays on its first rung")
    }

    /// A slow rung runs beside the connection loop: reconnecting goes on
    /// (here, all the way to the 15th attempt) while the rung is still running.
    func testASlowRungDoesNotHoldUpReconnecting() async {
        let service = FakeService(status: .enabled)
        service.repairActions = [.resetRegistration]
        service.repairDelay = 3_000_000_000
        let model = model(service)
        defer { model.shutdown() }
        await model.bootstrapIfNeeded()

        let started = await eventually(2) { service.repairsInFlight == 1 }
        let reachedFifteen = await eventually(2) { model.coreProblem != nil }

        XCTAssertTrue(started)
        XCTAssertTrue(reachedFifteen, "the loop must keep reconnecting while the rung runs")
        XCTAssertEqual(service.repairsInFlight, 1, "the rung is still running")
        XCTAssertEqual(service.repairSteps, [0], "no second rung overlaps the first")
        XCTAssertTrue(model.isRecoveringCore)
    }
}
