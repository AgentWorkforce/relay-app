import RelayDesktopCore
import XCTest

final class CodexHookCopyTests: XCTestCase {
    func testWordingMatchesLinux() {
        XCTAssertEqual(CodexHookCopy.title, "Codex mid-turn delivery")
        XCTAssertEqual(
            CodexHookCopy.turnedOn,
            "Agent Relay's Codex hook is in ~/.codex/hooks.json. A message for a Codex session that is working reaches it at the session's next tool call or as its turn ends, instead of waiting for the turn to finish. Its rules in ~/.codex/rules/agent-relay.rules let the session read and answer relayed messages through Agent Relay without asking."
        )
        XCTAssertEqual(
            CodexHookCopy.turnedOff,
            "A message for a Codex session that is working waits until its turn ends, however long that takes. Turn this on to add Agent Relay's Codex hook to ~/.codex/hooks.json, so the message reaches the session at its next tool call or as its turn ends. Its rules in ~/.codex/rules/agent-relay.rules let the session read and answer relayed messages through Agent Relay without asking."
        )
        XCTAssertEqual(CodexHookCopy.turnOnHelp, "Add Agent Relay's hook to Codex")
        XCTAssertEqual(CodexHookCopy.turnOffHelp, "Remove Agent Relay's hook from Codex")
    }
}
