import RelayDesktopCore
import XCTest

final class QuestionEscalationCopyTests: XCTestCase {
    func testWordingMatchesLinux() {
        XCTAssertEqual(QuestionEscalationCopy.title, "Send agents' questions to you")
        XCTAssertEqual(
            QuestionEscalationCopy.detail,
            "While this is on, a question an agent asks is sent to you over Agent Relay instead of being shown in the agent's own window. You answer by replying to it."
        )
        XCTAssertEqual(
            QuestionEscalationCopy.unavailable,
            "Not available: this version of Agent Relay's background service does not have this setting. Update Agent Relay to use it."
        )
        XCTAssertEqual(
            QuestionEscalationCopy.turnOnHelp,
            "Send agents' questions to you over Agent Relay"
        )
        XCTAssertEqual(
            QuestionEscalationCopy.turnOffHelp,
            "Show agents' questions in their own window again"
        )
        XCTAssertEqual(QuestionEscalationCopy.inbox("lead"), "Questions go to lead.")
    }

    private func settings(_ extra: String) throws -> SettingsValues {
        try JSONDecoder().decode(SettingsValues.self, from: Data("""
        {"allow_self_registration": true, "auto_activate": true\(extra)}
        """.utf8))
    }

    /// The row shows only what the core reports: an older core without the
    /// key reads as unavailable, and either reported value shows as given.
    func testRowShowsOnlyWhatTheCoreReports() throws {
        XCTAssertEqual(
            QuestionEscalationRow(try settings("")),
            QuestionEscalationRow(enabled: nil, status: nil, inbox: nil)
        )
        XCTAssertEqual(
            QuestionEscalationRow(nil),
            QuestionEscalationRow(enabled: nil, status: nil, inbox: nil)
        )
        for (reported, enabled) in [("off", false), ("on", true)] {
            let row = QuestionEscalationRow(try settings("""
            , "question_escalation": "\(reported)", "question_escalation_status": "Covers Claude Code sessions."
            """))
            XCTAssertEqual(row.enabled, enabled)
            XCTAssertEqual(row.status, "Covers Claude Code sessions.")
            XCTAssertNil(row.inbox)
        }
        let routed = QuestionEscalationRow(try settings("""
        , "question_escalation": "on", "question_escalation_inbox": "lead"
        """))
        XCTAssertNil(routed.status)
        XCTAssertEqual(routed.inbox, "Questions go to lead.")
        let empty = QuestionEscalationRow(try settings("""
        , "question_escalation": "off", "question_escalation_inbox": "", "question_escalation_status": " "
        """))
        XCTAssertNil(empty.inbox)
        XCTAssertNil(empty.status)
    }
}
