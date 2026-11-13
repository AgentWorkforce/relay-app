import XCTest
@testable import AgentRelayiPad

final class CloudDeviceGrantTests: XCTestCase {
    func testProgressStatesKeepPolling() {
        XCTAssertEqual(CloudDeviceGrantPoll.classify(message: "authorization_pending", status: 400), .keepPolling)
        XCTAssertEqual(CloudDeviceGrantPoll.classify(message: "slow_down", status: 400), .slowDown)
        XCTAssertEqual(CloudDeviceGrantPoll.classify(message: "Too many token requests", status: 429), .keepPolling)
        XCTAssertEqual(CloudDeviceGrantPoll.classify(message: "Unable to complete device authorization", status: 503), .keepPolling)
        XCTAssertEqual(CloudDeviceGrantPoll.classify(message: "Agent Relay returned HTTP 502", status: 502), .keepPolling)
    }

    func testDeadDeviceCodesStartOverWithReadableMessage() {
        for code in ["invalid_grant", "expired_token", "access_denied"] {
            guard case .startOver(let message) = CloudDeviceGrantPoll.classify(message: code, status: 400) else {
                return XCTFail("\(code) should start sign-in over")
            }
            XCTAssertFalse(message.contains("_"), "\(code) should not show the raw OAuth code")
        }
    }

    func testOtherErrorsFail() {
        XCTAssertEqual(CloudDeviceGrantPoll.classify(message: "invalid_request", status: 400), .fail)
    }
}
