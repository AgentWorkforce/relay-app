import XCTest
@testable import RelayDesktopCore

final class MobileNativeContinuationTests: XCTestCase {
    func testUnifiedMobileContinuationChangesBothProviderPermissions() throws {
        let values = try JSONDecoder().decode(SettingsValues.self, from: Data(#"{"allow_self_registration":true,"auto_activate":false,"mobile_claude_handoff":true}"#.utf8))
        XCTAssertNil(values.mobileNativeContinuation)
        XCTAssertNil(values.mobileNativeContinuationSupported)
        XCTAssertTrue(MobileNativeContinuationCopy.isEnabled(values))
        let legacyChange = MobileNativeContinuationCopy.change(enabled: false, settings: values)
        XCTAssertEqual(legacyChange.fields, ["mobile_claude_handoff"])
        XCTAssertEqual(values.applying(legacyChange).mobileClaudeHandoff, false)

        let current = try JSONDecoder().decode(SettingsValues.self, from: Data(#"{"allow_self_registration":true,"auto_activate":false,"mobile_claude_handoff":false,"mobile_native_continuation":false}"#.utf8))
        let change = MobileNativeContinuationCopy.change(enabled: true, settings: current)
        XCTAssertEqual(change.fields, ["mobile_claude_handoff", "mobile_native_continuation"])
        XCTAssertEqual(current.applying(change).mobileNativeContinuation, true)
        XCTAssertEqual(current.applying(change).mobileClaudeHandoff, true)
        let disabled = MobileNativeContinuationCopy.change(enabled: false, settings: current.applying(change))
        XCTAssertEqual(current.applying(change).applying(disabled).mobileNativeContinuation, false)
        XCTAssertEqual(current.applying(change).applying(disabled).mobileClaudeHandoff, false)
        XCTAssertFalse(MobileNativeContinuationCopy.isEnabled(current.applying(change).applying(disabled)))
    }

    func testUnsupportedAndCopyDescribeActualLimits() throws {
        let values = try JSONDecoder().decode(SettingsValues.self, from: Data(#"{"allow_self_registration":true,"auto_activate":false,"mobile_native_continuation":false,"mobile_native_continuation_supported":false}"#.utf8))
        XCTAssertEqual(values.mobileNativeContinuation, false)
        XCTAssertEqual(values.mobileNativeContinuationSupported, false)
        XCTAssertFalse(MobileNativeContinuationCopy.isSupported(values))
        XCTAssertEqual(MobileNativeContinuationCopy.title, "Continue agents from mobile")
        XCTAssertTrue(MobileNativeContinuationCopy.detail.hasPrefix("On unless turned off."))
        XCTAssertTrue(MobileNativeContinuationCopy.waiting.contains("ownership"))
        XCTAssertTrue(MobileNativeContinuationCopy.detail.contains("Claude Code 2.1.310"))
        XCTAssertTrue(MobileNativeContinuationCopy.detail.contains("command and file approval requests"))
        XCTAssertTrue(MobileNativeContinuationCopy.detail.contains("may finish after you turn this off"))
    }
}
