import XCTest

/// An agent's chat shows its stored session next to its relay messages.
/// Everything comes from the synthetic Debug fixture; nothing is signed in.
final class SessionStreamUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func open(_ name: String, _ extra: [String] = []) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-RelayUITestConversations", "-RelayUITestReset"] + extra
        app.launch()
        if app.buttons["Messages"].waitForExistence(timeout: 15) { app.buttons["Messages"].firstMatch.tap() }
        let row = app.staticTexts[name].firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: 15))
        row.tap()
        return app
    }

    private func attach(_ app: XCUIApplication, _ name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    /// Swipes the thread down (toward older rows) until `element` shows.
    private func scrollUp(to element: XCUIElement, in app: XCUIApplication) -> Bool {
        let thread = app.scrollViews["Conversation thread"].firstMatch
        for _ in 0..<8 {
            if element.waitForExistence(timeout: 2) { return true }
            thread.swipeDown()
        }
        return element.exists
    }

    func testLoadingRowGivesWayToTheMergedTimeline() throws {
        let app = open("builder", ["-RelayUITestSessionEventsDelay"])
        // Relay messages show at once; the session is still loading, so the
        // empty text never shows.
        XCTAssertTrue(app.staticTexts["Ship it"].waitForExistence(timeout: 10))
        let loading = app.descendants(matching: .any).matching(identifier: "Conversation loading").firstMatch
        XCTAssertTrue(loading.exists)
        XCTAssertFalse(app.staticTexts["No messages yet. Send one to keep this agent moving."].exists)
        attach(app, "loading")

        XCTAssertTrue(app.staticTexts["Running the build checks now."].waitForExistence(timeout: 10))
        XCTAssertFalse(loading.exists)
        // The session's copy of "Ship it" is the relay message, shown once.
        XCTAssertEqual(app.staticTexts.matching(NSPredicate(format: "label == 'Ship it'")).count, 1)
        XCTAssertTrue(app.staticTexts["Delivery status"].exists)
        attach(app, "merged")
        // Scrolling up reads the older page: the first prompt, shown
        // without its harness preamble.
        XCTAssertTrue(scrollUp(to: app.staticTexts["Set up the release build"], in: app))
        XCTAssertFalse(app.staticTexts["<environment_context>"].exists)
        attach(app, "older-page")
    }

    func testToolCallsShowAsCompactRows() throws {
        let app = open("builder")
        let tool = app.descendants(matching: .any).matching(identifier: "Session tool row").firstMatch
        XCTAssertTrue(tool.waitForExistence(timeout: 10))
        XCTAssertTrue(tool.staticTexts["Bash"].exists)
        XCTAssertTrue(tool.staticTexts["completed"].exists)
        XCTAssertTrue(tool.staticTexts["App.swift, Build.swift"].exists)
        let more = tool.buttons["Show more"]
        XCTAssertTrue(more.exists)
        more.tap()
        XCTAssertTrue(tool.buttons["Show less"].waitForExistence(timeout: 5))
        attach(app, "tool-row")
    }

    func testRecentToolCallShowsWorking() throws {
        let app = open("builder", ["-RelayUITestSessionWorking"])
        let working = app.descendants(matching: .any).matching(identifier: "Agent working").firstMatch
        XCTAssertTrue(working.waitForExistence(timeout: 10))
        XCTAssertTrue(working.label.contains("Working: Bash"))
        attach(app, "working")
    }
}
