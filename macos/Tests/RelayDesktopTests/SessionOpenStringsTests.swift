import Foundation
import XCTest
@testable import RelayDesktopCore

/// The macOS copy of the helper's fixed words is the helper's own: it must
/// equal `shared/session-open-strings.json` (`agent-relay-probe
/// session-open strings`, checked by desktop-core and GTK too).
final class SessionOpenStringsTests: XCTestCase {
    private func table() throws -> [String: String] {
        // Resolved, so a test runner that links this file in finds the same file.
        let url = URL(fileURLWithPath: #filePath).resolvingSymlinksInPath().deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("shared/session-open-strings.json")
        return try JSONDecoder().decode([String: String].self, from: Data(contentsOf: url))
    }

    func testTheFallbackIsTheCheckedInTable() throws {
        let fallback = try JSONDecoder().decode([String: String].self,
                                                from: JSONEncoder().encode(SessionOpen.Strings.fallback))
        XCTAssertEqual(fallback, try table(), "Update SessionOpen.Strings.fallback from shared/session-open-strings.json")
        XCTAssertEqual(Set(SessionOpen.Strings.CodingKeys.allCases.map(\.rawValue)), Set(try table().keys))
        // The table decodes to the fallback, word for word.
        let decoded = try JSONDecoder().decode(SessionOpen.Strings.self,
                                               from: JSONSerialization.data(withJSONObject: try table()))
        XCTAssertEqual(decoded, .fallback)
    }

    func testEveryFailureSaysOneOfTheSharedSentences() {
        let s = SessionOpen.Strings.fallback
        let cases: [(SessionOpenEngine.Failure, String)] = [
            (.unavailable, s.helperMissing),
            (.rejected, s.rejected),
            (.alreadyOpening, s.alreadyOpening),
            (.timedOut(.plan), s.planFailed),
            (.timedOut(.release), s.planFailed),
            (.timedOut(.open), s.openUnknown),
            (.noAnswer(.plan, 1), s.planFailed),
            (.noAnswer(.open, 1), s.openUnknown),
            (.malformedAnswer(.release), s.planFailed),
            (.malformedAnswer(.open), s.openUnknown),
            (.launchFailed(.plan), s.planFailed),
            (.launchFailed(.open), s.openNotStarted),
        ]
        for (failure, sentence) in cases {
            XCTAssertEqual(failure.sentence(s), sentence, "\(failure)")
            XCTAssertEqual(failure.localizedDescription, sentence, "\(failure)")
        }
        var other = s
        other.openUnknown = "Unbekannt."
        XCTAssertEqual(SessionOpenEngine.Failure.timedOut(.open).sentence(other), "Unbekannt.",
                       "The last answer's words win over the fallback")
    }

    /// Opt in like `SessionOpenEngineTests.testRealHelperPlansAMadeUpSession`.
    func testTheRealHelperPrintsTheSameTable() async throws {
        guard let path = ProcessInfo.processInfo.environment["RELAY_SESSION_OPEN_HELPER"] else {
            throw XCTSkip("Set RELAY_SESSION_OPEN_HELPER to the built agent-relay-desktop-probe")
        }
        let output = try await SessionOpenEngine.processRunner(executable: URL(fileURLWithPath: path))(
            ["session-open", "strings"], Data(), 8)
        XCTAssertEqual(output.status, 0)
        XCTAssertEqual(try JSONDecoder().decode([String: String].self, from: output.stdout), try table())
    }
}
