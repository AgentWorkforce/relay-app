import XCTest
@testable import AgentRelayiPad

@MainActor final class AgentRowSummaryLoaderTests: XCTestCase {
    /// A fetch the test answers by hand, one session at a time.
    @MainActor private final class Backend {
        var requested: [String] = []
        var allowed = true
        var now = Date(timeIntervalSince1970: 1_800_000_000)
        private var waiting: [String: CheckedContinuation<AgentRowSummary?, Never>] = [:]

        func fetch(_ id: String) async -> AgentRowSummary? {
            requested.append(id)
            return await withCheckedContinuation { waiting[id] = $0 }
        }

        var pending: Set<String> { Set(waiting.keys) }

        func answer(_ id: String, _ summary: AgentRowSummary?) {
            waiting.removeValue(forKey: id)?.resume(returning: summary)
        }
    }

    private func loader(_ backend: Backend, maxConcurrent: Int = 2) -> AgentRowSummaryLoader {
        AgentRowSummaryLoader(maxConcurrent: maxConcurrent, refreshInterval: 300, now: { backend.now },
                              canLoad: { backend.allowed }, fetch: { await backend.fetch($0) })
    }

    /// Lets the loader's tasks run up to their next suspension. The loader,
    /// the fetch and the test all run on the main actor, so each yield lets
    /// the queued main-actor jobs run; the margin covers task hops.
    private func settle() async {
        for _ in 0..<50 { await Task.yield() }
    }

    private func brief(_ title: String) -> AgentRowSummary { AgentRowSummary(title: title, line: "\(title) line") }

    func testLoadsOnlySessionsWhoseRowsAreVisible() async {
        let backend = Backend()
        let loader = loader(backend)
        loader.rowAppeared("row-a", sessionID: "a")
        loader.rowAppeared("row-person", sessionID: "  ")
        await settle()
        XCTAssertEqual(backend.requested, ["a"])
        backend.answer("a", brief("A"))
        await settle()
        XCTAssertEqual(loader.summary(for: "a"), brief("A"))
        XCTAssertEqual(backend.requested, ["a"])
        XCTAssertEqual(loader.running, 0)
    }

    func testAtMostTheCapRunAtOnceAndRowsThatLeftAreNeverRequested() async {
        let backend = Backend()
        let loader = loader(backend)
        for id in ["a", "b", "c", "d"] { loader.rowAppeared("row-\(id)", sessionID: id) }
        await settle()
        XCTAssertEqual(backend.requested, ["a", "b"])
        XCTAssertEqual(loader.running, 2)

        // `c` scrolls away before its turn.
        loader.rowDisappeared("row-c")
        backend.answer("a", brief("A"))
        await settle()
        XCTAssertEqual(backend.requested, ["a", "b", "d"])
        XCTAssertEqual(loader.running, 2)

        backend.answer("b", brief("B"))
        backend.answer("d", brief("D"))
        await settle()
        XCTAssertEqual(backend.requested, ["a", "b", "d"])
        XCTAssertEqual(loader.running, 0)
        XCTAssertNil(loader.summary(for: "c"))
    }

    func testTwoRowsOfOneSessionShareOneLoadAndUpdatesAreReported() async {
        let backend = Backend()
        let loader = loader(backend)
        var updates: [String: AgentRowSummary] = [:]
        loader.onUpdate = { updates[$0] = $1 }
        loader.rowAppeared("relay:builder", sessionID: "a")
        loader.rowAppeared("conversation:builder", sessionID: "a")
        await settle()
        XCTAssertEqual(backend.requested, ["a"])
        // One of the two rows leaving keeps the session visible.
        loader.rowDisappeared("relay:builder")
        backend.answer("a", brief("A"))
        await settle()
        XCTAssertEqual(updates, ["a": brief("A")])
    }

    func testACachedSessionIsNotRequestedAgainUntilItIsOldEnough() async {
        let backend = Backend()
        let loader = loader(backend)
        loader.rowAppeared("row-a", sessionID: "a")
        await settle()
        backend.answer("a", brief("A"))
        await settle()

        // Scrolling away and back, and the periodic refresh, reuse the cache.
        loader.rowDisappeared("row-a")
        loader.rowAppeared("row-a", sessionID: "a")
        backend.now += 299
        loader.refresh()
        await settle()
        XCTAssertEqual(backend.requested, ["a"])

        backend.now += 1
        loader.refresh()
        await settle()
        XCTAssertEqual(backend.requested, ["a", "a"])
        backend.answer("a", brief("A2"))
        await settle()
        XCTAssertEqual(loader.summary(for: "a"), brief("A2"))
    }

    func testAnOldSessionIsNotRefreshedWhileItsRowIsOffScreen() async {
        let backend = Backend()
        let loader = loader(backend)
        loader.rowAppeared("row-a", sessionID: "a")
        await settle()
        backend.answer("a", brief("A"))
        await settle()
        loader.rowDisappeared("row-a")
        backend.now += 3600
        loader.refresh()
        await settle()
        XCTAssertEqual(backend.requested, ["a"])
        XCTAssertEqual(loader.summary(for: "a"), brief("A"))
    }

    func testAFailedLoadKeepsTheEarlierSummaryAndWaitsBeforeRetrying() async {
        let backend = Backend()
        let loader = loader(backend)
        var updates = 0
        loader.onUpdate = { _, _ in updates += 1 }
        loader.rowAppeared("row-a", sessionID: "a")
        loader.rowAppeared("row-b", sessionID: "b")
        await settle()
        backend.answer("a", brief("A"))
        backend.answer("b", nil)
        await settle()
        XCTAssertNil(loader.summary(for: "b"))
        XCTAssertEqual(updates, 1)

        // The failure is not retried on the next refresh.
        loader.refresh()
        await settle()
        XCTAssertEqual(backend.requested, ["a", "b"])

        backend.now += 300
        loader.refresh()
        await settle()
        XCTAssertEqual(backend.requested, ["a", "b", "a", "b"])
        backend.answer("a", nil)
        backend.answer("b", brief("B"))
        await settle()
        XCTAssertEqual(loader.summary(for: "a"), brief("A"))
        XCTAssertEqual(loader.summary(for: "b"), brief("B"))
        XCTAssertEqual(updates, 2)
    }

    func testNothingLoadsWhileLoadingIsNotAllowed() async {
        let backend = Backend()
        backend.allowed = false
        let loader = loader(backend)
        loader.rowAppeared("row-a", sessionID: "a")
        loader.refresh()
        await settle()
        XCTAssertEqual(backend.requested, [])

        // Summaries are enabled later: the row still on screen loads.
        backend.allowed = true
        loader.refresh()
        await settle()
        XCTAssertEqual(backend.requested, ["a"])
        backend.answer("a", brief("A"))
        await settle()
    }

    func testALoadThatReturnsAfterLoadingStoppedIsDropped() async {
        let backend = Backend()
        let loader = loader(backend)
        var updates = 0
        loader.onUpdate = { _, _ in updates += 1 }
        loader.rowAppeared("row-a", sessionID: "a")
        await settle()
        // Summaries are turned off while `a` is loading.
        backend.allowed = false
        backend.answer("a", brief("A"))
        await settle()
        XCTAssertNil(loader.summary(for: "a"))
        XCTAssertEqual(updates, 0)
        XCTAssertEqual(loader.running, 0)

        // Turned on again: the row still on screen loads at once.
        backend.allowed = true
        loader.refresh()
        await settle()
        XCTAssertEqual(backend.requested, ["a", "a"])
        backend.answer("a", brief("A"))
        await settle()
        XCTAssertEqual(loader.summary(for: "a"), brief("A"))
        XCTAssertEqual(updates, 1)
    }

    func testResetAtSignOutClearsTheCacheAndDropsLoadsInFlight() async {
        let backend = Backend()
        let loader = loader(backend)
        var updates: [String] = []
        loader.onUpdate = { id, _ in updates.append(id) }
        loader.rowAppeared("row-a", sessionID: "a")
        loader.rowAppeared("row-b", sessionID: "b")
        await settle()
        backend.answer("a", brief("A"))
        await settle()
        XCTAssertEqual(loader.summary(for: "a"), brief("A"))

        // Signed out while `b` is still loading.
        backend.allowed = false
        loader.reset()
        XCTAssertNil(loader.summary(for: "a"))
        backend.answer("b", brief("B"))
        await settle()
        XCTAssertNil(loader.summary(for: "b"))
        XCTAssertEqual(updates, ["a"])
        XCTAssertEqual(loader.running, 0)
        loader.refresh()
        await settle()
        XCTAssertEqual(backend.requested, ["a", "b"])

        // The next account loads afresh for the rows on screen.
        backend.allowed = true
        loader.refresh()
        await settle()
        XCTAssertEqual(backend.requested, ["a", "b", "a", "b"])
        backend.answer("a", brief("A2"))
        backend.answer("b", brief("B2"))
        await settle()
        XCTAssertEqual(loader.summary(for: "a"), brief("A2"))
    }

    func testALoadAbandonedByResetIsNotAnAttemptAndTheVisibleRowLoadsAgainAtOnce() async {
        let backend = Backend()
        let loader = loader(backend, maxConcurrent: 1)
        loader.rowAppeared("row-a", sessionID: "a")
        await settle()
        XCTAssertEqual(backend.requested, ["a"])

        // The session changes while `a` is loading (loading stays allowed);
        // the old load is discarded upstream and comes back nil.
        loader.reset()
        loader.refresh()
        await settle()
        // The abandoned load still holds the only slot.
        XCTAssertEqual(backend.requested, ["a"])
        backend.answer("a", nil)
        await settle()

        // Freed, the slot goes straight to the row still on screen: no
        // refresh call and no time passing.
        XCTAssertEqual(backend.requested, ["a", "a"])
        XCTAssertNil(loader.summary(for: "a"))
        backend.answer("a", brief("A"))
        await settle()
        XCTAssertEqual(loader.summary(for: "a"), brief("A"))
        XCTAssertEqual(loader.running, 0)
    }

    func testAnAbandonedLoadStillCountsAgainstTheCap() async {
        let backend = Backend()
        let loader = loader(backend)
        loader.rowAppeared("row-a", sessionID: "a")
        loader.rowAppeared("row-b", sessionID: "b")
        loader.rowAppeared("row-c", sessionID: "c")
        await settle()
        loader.reset()
        loader.refresh()
        await settle()
        // Both abandoned loads are still out, so nothing new starts.
        XCTAssertEqual(backend.requested, ["a", "b"])
        backend.answer("a", brief("A"))
        await settle()
        XCTAssertEqual(loader.running, 2)
        XCTAssertNil(loader.summary(for: "a"))
        XCTAssertEqual(backend.pending.count, 2)
        for id in backend.pending { backend.answer(id, nil) }
        await settle()
        for id in backend.pending { backend.answer(id, nil) }
        await settle()
        for id in backend.pending { backend.answer(id, nil) }
        await settle()
        XCTAssertEqual(loader.running, 0)
    }

    func testRowBriefDecodesTheAnalysisResponseForItsOwnSessionOnly() {
        let response: JSONObject = ["sessionId": "session-1", "requests": [JSONObject](),
                                    "analyses": [["id": "analysis-1", "sessionId": "session-1", "status": "completed",
                                                  "result": ["brief": ["title": "Brief title", "description": "Brief line"]]]]]
        XCTAssertEqual(RelayClient.rowSummary(from: response, sessionID: "session-1"),
                       AgentRowSummary(title: "Brief title", line: "Brief line"))
        XCTAssertNil(RelayClient.rowSummary(from: response, sessionID: "session-2"))
        XCTAssertNil(RelayClient.rowSummary(from: ["unexpected": true], sessionID: "session-1"))
        XCTAssertEqual(RelayClient.rowSummary(from: ["sessionId": "session-1", "requests": [JSONObject](),
                                                     "analyses": [JSONObject]()], sessionID: "session-1"),
                       AgentRowSummary())
    }
}
