import RelayDesktopCore
import XCTest

final class HistoryRetentionCopyTests: XCTestCase {
    func testChoicesReadAsLinuxReadsThem() {
        XCTAssertEqual(HistoryRetentionCopy.choices.map(HistoryRetentionCopy.label),
                       ["7 days", "30 days (default)", "90 days", "Forever"])
    }

    func testAShortChoiceOrHeldUploadsSayHistoryIsKeptUntilItUploads() {
        XCTAssertEqual(HistoryRetentionCopy.warning(7, uploadsHeld: false),
                       "Unuploaded sessions are kept until delivery succeeds, so history may be retained longer than your selected limit.")
        for days in [30, 90, 0] {
            XCTAssertNil(HistoryRetentionCopy.warning(days, uploadsHeld: false), "\(days)")
        }
        for days in [7, 30, 90] {
            XCTAssertEqual(HistoryRetentionCopy.warning(days, uploadsHeld: true), HistoryRetentionCopy.note, "\(days)")
        }
        XCTAssertNil(HistoryRetentionCopy.warning(0, uploadsHeld: true))
    }

    func testAChoiceShowsAtOnceWhileItSaves() {
        let values = SettingsValues(allowSelfRegistration: true, autoActivate: true, historyRetentionDays: 30)
        let applied = values.applying(SettingsChange(historyRetentionDays: 7))
        XCTAssertEqual(applied.historyRetentionDays, 7)
        XCTAssertEqual(SettingsChange(historyRetentionDays: 0).fields, ["history_retention_days"])
    }
}
