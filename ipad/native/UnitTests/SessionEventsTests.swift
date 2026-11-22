import XCTest
@testable import AgentRelayiPad

final class SessionEventsTests: XCTestCase {
    private let base = Date(timeIntervalSince1970: 1_790_000_000)

    private func event(_ id: String, at seconds: TimeInterval, kind: String = "assistant", role: String = "assistant",
                       content: String = "text", tool: String = "", source: String = "codex") -> SessionEvent {
        SessionEvent(key: .init(source: source, machineID: "m1", kind: kind, eventID: id),
                     ts: base.addingTimeInterval(seconds), kind: kind, actorRole: role, content: content, toolName: tool)
    }

    private func page(_ events: [SessionEvent], next: String?, session: String = "s1") -> SessionEventPage {
        SessionEventPage(sessionID: session, events: events, nextCursor: next)
    }

    // MARK: Decode

    func testDecodesContractFields() throws {
        let data = Data(#"{"sessionId":"s1","nextCursor":"c2","events":[{"eventId":"e1","ts":"2026-11-20T10:00:00.123Z","kind":"tool_use","type":"tool","actorRole":"assistant","actorName":"agent","content":"ls","contentTruncated":true,"toolName":"Bash","toolStatus":"completed","filesTouched":["a/b.swift"],"machineId":"m1","source":"codex","subagentId":"sub"}]}"#.utf8)
        let page = try SessionEventPage.decode(data, sessionID: "s1")
        XCTAssertEqual(page.nextCursor, "c2")
        let event = try XCTUnwrap(page.events.first)
        XCTAssertEqual(event.key, .init(source: "codex", machineID: "m1", kind: "tool_use", eventID: "e1"))
        XCTAssertEqual(event.toolName, "Bash")
        XCTAssertEqual(event.toolStatus, "completed")
        XCTAssertEqual(event.filesTouched, ["a/b.swift"])
        XCTAssertTrue(event.contentTruncated)
        XCTAssertEqual(event.subagentID, "sub")
        XCTAssertEqual(event.role, .tool)
        XCTAssertEqual(event.ts?.timeIntervalSince1970 ?? 0, 1_795_168_800.123, accuracy: 0.001)
    }

    func testDecodeIsLenientAboutEventFields() throws {
        // Mistyped and missing fields, an unknown kind, a non-object entry
        // and an event with neither id nor time: the page survives.
        let data = Data(#"{"sessionId":"s1","nextCursor":null,"events":[{"eventId":"e1","kind":"brand_new_kind","content":7,"filesTouched":["x",3,null],"contentTruncated":"yes"},"junk",{"kind":"assistant","content":"no key"},{"kind":"assistant","ts":"2026-11-20T10:00:00Z","content":"keyed by time"},{"eventId":42,"ts":1795168800000}]}"#.utf8)
        let page = try SessionEventPage.decode(data, sessionID: "s1")
        XCTAssertNil(page.nextCursor)
        XCTAssertEqual(page.events.count, 3)
        XCTAssertEqual(page.events[0].role, .activity)
        XCTAssertEqual(page.events[0].content, "7")
        XCTAssertEqual(page.events[0].filesTouched, ["x"])
        XCTAssertFalse(page.events[0].contentTruncated)
        XCTAssertTrue(page.events[1].key.eventID.hasPrefix("ts:"))
        XCTAssertEqual(page.events[2].key.eventID, "42")
        XCTAssertNotNil(page.events[2].ts)
        // The same unkeyed event read again gets the same key.
        XCTAssertEqual(try SessionEventPage.decode(data, sessionID: "s1").events[1].key, page.events[1].key)
    }

    func testDecodeRejectsMalformedBodiesAndAnotherSession() {
        for body in ["[]", "{}", "not json", #"{"sessionId":"s1"}"#, #"{"sessionId":"s1","events":{}}"#] {
            XCTAssertThrowsError(try SessionEventPage.decode(Data(body.utf8), sessionID: "s1")) {
                XCTAssertEqual($0 as? SessionEventPage.DecodeError, .malformed, body)
            }
        }
        XCTAssertThrowsError(try SessionEventPage.decode(Data(#"{"sessionId":"other","events":[]}"#.utf8), sessionID: "s1")) {
            XCTAssertEqual($0 as? SessionEventPage.DecodeError, .wrongSession)
        }
    }

    func testLongContentIsCutInMemory() {
        let long = String(repeating: "a", count: SessionEvent.maximumStoredContent + 10)
        let event = SessionEvent(key: .init(source: "", machineID: "", kind: "", eventID: "e"), ts: nil, content: long)
        XCTAssertEqual(event.content.count, SessionEvent.maximumStoredContent)
        XCTAssertTrue(event.contentTruncated)
    }

    func testRoles() {
        XCTAssertEqual(event("a", at: 0, kind: "prompt", role: "").role, .person)
        XCTAssertEqual(event("a", at: 0, kind: "x", role: "user").role, .person)
        XCTAssertEqual(event("a", at: 0, kind: "message", role: "assistant").role, .agent)
        XCTAssertEqual(event("a", at: 0, kind: "tool_result", role: "user").role, .tool)
        XCTAssertEqual(event("a", at: 0, kind: "x", role: "tool_result").role, .tool)
        XCTAssertEqual(event("a", at: 0, kind: "x", role: "assistant", tool: "Read").role, .tool)
        XCTAssertEqual(event("a", at: 0, kind: "x", role: "").role, .activity)
        XCTAssertEqual(event("a", at: 0, kind: "reasoning", role: "assistant").role, .activity)
    }

    // MARK: Request

    func testRequestPath() {
        let ws = "9F1C2D3E-4B5A-4C6D-8E7F-901234567890"
        XCTAssertEqual(SessionEventsRequest.path(workspaceID: ws, sessionID: "s 1/x", spec: SessionEventsRequest.newest),
                       "/workspaces/9f1c2d3e-4b5a-4c6d-8e7f-901234567890/reflex/sessions/s%201%2Fx/events?limit=100&order=desc")
        XCTAssertEqual(SessionEventsRequest.path(workspaceID: ws, sessionID: "s1",
                                                 spec: .init(order: .desc, limit: 5000, cursor: "a+b/c=")),
                       "/workspaces/9f1c2d3e-4b5a-4c6d-8e7f-901234567890/reflex/sessions/s1/events?limit=1000&order=desc&cursor=a%2Bb%2Fc%3D")
        XCTAssertNil(SessionEventsRequest.path(workspaceID: "not-a-uuid", sessionID: "s1", spec: SessionEventsRequest.newest))
        XCTAssertNil(SessionEventsRequest.path(workspaceID: ws, sessionID: "", spec: SessionEventsRequest.newest))
        XCTAssertNil(SessionEventsRequest.path(workspaceID: ws, sessionID: String(repeating: "x", count: 513),
                                               spec: SessionEventsRequest.newest))
    }

    // MARK: Store

    func testMergeDedupesByKeyAndOrdersByTimeThenKey() {
        var store = SessionEventStore(sessionID: "s1")
        store.apply(page([event("b", at: 10), event("a", at: 10), event("c", at: 5)], next: "older"), for: SessionEventsRequest.newest)
        // A poll that repeats one event (updated) and adds a newer one.
        var updated = event("a", at: 10, content: "edited")
        store.apply(page([event("d", at: 20), updated], next: "older2"), for: SessionEventsRequest.newest)
        XCTAssertEqual(store.events.map(\.key.eventID), ["c", "a", "b", "d"])
        XCTAssertEqual(store.events[1].content, "edited")
        // The same event id from another source is another event.
        updated = event("a", at: 10, source: "claude")
        store.apply(page([event("d", at: 20), updated], next: "older3"), for: SessionEventsRequest.newest)
        XCTAssertEqual(store.events.count, 5)
        XCTAssertEqual(store.olderCursor, "older", "an overlapping newest page keeps the older cursor")
        // A page for another session is ignored.
        store.apply(page([event("z", at: 1)], next: nil, session: "s2"), for: SessionEventsRequest.newest)
        XCTAssertEqual(store.events.count, 5)
    }

    func testPagingOlderHistoryWithTheDescCursor() throws {
        var store = SessionEventStore(sessionID: "s1")
        XCTAssertNil(store.olderRequest, "no older paging before the first page")
        XCTAssertFalse(store.hasOlder)
        store.apply(page([event("3", at: 30), event("4", at: 40)], next: "c1"), for: SessionEventsRequest.newest)
        XCTAssertTrue(store.loaded)
        let request = try XCTUnwrap(store.olderRequest)
        XCTAssertEqual(request, .init(order: .desc, limit: 100, cursor: "c1"))
        store.apply(page([event("2", at: 20)], next: "c2"), for: request)
        XCTAssertEqual(store.olderRequest?.cursor, "c2")
        store.apply(page([event("1", at: 10)], next: nil), for: try XCTUnwrap(store.olderRequest))
        XCTAssertTrue(store.reachedStart)
        XCTAssertNil(store.olderRequest)
        XCTAssertFalse(store.hasOlder)
        XCTAssertEqual(store.events.map(\.key.eventID), ["1", "2", "3", "4"])
    }

    func testANewestPageWithAGapRestartsOlderPaging() {
        var store = SessionEventStore(sessionID: "s1")
        store.apply(page([event("1", at: 10)], next: nil), for: SessionEventsRequest.newest)
        XCTAssertTrue(store.reachedStart)
        // A full newest page sharing nothing with what is held: there may be
        // events between, so its cursor is followed again.
        store.apply(page([event("8", at: 80), event("9", at: 90)], next: "gap"), for: SessionEventsRequest.newest)
        XCTAssertFalse(store.reachedStart)
        XCTAssertEqual(store.olderRequest?.cursor, "gap")
        XCTAssertEqual(store.events.map(\.key.eventID), ["1", "8", "9"])
    }

    func testLostCursorRestartsFromTheNewestPageAndKeepsEvents() throws {
        for (status, code) in [(410, "cursor_expired"), (409, "cursor_backend_transition")] {
            var store = SessionEventStore(sessionID: "s1")
            store.apply(page([event("5", at: 50)], next: "c1"), for: SessionEventsRequest.newest)
            store.apply(page([event("4", at: 40)], next: "c2"), for: try XCTUnwrap(store.olderRequest))
            store.failed(status: status, code: code)
            XCTAssertEqual(store.failures, 1)
            XCTAssertEqual(store.events.count, 2, "what is shown stays")
            XCTAssertEqual(store.olderRequest, SessionEventsRequest.newest, "restart from the newest page")
            store.apply(page([event("5", at: 50)], next: "fresh"), for: SessionEventsRequest.newest)
            XCTAssertEqual(store.failures, 0)
            XCTAssertEqual(store.olderRequest?.cursor, "fresh")
            XCTAssertEqual(store.events.count, 2)
        }
    }

    func testTransientFailureKeepsTheCursor() {
        var store = SessionEventStore(sessionID: "s1")
        store.apply(page([event("5", at: 50)], next: "c1"), for: SessionEventsRequest.newest)
        store.failed(status: 503, code: "")
        store.failed(status: 502, code: "")
        XCTAssertEqual(store.failures, 2)
        XCTAssertEqual(store.olderRequest?.cursor, "c1")
        XCTAssertEqual(store.events.count, 1)
    }

    func testSnapshotRoundTrips() throws {
        var store = SessionEventStore(sessionID: "s1")
        store.apply(page([event("2", at: 20), event("1", at: 10)], next: "c1"), for: SessionEventsRequest.newest)
        let data = try JSONEncoder().encode(store.snapshot)
        let restored = SessionEventStore(snapshot: try JSONDecoder().decode(SessionEventSnapshot.self, from: data))
        XCTAssertEqual(restored.events, store.events)
        XCTAssertEqual(restored.olderRequest, store.olderRequest)
        XCTAssertTrue(restored.loaded)
        XCTAssertEqual(restored.olderRequest?.cursor, "c1")
    }

    func testARestoredStoreWithoutACursorRestartsFromTheNewestPage() {
        let trimmed = SessionEventSnapshot(sessionID: "s1", events: [event("9", at: 90)], olderCursor: nil, reachedStart: false)
        var store = SessionEventStore(snapshot: trimmed)
        XCTAssertEqual(store.olderRequest, SessionEventsRequest.newest)
        // The newest page overlaps what was cached, and still sets the cursor.
        XCTAssertTrue(store.apply(page([event("9", at: 90)], next: "c9"), for: SessionEventsRequest.newest))
        XCTAssertEqual(store.olderRequest?.cursor, "c9")
        XCTAssertFalse(store.apply(page([event("9", at: 90)], next: "c9"), for: SessionEventsRequest.newest),
                       "an unchanged poll writes nothing")
    }

    func testAutomaticOlderLoadingStopsAfterAnEmptyPage() throws {
        var store = SessionEventStore(sessionID: "s1")
        store.apply(page([event("5", at: 50)], next: "c1"), for: SessionEventsRequest.newest)
        XCTAssertFalse(store.olderAutoLoadStopped)
        store.apply(page([event("4", at: 40)], next: "c2"), for: try XCTUnwrap(store.olderRequest))
        XCTAssertFalse(store.olderAutoLoadStopped, "a page that added events keeps going")
        // A fresh cursor but nothing new: stop, though a tap can still load.
        store.apply(page([event("4", at: 40)], next: "c3"), for: try XCTUnwrap(store.olderRequest))
        XCTAssertTrue(store.olderAutoLoadStopped)
        XCTAssertEqual(store.olderRequest?.cursor, "c3")
        store.apply(page([], next: "c4"), for: try XCTUnwrap(store.olderRequest))
        XCTAssertTrue(store.olderAutoLoadStopped)
        // A tapped page that adds events resumes it.
        store.apply(page([event("3", at: 30)], next: "c5"), for: try XCTUnwrap(store.olderRequest))
        XCTAssertFalse(store.olderAutoLoadStopped)
    }

    func testAutomaticOlderLoadingStopsAfterAFailureOrRepeatedRestarts() throws {
        var store = SessionEventStore(sessionID: "s1")
        store.apply(page([event("5", at: 50)], next: "c1"), for: SessionEventsRequest.newest)
        store.failed(status: 503, code: "", older: false)
        XCTAssertFalse(store.olderAutoLoadStopped, "a failed poll does not stop older loading")
        store.failed(status: 503, code: "", older: true)
        XCTAssertTrue(store.olderAutoLoadStopped)

        var restarting = SessionEventStore(sessionID: "s1")
        restarting.apply(page([event("5", at: 50)], next: "c1"), for: SessionEventsRequest.newest)
        restarting.failed(status: 410, code: "cursor_expired", older: false)
        XCTAssertFalse(restarting.olderAutoLoadStopped, "one restart keeps going")
        restarting.apply(page([event("5", at: 50)], next: "c2"), for: SessionEventsRequest.newest)
        restarting.failed(status: 409, code: "cursor_backend_transition", older: false)
        XCTAssertTrue(restarting.olderAutoLoadStopped, "a second restart in a row stops it")
    }

    func testForgettingEventsKeepsTheFailure() {
        var store = SessionEventStore(sessionID: "s1")
        store.apply(page([event("5", at: 50)], next: "c1"), for: SessionEventsRequest.newest)
        store.failed(status: 403, code: "forbidden")
        store.forgetEvents()
        XCTAssertTrue(store.events.isEmpty)
        XCTAssertFalse(store.loaded)
        XCTAssertFalse(store.hasOlder)
        XCTAssertEqual(store.failures, 1)
    }

    func testAccessRules() {
        XCTAssertTrue(SessionEventsRequest.sessionGone(status: 404))
        XCTAssertTrue(SessionEventsRequest.accessRevoked(status: 401))
        XCTAssertTrue(SessionEventsRequest.accessRevoked(status: 403))
        for status in [nil, 404, 409, 410, 500, 502, 503] as [Int?] {
            XCTAssertFalse(SessionEventsRequest.accessRevoked(status: status))
        }
        for status in [nil, 401, 403, 409, 410, 500, 502, 503] as [Int?] {
            XCTAssertFalse(SessionEventsRequest.sessionGone(status: status))
        }
    }

    // MARK: Polling

    func testPollingInterval() {
        let now = base
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: now.addingTimeInterval(-30), now: now, failures: 0), 5)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: now.addingTimeInterval(-119), now: now, failures: 0), 5)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: now.addingTimeInterval(-121), now: now, failures: 0), 15)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: nil, now: now, failures: 0), 15)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: now, now: now, failures: 1), 10)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: nil, now: now, failures: 2), 60)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: nil, now: now, failures: 50), 120)
    }

    func testPollingTierBoundaries() {
        let now = base
        func at(_ age: TimeInterval) -> TimeInterval? {
            SessionEventPolling.interval(newestEventAt: now.addingTimeInterval(-age), now: now, failures: 0)
        }
        XCTAssertEqual(at(0), 5)
        XCTAssertEqual(at(2 * 60 - 1), 5)
        XCTAssertEqual(at(2 * 60), 15)
        XCTAssertEqual(at(60 * 60 - 1), 15)
        XCTAssertEqual(at(60 * 60), 60)
        XCTAssertEqual(at(24 * 60 * 60 - 1), 60)
        XCTAssertEqual(at(24 * 60 * 60), 300)
        XCTAssertEqual(at(7 * 24 * 60 * 60 - 1), 300)
        XCTAssertNil(at(7 * 24 * 60 * 60), "a week old: no timer")
        XCTAssertNil(at(30 * 24 * 60 * 60))
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: nil, chatActivityAt: nil, now: now, failures: 0), 15,
                       "no reference")
    }

    func testChatActivityNewerThanTheNewestEventLiftsTheTier() {
        let now = base
        let old = now.addingTimeInterval(-30 * 24 * 60 * 60)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: old, chatActivityAt: now.addingTimeInterval(-10),
                                                    now: now, failures: 0), 5)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: nil, chatActivityAt: now.addingTimeInterval(-2 * 24 * 60 * 60),
                                                    now: now, failures: 0), 300, "chat activity alone is a reference")
        // An older message does not slow a recent session.
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: now.addingTimeInterval(-10), chatActivityAt: old,
                                                    now: now, failures: 0), 5)
    }

    func testBackoffDoublesCapsAndNeverGoesBelowTheBase() {
        let now = base
        let fresh = now.addingTimeInterval(-10)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: fresh, now: now, failures: 1), 10)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: fresh, now: now, failures: 2), 20)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: fresh, now: now, failures: 4), 80)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: fresh, now: now, failures: 5), 120)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: fresh, now: now, failures: 500), 120)
        let days = now.addingTimeInterval(-2 * 24 * 60 * 60)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: days, now: now, failures: 0), 300)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: days, now: now, failures: 3), 300,
                       "a 5 minute base stays 5 minutes")
        let hours = now.addingTimeInterval(-2 * 60 * 60)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: hours, now: now, failures: 0), 60)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: hours, now: now, failures: 1), 120)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: hours, now: now, failures: 9), 120)
    }

    func testAFailedReadOfAWeekOldSessionIsRetriedEveryFiveMinutes() {
        let now = base
        let week = now.addingTimeInterval(-7 * 24 * 60 * 60)
        XCTAssertNil(SessionEventPolling.interval(newestEventAt: week, now: now, failures: 0), "no timer")
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: week, now: now, failures: 1), 300)
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: week, now: now, failures: 5), 300)
        XCTAssertNil(SessionEventPolling.interval(newestEventAt: week, now: now, failures: 1, unavailable: true))
        let almostWeek = now.addingTimeInterval(-(7 * 24 * 60 * 60 - 1))
        XCTAssertEqual(SessionEventPolling.interval(newestEventAt: almostWeek, now: now, failures: 1), 300)
    }

    func testAnUnavailableSessionHasNoTimer() {
        XCTAssertNil(SessionEventPolling.interval(newestEventAt: base, chatActivityAt: base, now: base, failures: 1,
                                                  unavailable: true))
        XCTAssertNil(SessionEventPolling.interval(newestEventAt: nil, now: base, failures: 0, unavailable: true))
    }

    // MARK: 501

    func testA501IsFinalForTheSession() throws {
        for code in ["events_canonical_too_large", "events_canonical_source_ambiguous", "history_floor", ""] {
            var store = SessionEventStore(sessionID: "s1")
            store.apply(page([event("5", at: 50), event("6", at: 60)], next: "c1"), for: SessionEventsRequest.newest)
            store.failed(status: 501, code: code)
            XCTAssertTrue(store.unavailable, code)
            XCTAssertEqual(store.events.map(\.key.eventID), ["5", "6"], "what was loaded is kept: \(code)")
            XCTAssertTrue(store.loaded, code)
            XCTAssertNil(store.olderRequest, code)
            XCTAssertFalse(store.hasOlder, code)
            XCTAssertNil(SessionEventPolling.interval(newestEventAt: base.addingTimeInterval(60), now: base.addingTimeInterval(61),
                                                      failures: store.failures, unavailable: store.unavailable), code)
            XCTAssertTrue(SessionEventsRequest.unavailable(status: 501, code: code), code)
            XCTAssertFalse(SessionEventsRequest.cursorLost(status: 501, code: code), code)
            // Not part of what is cached on disk.
            XCTAssertEqual(store.snapshot.events.count, 2, code)
            XCTAssertFalse(SessionEventStore(snapshot: store.snapshot).unavailable, code)
        }
        // Before anything loaded, too.
        var empty = SessionEventStore(sessionID: "s1")
        empty.failed(status: 501, code: "")
        XCTAssertTrue(empty.unavailable)
        XCTAssertFalse(empty.loaded)
        XCTAssertNil(empty.olderRequest)
    }

    func testA501CanonicalCursorScopeRestartsPaging() throws {
        var store = SessionEventStore(sessionID: "s1")
        store.apply(page([event("5", at: 50)], next: "c1"), for: SessionEventsRequest.newest)
        store.apply(page([event("4", at: 40)], next: "c2"), for: try XCTUnwrap(store.olderRequest))
        store.failed(status: 501, code: "canonical_cursor_scope")
        XCTAssertFalse(store.unavailable)
        XCTAssertTrue(SessionEventsRequest.cursorLost(status: 501, code: "canonical_cursor_scope"))
        XCTAssertFalse(SessionEventsRequest.unavailable(status: 501, code: "canonical_cursor_scope"))
        XCTAssertEqual(store.olderRequest, SessionEventsRequest.newest, "restart from the newest page")
        XCTAssertEqual(store.events.count, 2)
        XCTAssertNotNil(SessionEventPolling.interval(newestEventAt: base, now: base, failures: store.failures,
                                                     unavailable: store.unavailable))
    }

    func testServerAndTransportFailuresAreNotFinal() {
        for status in [500, 503, nil] as [Int?] {
            var store = SessionEventStore(sessionID: "s1")
            store.apply(page([event("5", at: 50)], next: "c1"), for: SessionEventsRequest.newest)
            store.failed(status: status, code: "")
            store.failed(status: status, code: "")
            XCTAssertFalse(store.unavailable, "\(String(describing: status))")
            XCTAssertFalse(SessionEventsRequest.unavailable(status: status, code: ""))
            XCTAssertEqual(store.olderRequest?.cursor, "c1")
            let now = base.addingTimeInterval(60)
            XCTAssertEqual(SessionEventPolling.interval(newestEventAt: store.newest?.ts, now: now, failures: store.failures,
                                                        unavailable: store.unavailable), 20, "retried with backoff")
        }
        for status in [401, 403, 404, 409, 410] {
            XCTAssertFalse(SessionEventsRequest.unavailable(status: status, code: ""))
        }
    }

    // MARK: Working

    func testWorkingWhileTheNewestEventIsARecentToolCall() {
        let events = [event("1", at: 0, kind: "prompt", role: "user"),
                      event("2", at: 10, kind: "tool_use", tool: "Bash"),
                      event("3", at: 12, kind: "tool_result", role: "tool_result")]
        XCTAssertEqual(SessionWorking.state(events: events, now: base.addingTimeInterval(40))?.label, "Working: Bash")
        XCTAssertNil(SessionWorking.state(events: events, now: base.addingTimeInterval(80)), "cleared by age")
    }

    func testALaterAgentReplyEndsWork() {
        let events = [event("2", at: 10, kind: "tool_use", tool: "Bash"),
                      event("3", at: 20, kind: "assistant", content: "Done.")]
        XCTAssertNil(SessionWorking.state(events: events, now: base.addingTimeInterval(25)))
    }

    func testUnnamedActivityAndPersonPrompts() {
        XCTAssertEqual(SessionWorking.state(events: [event("1", at: 0, kind: "reasoning", role: "assistant")],
                                            now: base.addingTimeInterval(5))?.label, "Working…")
        // A tool from before the agent's last reply does not name this run.
        let events = [event("1", at: 0, kind: "tool_use", tool: "Read"), event("2", at: 1, kind: "assistant", content: "Ok"),
                      event("3", at: 2, kind: "reasoning", role: "assistant")]
        XCTAssertEqual(SessionWorking.state(events: events, now: base.addingTimeInterval(5))?.label, "Working…")
        XCTAssertNil(SessionWorking.state(events: [event("1", at: 0, kind: "prompt", role: "user")], now: base.addingTimeInterval(5)))
        XCTAssertNil(SessionWorking.state(events: [], now: base))
    }
}

/// A final store, through the client, with a stand-in for Cloud.
@MainActor
final class SessionEventUnavailableClientTests: XCTestCase {
    private let agent = RelayAgent(relay: ["name": "builder", "status": "active",
                                           "metadata": ["session_id": "s1", "source": "relay-desktop"]])

    private func page() -> SessionEventPage {
        SessionEventPage(sessionID: "s1", events: [
            SessionEvent(key: .init(source: "fixture", machineID: "m", kind: "assistant", eventID: "e1"),
                         ts: Date(timeIntervalSince1970: 1_790_000_000), kind: "assistant",
                         actorRole: "assistant", content: "synthetic")
        ], nextCursor: "c1")
    }

    func testAFinalStoreMakesNoRequestAndKeepsTheCache() async {
        for code in ["events_canonical_too_large", ""] {
            let spy = SpySessionEventSnapshots()
            var calls = 0
            let client = RelayClient(sessionEventSnapshots: spy, sessionEventFetch: { _, _ in
                calls += 1
                if calls == 1 { return self.page() }
                throw RelayAPIError(message: "not available", status: 501, code: code)
            })
            await client.refreshSessionEvents(for: agent)
            await client.refreshSessionEvents(for: agent)
            XCTAssertEqual(calls, 2, code)
            XCTAssertEqual(client.sessionEvents["s1"]?.unavailable, true, code)
            await client.refreshSessionEvents(for: agent)
            await client.loadOlderSessionEvents(for: agent)
            XCTAssertEqual(calls, 2, "no read of a final session: \(code)")
            XCTAssertEqual(client.sessionEvents["s1"]?.events.count, 1, "what is shown stays: \(code)")
            XCTAssertTrue(spy.removed.isEmpty, "the cached copy stays: \(code)")
            XCTAssertEqual(spy.removeAllCount, 0, code)
            XCTAssertTrue(client.sessionEventsSettled(for: agent), code)
            XCTAssertNil(client.sessionEventPollDelay(for: agent, chatActivityAt: Date()), code)
            // Dropping the store (a new identity) ends it.
            client.advanceSessionGeneration()
            XCTAssertNil(client.sessionEvents["s1"])
        }
    }

    func testReadsOfOneSessionStayApart() async {
        var calls = 0
        let client = RelayClient(sessionEventSnapshots: SpySessionEventSnapshots(), sessionEventFetch: { _, _ in
            calls += 1
            return self.page()
        })
        XCTAssertEqual(client.sessionEventReadWait(for: agent), 0, "the first read goes at once")
        let start = Date()
        await client.refreshSessionEvents(for: agent)
        XCTAssertEqual(calls, 1)
        let wait = client.sessionEventReadWait(for: agent, now: start.addingTimeInterval(1))
        XCTAssertGreaterThan(wait, 3)
        XCTAssertLessThanOrEqual(wait, SessionEventPolling.minimumSpacing)
        XCTAssertEqual(client.sessionEventReadWait(for: agent, now: Date().addingTimeInterval(SessionEventPolling.minimumSpacing)), 0)
    }
}
