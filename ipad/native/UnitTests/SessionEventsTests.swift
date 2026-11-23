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

    func testADotSegmentSessionIDHasNoPath() {
        let ws = "9F1C2D3E-4B5A-4C6D-8E7F-901234567890"
        // "." and ".." would make the authorized path name another resource.
        XCTAssertNil(SessionEventsRequest.path(workspaceID: ws, sessionID: ".", spec: SessionEventsRequest.newest))
        XCTAssertNil(SessionEventsRequest.path(workspaceID: ws, sessionID: "..", spec: SessionEventsRequest.newest))
        // Dots inside an id are only characters.
        XCTAssertEqual(SessionEventsRequest.path(workspaceID: ws, sessionID: "...", spec: SessionEventsRequest.newest),
                       "/workspaces/9f1c2d3e-4b5a-4c6d-8e7f-901234567890/reflex/sessions/.../events?limit=100&order=desc")
        XCTAssertNotNil(SessionEventsRequest.path(workspaceID: ws, sessionID: ".s1", spec: SessionEventsRequest.newest))
    }

    func testAnOutOfRangeNumericTimeDropsTheEventInsteadOfCrashing() throws {
        // Without an eventId the event is keyed by its time in Int64 milliseconds;
        // 1e19 and -1e19 do not fit. The page keeps its other events.
        let data = Data(#"{"sessionId":"s1","events":[{"kind":"assistant","ts":1e19,"content":"far"},{"kind":"assistant","ts":-1e19,"content":"before"},{"kind":"assistant","ts":1795168800000,"content":"kept"},{"eventId":"e9","kind":"assistant","ts":1e19,"content":"has an id"}]}"#.utf8)
        let page = try SessionEventPage.decode(data, sessionID: "s1")
        XCTAssertEqual(page.events.map(\.content), ["kept", "has an id"])
        XCTAssertEqual(page.events.first?.key.eventID.hasPrefix("ts:1795168800000:"), true)
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

    private func events(_ range: Range<Int>) -> [SessionEvent] {
        range.map { event("e\($0)", at: TimeInterval($0)) }
    }

    /// A session's events e0, e1, … as the Cloud route pages them newest
    /// first, 100 a page; "before:N" is the cursor of events older than eN.
    private struct PagedSession {
        var count: Int
        func page(_ spec: SessionEventsRequest.Spec, events: (Range<Int>) -> [SessionEvent]) -> SessionEventPage {
            let top = spec.cursor.flatMap { Int($0.dropFirst("before:".count)) } ?? count
            let bottom = max(top - 100, 0)
            return SessionEventPage(sessionID: "s1", events: events(bottom..<top).reversed(),
                                    nextCursor: bottom > 0 ? "before:\(bottom)" : nil)
        }
    }

    func testTheLiveStoreKeepsTheNewestEvents() throws {
        var session = PagedSession(count: 100)
        var store = SessionEventStore(sessionID: "s1")
        XCTAssertEqual(store.cap, SessionEventStore.liveLimit)
        store.apply(session.page(SessionEventsRequest.newest, events: events), for: SessionEventsRequest.newest)
        // Polls of a busy session, 40 new events each.
        while session.count < SessionEventStore.liveLimit + 200 {
            session.count += 40
            store.apply(session.page(SessionEventsRequest.newest, events: events), for: SessionEventsRequest.newest)
        }
        XCTAssertEqual(store.events.count, SessionEventStore.liveLimit)
        XCTAssertEqual(store.events.last?.key.eventID, "e\(session.count - 1)", "the newest stay")
        XCTAssertEqual(store.events.first?.key.eventID, "e\(session.count - SessionEventStore.liveLimit)")
        XCTAssertEqual(store.snapshot.events.count, SessionEventStore.liveLimit)
        XCTAssertFalse(store.reachedStart)
        XCTAssertTrue(store.hasOlder)
    }

    func testOlderPagingAfterALiveTrimReachesBelowTheHeldEvents() throws {
        var session = PagedSession(count: 100)
        var store = SessionEventStore(sessionID: "s1")
        store.apply(session.page(SessionEventsRequest.newest, events: events), for: SessionEventsRequest.newest)
        while session.count < SessionEventStore.liveLimit + 200 {
            session.count += 40
            store.apply(session.page(SessionEventsRequest.newest, events: events), for: SessionEventsRequest.newest)
        }
        let heldOldest = session.count - SessionEventStore.liveLimit
        XCTAssertEqual(store.events.first?.key.eventID, "e\(heldOldest)")
        XCTAssertTrue(store.rewalking)
        // The person scrolls up. Older paging resumes from a page just above
        // the oldest held event, not from the newest page.
        var request = try XCTUnwrap(store.olderRequest)
        XCTAssertNotNil(request.cursor, "not sent back to the newest page")
        var reads = 0
        while store.events.first?.key.eventID == "e\(heldOldest)" {
            store.apply(session.page(request, events: events), for: request)
            reads += 1
            // A poll arrives in between, with a new event: it trims nothing
            // and leaves paging where it was.
            let cursor = store.olderRequest?.cursor
            session.count += 1
            store.apply(session.page(SessionEventsRequest.newest, events: events), for: SessionEventsRequest.newest)
            XCTAssertEqual(store.olderRequest?.cursor, cursor, "a poll does not move paging")
            XCTAssertNotNil(store.olderRequest?.cursor, "a poll does not send paging back to the newest page")
            XCTAssertFalse(store.olderAutoLoadStopped, "automatic loading stays on while re-walking held events")
            request = try XCTUnwrap(store.olderRequest)
            XCTAssertLessThan(reads, 3, "no long walk through held events")
            if reads >= 3 { break }
        }
        // Events below the held ones merged in, and nothing they loaded went.
        let firstIndex = try XCTUnwrap(store.events.first.flatMap { Int($0.key.eventID.dropFirst()) })
        XCTAssertLessThan(firstIndex, heldOldest)
        XCTAssertEqual(reads, 1, "the first older page already reaches below")
        XCTAssertFalse(store.rewalking)
        XCTAssertGreaterThan(store.events.count, SessionEventStore.liveLimit)
        XCTAssertEqual(store.cap, SessionEventStore.pagedBackLimit)
        // Paging goes on down to the start.
        while let next = store.olderRequest {
            store.apply(session.page(next, events: events), for: next)
        }
        XCTAssertTrue(store.reachedStart)
        XCTAssertEqual(store.events.first?.key.eventID, "e0")
        XCTAssertEqual(store.events.count, session.count)
    }

    func testARestoredTrimmedSnapshotRewalksWithoutStopping() throws {
        let session = PagedSession(count: 1_500)
        let held = SessionEventSnapshot(sessionID: "s1", events: events(500..<1_500), olderCursor: nil, reachedStart: false)
        var store = SessionEventStore(snapshot: held)
        XCTAssertTrue(store.rewalking)
        XCTAssertEqual(store.olderRequest, SessionEventsRequest.newest)
        // The walk down from the newest page re-reads held events first.
        while let request = store.olderRequest, store.events.first?.key.eventID == "e500" {
            store.apply(session.page(request, events: events), for: request)
            XCTAssertFalse(store.olderAutoLoadStopped)
        }
        XCTAssertEqual(store.events.first?.key.eventID, "e400")
        XCTAssertFalse(store.rewalking)
        // From then on, a page that adds nothing stops automatic loading as before.
    }

    func testAPollNeverTrimsTheHistoryAPersonLoaded() throws {
        var store = SessionEventStore(sessionID: "s1")
        let top = 10_000
        store.apply(page(events((top - 100)..<top), next: "c1"), for: SessionEventsRequest.newest)
        // The person scrolls back past the live limit.
        var oldest = top - 100
        while oldest > top - 1_500 {
            store.apply(page(events((oldest - 100)..<oldest), next: "c\(oldest)"), for: try XCTUnwrap(store.olderRequest))
            oldest -= 100
        }
        XCTAssertEqual(store.events.count, 1_500)
        XCTAssertEqual(store.cap, SessionEventStore.pagedBackLimit)
        // New events arrive while they read: nothing they loaded goes.
        store.apply(page(events((top - 5)..<(top + 50)), next: "c-poll"), for: SessionEventsRequest.newest)
        XCTAssertEqual(store.events.count, 1_550)
        XCTAssertEqual(store.events.first?.key.eventID, "e\(top - 1_500)")
        XCTAssertEqual(store.olderRequest?.cursor, "c\(oldest + 100)", "paging back goes on where it was")
        // Past the paged-back limit, the cap is what they loaded.
        while store.events.count < SessionEventStore.pagedBackLimit + 100 {
            store.apply(page(events((oldest - 100)..<oldest), next: "c\(oldest)"), for: try XCTUnwrap(store.olderRequest))
            oldest -= 100
        }
        let loaded = store.events.count
        XCTAssertEqual(store.cap, loaded)
        XCTAssertEqual(store.events.first?.key.eventID, "e\(oldest)")
        // A poll that repeats what is held trims nothing.
        store.apply(page(events((top + 40)..<(top + 50)), next: "c-poll"), for: SessionEventsRequest.newest)
        XCTAssertEqual(store.events.count, loaded)
        XCTAssertEqual(store.events.first?.key.eventID, "e\(oldest)")
        // Forgetting the events forgets the paged-back cap.
        store.forgetEvents()
        XCTAssertEqual(store.cap, SessionEventStore.liveLimit)
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
        for code in ["events_canonical_too_large", "events_canonical_source_ambiguous", "history_floor"] {
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
        // Before anything loaded, too; a 501 with no code is final only at
        // the third in a row.
        var empty = SessionEventStore(sessionID: "s1")
        empty.failed(status: 501, code: "")
        empty.failed(status: 501, code: "")
        XCTAssertFalse(empty.unavailable)
        empty.failed(status: 501, code: "")
        XCTAssertTrue(empty.unavailable)
        XCTAssertFalse(empty.loaded)
        XCTAssertNil(empty.olderRequest)
    }

    private func loadedStore(at seconds: TimeInterval = 0) -> SessionEventStore {
        var store = SessionEventStore(sessionID: "s1")
        store.apply(page([event("5", at: seconds)], next: "c1"), for: SessionEventsRequest.newest)
        return store
    }

    private func delay(_ store: SessionEventStore, age: TimeInterval) -> TimeInterval? {
        SessionEventPolling.interval(newestEventAt: store.newest?.ts, now: (store.newest?.ts ?? base).addingTimeInterval(age),
                                     failures: store.failures, unavailable: store.unavailable,
                                     unknownNotImplemented: store.unknownNotImplemented)
    }

    func testATransient501IsRetriedLikeA500() {
        for reason in ["session_partial", "history_floor_unknown_session"] {
            XCTAssertEqual(SessionEventsRequest.notImplemented(code: "unsupported_by_backend", reason: reason), .transient)
            for older in [false, true] {
                var store = loadedStore()
                for count in 1...10 {
                    store.failed(status: 501, code: "unsupported_by_backend", reason: reason, older: older)
                    XCTAssertFalse(store.unavailable, "\(reason) \(count)")
                    XCTAssertEqual(store.unknownNotImplemented, 0)
                    XCTAssertEqual(store.olderRequest?.cursor, "c1", "the cursor stays: \(reason)")
                    let expected = min(5 * pow(2, Double(count)), 120)
                    XCTAssertEqual(delay(store, age: 10), expected, "5 s tier backoff: \(reason) \(count)")
                    XCTAssertEqual(delay(store, age: 2 * 60 * 60), 120, "never below the 60 s tier: \(reason)")
                    XCTAssertEqual(delay(store, age: 2 * 24 * 60 * 60), 300, "never below the 5 minute tier: \(reason)")
                    XCTAssertEqual(delay(store, age: 8 * 24 * 60 * 60), 300, "no-timer tier retry: \(reason)")
                }
                XCTAssertEqual(store.failures, 10)
                XCTAssertEqual(store.olderAutoLoadStopped, older)
            }
        }
    }

    func testAFinal501ReasonIsFinalAtOnce() {
        let answers = [("unsupported_by_backend", "history_floor"), ("unsupported_by_backend", "events_canonical_too_large"),
                       ("unsupported_by_backend", "events_canonical_source_ambiguous"), ("events_canonical_too_large", "")]
        for (code, reason) in answers {
            XCTAssertEqual(SessionEventsRequest.notImplemented(code: code, reason: reason), .permanent, "\(code) \(reason)")
            var store = SessionEventStore(sessionID: "s1")
            store.apply(page([event("5", at: 50), event("6", at: 60)], next: "c1"), for: SessionEventsRequest.newest)
            store.failed(status: 501, code: code, reason: reason)
            XCTAssertTrue(store.unavailable, "\(code) \(reason)")
            XCTAssertNil(delay(store, age: 1), "\(code) \(reason)")
            XCTAssertEqual(store.events.map(\.key.eventID), ["5", "6"], "what was loaded is kept: \(code) \(reason)")
            XCTAssertNil(store.olderRequest, "\(code) \(reason)")
            XCTAssertFalse(store.hasOlder, "\(code) \(reason)")
        }
    }

    func testALostCursor501RestartsPaging() throws {
        let answers = [("unsupported_by_backend", "canonical_cursor_scope"), ("canonical_cursor_scope", ""),
                       ("unsupported_by_backend", "foreign_cursor")]
        for (code, reason) in answers {
            XCTAssertEqual(SessionEventsRequest.notImplemented(code: code, reason: reason), .cursorLost, "\(code) \(reason)")
            var store = loadedStore(at: 50)
            store.apply(page([event("4", at: 40)], next: "c2"), for: try XCTUnwrap(store.olderRequest))
            store.failed(status: 501, code: code, reason: reason)
            XCTAssertFalse(store.unavailable, "\(code) \(reason)")
            XCTAssertEqual(store.olderRequest, SessionEventsRequest.newest, "restart from the newest page: \(code) \(reason)")
            XCTAssertEqual(store.events.count, 2)
            XCTAssertNotNil(delay(store, age: 1))
        }
    }

    private let unknownAnswers = [("", ""), ("unsupported_by_backend", ""), ("unsupported_by_backend", "route")]

    func testAnUnknown501IsFinalAtTheThirdInARow() {
        // Age of the reference → the wait after an unknown 501.
        let tiers: [(age: TimeInterval, wait: TimeInterval)] = [
            (10, 120), (10 * 60, 120), (2 * 60 * 60, 120), (2 * 24 * 60 * 60, 300), (8 * 24 * 60 * 60, 300),
        ]
        for (code, reason) in unknownAnswers {
            XCTAssertEqual(SessionEventsRequest.notImplemented(code: code, reason: reason), .unknown, "\(code) \(reason)")
            XCTAssertFalse(SessionEventsRequest.unavailable(status: 501, code: code, reason: reason))
            var store = loadedStore()
            for count in 1...2 {
                store.failed(status: 501, code: code, reason: reason)
                XCTAssertFalse(store.unavailable, "\(code) \(reason) \(count)")
                XCTAssertEqual(store.unknownNotImplemented, count)
                XCTAssertEqual(store.olderRequest?.cursor, "c1")
                for tier in tiers {
                    XCTAssertEqual(delay(store, age: tier.age), tier.wait, "\(code) \(reason) \(count) age \(tier.age)")
                }
            }
            store.failed(status: 501, code: code, reason: reason)
            XCTAssertTrue(store.unavailable, "\(code) \(reason)")
            XCTAssertNil(delay(store, age: 10))
            XCTAssertNil(store.olderRequest)
            XCTAssertEqual(store.events.count, 1, "what was loaded is kept")
        }
    }

    func testASuccessfulReadResetsTheUnknown501Count() {
        for (code, reason) in unknownAnswers {
            var store = loadedStore()
            store.failed(status: 501, code: code, reason: reason)
            store.failed(status: 501, code: code, reason: reason)
            store.apply(page([event("6", at: 5)], next: nil), for: SessionEventsRequest.newest)
            XCTAssertEqual(store.unknownNotImplemented, 0)
            store.failed(status: 501, code: code, reason: reason)
            store.failed(status: 501, code: code, reason: reason)
            XCTAssertFalse(store.unavailable, "\(code) \(reason)")
            store.failed(status: 501, code: code, reason: reason)
            XCTAssertTrue(store.unavailable, "\(code) \(reason)")
        }
    }

    func testAnotherFailureResetsTheUnknown501Count() {
        for (code, reason) in unknownAnswers {
            var store = loadedStore()
            store.failed(status: 501, code: code, reason: reason)
            store.failed(status: 501, code: code, reason: reason)
            store.failed(status: 500, code: "")
            XCTAssertEqual(store.unknownNotImplemented, 0)
            XCTAssertEqual(delay(store, age: 10), 40, "the 500 backoff, three failures in a row")
            store.failed(status: 501, code: code, reason: reason)
            store.failed(status: 501, code: code, reason: reason)
            XCTAssertFalse(store.unavailable, "\(code) \(reason)")
            store.failed(status: 501, code: "unsupported_by_backend", reason: "session_partial")
            store.failed(status: 501, code: code, reason: reason)
            store.failed(status: 501, code: code, reason: reason)
            XCTAssertFalse(store.unavailable, "a transient 501 resets it too: \(code) \(reason)")
            store.failed(status: 501, code: code, reason: reason)
            XCTAssertTrue(store.unavailable, "\(code) \(reason)")
        }
    }

    func testErrorBodyIdentifiers() throws {
        func identifiers(_ body: String) throws -> (code: String, reason: String) {
            let json = try XCTUnwrap(try JSONSerialization.jsonObject(with: Data(body.utf8)) as? JSONObject)
            return RelayErrorBody.identifiers(in: json)
        }
        let server = try identifiers(#"{"error":{"code":"unsupported_by_backend","message":"synthetic","reason":"session_partial"},"correlationId":"c-1"}"#)
        XCTAssertEqual(server.code, "unsupported_by_backend")
        XCTAssertEqual(server.reason, "session_partial")
        let plain = try identifiers(#"{"error":"Failed to load session events"}"#)
        XCTAssertEqual(plain.code, "")
        XCTAssertEqual(plain.reason, "")
        let nested = try identifiers(#"{"error":{"code":"cursor_expired","message":"x"}}"#)
        XCTAssertEqual(nested.code, "cursor_expired")
        XCTAssertEqual(nested.reason, "")
        let topLevel = try identifiers(#"{"error":"x","code":"history_customer_hosted"}"#)
        XCTAssertEqual(topLevel.code, "history_customer_hosted")
        XCTAssertEqual(topLevel.reason, "")
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
        // An unknown 501 (no code) is final at the third in a row.
        for (code, failing) in [("events_canonical_too_large", 1), ("", 3)] {
            let spy = SpySessionEventSnapshots()
            var calls = 0
            let client = RelayClient(sessionEventSnapshots: spy, sessionEventFetch: { _, _ in
                calls += 1
                if calls == 1 { return self.page() }
                throw RelayAPIError(message: "not available", status: 501, code: code)
            })
            await client.refreshSessionEvents(for: agent)
            for _ in 1...failing {
                XCTAssertNotEqual(client.sessionEvents["s1"]?.unavailable, true, code)
                await client.refreshSessionEvents(for: agent)
            }
            XCTAssertEqual(calls, 1 + failing, code)
            XCTAssertEqual(client.sessionEvents["s1"]?.unavailable, true, code)
            await client.refreshSessionEvents(for: agent)
            await client.loadOlderSessionEvents(for: agent)
            XCTAssertEqual(calls, 1 + failing, "no read of a final session: \(code)")
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

    func testTheClientCarriesTheReasonToTheStore() async {
        var calls = 0
        let client = RelayClient(sessionEventSnapshots: SpySessionEventSnapshots(), sessionEventFetch: { _, _ in
            calls += 1
            if calls == 1 { return self.page() }
            throw RelayAPIError(message: "not yet", status: 501, code: "unsupported_by_backend", reason: "session_partial")
        })
        await client.refreshSessionEvents(for: agent)
        for _ in 1...4 { await client.refreshSessionEvents(for: agent) }
        XCTAssertEqual(calls, 5, "a transient 501 is read again")
        XCTAssertEqual(client.sessionEvents["s1"]?.unavailable, false)
        XCTAssertEqual(client.sessionEvents["s1"]?.failures, 4)
        XCTAssertNotNil(client.sessionEventPollDelay(for: agent, chatActivityAt: Date()))
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
