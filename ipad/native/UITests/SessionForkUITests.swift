import XCTest

final class SessionForkUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func launch(placement: String, oldComputer: Bool = false) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-RelayUITestConversations", "-RelayUITestReset", "-RelayUITestFork",
                               "-RelayForkPlacement", placement]
        if oldComputer { app.launchArguments.append("-RelayUITestForkOldComputer") }
        app.launch()
        XCTAssertTrue(app.staticTexts["planner"].firstMatch.waitForExistence(timeout: 15))
        return app
    }

    /// Seconds to linger on each step when recording the walkthrough.
    private func pause(_ seconds: Double = 1.2) {
        guard let pace = ProcessInfo.processInfo.environment["FORK_DEMO_PACE"], let scale = Double(pace) else { return }
        Thread.sleep(forTimeInterval: seconds * scale)
    }

    private func keep(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }

    private func openPlanner(_ app: XCUIApplication) {
        app.staticTexts["planner"].firstMatch.tap()
        XCTAssertTrue(app.staticTexts.containing(NSPredicate(format: "label CONTAINS 'Next up: step 3'")).firstMatch
                        .waitForExistence(timeout: 10))
    }

    func testPlacementHeaderButton() {
        let app = launch(placement: "header")
        openPlanner(app)
        XCTAssertTrue(app.buttons["Fork session"].waitForExistence(timeout: 5))
        keep(app, "fork-placement-1-header-button")
    }

    func testPlacementActionsMenu() {
        let app = launch(placement: "menu")
        openPlanner(app)
        XCTAssertFalse(app.buttons["Fork session"].exists)
        let menu = app.buttons["Agent actions"].firstMatch
        XCTAssertTrue(menu.waitForExistence(timeout: 5))
        menu.tap()
        XCTAssertTrue(app.buttons["Fork session"].waitForExistence(timeout: 5))
        keep(app, "fork-placement-2-actions-menu")
    }

    func testPlacementRowSwipe() {
        let app = launch(placement: "row")
        let row = app.staticTexts["planner"].firstMatch
        row.swipeRight()
        XCTAssertTrue(app.buttons["Fork"].waitForExistence(timeout: 5))
        keep(app, "fork-placement-3-row-swipe")
    }

    func testForkingContinuesTheSessionInANewAgent() {
        let app = launch(placement: "header")
        pause()
        openPlanner(app)
        pause(2.5)
        app.buttons["Fork session"].tap()
        let start = app.buttons["Start fork"]
        XCTAssertTrue(start.waitForExistence(timeout: 5))
        pause(2)
        keep(app, "fork-sheet")
        XCTAssertEqual(app.textFields["Fork name"].value as? String, "planner-fork")
        XCTAssertTrue(start.isEnabled)
        let task = app.textViews["Fork task"]
        task.tap()
        task.typeText("Take step 3, the payment sheet, in a new branch.")
        pause(1.5)
        start.tap()
        XCTAssertTrue(app.buttons.matching(NSPredicate(format: "label CONTAINS 'Forking'")).firstMatch
                        .waitForExistence(timeout: 5))
        keep(app, "fork-starting")
        let reply = app.staticTexts.containing(NSPredicate(format: "label CONTAINS %@", "Picked up from planner's checkout plan")).firstMatch
        XCTAssertTrue(reply.waitForExistence(timeout: 30))
        pause(3)
        keep(app, "fork-conversation")
        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(app.staticTexts["planner-fork"].firstMatch.waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts["planner"].firstMatch.exists)
        pause(3)
        keep(app, "fork-roster")
    }

    func testAComputerThatCannotForkSaysSo() {
        let app = launch(placement: "header", oldComputer: true)
        openPlanner(app)
        app.buttons["Fork session"].tap()
        XCTAssertTrue(app.staticTexts.containing(NSPredicate(format: "label CONTAINS 'can fork sessions'")).firstMatch
                        .waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["Start fork"].isEnabled)
        keep(app, "fork-unavailable")
    }
}
