import XCTest
@testable import RelayDesktopCore

final class ReadBackoffTests: XCTestCase {
    func testTheFirstFailureWaitsTheNormalCadenceAndEachFurtherOneDoubles() {
        var backoff = ReadBackoff()
        XCTAssertEqual(backoff.failed(random: 0), 10)
        XCTAssertEqual(backoff.failed(random: 0), 20)
        XCTAssertEqual(backoff.failed(random: 0), 40)
        XCTAssertEqual(backoff.failed(random: 0), 80)
        XCTAssertEqual(backoff.failed(random: 0), 160)
        XCTAssertEqual(backoff.failures, 5)
    }

    func testTheDoublingStopsAtFiveMinutes() {
        var backoff = ReadBackoff()
        let waits = (0..<40).map { _ in backoff.failed(random: 0.99) }
        XCTAssertEqual(waits.max(), 300)
        XCTAssertEqual(waits.last, 300)
        XCTAssertTrue(waits.allSatisfy { $0.isFinite && $0 > 0 })
    }

    func testJitterAddsAtMostAQuarterAndNeverPassesTheCap() {
        var low = ReadBackoff()
        var high = ReadBackoff()
        XCTAssertEqual(low.failed(random: 0), 10)
        XCTAssertEqual(high.failed(random: 1), 12.5)
        for _ in 0..<200 {
            var backoff = ReadBackoff()
            for failure in 1...8 {
                let base = min(10 * pow(2, Double(failure - 1)), 300)
                let wait = backoff.failed()
                XCTAssertGreaterThanOrEqual(wait, base)
                XCTAssertLessThanOrEqual(wait, min(base * 1.25, 300))
            }
        }
    }

    func testRetryAfterWinsAndIsHeldToASecond() {
        var backoff = ReadBackoff()
        XCTAssertEqual(backoff.failed(retryAfter: 42, random: 0.5), 42)
        XCTAssertEqual(backoff.failed(retryAfter: 0.2), 1)
        XCTAssertEqual(backoff.failed(retryAfter: 0), 1)
        // Without one, the doubling goes on from the failures counted.
        XCTAssertEqual(backoff.failed(random: 0), 80)
    }

    func testASuccessGoesBackToTheNormalCadence() {
        var backoff = ReadBackoff()
        _ = backoff.failed(random: 0)
        _ = backoff.failed(random: 0)
        _ = backoff.failed(random: 0)
        backoff.succeeded()
        XCTAssertEqual(backoff.failures, 0)
        XCTAssertEqual(backoff, ReadBackoff())
        XCTAssertEqual(backoff.failed(random: 0), 10)
    }

    func testTheConversationSaysTheReadFailedAndWhenItIsTriedAgain() {
        XCTAssertEqual(ReadBackoff.failureText("Agent Relay could not complete the request.", retryingIn: 20),
                       "Could not read the conversation: Agent Relay could not complete the request. Trying again in 20 s.")
        XCTAssertEqual(ReadBackoff.failureText("Relaycast is busy", retryingIn: 12.1),
                       "Could not read the conversation: Relaycast is busy. Trying again in 13 s.")
        XCTAssertEqual(ReadBackoff.recipientFailureText("Relaycast is busy.", retryingIn: 20),
                       "Could not look up the recipient: Relaycast is busy. Trying again in 20 s.")
        XCTAssertEqual(ReadBackoff.wait(0.2), "1 s")
        XCTAssertEqual(ReadBackoff.wait(59.5), "1 min")
        XCTAssertEqual(ReadBackoff.wait(150), "2 min 30 s")
        XCTAssertEqual(ReadBackoff.wait(300), "5 min")
    }
}
