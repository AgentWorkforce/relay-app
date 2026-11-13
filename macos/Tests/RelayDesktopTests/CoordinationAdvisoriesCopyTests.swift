import RelayDesktopCore
import XCTest

/// The GTK test `the_copy_matches_the_mac_word_for_word` reads
/// CoordinationAdvisoriesCopy.swift, so a change here fails there too.
final class CoordinationAdvisoriesCopyTests: XCTestCase {
    func testCopyAndSettingKey() {
        XCTAssertEqual(CoordinationAdvisoriesCopy.title, "Show teammates' overlapping work at the end of a turn")
        XCTAssertEqual(
            CoordinationAdvisoriesCopy.detail,
            "Off: nothing is added. On: when a Claude Code, Codex, Copilot, Devin or Cursor CLI session ends a turn, it is shown a short note about teammates' work that overlaps with its own, once per note. Your workspace's Cloud must offer these notes."
        )
        XCTAssertEqual(CoordinationAdvisoriesCopy.settingKey, "coordination_advisories")
    }

    func testStateDecodesTheCoresAnswer() throws {
        let state = try JSONDecoder().decode(CoordinationAdvisoriesState.self, from: Data(#"{"enabled":true}"#.utf8))
        XCTAssertTrue(state.enabled)
        let params = try JSONEncoder().encode(CoordinationAdvisoriesParams(enabled: false))
        XCTAssertEqual(String(decoding: params, as: UTF8.self), #"{"enabled":false}"#)
    }
}
