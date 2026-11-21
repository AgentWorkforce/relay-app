import RelayDesktopCore
import XCTest

final class QuestionEscalationCopyTests: XCTestCase {
    func testWordingMatchesLinux() {
        XCTAssertEqual(QuestionEscalationCopy.title, "Send agents' questions to you")
        XCTAssertEqual(
            QuestionEscalationCopy.detail,
            "While this is on, questions from the sessions Agent Relay can reach are sent to you over Agent Relay instead of being asked in the session. You answer by replying to them."
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
            "Ask agents' questions in the session again"
        )
        XCTAssertEqual(QuestionEscalationCopy.inbox("lead"), "Questions go to lead.")
        XCTAssertEqual(QuestionEscalationCopy.coverageTitle, "Which sessions this reaches")
        XCTAssertEqual(
            QuestionEscalationCopy.coverageClosing,
            "Where questions are not sent to you, they are asked in the session as usual."
        )
        XCTAssertEqual(
            [QuestionCoverageState.covered, .partial, .notCovered, .notApplicable]
                .map(QuestionEscalationCopy.coverageHeading),
            ["Sent to you", "Sent to you in some cases", "Asked in the session as usual", "Does not apply"]
        )
        XCTAssertNil(QuestionEscalationCopy.coverageHeading(.unknown("someday")))
        XCTAssertEqual(QuestionCoverageEntry(name: "Codex", reason: "Plan mode only.").line, "Codex: Plan mode only.")
        XCTAssertEqual(QuestionCoverageEntry(name: "Codex", reason: nil).line, "Codex")
    }

    /// The core's `KINDS` order, the names the app already shows for each
    /// kind (`SessionKind.label`) and the qualifiers, word for word as
    /// Linux's `KINDS`, `kind_label` and `question_coverage_qualifier`.
    func testNamesMatchLinux() {
        let names = QuestionCoverageList.order.map {
            [$0.rawValue, $0.label, QuestionEscalationCopy.qualifier($0) ?? ""]
        }
        XCTAssertEqual(names, [
            ["claude-terminal", "Claude Code", ""],
            ["claude-cloud", "Claude app", ""],
            ["codex", "Codex", ""],
            ["cursor-cli", "Cursor CLI", ""],
            ["copilot", "Copilot", ""],
            ["opencode", "OpenCode", "on Agent Relay's server"],
            ["opencode-open", "OpenCode", "in a terminal"],
            ["grok", "Grok", "run by Agent Relay"],
            ["grok-open", "Grok", "already open"],
            ["devin", "Devin", ""],
            ["droid", "Factory Droid", ""],
            ["pi", "Pi", ""],
            ["omp", "Oh My Pi", ""],
            ["muse-open", "Muse Code", ""],
            ["openclaw", "OpenClaw", ""],
            ["hermes", "Hermes", ""],
        ])
    }

    private func coverage(_ json: String) throws -> [String: QuestionCoverage] {
        try JSONDecoder().decode([String: QuestionCoverage].self, from: Data(json.utf8))
    }

    private func lines(_ groups: [QuestionCoverageGroup]) -> [[String]] {
        groups.map { [$0.heading] + $0.entries.map(\.line) }
    }

    func testCoverageAbsentOrEmptyIsNoList() throws {
        XCTAssertEqual(QuestionCoverageList.groups(nil), [])
        XCTAssertEqual(QuestionCoverageList.groups([:]), [])
    }

    func testCoverageGroupsByStateInTheCoreOrder() throws {
        let groups = QuestionCoverageList.groups(try coverage("""
        {"hermes": {"state": "not_applicable", "reason": "No dialog."},
         "pi": {"state": "not_covered", "reason": " Not yet. "},
         "codex": {"state": "partial", "reason": "Plan mode only."},
         "devin": {"state": "partial", "reason": "Hooks off until turned on."},
         "claude-terminal": {"state": "covered", "reason": "Through the hook."},
         "claude-cloud": {"state": "not_covered", "reason": "Runs in the cloud."}}
        """))
        XCTAssertEqual(lines(groups), [
            ["Sent to you", "Claude Code: Through the hook."],
            ["Sent to you in some cases", "Codex: Plan mode only.", "Devin: Hooks off until turned on."],
            ["Asked in the session as usual", "Claude app: Runs in the cloud.", "Pi: Not yet."],
            ["Does not apply", "Hermes: No dialog."],
        ])
        // A group with no entry is not shown.
        let coveredOnly = QuestionCoverageList.groups(try coverage("""
        {"codex": {"state": "covered", "reason": "Yes."}}
        """))
        XCTAssertEqual(lines(coveredOnly), [["Sent to you", "Codex: Yes."]])
    }

    /// The pinned core's shape: the Grok pair shares name, state and reason
    /// and is one entry; the OpenCode pair differs in reason, so each is
    /// shown with its qualifier.
    func testCoverageMergesASharedName() throws {
        let groups = QuestionCoverageList.groups(try coverage("""
        {"opencode": {"state": "not_covered", "reason": "Not yet: the server's events."},
         "opencode-open": {"state": "not_covered", "reason": "Not yet: the plugin."},
         "grok": {"state": "not_covered", "reason": "Not yet."},
         "grok-open": {"state": "not_covered", "reason": "Not yet."}}
        """))
        XCTAssertEqual(lines(groups), [[
            "Asked in the session as usual",
            "OpenCode (on Agent Relay's server): Not yet: the server's events.",
            "OpenCode (in a terminal): Not yet: the plugin.",
            "Grok: Not yet.",
        ]])
        let merged = QuestionCoverageList.groups(try coverage("""
        {"opencode": {"state": "not_covered", "reason": "Not yet."},
         "opencode-open": {"state": "not_covered", "reason": "Not yet."}}
        """))
        XCTAssertEqual(lines(merged), [["Asked in the session as usual", "OpenCode: Not yet."]])
    }

    /// Shaped like relay pull request #2234's table: the OpenCode kinds
    /// differ in state, so each is shown with its qualifier.
    func testCoverageKeepsASharedNameApartWithQualifiers() throws {
        let groups = QuestionCoverageList.groups(try coverage("""
        {"opencode": {"state": "partial", "reason": "On a server Agent Relay runs."},
         "opencode-open": {"state": "not_covered", "reason": "Not through the plugin yet."},
         "grok": {"state": "not_covered", "reason": "Not yet."},
         "grok-open": {"state": "not_covered", "reason": "Not yet."}}
        """))
        XCTAssertEqual(lines(groups), [
            ["Sent to you in some cases", "OpenCode (on Agent Relay's server): On a server Agent Relay runs."],
            ["Asked in the session as usual", "OpenCode (in a terminal): Not through the plugin yet.", "Grok: Not yet."],
        ])
        let reasons = QuestionCoverageList.groups(try coverage("""
        {"grok": {"state": "not_covered", "reason": "One."},
         "grok-open": {"state": "not_covered", "reason": "Two."}}
        """))
        XCTAssertEqual(lines(reasons), [[
            "Asked in the session as usual",
            "Grok (run by Agent Relay): One.",
            "Grok (already open): Two.",
        ]])
    }

    func testCoverageShowsAnUnknownKeyByItsKeyLast() throws {
        let groups = QuestionCoverageList.groups(try coverage("""
        {"zeta-cli": {"state": "covered", "reason": "New."},
         "alpha-cli": {"state": "covered", "reason": ""},
         "omp": {"state": "covered", "reason": "Through the extension."}}
        """))
        XCTAssertEqual(lines(groups), [
            ["Sent to you", "Oh My Pi: Through the extension.", "alpha-cli", "zeta-cli: New."],
        ])
    }

    /// A state this build does not know decodes, and its entry is left out.
    func testCoverageLeavesOutAnUnknownState() throws {
        let groups = QuestionCoverageList.groups(try coverage("""
        {"codex": {"state": "someday", "reason": "Later."},
         "devin": {"state": "partial", "reason": "Hooks."}}
        """))
        XCTAssertEqual(lines(groups), [["Sent to you in some cases", "Devin: Hooks."]])
        let onlyUnknown = QuestionCoverageList.groups(try coverage("""
        {"codex": {"state": "someday", "reason": "Later."}}
        """))
        XCTAssertEqual(onlyUnknown, [])
    }

    func testCoverageShowsABlankReasonAsTheNameAlone() throws {
        let groups = QuestionCoverageList.groups(try coverage("""
        {"codex": {"state": "partial", "reason": "  "}}
        """))
        XCTAssertEqual(groups, [QuestionCoverageGroup(
            heading: "Sent to you in some cases",
            entries: [QuestionCoverageEntry(name: "Codex", reason: nil)]
        )])
    }

    /// The list stands in for the status paragraph, on or off; without a
    /// map, or on a core without the setting, there is no list.
    func testRowShowsTheListInsteadOfTheStatus() throws {
        for reported in ["on", "off"] {
            let row = QuestionEscalationRow(try settings("""
            , "question_escalation": "\(reported)", "question_escalation_status": "Available for Claude Code.",
              "question_escalation_coverage": {"claude-terminal": {"state": "covered", "reason": "Through the hook."}}
            """))
            XCTAssertNil(row.status)
            XCTAssertEqual(lines(row.coverage), [["Sent to you", "Claude Code: Through the hook."]])
        }
        let empty = QuestionEscalationRow(try settings("""
        , "question_escalation": "on", "question_escalation_status": "Available for Claude Code.",
          "question_escalation_coverage": {}
        """))
        XCTAssertEqual(empty.status, "Available for Claude Code.")
        XCTAssertEqual(empty.coverage, [])
        let older = QuestionEscalationRow(try settings("""
        , "question_escalation_coverage": {"codex": {"state": "covered", "reason": "Yes."}}
        """))
        XCTAssertNil(older.enabled)
        XCTAssertEqual(older.coverage, [])
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
