@testable import RelayDesktopCore
import XCTest

final class CursorHooksTests: XCTestCase {
    func testWordingMatchesLinux() {
        XCTAssertEqual(CursorHooks.title, "Cursor sessions")
        XCTAssertEqual(CursorHooks.onExplanation, "Agent Relay's hooks are in ~/.cursor/hooks.json. A Cursor editor or Cursor CLI session gets its messages at its next tool call or when its turn ends. A Cursor editor chat that has finished its turn gets them as soon as they arrive, for up to an hour; a Cursor CLI session waiting for you gets them during the next turn you start. Start Cursor CLI sessions that were open before this was on again.")
        XCTAssertEqual(CursorHooks.offExplanation, "Without Agent Relay's hooks, messages for a Cursor session wait in its inbox, unheard. Turn this on to add them to ~/.cursor/hooks.json, so a Cursor editor or Cursor CLI session you put on the relay gets its messages at its next tool call or when its turn ends, and a Cursor editor chat that has finished its turn gets them as soon as they arrive.")
        XCTAssertEqual(CursorHooks.turnOnHelp, "Add Agent Relay's hooks to Cursor")
        XCTAssertEqual(CursorHooks.turnOffHelp, "Remove Agent Relay's hooks from Cursor")
    }

    func testSettingIsAbsentUntilCursorRanAndSetsOnlyItsOwnKey() async throws {
        let core = FakeCore()
        defer { core.stop() }
        core.on("settings.get") { _ in
            ["allow_self_registration": true, "auto_activate": false, "cursor_direct_delivery": false]
        }
        core.on("settings.set") { params in
            XCTAssertEqual(params as? [String: Bool], ["cursor_direct_delivery": true])
            return ["allow_self_registration": true, "auto_activate": false, "cursor_direct_delivery": true]
        }
        let client = CoreClient(socketPath: core.path)
        let before = try await client.settingsGet()
        XCTAssertEqual(before.cursorDirectDelivery, false)
        let after = try await client.settingsSet(SettingsChange(cursorDirectDelivery: true))
        XCTAssertEqual(after.cursorDirectDelivery, true)
    }

    func testCursorCLISessionsAreLabelledApartFromCursorCloud() throws {
        let kind = try JSONDecoder().decode(SessionKind.self, from: Data("\"cursor-cli\"".utf8))
        XCTAssertEqual(kind, .cursorCli)
        XCTAssertEqual(kind.label, "Cursor CLI")
        XCTAssertEqual(kind.source, "cursor")
        XCTAssertNil(Target(id: "6e0af963-e21c-4fb8-a8e8-7f34765b6d64", kind: kind).uploadKey)
    }

    func testCopilotSessionsAreLabelledAndKeyedLikeRelayNode() throws {
        let kind = try JSONDecoder().decode(SessionKind.self, from: Data("\"copilot\"".utf8))
        XCTAssertEqual(kind, .copilot)
        XCTAssertEqual(kind.label, "Copilot")
        XCTAssertEqual(kind.source, "copilot")
        XCTAssertEqual(SessionKind(rawValue: kind.rawValue), .copilot)
        XCTAssertEqual(Target(id: "x", kind: .copilot).uploadKey, "copilot:x")
    }
}
