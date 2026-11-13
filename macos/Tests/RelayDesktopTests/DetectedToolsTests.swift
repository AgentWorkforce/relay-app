import RelayDesktopCore
import XCTest

final class DetectedToolsTests: XCTestCase {
    private func settings(_ tools: [String]?) throws -> SettingsValues {
        var json: [String: Any] = ["allow_self_registration": true, "auto_activate": true]
        if let tools { json["detected_tools"] = tools }
        return try JSONDecoder().decode(SettingsValues.self, from: JSONSerialization.data(withJSONObject: json))
    }

    func testTheLineNamesEachToolLikeLinux() throws {
        XCTAssertNil(DetectedTools.label(nil))
        XCTAssertNil(DetectedTools.label(try settings(nil)))
        XCTAssertNil(DetectedTools.label(try settings([])))
        XCTAssertEqual(DetectedTools.label(try settings(["Claude Code"])), "Connect Claude Code to Agent Relay")
        XCTAssertEqual(DetectedTools.label(try settings(["Claude Code", "Grok", "Codex"])),
                       "Connect Claude Code, Grok and Codex to Agent Relay")
        XCTAssertEqual(DetectedTools.detail,
                       "After you sign in, Agent Relay adds its settings to each one's config so its sessions can get relay messages. Turn any of them off later in Settings.")
        XCTAssertEqual(DetectedTools.unreadSettings,
                       "Agent Relay could not read its settings, so it cannot show which tools signing in connects. Try again.")
    }

    func testManagedDetail() {
        XCTAssertTrue(ManagedDirectDelivery.detail("refuse").contains("drops relayed messages"))
        XCTAssertTrue(ManagedDirectDelivery.detail("hold").contains("asks before showing each message"))
    }
}
