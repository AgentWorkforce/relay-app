import XCTest

final class ConversationReadStateUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func attach(_ app: XCUIApplication, _ name: String) {
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    private func waitFor(_ format: String, _ element: XCUIElement, timeout: TimeInterval = 10) -> Bool {
        let expectation = XCTNSPredicateExpectation(predicate: NSPredicate(format: format), object: element)
        return XCTWaiter.wait(for: [expectation], timeout: timeout) == .completed
    }

    func testOpeningAnUnreadConversationPostsOneReadReceipt() {
        let app = XCUIApplication()
        app.launchArguments = ["-RelayUITestConversations", "-RelayUITestReset", "-RelayUITestNewMessage"]
        app.launch()
        if app.buttons["Messages"].waitForExistence(timeout: 15) { app.buttons["Messages"].firstMatch.tap() }
        let reviewer = app.staticTexts["reviewer"].firstMatch
        XCTAssertTrue(reviewer.waitForExistence(timeout: 15))

        // The server lists the reviewer's conversation as unread.
        let unread = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label CONTAINS 'Unread messages'")).firstMatch
        XCTAssertTrue(unread.waitForExistence(timeout: 15), "The unread conversation must show its marker")
        let posts = app.descendants(matching: .any)["ui-test-read-posts"].firstMatch
        XCTAssertTrue(posts.waitForExistence(timeout: 5))
        XCTAssertEqual(posts.label, "none", "Nothing is marked read before a conversation opens")
        attach(app, "1-unread")

        // Opening it marks its newest incoming message read, once.
        reviewer.tap()
        XCTAssertTrue(app.staticTexts["One more thing on the PR"].firstMatch.waitForExistence(timeout: 10))
        XCTAssertTrue(waitFor("label == 'm_reviewer_2'", posts), "Got read posts: \(posts.label)")
        attach(app, "2-opened")

        // Back on the list (a phone), or still open beside it (an iPad).
        if app.frame.width < 700 {
            app.navigationBars.buttons.element(boundBy: 0).tap()
            XCTAssertTrue(reviewer.waitForExistence(timeout: 10))
        }

        // A reload from the server keeps it read and posts nothing more.
        XCUIDevice.shared.press(.home)
        app.activate()
        XCTAssertTrue(reviewer.waitForExistence(timeout: 15))
        // Give the reload started by returning to the foreground time to land.
        Thread.sleep(forTimeInterval: 4)
        XCTAssertTrue(waitFor("exists == false", unread), "The marker must stay gone after a reload")
        XCTAssertEqual(posts.label, "m_reviewer_2", "Exactly one read receipt must be posted")
        attach(app, "3-read-after-reload")
    }
}
