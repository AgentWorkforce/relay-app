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
        // A run of tool work is one collapsed Activity group; a line opens its row.
        let group = app.buttons.matching(identifier: "Session activity group").firstMatch
        XCTAssertTrue(group.waitForExistence(timeout: 10))
        let tool = app.descendants(matching: .any).matching(identifier: "Session tool row").firstMatch
        XCTAssertFalse(tool.exists)
        group.tap()
        let line = app.buttons.matching(identifier: "Session activity line").firstMatch
        XCTAssertTrue(line.waitForExistence(timeout: 5))
        XCTAssertTrue(line.label.contains("make release"), line.label)
        line.tap()
        XCTAssertTrue(tool.waitForExistence(timeout: 5))
        XCTAssertTrue(tool.staticTexts["Bash"].exists)
        XCTAssertTrue(tool.staticTexts["completed"].exists)
        XCTAssertTrue(tool.staticTexts["App.swift, Build.swift"].exists)
        let more = tool.buttons["Show more"]
        XCTAssertTrue(more.exists)
        more.tap()
        XCTAssertTrue(tool.buttons["Show less"].waitForExistence(timeout: 5))
        attach(app, "tool-row")
    }

    func testRelayTrafficShowsAsMarkedMessages() throws {
        let app = open("builder")
        let sent = app.staticTexts["Release build is ready for review."]
        XCTAssertTrue(sent.waitForExistence(timeout: 10))
        // The send and Cloud's text copy of it show once, marked.
        XCTAssertEqual(app.staticTexts.matching(NSPredicate(format: "label == 'Release build is ready for review.'")).count, 1)
        let markers = app.staticTexts.matching(identifier: "Relay marker")
        XCTAssertTrue(markers.matching(NSPredicate(format: "label == '⇄ Agent Relay · to @scout'")).firstMatch.exists)
        // A message read from the inbox: only session text names its sender.
        XCTAssertTrue(app.staticTexts["Looks good from here."].exists)
        XCTAssertTrue(markers.matching(NSPredicate(format: "label == '⇄ Agent Relay'")).firstMatch.exists)
        XCTAssertFalse(app.staticTexts["Weighing whether to tag the release"].exists)
        attach(app, "relay-marked")
    }

    func testRecentToolCallShowsWorking() throws {
        let app = open("builder", ["-RelayUITestSessionWorking"])
        let working = app.descendants(matching: .any).matching(identifier: "Agent working").firstMatch
        XCTAssertTrue(working.waitForExistence(timeout: 10))
        XCTAssertTrue(working.label.contains("Working…"))
        attach(app, "working")
    }

    /// Opens the builder with an older page behind its first one, lets the
    /// page load on its own (its row is on screen in this short chat), and
    /// returns once it has: the "Load earlier activity" row is gone. No
    /// tap: the row can go between a check and the tap.
    private func loadOlderPage(_ argument: String, firstRow: (XCUIApplication) -> XCUIElement) -> XCUIApplication {
        let app = open("builder", [argument])
        XCTAssertTrue(firstRow(app).waitForExistence(timeout: 10))
        let older = app.buttons["Load earlier activity"]
        let gone = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: older)
        XCTAssertEqual(XCTWaiter.wait(for: [gone], timeout: 10), .completed, "the older page never loaded")
        return app
    }

    /// The row the reader was looking at is on screen, not pushed below it
    /// by the rows that loaded above.
    private func assertOnScreen(_ element: XCUIElement, in app: XCUIApplication) {
        let thread = app.scrollViews["Conversation thread"].firstMatch
        XCTAssertTrue(element.isHittable, "\(element.frame) is not on screen")
        XCTAssertTrue(thread.frame.intersects(element.frame), "\(element.frame) is outside \(thread.frame)")
    }

    func testOlderToolWorkJoiningTheTopGroupKeepsItInView() throws {
        // The older page's tool work joins the top group (three steps become
        // five) and more than a screen of notes lands above it.
        let app = loadOlderPage("-RelayUITestSessionOlderTools") {
            $0.buttons.matching(identifier: "Session activity group").firstMatch
        }
        let joined = app.buttons.matching(NSPredicate(format: "identifier == 'Session activity group' AND label CONTAINS '5 steps'")).firstMatch
        XCTAssertTrue(joined.waitForExistence(timeout: 10))
        assertOnScreen(joined, in: app)
        attach(app, "older-tools-joined")
    }

    func testOlderNotesKeepTheFirstReplyInView() throws {
        // More than a screen of notes lands above a plain reply.
        let app = loadOlderPage("-RelayUITestSessionOlderNotes") { $0.staticTexts["Picking up the lantern checks."] }
        assertOnScreen(app.staticTexts["Picking up the lantern checks."], in: app)
        attach(app, "older-notes")
    }
}
