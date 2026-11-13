import Foundation
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

@MainActor
final class StartupSettingsTests: XCTestCase {
    func testOneStartupChoiceControlsBothRegistrations() {
        let service = FakeService(status: .enabled)
        let login = FakeLoginItem()
        let suite = "startup-settings-test-\(UUID())"
        let defaults = UserDefaults(suiteName: suite)!
        let model = AppModel(client: CoreClient(socketPath: "/tmp/startup-test-\(UUID()).sock"),
                             service: service, manageSystemServices: false, defaults: defaults, loginItem: login)
        defer { model.shutdown(); defaults.removePersistentDomain(forName: suite) }

        model.setLaunchAtLogin(false)
        XCTAssertFalse(login.isEnabled)
        XCTAssertFalse(service.startsAtLogin)
        XCTAssertFalse(model.launchAtLogin)
        XCTAssertEqual(service.status, .enabled, "manual use must still have a core")
        XCTAssertEqual(service.unregistered, 0)

        model.setLaunchAtLogin(true)
        XCTAssertTrue(login.isEnabled)
        XCTAssertTrue(service.startsAtLogin)
        XCTAssertTrue(model.launchAtLogin)
        XCTAssertNil(model.lastError)
    }

    func testServiceFailureRestoresTheGUIStartupChoice() {
        let service = FakeService(status: .enabled)
        service.startupError = NSError(domain: "startup-test", code: 1,
            userInfo: [NSLocalizedDescriptionKey: "service registration failed"])
        let login = FakeLoginItem()
        let suite = "startup-settings-test-\(UUID())"
        let defaults = UserDefaults(suiteName: suite)!
        let model = AppModel(client: CoreClient(socketPath: "/tmp/startup-test-\(UUID()).sock"),
                             service: service, manageSystemServices: false, defaults: defaults, loginItem: login)
        defer { model.shutdown(); defaults.removePersistentDomain(forName: suite) }

        model.setLaunchAtLogin(false)
        XCTAssertTrue(login.isEnabled)
        XCTAssertTrue(service.startsAtLogin)
        XCTAssertTrue(model.launchAtLogin)
        XCTAssertTrue(model.lastError?.contains("service registration failed") == true)
    }

    func testGUIFailureLeavesTheServiceStartupChoiceAlone() {
        let service = FakeService(status: .enabled)
        let login = FakeLoginItem()
        login.refusesChange = true
        let suite = "startup-settings-test-\(UUID())"
        let defaults = UserDefaults(suiteName: suite)!
        let model = AppModel(client: CoreClient(socketPath: "/tmp/startup-test-\(UUID()).sock"),
                             service: service, manageSystemServices: false, defaults: defaults, loginItem: login)
        defer { model.shutdown(); defaults.removePersistentDomain(forName: suite) }

        model.setLaunchAtLogin(false)
        XCTAssertTrue(login.isEnabled)
        XCTAssertTrue(service.startsAtLogin)
        XCTAssertTrue(model.launchAtLogin)
        XCTAssertNotNil(model.lastError)
    }
}
