import RelayDesktopCore
import XCTest

final class SharingReplayCopyTests: XCTestCase {
    /// The question before a switch that re-sends the whole local history
    /// reads as Linux and Windows read it (`SHARING_REPLAY_*` in
    /// desktop-core's session model).
    func testTheReplayQuestionReadsAsLinuxReadsIt() {
        XCTAssertEqual(SharingMode.replayConfirmationTitle, "Share all sessions?")
        XCTAssertEqual(SharingMode.replayConfirmationDetail,
                       "Sharing all sessions re-sends your whole local history to Agent Relay Cloud. That can take hours, and new sessions wait behind it.")
        XCTAssertEqual(SharingMode.replayConfirmationAction, "Share all sessions")
    }
}
