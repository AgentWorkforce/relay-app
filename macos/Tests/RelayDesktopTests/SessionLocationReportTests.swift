import Foundation
import XCTest
@testable import RelayDesktopCore

final class SessionLocationReportTests: XCTestCase {
    /// `desktop-core/src/session_locations/tests.rs` serializes the same sample,
    /// so the generated Swift bridge retains the Rust wire format.
    func testDecodesTheSharedSample() throws {
        let file = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("shared/session-location-report.json")
        let report = try JSONDecoder().decode(SessionLocationReport.self, from: Data(contentsOf: file))
        XCTAssertEqual(report, SessionLocationReport(
            current: [SessionLocation(application: "Ghostty", bundleId: "com.mitchellh.ghostty", observedAtMs: 1000,
                                      pid: 30, processStart: 31, tty: "/dev/ttys004")],
            desktopLink: "claude://code/continue?session=local_exact",
            previous: [SessionLocation(application: "Claude", bundleId: "com.anthropic.claudefordesktop", observedAtMs: 900,
                                       pid: 40, processStart: 41)],
            running: true, supported: true))
    }
}
