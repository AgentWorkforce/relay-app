import XCTest

/// The agent list rows: name, session title, one-line summary, and a caption
/// with the CLI and the machine. Fixture data only.
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
        let row = app.buttons["reviewer, Mobile release review, Codex, Test Mini, Online"].firstMatch
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
        XCTAssertTrue(app.buttons["reviewer, Codex, Test Mini, Online"].firstMatch.exists)
        // A person has no CLI, machine or session lines.
        XCTAssertTrue(app.buttons["Sam Lee, Online"].firstMatch.exists)
    }
}
