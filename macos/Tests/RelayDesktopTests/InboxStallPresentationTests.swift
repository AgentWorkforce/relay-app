import XCTest
@testable import RelayDesktopCore

final class InboxStallPresentationTests: XCTestCase {
    func testSessionRegistrationReadsAndClearsTheCoreMark() throws {
        let stalled = Data(#"{"name":"worker","address":"worker@desktop","added_by_session":false,"inbox_stall":{"undrained_count":2,"oldest_undrained_age_ms":600000}}"#.utf8)
        let clear = Data(#"{"name":"worker","address":"worker@desktop","added_by_session":false}"#.utf8)
        let decoder = JSONDecoder()
        XCTAssertEqual(try decoder.decode(SessionRegistration.self, from: stalled).inboxStall?.waitingSummary,
                       "Stalled · 2 messages waiting · oldest 10 min")
        XCTAssertNil(try decoder.decode(SessionRegistration.self, from: clear).inboxStall)
    }

    func testWaitingCountAndAgeMatchLinux() {
        XCTAssertEqual(InboxStall(oldestUndrainedAgeMs: 600_000, undrainedCount: 1).waitingSummary,
                       "Stalled · 1 message waiting · oldest 10 min")
        XCTAssertEqual(InboxStall(oldestUndrainedAgeMs: 1_259_000, undrainedCount: 3).waitingSummary,
                       "Stalled · 3 messages waiting · oldest 20 min")
    }
}
