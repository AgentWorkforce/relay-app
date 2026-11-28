import XCTest
@testable import RelayDesktopCore

final class ClaudeSignInNoticeTests: XCTestCase {
    private let offered = ClaudeCloudHealth(claudeCodeSignIn: true, reason: "Claude Code is not signed in", state: .needsAttention)
    private let working = ClaudeCloudHealth(claudeCodeSignIn: false, state: .working)
    private let unsaid = ClaudeCloudHealth(state: .blocked)

    /// Runs the health a status refresh brings through `decide`, carrying
    /// the flag as the app keeps it; whether each one posted.
    private func posts(_ healths: [ClaudeCloudHealth?], notified: inout Bool) -> [Bool] {
        healths.map { health in
            let decision = ClaudeSignInNotice.decide(health, notified: notified)
            notified = decision.notified
            return decision.notify
        }
    }

    func testTheFirstOfferNotifiesOnce() {
        var notified = false
        XCTAssertEqual(posts([working, offered], notified: &notified), [false, true])
        XCTAssertTrue(notified)
    }

    func testAnOfferThatContinuesDoesNotNotifyAgain() {
        var notified = false
        XCTAssertEqual(posts([offered, offered, offered], notified: &notified), [true, false, false])
    }

    func testARelaunchDuringTheSameEpisodeDoesNotNotify() {
        // The flag persisted by the earlier run, before and after the first
        // status arrives.
        var notified = true
        XCTAssertEqual(posts([nil, offered], notified: &notified), [false, false])
        XCTAssertTrue(notified)
    }

    func testANewEpisodeNotifiesAgain() {
        var notified = false
        XCTAssertEqual(posts([offered, working, offered], notified: &notified), [true, false, true])
        notified = true
        XCTAssertEqual(posts([unsaid, offered], notified: &notified), [false, true])
    }

    func testNoHealthNeitherNotifiesNorEndsTheEpisode() {
        var notified = false
        XCTAssertEqual(posts([nil, nil], notified: &notified), [false, false])
        XCTAssertFalse(notified)
        notified = true
        XCTAssertEqual(posts([nil], notified: &notified), [false])
        XCTAssertTrue(notified)
    }

    func testTheWordsMatchLinuxAndWindows() {
        XCTAssertEqual(ClaudeSignInNotice.title, "Claude Code cloud sessions stopped")
        XCTAssertEqual(ClaudeSignInNotice.body, "Sign in to Claude Code to resume them.")
        XCTAssertEqual(ClaudeSignInNotice.actionLabel, "Sign in")
    }
}
