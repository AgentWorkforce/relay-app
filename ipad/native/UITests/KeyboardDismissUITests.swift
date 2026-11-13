import XCTest

/// Ways to put the keyboard away. Mobile CI runs this class only when a
/// manual dispatch names it in `extra-ui-tests`.
final class KeyboardDismissUITests: XCTestCase {
    private func launch() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-RelayUITestConversations", "-RelayUITestReset"]
        app.launch()
        XCTAssertTrue(app.staticTexts["reviewer"].firstMatch.waitForExistence(timeout: 15))
        return app
    }

    private func openReviewerThread() -> XCUIApplication {
        let app = launch()
        let messages = app.buttons["Messages"].firstMatch
        XCTAssertTrue(messages.waitForExistence(timeout: 15))
        messages.tap()
        let reviewer = app.staticTexts["reviewer"].firstMatch
        XCTAssertTrue(reviewer.waitForExistence(timeout: 15))
        reviewer.tap()
        XCTAssertTrue(app.textFields["Message composer"].firstMatch.waitForExistence(timeout: 10))
        return app
    }

    /// Same focus signal as MobileHandoffUITests: the composer's inline
    /// Hide keyboard button exists only while messageFocused is true.
    private func focusComposer(in app: XCUIApplication) -> Bool {
        let composer = app.textFields["Message composer"].firstMatch
        for _ in 0..<2 {
            composer.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
            let focused = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
                app.buttons["Hide keyboard"].exists
            }, object: nil)
            if XCTWaiter.wait(for: [focused], timeout: 5) == .completed { return true }
        }
        return false
    }

    /// Waits for the dismiss control to go, then for the software keyboard
    /// to go when the simulator shows one. A simulator with a hardware
    /// keyboard attached never exposes app.keyboards, so the control is the
    /// proxy there.
    private func expectKeyboardHidden(_ control: XCUIElement, in app: XCUIApplication, softwareKeyboard: Bool) {
        expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: control)
        if softwareKeyboard {
            expectation(for: NSPredicate(format: "count == 0"), evaluatedWith: app.keyboards)
        }
        waitForExpectations(timeout: 5)
    }

    func testKeyboardToolbarHidesKeyboardRaisedBySearch() {
        let app = launch()
        let search = app.textFields.matching(NSPredicate(format: "placeholderValue BEGINSWITH 'Search'")).firstMatch
        XCTAssertTrue(search.waitForExistence(timeout: 5))
        search.tap()
        // Both layouts put an inline button in the search field. The phone
        // used a keyboard toolbar at its stack root until build 51, and that
        // bar also covered the composer of every conversation pushed on top.
        let hide = app.buttons["Hide keyboard inline"].firstMatch
        XCTAssertTrue(hide.waitForExistence(timeout: 5))
        let softwareKeyboard = app.keyboards.count > 0
        hide.tap()
        expectKeyboardHidden(hide, in: app, softwareKeyboard: softwareKeyboard)
    }

    func testTapOnEmptyThreadSpaceHidesKeyboard() {
        let app = openReviewerThread()
        guard focusComposer(in: app) else {
            XCTFail("Tapping the composer must give it keyboard focus")
            return
        }
        // Not scrollViews.firstMatch: the composer's multi-line text field is
        // a scroll view of its own, right above the keyboard.
        let thread = app.scrollViews["Conversation thread"]
        XCTAssertTrue(thread.waitForExistence(timeout: 5))
        let softwareKeyboard = app.keyboards.count > 0
        // Empty space: just below the last bubble when that fits in the
        // thread, otherwise 85% down, which is below the fixture's short
        // conversation and above the composer.
        let frame = thread.frame
        var point = CGPoint(x: frame.midX, y: frame.minY + frame.height * 0.85)
        let lastBubble = app.staticTexts["Review is done"].firstMatch
        if lastBubble.exists {
            let below = lastBubble.frame.maxY + 24
            if below < frame.maxY - 10 { point = CGPoint(x: frame.midX, y: below) }
        }
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "keyboard-dismiss-before-thread-tap"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        let frames = XCTAttachment(string: "thread frame \(frame); last bubble \(lastBubble.exists ? "\(lastBubble.frame)" : "absent"); tap \(point)")
        frames.name = "keyboard-dismiss-thread-tap-point"
        frames.lifetime = .keepAlways
        add(frames)
        thread.coordinate(withNormalizedOffset: .zero)
            .withOffset(CGVector(dx: point.x - frame.minX, dy: point.y - frame.minY)).tap()
        expectKeyboardHidden(app.buttons["Hide keyboard"], in: app, softwareKeyboard: softwareKeyboard)
    }

    /// Build 51 on iPhone: in a chat with no messages the keyboard covered
    /// most of the composer and left no visible way to put it away.
    func testEmptyThreadKeepsComposerAboveKeyboardAndCanHideIt() {
        let app = launch()
        let messages = app.buttons["Messages"].firstMatch
        XCTAssertTrue(messages.waitForExistence(timeout: 15))
        messages.tap()
        // The fixture serves no messages for the Sam Lee DM.
        let sam = app.staticTexts["Sam Lee"].firstMatch
        XCTAssertTrue(sam.waitForExistence(timeout: 15))
        sam.tap()
        let composer = app.textFields["Message composer"].firstMatch
        XCTAssertTrue(composer.waitForExistence(timeout: 10))
        guard focusComposer(in: app) else {
            XCTFail("Tapping the composer must give it keyboard focus")
            return
        }
        XCTAssertTrue(composer.isHittable)
        let keyboard = app.keyboards.firstMatch
        let softwareKeyboard = app.keyboards.count > 0
        if softwareKeyboard {
            XCTAssertLessThanOrEqual(composer.frame.maxY, keyboard.frame.minY + 1,
                                     "composer \(composer.frame) overlaps keyboard \(keyboard.frame)")
        }
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "keyboard-empty-thread-composer"
        screenshot.lifetime = .keepAlways
        add(screenshot)
        // The phone adds a navigation-bar button, which the keyboard never
        // covers; the wide layout keeps only the composer's own button.
        let hide = app.frame.width < 700
            ? app.buttons["Hide keyboard navigation"].firstMatch
            : app.buttons.matching(NSPredicate(format: "label == 'Hide keyboard' AND identifier != 'Hide keyboard navigation'")).firstMatch
        XCTAssertTrue(hide.waitForExistence(timeout: 5))
        XCTAssertTrue(hide.isHittable)
        hide.tap()
        expectKeyboardHidden(hide, in: app, softwareKeyboard: softwareKeyboard)
    }

    func testThreadButtonStillWorksWithKeyboardUp() {
        let app = openReviewerThread()
        guard focusComposer(in: app) else {
            XCTFail("Tapping the composer must give it keyboard focus")
            return
        }
        // The overview toggle sits in the thread content above the tap
        // target; it must keep its tap.
        let collapse = app.buttons["Collapse agent overview"].firstMatch
        XCTAssertTrue(collapse.waitForExistence(timeout: 5))
        let softwareKeyboard = app.keyboards.count > 0
        collapse.tap()
        XCTAssertTrue(app.buttons["Expand agent overview"].firstMatch.waitForExistence(timeout: 5))
        // The button took the tap, so the keyboard stays up.
        XCTAssertTrue(app.buttons["Hide keyboard"].exists)
        if softwareKeyboard { XCTAssertGreaterThan(app.keyboards.count, 0) }
    }
}
