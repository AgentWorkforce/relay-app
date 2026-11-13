import XCTest

final class MobileHandoffUITests: XCTestCase {
    private func open(scenario: String) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-RelayUITestConversations", "-RelayUITestReset", "-RelayUITestMobileHandoff", scenario]
        app.launch()
        let messages = app.buttons["Messages"].firstMatch
        XCTAssertTrue(messages.waitForExistence(timeout: 15))
        messages.tap()
        let reviewer = app.staticTexts["reviewer"].firstMatch
        XCTAssertTrue(reviewer.waitForExistence(timeout: 15))
        reviewer.tap()
        XCTAssertTrue(app.staticTexts["Reading this conversation sends no message."].waitForExistence(timeout: 10))
        XCTAssertFalse(app.descendants(matching: .any)["Mobile handoff status"].exists)
        return app
    }

    private func send(_ app: XCUIApplication) {
        let composer = app.textFields["Message composer"].firstMatch
        XCTAssertTrue(composer.waitForExistence(timeout: 10))
        let visible = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
            guard composer.exists else { return false }
            let frame = composer.frame
            return !frame.isEmpty && !frame.isInfinite && !frame.isNull &&
                app.frame.contains(CGPoint(x: frame.midX, y: frame.midY))
        }, object: nil)
        guard XCTWaiter.wait(for: [visible], timeout: 10) == .completed else {
            XCTFail("The message composer must have an onscreen center before sending")
            return
        }
        guard focusComposer(composer, in: app) else {
            XCTFail("Tapping the composer must give it keyboard focus before typing")
            return
        }
        let text = "Continue the task"
        composer.typeText(text)
        let send = app.buttons["Send message"].firstMatch
        // Resolve readiness in the accessibility query instead of reading
        // value/isEnabled inside a polling closure. A direct attribute lookup
        // can spend the entire wait retrying one stale iOS 26 element.
        let completeDraft = app.textFields.matching(identifier: "Message composer")
            .matching(NSPredicate(format: "value == %@", text)).firstMatch
        let enabledSend = app.buttons.matching(identifier: "Send message")
            .matching(NSPredicate(format: "enabled == true")).firstMatch
        let exists = NSPredicate(format: "exists == true")
        let draftReady = XCTNSPredicateExpectation(predicate: exists, object: completeDraft)
        let sendReady = XCTNSPredicateExpectation(predicate: exists, object: enabledSend)
        // Both conditions share the original deadline; neither types or sends.
        guard XCTWaiter.wait(for: [draftReady, sendReady], timeout: 10) == .completed else {
            XCTFail("The complete draft and enabled Send button must be ready before submission")
            return
        }
        send.tap()
        let outgoing = app.staticTexts[text].firstMatch
        let consumed = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
            outgoing.exists || (composer.exists && composer.value as? String != text)
        }, object: nil)
        if XCTWaiter.wait(for: [consumed], timeout: 5) != .completed {
            // The simulator reported a tap while this draft stayed untouched.
            // sendDraft clears the draft synchronously, before
            // its network task starts. Retry only an untouched, enabled draft;
            // never retry a pending or failed delivery that already consumed it.
            capture(app, "mobile-handoff-unconsumed-send-tap")
            guard composer.value as? String == text, !outgoing.exists, send.isEnabled else {
                XCTFail("The send state changed without an observable submitted message")
                return
            }
            send.tap()
        }
        XCTAssertTrue(outgoing.waitForExistence(timeout: 10))
        let queued = NSPredicate(format: "label == 'Queued'")
        expectation(for: queued, evaluatedWith: app.staticTexts["Delivery status"])
        waitForExpectations(timeout: 10)
    }

    private func focusComposer(_ composer: XCUIElement, in app: XCUIApplication) -> Bool {
        // SwiftUI's existing control appears only while messageFocused is true.
        // Wait for that focus transition, including when a simulator tap lands
        // during a layout change; do not type into an unfocused element.
        for _ in 0..<2 {
            // iOS 26 can reject a valid TextField activation point while its
            // measured frame is onscreen. Tap that center, then prove focus.
            composer.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
            let focused = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
                app.buttons["Hide keyboard"].exists
            }, object: nil)
            if XCTWaiter.wait(for: [focused], timeout: 5) == .completed { return true }
        }
        return false
    }

    private func capture(_ app: XCUIApplication, _ name: String) {
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = name
        screenshot.lifetime = .keepAlways
        add(screenshot)
    }

    func testReadingDoesNothingAndFirstMessageShowsConnecting() {
        let app = open(scenario: "connecting")
        send(app)
        XCTAssertTrue(app.staticTexts["Connecting to Claude…"].firstMatch.waitForExistence(timeout: 10))
        XCTAssertFalse(app.buttons["Authorize"].exists)
        capture(app, "mobile-handoff-connecting")
    }

    func testQueuedMessageDoesNotClaimAContinuationOrDelivery() {
        let app = open(scenario: "queued")
        send(app)
        XCTAssertFalse(app.descendants(matching: .any)["Mobile handoff status"].exists)
        XCTAssertFalse(app.staticTexts["Delivered"].exists)
        capture(app, "mobile-handoff-queued")
    }

    func testConfirmedContinuationKeepsTheSameConversation() {
        let app = open(scenario: "ready")
        send(app)
        XCTAssertTrue(app.staticTexts["Continuing in Claude"].firstMatch.waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts["reviewer"].firstMatch.exists)
        XCTAssertFalse(app.staticTexts["Delivered"].exists)
        capture(app, "mobile-handoff-resumed")
    }

    func testFailureExplainsTheProblemWithoutInventingPermission() {
        let app = open(scenario: "failed")
        send(app)
        XCTAssertTrue(app.staticTexts["Could not continue Claude"].firstMatch.waitForExistence(timeout: 10))
        XCTAssertTrue(app.staticTexts["Claude's mobile connection could not be confirmed. Check the message status before sending again."].exists)
        XCTAssertFalse(app.buttons["Authorize"].exists)
        capture(app, "mobile-handoff-failure")
    }
}
