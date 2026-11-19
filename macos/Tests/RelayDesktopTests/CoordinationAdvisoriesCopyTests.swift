import RelayDesktopCore
import XCTest

/// The GTK test `the_copy_matches_the_mac_word_for_word` reads
/// CoordinationAdvisoriesCopy.swift, so a change here fails there too.
final class CoordinationAdvisoriesCopyTests: XCTestCase {
    func testCopyAndSettingKey() {
        XCTAssertEqual(CoordinationAdvisoriesCopy.title, "Show agents teammates' overlapping work")
        XCTAssertEqual(
            CoordinationAdvisoriesCopy.detail,
            "Off: nothing is added. On: urgent notes may reach registered agents before a turn ends. Other notes appear once at the end of a supported Claude Code, Codex, Copilot, Devin or Cursor CLI turn. Your workspace must provide coordination notes."
        )
        XCTAssertEqual(CoordinationAdvisoriesCopy.settingKey, "coordination_advisories")
    }

    func testStateDecodesTheCoresAnswer() throws {
        let state = try JSONDecoder().decode(CoordinationAdvisoriesState.self, from: Data(#"{"enabled":true,"notifications_enabled":false,"notification_status":"Urgent system notifications are off."}"#.utf8))
        XCTAssertTrue(state.enabled)
        XCTAssertFalse(state.notificationsEnabled)
        XCTAssertEqual(state.notificationStatus, "Urgent system notifications are off.")
        let params = try JSONEncoder().encode(CoordinationAdvisoriesParams(enabled: false))
        XCTAssertEqual(String(decoding: params, as: UTF8.self), #"{"enabled":false}"#)
    }
}
