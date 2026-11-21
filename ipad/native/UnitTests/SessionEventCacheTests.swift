import XCTest
@testable import AgentRelayiPad

final class SessionEventCacheTests: XCTestCase {
    private var directory: URL!

    override func setUpWithError() throws {
        directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("SessionEventCacheTests-\(UUID().uuidString)", isDirectory: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: directory)
    }

    private func snapshot(_ session: String, events count: Int, content: String = "synthetic",
                          cursor: String? = "c1") -> SessionEventSnapshot {
        let events = (0..<count).map {
            SessionEvent(key: .init(source: "fixture", machineID: "m", kind: "assistant", eventID: "\(session)-\($0)"),
                         ts: Date(timeIntervalSince1970: 1_790_000_000 + Double($0)), kind: "assistant",
                         actorRole: "assistant", content: content)
        }
        return SessionEventSnapshot(sessionID: session, events: events, olderCursor: cursor, reachedStart: false)
    }

    private func cache(eventsPerSession: Int = 100, bytesPerSession: Int = 1_000_000,
                       totalBytes: Int = 10_000_000) -> SessionEventDiskCache {
        SessionEventDiskCache(directory: directory, eventsPerSession: eventsPerSession,
                              bytesPerSession: bytesPerSession, totalBytes: totalBytes)
    }

    private func exists(_ cache: SessionEventDiskCache, _ session: String, workspace: String = "ws") -> Bool {
        FileManager.default.fileExists(atPath: cache.fileURL(workspaceID: workspace, sessionID: session).path)
    }

    func testSnapshotRoundTripsThroughDisk() async throws {
        let cache = cache()
        let saved = snapshot("s1", events: 3)
        cache.save(saved, workspaceID: "ws")
        cache.flush()
        let loaded = await cache.load(workspaceID: "ws", sessionID: "s1")
        XCTAssertEqual(loaded, saved)
        let otherWorkspace = await cache.load(workspaceID: "other", sessionID: "s1")
        XCTAssertNil(otherWorkspace, "one file per workspace and session")
        XCTAssertFalse(cache.fileURL(workspaceID: "ws", sessionID: "s1").lastPathComponent.contains("s1"),
                       "file names do not show ids")
    }

    func testAnotherVersionOrACorruptFileIsDeleted() async throws {
        let cache = cache()
        cache.save(snapshot("s1", events: 1), workspaceID: "ws")
        cache.flush()
        let url = cache.fileURL(workspaceID: "ws", sessionID: "s1")
        var json = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
        json["version"] = SessionEventDiskCache.formatVersion + 1
        try JSONSerialization.data(withJSONObject: json).write(to: url)
        let wrongVersion = await cache.load(workspaceID: "ws", sessionID: "s1")
        XCTAssertNil(wrongVersion)
        XCTAssertFalse(exists(cache, "s1"))

        cache.save(snapshot("s1", events: 1), workspaceID: "ws")
        cache.flush()
        try Data("{not json".utf8).write(to: url)
        let corrupt = await cache.load(workspaceID: "ws", sessionID: "s1")
        XCTAssertNil(corrupt)
        XCTAssertFalse(exists(cache, "s1"))
    }

    func testPerSessionCapsKeepTheNewestEvents() async throws {
        let byCount = cache(eventsPerSession: 5)
        byCount.save(snapshot("s1", events: 12), workspaceID: "ws")
        byCount.flush()
        let loaded = await byCount.load(workspaceID: "ws", sessionID: "s1")
        let kept = try XCTUnwrap(loaded)
        XCTAssertEqual(kept.events.map(\.key.eventID), (7..<12).map { "s1-\($0)" })
        XCTAssertNil(kept.olderCursor, "a trimmed snapshot restarts paging")
        XCTAssertFalse(kept.reachedStart)
        // Restored, it asks for the newest page before any older one.
        XCTAssertEqual(SessionEventStore(snapshot: kept).olderRequest, SessionEventsRequest.newest)

        let bySize = cache(eventsPerSession: 1_000, bytesPerSession: 20_000)
        bySize.save(snapshot("s2", events: 200, content: String(repeating: "x", count: 500)), workspaceID: "ws")
        bySize.flush()
        let loadedSmall = await bySize.load(workspaceID: "ws", sessionID: "s2")
        let small = try XCTUnwrap(loadedSmall)
        XCTAssertLessThan(small.events.count, 200)
        XCTAssertEqual(small.events.last?.key.eventID, "s2-199")
        let size = try FileManager.default.attributesOfItem(atPath: bySize.fileURL(workspaceID: "ws", sessionID: "s2").path)[.size] as? Int
        XCTAssertLessThanOrEqual(size ?? .max, 20_000)
    }

    func testTotalCapEvictsTheLeastRecentlyUsed() async throws {
        let probe = cache()
        probe.save(snapshot("probe", events: 20), workspaceID: "ws")
        probe.flush()
        let one = try XCTUnwrap(FileManager.default.attributesOfItem(
            atPath: probe.fileURL(workspaceID: "ws", sessionID: "probe").path)[.size] as? Int)
        probe.removeAll()

        // Room for three files of this size, not four.
        let cache = cache(totalBytes: one * 3 + one / 2)
        for session in ["a", "b", "c"] { cache.save(snapshot(session, events: 20), workspaceID: "ws") }
        cache.flush()
        let now = Date()
        for (session, age) in [("a", 300.0), ("b", 200.0), ("c", 100.0)] {
            try FileManager.default.setAttributes([.modificationDate: now.addingTimeInterval(-age)],
                                                  ofItemAtPath: cache.fileURL(workspaceID: "ws", sessionID: session).path)
        }
        // Reading "a" makes it the most recently used; "b" goes first.
        _ = await cache.load(workspaceID: "ws", sessionID: "a")
        cache.save(snapshot("d", events: 20), workspaceID: "ws")
        cache.flush()
        XCTAssertTrue(exists(cache, "a"))
        XCTAssertFalse(exists(cache, "b"))
        XCTAssertTrue(exists(cache, "c"))
        XCTAssertTrue(exists(cache, "d"))
    }

    func testRemoveDeletesOneSessionAndRemoveAllEverything() {
        let cache = cache()
        for session in ["a", "b"] { cache.save(snapshot(session, events: 2), workspaceID: "ws") }
        cache.remove(workspaceID: "ws", sessionID: "a")
        cache.flush()
        XCTAssertFalse(exists(cache, "a"))
        XCTAssertTrue(exists(cache, "b"))
        cache.save(snapshot("c", events: 2), workspaceID: "ws")
        cache.removeAll()
        cache.flush()
        XCTAssertFalse(exists(cache, "b"))
        XCTAssertFalse(exists(cache, "c"), "a save queued before removeAll is deleted too")
        XCTAssertFalse(FileManager.default.fileExists(atPath: directory.path))
    }

    func testDirectoryAndFilesAreExcludedFromBackup() throws {
        let cache = cache()
        cache.save(snapshot("s1", events: 1), workspaceID: "ws")
        cache.flush()
        XCTAssertEqual(try directory.resourceValues(forKeys: [.isExcludedFromBackupKey]).isExcludedFromBackup, true)
        XCTAssertEqual(try cache.fileURL(workspaceID: "ws", sessionID: "s1")
            .resourceValues(forKeys: [.isExcludedFromBackupKey]).isExcludedFromBackup, true)
    }

    func testAFileForAnotherWorkspaceOrSessionIsDeleted() async throws {
        let cache = cache()
        cache.save(snapshot("s1", events: 1), workspaceID: "ws")
        cache.flush()
        // The same bytes under the name of another session or workspace.
        let data = try Data(contentsOf: cache.fileURL(workspaceID: "ws", sessionID: "s1"))
        for (workspace, session) in [("ws", "s2"), ("other", "s1")] {
            try data.write(to: cache.fileURL(workspaceID: workspace, sessionID: session))
            let loaded = await cache.load(workspaceID: workspace, sessionID: session)
            XCTAssertNil(loaded)
            XCTAssertFalse(exists(cache, session, workspace: workspace))
        }
        let original = await cache.load(workspaceID: "ws", sessionID: "s1")
        XCTAssertNotNil(original)
    }

    func testAnOversizedFileIsDeletedUnread() async throws {
        let cache = cache(bytesPerSession: 2_000)
        cache.save(snapshot("s1", events: 1), workspaceID: "ws")
        cache.flush()
        let url = cache.fileURL(workspaceID: "ws", sessionID: "s1")
        try Data(repeating: 0x20, count: 2_001).write(to: url)
        let loaded = await cache.load(workspaceID: "ws", sessionID: "s1")
        XCTAssertNil(loaded)
        XCTAssertFalse(exists(cache, "s1"))
    }

    func testDefaultCaps() {
        XCTAssertEqual(SessionEventDiskCache.formatVersion, 1)
        XCTAssertEqual(SessionEventDiskCache.defaultEventsPerSession, 1_000)
        XCTAssertEqual(SessionEventDiskCache.defaultBytesPerSession, 4 * 1024 * 1024)
        XCTAssertEqual(SessionEventDiskCache.defaultTotalBytes, 50 * 1024 * 1024)
        let standard = SessionEventDiskCache.standard()
        XCTAssertEqual(standard?.directory.lastPathComponent, "SessionEventCache")
        XCTAssertEqual(standard?.directory.deletingLastPathComponent().lastPathComponent, "Application Support")
    }

    func testFilesAskForProtectionUntilFirstUnlock() throws {
        let cache = cache()
        cache.save(snapshot("s1", events: 1), workspaceID: "ws")
        cache.flush()
        // The simulator records the class it is asked for but does not
        // enforce it; a device does.
        let attributes = try FileManager.default.attributesOfItem(atPath: cache.fileURL(workspaceID: "ws", sessionID: "s1").path)
        let protection = attributes[.protectionKey] as? FileProtectionType
        XCTAssertTrue(protection == nil || protection == .completeUntilFirstUserAuthentication,
                      "protection: \(String(describing: protection))")
    }
}

/// Records what the client asks of the persistence seam.
final class SpySessionEventSnapshots: SessionEventSnapshotStore, @unchecked Sendable {
    var removeAllCount = 0
    var removed: [String] = []
    var saved: [SessionEventSnapshot] = []
    func load(workspaceID: String, sessionID: String) async -> SessionEventSnapshot? { nil }
    func save(_ snapshot: SessionEventSnapshot, workspaceID: String) { saved.append(snapshot) }
    func remove(workspaceID: String, sessionID: String) { removed.append(sessionID) }
    func removeAll() { removeAllCount += 1 }
}

/// The client's session event lifecycle, with a stand-in for Cloud.
@MainActor
final class SessionEventCacheLifecycleTests: XCTestCase {
    private let agent = RelayAgent(relay: ["name": "builder", "status": "active",
                                           "metadata": ["session_id": "s1", "source": "relay-desktop"]])

    private func page(_ id: String) -> SessionEventPage {
        SessionEventPage(sessionID: "s1", events: [
            SessionEvent(key: .init(source: "fixture", machineID: "m", kind: "assistant", eventID: id),
                         ts: Date(timeIntervalSince1970: 1_790_000_000), kind: "assistant",
                         actorRole: "assistant", content: "synthetic")
        ], nextCursor: nil)
    }

    func testAWorkspaceSwitchDeletesTheCache() {
        let spy = SpySessionEventSnapshots()
        let client = RelayClient(sessionEventSnapshots: spy)
        client.cloudWorkspaceID = UUID().uuidString.lowercased()
        XCTAssertEqual(spy.removeAllCount, 1)
        let same = client.cloudWorkspaceID
        client.cloudWorkspaceID = same
        XCTAssertEqual(spy.removeAllCount, 1, "setting the same workspace keeps the cache")
    }

    func testANewIdentityInTheSameWorkspaceForgetsSessionEvents() async {
        let spy = SpySessionEventSnapshots()
        let client = RelayClient(sessionEventSnapshots: spy, sessionEventFetch: { _, _ in self.page("e1") })
        await client.refreshSessionEvents(for: agent)
        XCTAssertEqual(client.sessionEvents["s1"]?.events.count, 1)
        // What pairing again (same workspace id, another messaging identity) calls.
        client.advanceSessionGeneration()
        XCTAssertTrue(client.sessionEvents.isEmpty)
        XCTAssertEqual(spy.removeAllCount, 1)
    }

    func testSignOutDeletesLoadedEvents() async {
        // Runs on the simulator test host, which has no real account.
        let spy = SpySessionEventSnapshots()
        let client = RelayClient(sessionEventSnapshots: spy, sessionEventFetch: { _, _ in self.page("e1") })
        await client.refreshSessionEvents(for: agent)
        XCTAssertEqual(client.sessionEvents["s1"]?.events.count, 1)
        XCTAssertEqual(spy.saved.count, 1)
        await client.signOut()
        XCTAssertTrue(client.sessionEvents.isEmpty)
        XCTAssertGreaterThanOrEqual(spy.removeAllCount, 1)
    }

    func testAReadFinishingAfterAWorkspaceSwitchStoresNothing() async throws {
        let spy = SpySessionEventSnapshots()
        let client = RelayClient(sessionEventSnapshots: spy, sessionEventFetch: { _, _ in
            try await Task.sleep(for: .milliseconds(300))
            return self.page("late")
        })
        let read = Task { await client.refreshSessionEvents(for: agent) }
        try await Task.sleep(for: .milliseconds(50))
        client.cloudWorkspaceID = UUID().uuidString.lowercased()
        await read.value
        XCTAssertNil(client.sessionEvents["s1"]?.events.first, "the old workspace's read is dropped")
        XCTAssertTrue(spy.saved.isEmpty)
    }

    func testA404DeletesTheCachedSessionAndKeepsWhatIsShown() async {
        let spy = SpySessionEventSnapshots()
        var calls = 0
        let client = RelayClient(sessionEventSnapshots: spy, sessionEventFetch: { _, _ in
            calls += 1
            if calls == 1 { return self.page("e1") }
            throw RelayAPIError(message: "gone", status: 404, code: "session_not_found")
        })
        await client.refreshSessionEvents(for: agent)
        await client.refreshSessionEvents(for: agent)
        XCTAssertEqual(spy.removed, ["s1"])
        XCTAssertEqual(client.sessionEvents["s1"]?.events.count, 1)
        XCTAssertEqual(client.sessionEvents["s1"]?.failures, 1)
    }

    func testRevokedAccessClearsTheScreenAndTheDisk() async {
        for status in [401, 403] {
            let spy = SpySessionEventSnapshots()
            var calls = 0
            let client = RelayClient(sessionEventSnapshots: spy, sessionEventFetch: { _, _ in
                calls += 1
                if calls == 1 { return self.page("e1") }
                throw RelayAPIError(message: "no access", status: status, code: "forbidden")
            })
            await client.refreshSessionEvents(for: agent)
            await client.refreshSessionEvents(for: agent)
            XCTAssertEqual(spy.removed, ["s1"], "status \(status)")
            XCTAssertEqual(client.sessionEvents["s1"]?.events.count, 0, "status \(status)")
            XCTAssertTrue(client.sessionEventsSettled(for: agent), "the chat does not spin")
        }
    }
}
