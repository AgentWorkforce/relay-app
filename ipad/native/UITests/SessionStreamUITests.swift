import XCTest
import UIKit

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

    private func waitFor(_ format: String, _ element: XCUIElement, timeout: TimeInterval = 10) -> Bool {
        let expectation = XCTNSPredicateExpectation(predicate: NSPredicate(format: format), object: element)
        return XCTWaiter.wait(for: [expectation], timeout: timeout) == .completed
    }

    /// Why the working row is missing, recorded before the test fails: a
    /// screenshot, the element tree, and the facts that tell the causes apart.
    private func diagnoseMissingWorking(_ working: XCUIElement, in app: XCUIApplication) -> String {
        attach(app, "working-missing")
        let tree = XCTAttachment(string: app.debugDescription)
        tree.name = "working-missing-tree"
        tree.lifetime = .keepAlways
        add(tree)
        let newest = app.descendants(matching: .any)
            .matching(NSPredicate(format: "label CONTAINS 'All release checks passed.'")).firstMatch
        let group = app.descendants(matching: .any).matching(identifier: "Session activity group").firstMatch
        let thread = app.scrollViews["Conversation thread"].firstMatch
        let threadShown = thread.exists
        var facts = [
            "b5 'All release checks passed.' exists: \(newest.exists)",
            "Session activity group exists: \(group.exists)",
            "Conversation thread exists: \(threadShown)" + (threadShown ? ", frame \(thread.frame)" : ""),
            "window frame: \(app.windows.firstMatch.frame)",
        ]
        if threadShown && thread.isHittable {
            thread.swipeUp()
            facts.append("working after one swipeUp of the thread: \(working.waitForExistence(timeout: 3))")
        } else {
            facts.append("working after one swipeUp of the thread: not tried, the thread is not hittable")
        }
        let summary = facts.joined(separator: "; ")
        let text = XCTAttachment(string: summary)
        text.name = "working-missing-facts"
        text.lifetime = .keepAlways
        add(text)
        return summary
    }

    func testLoadingRowGivesWayToTheMergedTimeline() throws {
        let app = open("builder", ["-RelayUITestSessionEventsDelay"])
        // Relay messages show at once; the fixture holds the session's first
        // page until the test releases it, so the loading row shows and the
        // empty text never does.
        XCTAssertTrue(app.staticTexts["Ship it"].waitForExistence(timeout: 10))
        let loading = app.descendants(matching: .any).matching(identifier: "Conversation loading").firstMatch
        XCTAssertTrue(loading.waitForExistence(timeout: 10), "The loading row must show while the first page is held")
        XCTAssertFalse(app.staticTexts["No messages yet. Send one to keep this agent moving."].exists)
        attach(app, "loading")

        // Release the held page. A hold that ran out reads "timed out" and
        // fails here, rather than letting the timeline arrive on a timer.
        let release = app.buttons["ui-test-release-session-events"].firstMatch
        XCTAssertTrue(release.waitForExistence(timeout: 5), "The fixture's release control must show")
        XCTAssertEqual(release.value as? String, "held", "The fixture's hold ran out before the test released it")
        release.tap()
        if !waitFor("value == 'released'", release, timeout: 3) {
            release.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        }
        XCTAssertTrue(waitFor("value == 'released'", release, timeout: 5),
                      "The fixture's hold ran out before the test released it (control reads \((release.value as? String) ?? "nil"))")

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
        let shown = working.waitForExistence(timeout: 10)
        let facts = shown ? "" : diagnoseMissingWorking(working, in: app)
        XCTAssertTrue(shown, "The working row did not show. \(facts)")
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

    func testInlineScreenshotRendersPixelsAfterLaunchAndRelaunch() throws {
        let app = open("reviewer", ["-RelayUITestInlineScreenshot"])
        try assertInlineScreenshot(in: app, capture: "inline-screenshot-first-launch")
        app.terminate()

        // A new process has an empty AttachmentImageStore.
        let relaunched = open("reviewer", ["-RelayUITestInlineScreenshot"])
        try assertInlineScreenshot(in: relaunched, capture: "inline-screenshot-relaunch")
    }

    private func assertInlineScreenshot(in app: XCUIApplication, capture: String) throws {
        var captured: XCUIScreenshot?
        defer {
            let attachment = XCTAttachment(screenshot: captured ?? app.screenshot())
            attachment.name = capture
            attachment.lifetime = .keepAlways
            add(attachment)
        }

        let thread = app.scrollViews["Conversation thread"].firstMatch
        let caption = thread.staticTexts["Simulator screenshot fixture"].firstMatch
        let picture = thread.buttons.matching(NSPredicate(format: "label == %@", "inline-fixture.png")).firstMatch
        XCTAssertTrue(caption.waitForExistence(timeout: 15))
        XCTAssertTrue(picture.waitForExistence(timeout: 15))
        XCTAssertTrue(waitFor("hittable == true", picture))
        XCTAssertEqual(picture.label, "inline-fixture.png")

        let pictureFrame = picture.frame
        let appFrame = app.frame
        let visibleThread = thread.frame.intersection(appFrame)
        XCTAssertTrue(visibleThread.contains(pictureFrame), "Picture frame: \(pictureFrame), thread: \(visibleThread)")
        XCTAssertTrue(visibleThread.contains(caption.frame))
        XCTAssertLessThanOrEqual(caption.frame.maxY, pictureFrame.minY)

        let screenshot = app.screenshot()
        captured = screenshot
        let image = try XCTUnwrap(screenshot.image.cgImage)
        // XCUI frames are in points. Map them to raster pixels using the
        // app screenshot's dimensions on the full-screen simulator.
        let scaleX = CGFloat(image.width) / appFrame.width
        let scaleY = CGFloat(image.height) / appFrame.height
        // The 2:1 image is leading-aligned in a button that may be wider.
        let imageWidth = min(pictureFrame.width, pictureFrame.height * 2)
        XCTAssertGreaterThan(imageWidth, 40)
        XCTAssertGreaterThan(pictureFrame.height, 20)

        let colors = try [0.25, 0.75].map { (fraction: CGFloat) in
            let x = pictureFrame.minX + fraction * imageWidth
            let patch = CGRect(x: (x - 3 - appFrame.minX) * scaleX,
                               y: (pictureFrame.midY - 3 - appFrame.minY) * scaleY,
                               width: 6 * scaleX, height: 6 * scaleY).integral
            return try averageRGB(in: image, patch: patch)
        }
        let magenta = colors[0]
        let green = colors[1]
        XCTAssertTrue(magenta[0] > 200 && magenta[1] < 70 && magenta[2] > 100,
                      "Expected magenta on the left, observed RGB \(magenta)")
        XCTAssertTrue(green[0] < 70 && green[1] > 180 && (60...145).contains(green[2]),
                      "Expected green on the right, observed RGB \(green)")
    }

    private func averageRGB(in image: CGImage, patch: CGRect) throws -> [UInt8] {
        let cropped = try XCTUnwrap(image.cropping(to: patch))
        let colorSpace = try XCTUnwrap(CGColorSpace(name: CGColorSpace.sRGB))
        var pixel = [UInt8](repeating: 0, count: 4)
        try pixel.withUnsafeMutableBytes { bytes in
            let context = try XCTUnwrap(CGContext(data: bytes.baseAddress, width: 1, height: 1,
                                                 bitsPerComponent: 8, bytesPerRow: 4, space: colorSpace,
                                                 bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
                                                     | CGBitmapInfo.byteOrder32Big.rawValue))
            context.interpolationQuality = .high
            context.draw(cropped, in: CGRect(x: 0, y: 0, width: 1, height: 1))
        }
        return Array(pixel.prefix(3))
    }
}
