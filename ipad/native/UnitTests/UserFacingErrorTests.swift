import XCTest
@testable import AgentRelayiPad

/// What a person reads when something fails: the app's words, never a
/// server's error text, HTTP status or header name.
final class UserFacingErrorTests: XCTestCase {
    private let serverText = "DM reads for this agent are backing off after repeated failures. Retry after the interval in the Retry-After header."

    private func server(_ status: Int, code: String = "", retryAfter: Int? = nil) -> RelayAPIError {
        RelayAPIError(message: serverText, status: status, code: code, retryAfterSeconds: retryAfter, fromServer: true)
    }

    func testServerTextIsNeverShown() {
        for status in [400, 401, 403, 404, 409, 422, 429, 500, 502, 503] {
            for retryAfter in [nil, 30] {
                let failure = server(status, retryAfter: retryAfter)
                let shown = UserFacingError.message(for: failure) ?? ""
                XCTAssertFalse(shown.contains("Retry-After"), "HTTP \(status)")
                XCTAssertFalse(shown.contains("backing off"), "HTTP \(status)")
                XCTAssertFalse(shown.contains("\(status)"), "HTTP \(status)")
                XCTAssertEqual(failure.localizedDescription, shown)
            }
        }
    }

    /// A send, start or sign-in the server shed is not retried for the
    /// person: they are told to try again, not that the app will.
    func testBackoffOnAUserInitiatedPathAsksThePersonToTryAgain() {
        XCTAssertEqual(UserFacingError.busy, "Agent Relay is busy. Try again in a moment.")
        XCTAssertEqual(UserFacingError.message(for: server(429)), UserFacingError.busy)
        XCTAssertEqual(UserFacingError.message(for: server(503, code: "dm_unavailable", retryAfter: 60)), UserFacingError.busy)
        XCTAssertEqual(UserFacingError.message(for: server(500, retryAfter: 5)), UserFacingError.busy)
        XCTAssertEqual(server(429).localizedDescription, UserFacingError.busy)
        let paused = RelayAPIError(message: UserFacingError.busy, code: "read_paused", retryAfterSeconds: 20)
        XCTAssertEqual(UserFacingError.message(for: paused), UserFacingError.busy)
    }

    /// Only an automatic read, which the app retries itself, says so.
    func testBackoffOnAnAutomaticReadSaysTheAppRetries() {
        XCTAssertEqual(UserFacingError.busyRetrying, "Agent Relay is busy. Trying again shortly.")
        XCTAssertEqual(UserFacingError.message(for: server(429), retriesAutomatically: true), UserFacingError.busyRetrying)
        XCTAssertEqual(UserFacingError.message(for: server(503, code: "dm_unavailable", retryAfter: 60), retriesAutomatically: true),
                       UserFacingError.busyRetrying)
        var reports = ConnectionErrorReports()
        let ticket = reports.begin(.history)
        XCTAssertEqual(reports.decision(for: server(429), from: ticket), .show(UserFacingError.busyRetrying))
    }

    /// A channel's history is not re-read on a timer, so its notice
    /// promises no retry; a DM's is, and does.
    func testStaleNoticesPromiseARetryOnlyWhereOneHappens() {
        XCTAssertTrue(UserFacingError.staleMessages.contains("Retrying"))
        XCTAssertEqual(UserFacingError.staleChannelMessages, "Couldn't refresh messages.")
    }

    func testStatusesMapToTheirCopy() {
        XCTAssertEqual(UserFacingError.message(for: server(401)), UserFacingError.signIn)
        XCTAssertEqual(UserFacingError.message(for: server(403)), UserFacingError.signIn)
        XCTAssertEqual(UserFacingError.message(for: server(502)), UserFacingError.serverTrouble)
        XCTAssertEqual(UserFacingError.message(for: server(404)), UserFacingError.generic)
    }

    func testTransportAndCancellation() {
        XCTAssertEqual(UserFacingError.message(for: URLError(.notConnectedToInternet)), UserFacingError.unreachable)
        XCTAssertEqual(UserFacingError.message(for: URLError(.timedOut)), UserFacingError.unreachable)
        XCTAssertNil(UserFacingError.message(for: CancellationError()))
        XCTAssertNil(UserFacingError.message(for: URLError(.cancelled)))
    }

    /// Copy the app wrote itself is shown as written.
    func testTheAppsOwnCopyIsKept() {
        XCTAssertEqual(UserFacingError.message(for: RelayAPIError(message: "Sign in to Agent Relay")), "Sign in to Agent Relay")
        let overloaded = RelayAPIError(message: "Agent Relay is busy. Try again in a moment.", status: 503,
                                       code: "database_overloaded")
        XCTAssertEqual(overloaded.localizedDescription, "Agent Relay is busy. Try again in a moment.")
        XCTAssertEqual(UserFacingError.message(for: WorkspaceKeyRecoveryError.signInAgain),
                       WorkspaceKeyRecoveryError.signInAgain.localizedDescription)
    }

    /// A start the node refused shows the app's sentence; the raw detail stays
    /// in `message` only, for classification (SessionFork.triesNextNode).
    func testSpawnFailureShowsTheSentenceOnly() {
        let failure = SpawnFailure.error("spawn_fork_not_found: @planner is not an agent", node: "mini",
                                         name: "helper", harness: "claude", status: 404)
        XCTAssertEqual(failure.localizedDescription, "mini no longer has this agent. Refresh Agents and try again.")
        XCTAssertFalse(failure.localizedDescription.contains("@planner"))
        XCTAssertTrue(SessionFork.triesNextNode(after: failure))
    }
}

/// The server asked the app to wait: every read of that kind waits that
/// long, with no early retry, and a good read clears the wait.
final class ReadBackoffTests: XCTestCase {
    private let start = Date(timeIntervalSince1970: 1_000_000)
    private let token = "at_live_phone"

    func testDMUnavailablePausesEveryDMReadForTheRetryAfter() {
        var backoff = ReadBackoff()
        let shed = RelayAPIError(message: "DM reads are backing off", status: 503, code: "dm_unavailable",
                                 retryAfterSeconds: 120, fromServer: true)
        let failed = ReadBackoff.scope(path: "/dm/dm_planner/messages?limit=100", method: "GET")!
        backoff.failed(failed, token: token, error: shed, now: start)

        // Every DM read for the token waits: another conversation and the list too.
        for path in ["/dm/dm_planner/messages?limit=100", "/dm/dm_reviewer/messages?limit=100", "/dm/conversations"] {
            let scope = ReadBackoff.scope(path: path, method: "GET")!
            XCTAssertEqual(backoff.remaining(scope, token: token, now: start), 120, path)
            XCTAssertEqual(backoff.remaining(scope, token: token, now: start.addingTimeInterval(119.5)), 1, path)
            XCTAssertNil(backoff.remaining(scope, token: token, now: start.addingTimeInterval(120)), path)
        }
        // Sending is not a read; other reads and another credential don't wait.
        XCTAssertNil(ReadBackoff.scope(path: "/dm", method: "POST"))
        XCTAssertNil(backoff.remaining(ReadBackoff.scope(path: "/agents?status=all", method: "GET")!, token: token, now: start))
        XCTAssertNil(backoff.remaining(failed, token: "at_live_other", now: start))
    }

    func testASuccessClearsThePause() {
        var backoff = ReadBackoff()
        let scope = ReadBackoff.scope(path: "/dm/conversations", method: "GET")!
        backoff.failed(scope, token: token, error: RelayAPIError(message: "", status: 503, code: "dm_unavailable",
                                                                   retryAfterSeconds: 300, fromServer: true), now: start)
        XCTAssertNotNil(backoff.remaining(scope, token: token, now: start))
        backoff.succeeded(scope)
        XCTAssertNil(backoff.remaining(scope, token: token, now: start))
    }

    func testRetryAfterIsHonouredWithinBounds() {
        func delay(_ status: Int, code: String = "", retryAfter: Int?) -> Int? {
            ReadBackoff.delay(after: RelayAPIError(message: "", status: status, code: code,
                                                   retryAfterSeconds: retryAfter, fromServer: true))
        }
        XCTAssertEqual(delay(429, retryAfter: 45), 45)
        XCTAssertEqual(delay(429, retryAfter: nil), ReadBackoff.defaultDelay)
        XCTAssertEqual(delay(503, code: "dm_unavailable", retryAfter: nil), ReadBackoff.defaultDelay)
        XCTAssertEqual(delay(429, retryAfter: 0), 1)
        XCTAssertEqual(delay(429, retryAfter: 86_400), 300)
        // A failure that asks for no wait does not pause reads.
        XCTAssertNil(delay(500, retryAfter: nil))
        XCTAssertNil(delay(404, retryAfter: nil))
        XCTAssertNil(ReadBackoff.delay(after: URLError(.timedOut)))
    }
}
