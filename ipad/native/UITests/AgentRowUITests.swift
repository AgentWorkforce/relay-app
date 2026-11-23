import XCTest

/// The agent list rows: name with the agent's Working, Idle or Offline badge,
/// session title, one-line summary, and a caption with the CLI and the
/// machine; and the list's order. Fixture data only.
final class AgentRowUITests: XCTestCase {
    private func launch(_ arguments: [String] = []) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-RelayUITestConversations", "-RelayUITestReset"] + arguments
        app.launch()
        XCTAssertTrue(app.staticTexts["reviewer"].firstMatch.waitForExistence(timeout: 15))
        return app
    }

    func testRowShowsNameTitleSummaryCLIAndMachine() {
        let app = launch(["-RelayUITestRowSummaries"])
        XCTAssertTrue(app.staticTexts["Mobile release review"].firstMatch.waitForExistence(timeout: 10))
        // The brief's two lines are shown as one.
        XCTAssertTrue(app.staticTexts["Checked the release checklist and the CI status."].firstMatch.exists)
        XCTAssertTrue(app.staticTexts["Codex · Test Mini"].firstMatch.exists)
        XCTAssertTrue(app.staticTexts["builder"].firstMatch.exists)
        XCTAssertTrue(app.staticTexts["Claude Code · test-mini"].firstMatch.exists)

        // VoiceOver reads the row as one element, in a fixed order.
        let row = app.buttons["reviewer, Mobile release review, Codex, Test Mini, Idle"].firstMatch
        XCTAssertTrue(row.exists)
        XCTAssertGreaterThanOrEqual(row.frame.height, 72)

        // Search matches the loaded title and the CLI the caption names.
        // "Search" in the iPad sidebar, "Search agents" on iPhone.
        let search = app.textFields.matching(NSPredicate(format: "placeholderValue BEGINSWITH 'Search'")).firstMatch
        XCTAssertTrue(search.waitForExistence(timeout: 5))
        search.tap()
        search.typeText("release review")
        XCTAssertTrue(app.staticTexts["reviewer"].firstMatch.waitForExistence(timeout: 5))
        let builderCaption = app.staticTexts["Claude Code · test-mini"]
        wait(for: [expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: builderCaption)], timeout: 5)
    }

    func testRowKeepsRosterTextWhenNoSummaryIsLoaded() {
        let app = launch()
        XCTAssertTrue(app.staticTexts["Codex · Test Mini"].firstMatch.waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts["Claude Code · test-mini"].firstMatch.exists)
        XCTAssertFalse(app.staticTexts["Mobile release review"].exists)
        XCTAssertTrue(app.buttons["reviewer, Codex, Test Mini, Idle"].firstMatch.exists)
        // A person has no CLI, machine or session lines.
        XCTAssertTrue(app.buttons["Sam Lee, Online"].firstMatch.exists)
    }

    func testWorkingAgentLeadsThenTheRestByName() {
        // The roster lists builder, sam-lee, then reviewer; builder reports
        // a working turn and what it is doing, the others are only online.
        let app = launch(["-RelayUITestMixedPresence"])
        let builder = app.buttons["builder, Claude Code, test-mini, Working, Waiting"].firstMatch
        let reviewer = app.buttons["reviewer, Codex, Test Mini, Idle"].firstMatch
        let person = app.buttons["Sam Lee, Online"].firstMatch
        XCTAssertTrue(builder.waitForExistence(timeout: 10))
        XCTAssertTrue(reviewer.exists)
        XCTAssertTrue(person.exists)
        // With no brief loaded, what builder says it is doing is its summary.
        XCTAssertTrue(app.staticTexts["Running the fixture checks"].firstMatch.exists)
        let badge = { (label: String) in
            app.descendants(matching: .any)
                .matching(NSPredicate(format: "identifier == 'Agent row activity' AND label == %@", label)).firstMatch
        }
        XCTAssertTrue(badge("Working").exists)
        XCTAssertTrue(badge("Idle").exists)
        // Working first, then by name: reviewer before Sam Lee.
        XCTAssertLessThan(builder.frame.minY, reviewer.frame.minY)
        XCTAssertLessThan(reviewer.frame.minY, person.frame.minY)
    }
}
