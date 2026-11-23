import RelayDesktopCore
import UserNotifications
import XCTest

/// What the app tells the core about macOS's saved notification choice.
final class NotificationPermissionTests: XCTestCase {
    func testAuthorizationStatusMapsToThePermissionTheCoreReads() {
        XCTAssertEqual(NotificationPermission(authorization: .authorized), .allowed)
        XCTAssertEqual(NotificationPermission(authorization: .provisional), .allowed)
        XCTAssertEqual(NotificationPermission(authorization: .denied), .denied)
        XCTAssertEqual(NotificationPermission(authorization: .notDetermined), .unknown)
    }

    func testDisplayReportSendsItsMethodAndPermission() throws {
        XCTAssertEqual(CoreMethod.coordinationAdvisoriesNotificationsDisplay, "coordination_advisories.notifications.display")
        let denied = try JSONEncoder().encode(CoordinationNotificationsDisplayParams(permission: .denied))
        XCTAssertEqual(String(decoding: denied, as: UTF8.self), #"{"permission":"denied"}"#)
    }
}
