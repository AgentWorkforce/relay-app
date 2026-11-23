import XCTest
@testable import RelayDesktopCore

final class SessionNavigationTests: XCTestCase {
    func testBuildsCloudSessionURLFromHostAndEncodesSessionAsOneComponent() {
        XCTAssertEqual(
            SessionNavigation.cloudURL(
                siteURL: "https://agentrelay.com/an/ignored/path?old=query",
                sessionId: "session/with spaces"
            )?.absoluteString,
            "https://agentrelay.com/cloud/dashboard/sessions/session%2Fwith%20spaces"
        )
    }

    func testRejectsNonWebAndEmptySessionURLs() {
        XCTAssertNil(SessionNavigation.cloudURL(siteURL: "file:///tmp", sessionId: "session"))
        XCTAssertNil(SessionNavigation.cloudURL(siteURL: "https://agentrelay.com", sessionId: ""))
    }

    func testOnlyUploadedSessionsHaveACloudPage() {
        let uploaded = Fixture.uploadSession(sessionId: "one", status: .uploaded)
        XCTAssertEqual(
            SessionNavigation.cloudURL(siteURL: "https://agentrelay.com", session: uploaded, workspaceID: "w")?.absoluteString,
            "https://agentrelay.com/cloud/dashboard/sessions/one?source=\(uploaded.source)&workspace=w"
        )
        XCTAssertNil(SessionNavigation.cloudURL(siteURL: "https://agentrelay.com",
                                                session: Fixture.uploadSession(sessionId: "one", status: .notShared),
                                                workspaceID: "w"))
    }

    func testCloudURLNamesSourceAndWorkspaceLikeDesktopCore() {
        XCTAssertEqual(
            SessionNavigation.cloudURL(siteURL: "https://agentrelay.com", sessionId: "a b", source: "cursor",
                                       workspaceID: "0f8fad5b-d9cb-469f-a165-70867728950e")?.absoluteString,
            "https://agentrelay.com/cloud/dashboard/sessions/a%20b?source=cursor&workspace=0f8fad5b-d9cb-469f-a165-70867728950e"
        )
        XCTAssertEqual(
            SessionNavigation.cloudURL(siteURL: "https://agentrelay.com", sessionId: "s", source: "a&b=c",
                                       workspaceID: " ")?.absoluteString,
            "https://agentrelay.com/cloud/dashboard/sessions/s?source=a%26b%3Dc"
        )
        XCTAssertEqual(
            SessionNavigation.cloudURL(siteURL: "https://agentrelay.com", sessionId: "s", workspaceID: "w")?.absoluteString,
            "https://agentrelay.com/cloud/dashboard/sessions/s?workspace=w"
        )
    }
}
