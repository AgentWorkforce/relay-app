import RelayDesktopCore
import XCTest

final class ClaudeSessionHookCopyTests: XCTestCase {
    func testWordingMatchesLinux() {
        XCTAssertEqual(ClaudeSessionHookCopy.title, "Ask Claude chats to join")
        XCTAssertEqual(
            ClaudeSessionHookCopy.turnedOff,
            "Ask at the beginning of the session if it should go on the relay."
        )
        XCTAssertEqual(
            ClaudeSessionHookCopy.turnedOn,
            "Ask at the beginning of the session if it should go on the relay."
        )
        XCTAssertEqual(
            ClaudeSessionHookCopy.turnOnHelp,
            "Add the session-start prompt and permissions for Agent Relay's own read and reply commands. Registration still needs approval."
        )
        XCTAssertEqual(
            ClaudeSessionHookCopy.turnOffHelp,
            "Remove the session-start prompt and its Agent Relay command permissions"
        )
        XCTAssertFalse(ClaudeSessionHookCopy.turnedOn.contains("auto_activate"))
        XCTAssertEqual(ClaudeSessionHookCopy.turnedOff, ClaudeSessionHookCopy.turnedOn)
    }
}
