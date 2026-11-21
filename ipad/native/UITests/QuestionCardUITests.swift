import XCTest

/// An agent's question shows as a card, and one tap sends one answer.
/// Everything comes from the synthetic Debug fixture; nothing is signed in.
final class QuestionCardUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func openQuestion() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-RelayUITestConversations", "-RelayUITestReset", "-RelayUITestQuestionCard"]
        app.launch()
        if app.buttons["Messages"].waitForExistence(timeout: 15) { app.buttons["Messages"].firstMatch.tap() }
        let row = app.staticTexts["planner"].firstMatch
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

    /// The accessibility tree and a screenshot, kept in the run's results,
    /// so a failure on CI shows what was on screen.
    private func explain(_ app: XCUIApplication, _ name: String) {
        let tree = XCTAttachment(string: app.debugDescription)
        tree.name = "\(name)-accessibility-tree"
        tree.lifetime = .keepAlways
        add(tree)
        attach(app, "\(name)-screen")
    }

    /// The message list only: the chat header repeats the last message's text.
    private func thread(_ app: XCUIApplication) -> XCUIElement {
        app.scrollViews["Conversation thread"].firstMatch
    }

    private func element(_ app: XCUIApplication, _ identifier: String) -> XCUIElement {
        app.descendants(matching: .any).matching(identifier: identifier).firstMatch
    }

    func testTheQuestionShowsAsACard() throws {
        let app = openQuestion()
        let card = element(app, "Question card")
        XCTAssertTrue(card.waitForExistence(timeout: 10))
        explain(app, "card")
        XCTAssertTrue(app.buttons["Option 1: Tuesday"].exists)
        XCTAssertTrue(app.buttons["Option 2: Thursday, after the freeze lifts"].exists)
        XCTAssertTrue(element(app, "Question free text 1").exists, "this question allows free text")
        let status = element(app, "Question state")
        XCTAssertTrue(status.label.hasPrefix("Expires in"), status.label)
        // The raw question text is not shown as a bubble in the thread.
        XCTAssertTrue(thread(app).exists)
        XCTAssertFalse(thread(app).staticTexts.matching(NSPredicate(format: "label BEGINSWITH '[Question '")).firstMatch.exists)
        attach(app, "question-card")
    }

    func testOneTapSendsOneAnswer() throws {
        let app = openQuestion()
        let option = app.buttons["Option 2: Thursday, after the freeze lifts"]
        XCTAssertTrue(option.waitForExistence(timeout: 10))
        option.tap()
        let status = element(app, "Question state")
        let sent = NSPredicate(format: "label == 'Answer sent.'")
        expectation(for: sent, evaluatedWith: status)
        waitForExpectations(timeout: 15)
        explain(app, "after-tap")
        // Exactly one answer line in the thread, sent as the person to this agent.
        // (The pending bubble gives way to the history row; wait for that.)
        let answers = thread(app).staticTexts.matching(NSPredicate(format: "label == 'answer q1a2b3c4d 2'"))
        expectation(for: NSPredicate(format: "count == 1"), evaluatedWith: answers)
        waitForExpectations(timeout: 10)
        // The card no longer sends.
        XCTAssertFalse(option.isEnabled)
        option.tap()
        XCTAssertEqual(thread(app).staticTexts.matching(NSPredicate(format: "label == 'answer q1a2b3c4d 2'")).count, 1)
        attach(app, "question-sent")
    }
}
