import XCTest

final class SummarySearchUITests: XCTestCase {
    private func launch(_ arguments: [String] = []) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-RelayUITestConversations", "-RelayUITestReset"] + arguments
        app.launch()
        XCTAssertTrue(app.staticTexts["reviewer"].firstMatch.waitForExistence(timeout: 15))
        let openSearch = app.buttons["Open summary search"]
        XCTAssertTrue(openSearch.waitForExistence(timeout: 5))
        openSearch.tap()
        XCTAssertTrue(app.textFields["Summary search field"].waitForExistence(timeout: 5))
        return app
    }

    private func search(_ query: String, in app: XCUIApplication) {
        let field = app.textFields["Summary search field"]
        field.tap()
        field.typeText(query)
    }

    func testSearchesSummariesAndOpensConversation() {
        let app = launch(["-RelayUITestSummarySearchDelay"])
        search("release", in: app)
        XCTAssertTrue(app.staticTexts["Searching summaries…"].waitForExistence(timeout: 2))

        let result = app.buttons["Summary result session-reviewer"]
        XCTAssertTrue(result.waitForExistence(timeout: 8))
        XCTAssertTrue(app.staticTexts["Confirmed the release checklist and CI status before distribution."].exists)
        result.tap()

        XCTAssertTrue(app.staticTexts["reviewer"].firstMatch.waitForExistence(timeout: 5))
        XCTAssertTrue(app.descendants(matching: .any).matching(identifier: "Message composer").firstMatch.exists)
    }

    func testEmptyErrorAndOfflineStates() {
        var app = launch()
        search("nothing matches this", in: app)
        XCTAssertTrue(app.staticTexts["No matching summaries"].waitForExistence(timeout: 5))

        app.terminate()
        app = launch(["-RelayUITestSummarySearchError"])
        search("release", in: app)
        XCTAssertTrue(app.staticTexts["Search unavailable"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["Try again"].exists)

        app.terminate()
        app = launch(["-RelayUITestSummarySearchOffline"])
        search("release", in: app)
        XCTAssertTrue(app.staticTexts["You're offline"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["Try again"].exists)
    }

    func testLoadsAnotherPageWithoutDuplicatingBoundaryResult() {
        let app = launch()
        search("release", in: app)
        let first = app.buttons["Summary result session-reviewer"]
        XCTAssertTrue(first.waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["Summary result session-builder"].exists)

        let more = app.buttons["Load more summary results"]
        XCTAssertTrue(more.waitForExistence(timeout: 5))
        more.tap()

        XCTAssertTrue(app.buttons["Summary result session-builder"].waitForExistence(timeout: 5))
        XCTAssertEqual(app.buttons.matching(identifier: "Summary result session-reviewer").count, 1)
        XCTAssertFalse(app.buttons["Load more summary results"].exists)
    }
}
