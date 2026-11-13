import XCTest
@testable import AgentRelayiPad

/// The launch-timeout alert: requests that crossed a suspension, were
/// superseded or were cancelled stay out of the alert; a service that is
/// really unreachable still raises it.
final class ConnectionErrorReportsTests: XCTestCase {
    private let timedOut = URLError(.timedOut)

    /// The reported case: a refresh left in flight when iOS suspended the app
    /// fails with "The request timed out." when it comes back, while the
    /// refresh started on returning is slow but succeeds. No alert.
    func testSlowButSuccessfulStartupAfterSuspensionShowsNoTimeout() {
        var reports = ConnectionErrorReports()
        let beforeSuspension = reports.begin(.presence)
        reports.sceneChanged(foreground: false)
        reports.sceneChanged(foreground: true)
        let onReturn = reports.begin(.presence)
        reports.succeeded(onReturn)

        XCTAssertEqual(reports.decision(for: timedOut, from: beforeSuspension), .drop(.fromEarlierEpoch))
    }

    /// The order can also be the other way round: the old request fails
    /// before the slow foreground refresh has answered. Still no alert.
    func testSuspendedRequestFailingBeforeTheForegroundRefreshAnswersShowsNoTimeout() {
        var reports = ConnectionErrorReports()
        let beforeSuspension = reports.begin(.history)
        reports.sceneChanged(foreground: false)
        reports.sceneChanged(foreground: true)
        _ = reports.begin(.history)

        XCTAssertEqual(reports.decision(for: timedOut, from: beforeSuspension), .drop(.fromEarlierEpoch))
    }

    /// A push wake starts a refresh in the background and iOS suspends the app
    /// with it in flight.
    func testFailureWhileInTheBackgroundIsNotShown() {
        var reports = ConnectionErrorReports()
        reports.sceneChanged(foreground: false)
        let pushRefresh = reports.begin(.history)

        XCTAssertEqual(reports.decision(for: URLError(.networkConnectionLost), from: pushRefresh), .drop(.inBackground))
    }

    func testTransportFailureSupersededByANewerSuccessIsNotShown() {
        var reports = ConnectionErrorReports()
        let older = reports.begin(.history)
        let newer = reports.begin(.history)
        reports.succeeded(newer)

        XCTAssertEqual(reports.decision(for: timedOut, from: older), .drop(.superseded))
    }

    func testCancellationIsNeverShown() {
        var reports = ConnectionErrorReports()
        let ticket = reports.begin(.presence)

        XCTAssertEqual(reports.decision(for: CancellationError(), from: ticket), .drop(.cancelled))
        XCTAssertEqual(reports.decision(for: URLError(.cancelled), from: ticket), .drop(.cancelled))
    }

    /// A service that does not answer within the request timeout, with the
    /// app on screen the whole time, still raises the alert.
    func testUnreachableServiceStillShowsTheTimeout() {
        var reports = ConnectionErrorReports()
        let ticket = reports.begin(.presence)

        XCTAssertEqual(reports.decision(for: timedOut, from: ticket), .show(timedOut.localizedDescription))
    }

    func testOfflineAfterReturningStillShowsTheError() {
        var reports = ConnectionErrorReports()
        reports.sceneChanged(foreground: false)
        reports.sceneChanged(foreground: true)
        let onReturn = reports.begin(.presence)
        let offline = URLError(.notConnectedToInternet)

        XCTAssertEqual(reports.decision(for: offline, from: onReturn), .show(offline.localizedDescription))
    }

    /// A success of the other operation does not hide a presence failure.
    func testSuccessOfAnotherOperationDoesNotSupersede() {
        var reports = ConnectionErrorReports()
        let presence = reports.begin(.presence)
        let history = reports.begin(.history)
        reports.succeeded(history)

        XCTAssertEqual(reports.decision(for: timedOut, from: presence), .show(timedOut.localizedDescription))
    }

    /// A server answer is not a transport failure: it is shown even when a
    /// newer request of the same kind succeeded.
    func testServerErrorIsShownEvenWhenSuperseded() {
        var reports = ConnectionErrorReports()
        let older = reports.begin(.presence)
        reports.succeeded(reports.begin(.presence))
        let failure = RelayAPIError(message: "Unauthorized", status: 401, code: "unauthorized")

        XCTAssertEqual(reports.decision(for: failure, from: older), .show(failure.localizedDescription))
    }

    /// Control Center and the app switcher make the scene inactive, not
    /// background; requests keep running there.
    func testInactiveIsNotASuspension() {
        var reports = ConnectionErrorReports()
        let ticket = reports.begin(.presence)
        reports.sceneChanged(foreground: true)

        XCTAssertEqual(reports.decision(for: timedOut, from: ticket), .show(timedOut.localizedDescription))
    }

    func testDiagnosticNamesCodesOnly() {
        let secret = "https://relay.example/v1/agents?token=s3cret"
        let url = URLError(.timedOut, userInfo: [NSURLErrorFailingURLStringErrorKey: secret])
        let api = RelayAPIError(message: "message text from a person", status: 503, code: "database_overloaded")

        let urlLine = ConnectionErrorReports.diagnostic(.presence, .fromEarlierEpoch, url)
        let apiLine = ConnectionErrorReports.diagnostic(.history, .superseded, api)

        XCTAssertTrue(urlLine.contains("URLError -1001"))
        XCTAssertFalse(urlLine.contains("s3cret"))
        XCTAssertFalse(urlLine.contains("relay.example"))
        XCTAssertTrue(apiLine.contains("HTTP 503"))
        XCTAssertFalse(apiLine.contains("message text"))
    }
}
