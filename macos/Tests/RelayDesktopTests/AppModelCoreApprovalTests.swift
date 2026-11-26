import Foundation
import XCTest
import RelayDesktopCore
@testable import RelayDesktop

@MainActor
final class AppModelCoreApprovalTests: XCTestCase {
    func testRegistrationFailureUsesCurrentStatusWithoutReadingAgain() async {
        await checkOperation(statuses: [.notRegistered], operationFails: true,
                             expectedStatus: .notRegistered, message: "could not start") {
            await $0.ensureService()
        }
    }

    func testRecoveryFailureUsesCurrentStatusWithoutReadingAgain() async {
        await checkOperation(statuses: [.enabled], operationFails: true,
                             expectedStatus: .enabled, message: "backup background service") {
            await $0.retryBackgroundService()
        }
    }

    func testFailedFollowUpReadClearsEarlierStatus() async {
        await checkOperation(statuses: [.notRegistered, nil], operationFails: false,
                             expectedStatus: nil, message: "could not start") {
            await $0.ensureService()
        }
    }

    func testRegistrationShowsApprovalConfirmedByNormalFollowUpRead() async {
        await checkOperation(statuses: [.notRegistered, .requiresApproval], operationFails: false,
                             expectedStatus: .requiresApproval, message: "Allow Agent Relay",
                             previousStatus: .enabled) {
            await $0.ensureService()
        }
    }

    func testRecoveryShowsApprovalConfirmedByNormalFollowUpRead() async {
        await checkOperation(statuses: [.enabled, .requiresApproval], operationFails: false,
                             expectedStatus: .requiresApproval, message: "Allow Agent Relay",
                             previousStatus: .enabled) {
            await $0.retryBackgroundService()
        }
    }

    func testCancelledRegistrationFailureLeavesPresentationAlone() async {
        let suite = "AppModelCoreApprovalTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let service = FailingApprovalService(statuses: [.requiresApproval, .notRegistered],
                                             operationFails: true)
        service.cancelOnRegister = true
        let model = AppModel(service: service, bundledCoreVersion: nil,
                             manageSystemServices: false, defaults: defaults)
        await model.ensureService()
        let approvalProblem = model.coreProblem

        await Task { await model.ensureService() }.value

        XCTAssertTrue(model.coreNeedsApproval)
        XCTAssertEqual(model.coreProblem, approvalProblem)
        XCTAssertEqual(service.statusReads, 2)
        XCTAssertEqual(service.operationAttempts, 1)
    }

    private func checkOperation(statuses: [CoreServiceStatus?], operationFails: Bool,
                                expectedStatus: CoreServiceStatus?, message: String,
                                previousStatus: CoreServiceStatus = .requiresApproval,
                                run: (AppModel) async -> Void) async {
        let suite = "AppModelCoreApprovalTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        // The final approval result would be consumed only by an extra read.
        let service = FailingApprovalService(statuses: [previousStatus] + statuses + [.requiresApproval],
                                             operationFails: operationFails)
        let model = AppModel(service: service, bundledCoreVersion: nil,
                             manageSystemServices: false, defaults: defaults)

        await model.ensureService()
        XCTAssertEqual(model.coreNeedsApproval, previousStatus == .requiresApproval)

        await run(model)

        XCTAssertEqual(model.backgroundServiceStatus, expectedStatus)
        XCTAssertEqual(model.coreNeedsApproval, expectedStatus == .requiresApproval)
        XCTAssertTrue(model.coreProblem?.contains(message) == true, "Expected problem containing \(message)")
        XCTAssertEqual(service.statusReads, 1 + statuses.count)
        XCTAssertEqual(service.operationAttempts, 1)
    }
}

private final class FailingApprovalService: CoreService {
    let status: CoreServiceStatus = .requiresApproval
    let binaryIdentity: String? = nil
    private let operationFails: Bool
    var cancelOnRegister = false
    private var statuses: [CoreServiceStatus?]
    private(set) var statusReads = 0
    private(set) var operationAttempts = 0

    init(statuses: [CoreServiceStatus?], operationFails: Bool) {
        self.statuses = statuses
        self.operationFails = operationFails
    }

    @MainActor func statusAsync() async throws -> CoreServiceStatus {
        statusReads += 1
        guard !statuses.isEmpty, let status = statuses.removeFirst() else {
            throw NSError(domain: "StatusRead", code: 1)
        }
        return status
    }

    func register() throws {
        operationAttempts += 1
        if cancelOnRegister { withUnsafeCurrentTask { $0?.cancel() } }
        if operationFails { throw NSError(domain: "Registration", code: 1) }
    }

    func recoverSpawnFailure() throws -> Bool {
        operationAttempts += 1
        if operationFails { throw NSError(domain: LaunchAgentService.errorDomain, code: 1) }
        return false
    }

    func setStartsAtLogin(_ enabled: Bool) throws {}
    func unregister() throws {}
    func openSystemSettings() {}
}
