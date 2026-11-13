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
    func testSharedNavigationInputsDecodeWithoutASecondClientPlanner() throws {
        struct Expected: Decodable { let action: SessionOpenAction }
        struct Scenario: Decodable {
            let name: String
            let request: SessionOpenRequest
            let report: SessionLocationReport
            let expected: Expected
        }
        let file = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("shared/session-navigation-cases.json")
        let fixtures = try JSONDecoder().decode([Scenario].self, from: Data(contentsOf: file))
        XCTAssertGreaterThanOrEqual(fixtures.count, 12)
        let encoder = JSONEncoder(), decoder = JSONDecoder()
        for fixture in fixtures {
            XCTAssertEqual(try decoder.decode(SessionOpenRequest.self, from: encoder.encode(fixture.request)), fixture.request, fixture.name)
            XCTAssertEqual(try decoder.decode(SessionLocationReport.self, from: encoder.encode(fixture.report)), fixture.report, fixture.name)
            XCTAssertTrue(SessionOpenAction.allCases.contains(fixture.expected.action), fixture.name)
        }
        let selected = try XCTUnwrap(fixtures.first { $0.name == "selected-second-pane" }?.request.selected)
        XCTAssertEqual(selected.tty, "/dev/ttys005")
        XCTAssertEqual(selected.hostPid, 10)
        XCTAssertEqual(selected.bootId, "test-boot")
        XCTAssertFalse(fixtures[0].request.useDefaultTerminal)
    }

}
