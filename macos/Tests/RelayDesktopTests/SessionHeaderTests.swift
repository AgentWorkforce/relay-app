import XCTest
@testable import RelayDesktopCore

final class SessionHeaderTests: XCTestCase {
    func testSecondLineNamesTheProviderThenTheAgent() {
        XCTAssertEqual(SessionHeader.identity(provider: "Claude Code", agentName: "api-fixer"), "Claude Code · @api-fixer")
        XCTAssertEqual(SessionHeader.identity(provider: "Codex", agentName: nil), "Codex")
        XCTAssertEqual(SessionHeader.identity(provider: "Codex", agentName: ""), "Codex")
    }

    func testPresenceBadgeShowsOnlyOnTheRelay() {
        XCTAssertEqual(RelayPresence.online.headerBadge, "Online")
        XCTAssertEqual(RelayPresence.connecting.headerBadge, "Connecting…")
        XCTAssertEqual(RelayPresence.joining.headerBadge, "Connecting…")
        XCTAssertEqual(RelayPresence.offline.headerBadge, "Offline")
        XCTAssertNil(RelayPresence.off.headerBadge)
        XCTAssertNil(RelayPresence.leaving.headerBadge)
    }
}
