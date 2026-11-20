import XCTest

/// The iPad sidebar toggle. Phones keep their navigation stack and have no
/// toggle, so this class skips below 700 points.
final class SidebarCollapseUITests: XCTestCase {
    private func launch(reset: Bool = false) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-RelayUITestConversations"]
        if reset { app.launchArguments.append("-RelayUITestReset") }
        app.launch()
        return app
    }

    private func attach(_ app: XCUIApplication, _ name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    private func waitUntilGone(_ element: XCUIElement, timeout: TimeInterval = 5) -> Bool {
        let gone = XCTNSPredicateExpectation(predicate: NSPredicate(format: "exists == false"), object: element)
        return XCTWaiter.wait(for: [gone], timeout: timeout) == .completed
    }

    func testToggleHidesTheSidebarAndRemembersTheChoice() throws {
        var app = launch(reset: true)
        let toggle = app.buttons["Toggle sidebar"].firstMatch
        XCTAssertTrue(app.buttons["Messages"].firstMatch.waitForExistence(timeout: 15))
        if app.frame.width < 700 {
            throw XCTSkip("The sidebar toggle is iPad-only; this simulator uses the phone layout")
        }
        // The rail's Settings button and the sidebar's Start an agent button
        // stand for the left side; the placeholder offers "New agent" instead.
        let rail = app.buttons["Settings"].firstMatch
        let sidebar = app.buttons["Start an agent"].firstMatch
        XCTAssertTrue(toggle.waitForExistence(timeout: 10))
        XCTAssertTrue(sidebar.waitForExistence(timeout: 10))
        XCTAssertTrue(rail.exists)
        XCTAssertTrue(app.staticTexts["Choose an agent"].exists)
        attach(app, "1-expanded")

        toggle.tap()
        XCTAssertTrue(waitUntilGone(sidebar))
        XCTAssertTrue(waitUntilGone(rail))
        XCTAssertTrue(toggle.exists)
        XCTAssertTrue(app.staticTexts["Choose an agent"].exists)
        attach(app, "2-collapsed")

        // Still collapsed after relaunch.
        app.terminate()
        app = launch()
        XCTAssertTrue(app.buttons["Toggle sidebar"].firstMatch.waitForExistence(timeout: 15))
        XCTAssertFalse(app.buttons["Start an agent"].firstMatch.exists)
        XCTAssertFalse(app.buttons["Settings"].firstMatch.exists)
        attach(app, "3-collapsed-after-relaunch")

        // Expanded again, and that is remembered too, so later tests start
        // with the sidebar on screen.
        app.buttons["Toggle sidebar"].firstMatch.tap()
        XCTAssertTrue(app.buttons["Start an agent"].firstMatch.waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["Settings"].firstMatch.exists)
        app.terminate()
        app = launch()
        XCTAssertTrue(app.buttons["Start an agent"].firstMatch.waitForExistence(timeout: 15))
        XCTAssertTrue(app.buttons["Toggle sidebar"].firstMatch.exists)

        // The toggle stays in the header of an open conversation.
        app.buttons["Messages"].firstMatch.tap()
        let reviewer = app.staticTexts["reviewer"].firstMatch
        XCTAssertTrue(reviewer.waitForExistence(timeout: 15))
        reviewer.tap()
        XCTAssertTrue(app.buttons["Toggle sidebar"].firstMatch.waitForExistence(timeout: 5))
        attach(app, "4-expanded-conversation")
    }
}
