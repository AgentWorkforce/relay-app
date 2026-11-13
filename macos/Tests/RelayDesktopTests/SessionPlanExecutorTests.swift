import XCTest
@testable import RelayDesktopCore
@testable import RelayDesktop

@MainActor final class SessionPlanExecutorTests: XCTestCase {
    private func target(pid: Int = 30, start: Int = 31, observed: Int64 = 1000, tty: String = "/dev/ttys004") -> SessionLocation {
        SessionLocation(application: "Ghostty", bundleId: "com.mitchellh.ghostty", observedAtMs: observed,
                        pid: pid, processStart: start, tty: tty)
    }
    private func plan(_ action: SessionOpenAction = .focus, location: SessionLocation? = nil,
                      command: SessionResumeCommand? = nil, url: String? = nil) -> SessionOpenPlan {
        SessionOpenPlan(action: action, canResumeDefault: false, choices: [], command: command, location: location, url: url)
    }
    private func operations(focus: @escaping (SessionLocation) async -> String? = { _ in nil },
                            authorize: @escaping (String) async -> String? = { _ in nil },
                            isCurrent: @escaping (SessionLocation) -> Bool = { _ in true },
                            link: @escaping (URL) -> Bool = { _ in true },
                            resume: @escaping (SessionResumeCommand, SessionLocation?) async -> String? = { _, _ in nil }) -> SessionPlanExecutor.Operations {
        .init(isCurrent: isCurrent, authorize: authorize, focus: focus, show: { _ in true }, openLink: link, resume: resume)
    }

    func testFreshObservationOfSameTargetStillFocuses() async {
        let old = plan(location: target()), fresh = plan(location: target(observed: 2000))
        var focused = 0
        let executor = SessionPlanExecutor(store: TestPendingResumeStore(), operations: operations(focus: { _ in focused += 1; return nil }))
        let result = await executor.execute(key: "codex:id", plan: old) { fresh }
        XCTAssertEqual(result, .focused)
        XCTAssertEqual(focused, 1)
    }

    func testMovedOrClosedTargetAfterPermissionNeverFocusesOrResumes() async {
        let old = plan(location: target())
        for fresh in [plan(location: target(start: 32)), plan(location: target(tty: "/dev/ttys005")), plan(.unavailable)] {
            var authorized = false, focused = false, resumed = false
            let executor = SessionPlanExecutor(store: TestPendingResumeStore(), operations: operations(focus: { _ in focused = true; return nil }, authorize: { _ in authorized = true; return nil }, resume: { _, _ in resumed = true; return nil }))
            let result = await executor.execute(key: "claude:id", plan: old) {
                XCTAssertTrue(authorized)
                return fresh
            }
            guard case .failed = result else { return XCTFail("A changed target must be refused") }
            XCTAssertFalse(focused)
            XCTAssertFalse(resumed)
        }
    }

    func testStaleKernelIdentityAndDeniedPermissionBlockDispatch() async {
        let expected = plan(location: target())
        for denied in [true, false] {
            var focused = false
            let executor = SessionPlanExecutor(store: TestPendingResumeStore(), operations: operations(focus: { _ in focused = true; return nil }, authorize: { _ in denied ? "Permission denied" : nil }, isCurrent: { _ in false }))
            let result = await executor.execute(key: "id", plan: expected) { expected }
            guard case .failed = result else { return XCTFail("Permission or stale identity must block focus") }
            XCTAssertFalse(focused)
        }
    }

    func testFailedDeepLinkNeverFallsBackToResume() async {
        let expected = plan(.openLink, url: "claude://code/continue?session=local_exact")
        var resumed = false, links = [String]()
        let executor = SessionPlanExecutor(store: TestPendingResumeStore(), operations: operations(link: { links.append($0.absoluteString); return false }, resume: { _, _ in resumed = true; return nil }))
        let result = await executor.execute(key: "claude:id", plan: expected) { expected }
        guard case .failed = result else { return XCTFail("Failed exact link must be reported") }
        XCTAssertEqual(links, ["claude://code/continue?session=local_exact"])
        XCTAssertFalse(resumed)
    }

    func testResumePreservesPlannerCommandAndOriginalHostAndReportsDispatchOnly() async {
        let command = SessionResumeCommand(args: ["--resume", "cli-id"], cwd: "/my repo", executable: "claude")
        let expected = plan(.resume, location: target(), command: command)
        var received: SessionResumeCommand?, host: SessionLocation?
        let executor = SessionPlanExecutor(store: TestPendingResumeStore(), operations: operations(resume: { received = $0; host = $1; return nil }))
        let result = await executor.execute(key: "claude:cli-id", plan: expected) { expected }
        XCTAssertEqual(received, command)
        XCTAssertEqual(host, target())
        guard case .dispatched = result else { return XCTFail("Launching does not prove provider session selection") }
    }

    func testDefaultTerminalAlternativeRequiresCoreCapability() throws {
        let noAlternative = try JSONDecoder().decode(SessionOpenPlan.self, from: Data(#"{"action":"unavailable","choices":[],"can_resume_default":false}"#.utf8))
        XCTAssertFalse(SessionLocationNavigation.offersDefaultResume(noAlternative, afterFailure: true))
        let unavailableHost = SessionOpenPlan(action: .unavailable, canResumeDefault: true, choices: [])
        XCTAssertTrue(SessionLocationNavigation.offersDefaultResume(unavailableHost, afterFailure: false))
        let originalHost = SessionOpenPlan(action: .resume, canResumeDefault: true, choices: [])
        XCTAssertFalse(SessionLocationNavigation.offersDefaultResume(originalHost, afterFailure: false))
        XCTAssertTrue(SessionLocationNavigation.offersDefaultResume(originalHost, afterFailure: true))
    }

    func testSessionThatMovesDuringNativeFocusIsNotReportedAsVerified() async {
        let expected = plan(location: target())
        var focused = false
        let executor = SessionPlanExecutor(store: TestPendingResumeStore(), operations: operations(focus: { _ in focused = true; return nil }))
        let result = await executor.execute(key: "codex:id", plan: expected) {
            focused ? self.plan(.unavailable) : expected
        }
        guard case .failed = result else { return XCTFail("Native dispatch alone cannot prove this provider still holds the pane") }
        XCTAssertTrue(focused)
    }

    func testShowingApplicationRequiresCapturedHostPID() {
        let captured = SessionLocation(application: "Terminal", bundleId: "com.apple.Terminal", hostPid: 73,
                                       observedAtMs: 1, pid: 30, processStart: 31, tty: "/dev/ttys004")
        var lookedUp: [Int32] = []
        XCTAssertFalse(SessionPlanExecutor.showApplication(captured) { lookedUp.append($0); return nil })
        XCTAssertEqual(lookedUp, [73], "The child session PID and any other instance of this bundle are not the captured host")
        lookedUp = []
        XCTAssertFalse(SessionPlanExecutor.showApplication(target()) { lookedUp.append($0); return nil })
        XCTAssertTrue(lookedUp.isEmpty, "Missing host identity is unavailable, never an arbitrary app instance")
    }

    func testConcurrentClicksAreSerializedAcrossCallers() async {
        let expected = plan(location: target())
        var release: CheckedContinuation<Void, Never>?, started = false, focused = 0
        let executor = SessionPlanExecutor(store: TestPendingResumeStore(), operations: operations(focus: { _ in
            started = true
            await withCheckedContinuation { release = $0 }
            focused += 1
            return nil
        }))
        let first = Task { await executor.execute(key: "codex:id", plan: expected) { expected } }
        while !started { await Task.yield() }
        let second = await executor.execute(key: "codex:id", plan: expected) { expected }
        guard case .failed = second else { return XCTFail("A concurrent click must not dispatch") }
        release?.resume()
        let result = await first.value
        XCTAssertEqual(result, .focused)
        XCTAssertEqual(focused, 1)
    }

    func testSlowProviderStartupBlocksRepeatedResumeAfterTerminalCreation() async {
        let command = SessionResumeCommand(args: ["resume", "id"], cwd: "/repo", executable: "codex")
        let expected = plan(.resume, location: target(), command: command)
        var terminalsCreated = 0
        let executor = SessionPlanExecutor(store: TestPendingResumeStore(), operations: operations(resume: { _, _ in
            // Terminal has returned, but .zshrc can still be waiting and the
            // provider has not started: the core continues to report stopped.
            terminalsCreated += 1
            return nil
        }))
        let first = await executor.execute(key: "codex:id", plan: expected) { expected }
        guard case .dispatched = first else { return XCTFail("The first request should dispatch") }
        for _ in 0..<10 {
            let result = await executor.execute(key: "codex:id", plan: expected) { expected }
            XCTAssertEqual(result, .failed(SessionPlanExecutor.pendingResumeMessage))
        }
        let alternative = plan(.resume, command: command)
        let result = await executor.execute(key: "codex:id", plan: alternative) { alternative }
        XCTAssertEqual(result, .failed(SessionPlanExecutor.pendingResumeMessage))
        XCTAssertEqual(terminalsCreated, 1, "Switching to the default terminal cannot bypass a pending launch")
        XCTAssertTrue(executor.pendingResumes.contains("codex:id"))
    }

    func testOnlyObservedOwnershipAcknowledgesPendingResume() async {
        let command = SessionResumeCommand(args: ["resume", "id"], cwd: "/repo", executable: "codex")
        let resume = plan(.resume, command: command)
        for owner in [plan(location: target()), plan(.showApplication, location: target()),
                      plan(.openLink, location: target(), url: "codex://threads/id"),
                      SessionOpenPlan(action: .choose, canResumeDefault: false, choices: [target()])] {
            var launches = 0
            let executor = SessionPlanExecutor(store: TestPendingResumeStore(), operations: operations(resume: { _, _ in launches += 1; return nil }))
            _ = await executor.execute(key: "id", plan: resume) { resume }
            for unconfirmed in [resume, plan(.unavailable), plan(.openLink, url: "codex://threads/id"), plan(.choose)] {
                executor.observe(key: "id", plan: unconfirmed)
                XCTAssertTrue(executor.pendingResumes.contains("id"))
            }
            executor.observe(key: "other-id", plan: owner)
            XCTAssertTrue(executor.pendingResumes.contains("id"))
            executor.observe(key: "id", plan: owner)
            XCTAssertFalse(executor.pendingResumes.contains("id"))
            // Once the acknowledged owner later stops, a new resume is valid.
            _ = await executor.execute(key: "id", plan: resume) { resume }
            XCTAssertEqual(launches, 2)
        }
    }

    func testFreshOwnershipOnRepeatedOpenAcknowledgesWithoutLaunchingAgain() async {
        let command = SessionResumeCommand(args: ["resume", "id"], cwd: "/repo", executable: "codex")
        let resume = plan(.resume, command: command), owner = plan(location: target())
        var launches = 0
        let executor = SessionPlanExecutor(store: TestPendingResumeStore(), operations: operations(resume: { _, _ in launches += 1; return nil }))
        _ = await executor.execute(key: "id", plan: resume) { resume }
        let result = await executor.execute(key: "id", plan: resume) { owner }
        guard case .failed = result else { return XCTFail("A stale resume must not dispatch") }
        XCTAssertFalse(executor.pendingResumes.contains("id"))
        XCTAssertEqual(launches, 1)
        let focused = await executor.execute(key: "id", plan: owner) { owner }
        XCTAssertEqual(focused, .focused)
    }

    func testExplicitResolutionAllowsRetryButCannotClearActiveDispatch() async {
        let command = SessionResumeCommand(args: ["--resume", "id"], cwd: "/repo", executable: "claude")
        let resume = plan(.resume, command: command)
        var release: CheckedContinuation<Void, Never>?, started = false, launches = 0
        let executor = SessionPlanExecutor(store: TestPendingResumeStore(), operations: operations(resume: { _, _ in
            launches += 1
            if launches == 1 {
                started = true
                await withCheckedContinuation { release = $0 }
            }
            return nil
        }))
        let first = Task { await executor.execute(key: "id", plan: resume) { resume } }
        while !started { await Task.yield() }
        XCTAssertFalse(executor.resolvePendingResume(key: "id"))
        release?.resume()
        _ = await first.value
        XCTAssertTrue(executor.resolvePendingResume(key: "id"))
        XCTAssertFalse(executor.resolvePendingResume(key: "id"))
        _ = await executor.execute(key: "id", plan: resume) { resume }
        XCTAssertEqual(launches, 2)
    }

    func testAmbiguousResumeErrorStillNeedsOwnershipOrExplicitResolution() async {
        let command = SessionResumeCommand(args: ["resume", "id"], cwd: "/repo", executable: "codex")
        let resume = plan(.resume, command: command)
        var launches = 0
        let executor = SessionPlanExecutor(store: TestPendingResumeStore(), operations: operations(resume: { _, _ in
            launches += 1
            return "Terminal did not acknowledge the request."
        }))
        _ = await executor.execute(key: "id", plan: resume) { resume }
        let result = await executor.execute(key: "id", plan: resume) { resume }
        XCTAssertEqual(result, .failed(SessionPlanExecutor.pendingResumeMessage))
        XCTAssertEqual(launches, 1)
        XCTAssertTrue(executor.resolvePendingResume(key: "id"))
    }

    private func temporaryStoreFile() throws -> URL {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("relay-pending-test-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: directory) }
        return directory.appendingPathComponent("pending.json")
    }

    func testRecreatedExecutorBlocksResumeWhileEarlierShellHasNotStartedProvider() async throws {
        let file = try temporaryStoreFile()
        let command = SessionResumeCommand(args: ["resume", "id"], cwd: "/repo", executable: "codex")
        let resume = plan(.resume, command: command)
        var launches = 0
        let native = operations(resume: { _, _ in
            // This assertion runs before the adapter can start a slow shell.
            XCTAssertEqual(try? SessionPendingResumeStore(file: file).load(), ["codex:id"])
            launches += 1
            return nil
        })
        var executor: SessionPlanExecutor? = SessionPlanExecutor(store: SessionPendingResumeStore(file: file), operations: native)
        _ = await executor!.execute(key: "codex:id", plan: resume) { resume }
        executor = nil // Closing the desktop does not stop a starting terminal.
        let reopened = SessionPlanExecutor(store: SessionPendingResumeStore(file: file), operations: native)
        XCTAssertTrue(reopened.pendingResumes.contains("codex:id"))
        for _ in 0..<3 {
            let result = await reopened.execute(key: "codex:id", plan: resume) { resume }
            XCTAssertEqual(result, .failed(SessionPlanExecutor.pendingResumeMessage))
        }
        XCTAssertEqual(launches, 1)
    }

    func testOwnershipAndExplicitResolutionPersistAcrossExecutorInstances() async throws {
        let command = SessionResumeCommand(args: ["resume", "id"], cwd: "/repo", executable: "codex")
        let resume = plan(.resume, command: command)
        for hasFreshOwner in [true, false] {
            let file = try temporaryStoreFile()
            var launches = 0
            let native = operations(resume: { _, _ in launches += 1; return nil })
            let first = SessionPlanExecutor(store: SessionPendingResumeStore(file: file), operations: native)
            _ = await first.execute(key: "id", plan: resume) { resume }
            let reopened = SessionPlanExecutor(store: SessionPendingResumeStore(file: file), operations: native)
            if hasFreshOwner { reopened.observe(key: "id", plan: plan(location: target())) }
            else { XCTAssertTrue(reopened.resolvePendingResume(key: "id")) }
            let third = SessionPlanExecutor(store: SessionPendingResumeStore(file: file), operations: native)
            XCTAssertTrue(third.pendingResumes.isEmpty)
            _ = await third.execute(key: "id", plan: resume) { resume }
            XCTAssertEqual(launches, 2)
        }
    }

    func testOverlappingExecutorsReloadReservationBeforeDispatch() async throws {
        let file = try temporaryStoreFile()
        let command = SessionResumeCommand(args: ["resume", "id"], cwd: "/repo", executable: "codex")
        let resume = plan(.resume, command: command)
        var release: CheckedContinuation<Void, Never>?, started = false, launches = 0
        let native = operations(resume: { _, _ in
            launches += 1
            started = true
            await withCheckedContinuation { release = $0 }
            return nil
        })
        let first = SessionPlanExecutor(store: SessionPendingResumeStore(file: file), operations: native)
        let second = SessionPlanExecutor(store: SessionPendingResumeStore(file: file), operations: native)
        let task = Task { await first.execute(key: "id", plan: resume) { resume } }
        while !started { await Task.yield() }
        let result = await second.execute(key: "id", plan: resume) { resume }
        XCTAssertEqual(result, .failed(SessionPlanExecutor.pendingResumeMessage))
        release?.resume()
        _ = await task.value
        XCTAssertEqual(launches, 1)
    }

    func testStaleExecutorDoesNotRestoreAnotherInstancesClearedReservation() async throws {
        for observedOwner in [false, true] {
            for reserveAnother in [false, true] {
                let file = try temporaryStoreFile()
                let store = SessionPendingResumeStore(file: file)
                _ = try store.reserve("a")
                _ = try store.reserve("c")
                let stale = SessionPlanExecutor(store: SessionPendingResumeStore(file: file), operations: operations())
                let clearer = SessionPlanExecutor(store: SessionPendingResumeStore(file: file), operations: operations())
                if observedOwner { clearer.observe(key: "a", plan: plan(location: target())) }
                else { XCTAssertTrue(clearer.resolvePendingResume(key: "a")) }
                XCTAssertTrue(stale.pendingResumes.contains("a"))
                if reserveAnother {
                    let resume = plan(.resume, command: SessionResumeCommand(args: ["resume", "b"], cwd: "/repo", executable: "codex"))
                    _ = await stale.execute(key: "b", plan: resume) { resume }
                    XCTAssertEqual(try store.load(), ["b", "c"])
                } else {
                    XCTAssertTrue(stale.resolvePendingResume(key: "c"))
                    XCTAssertTrue(try store.load().isEmpty)
                }
            }
        }
    }

    func testReadAndReservationWriteFailuresNeverDispatchResume() async {
        let command = SessionResumeCommand(args: ["resume", "id"], cwd: "/repo", executable: "codex")
        let resume = plan(.resume, command: command)
        for failRead in [true, false] {
            let store = TestPendingResumeStore()
            if failRead { store.readError = CocoaError(.fileReadNoPermission) }
            else { store.writeError = CocoaError(.fileWriteNoPermission) }
            var launches = 0
            let executor = SessionPlanExecutor(store: store, operations: operations(resume: { _, _ in launches += 1; return nil }))
            for _ in 0..<2 {
                let result = await executor.execute(key: "id", plan: resume) { resume }
                XCTAssertEqual(result, .failed(executor.resumeStorageError!))
            }
            XCTAssertTrue(executor.resumeStorageError!.contains(store.locationDescription))
            XCTAssertEqual(launches, 0)
        }
    }

    func testReservationRechecksDiskAfterPermissionDialog() async throws {
        let file = try temporaryStoreFile()
        let command = SessionResumeCommand(args: ["resume", "id"], cwd: "/repo", executable: "codex")
        let resume = plan(.resume, command: command)
        let otherInstance = SessionPendingResumeStore(file: file)
        var launches = 0
        let executor = SessionPlanExecutor(store: SessionPendingResumeStore(file: file), operations: operations(authorize: { _ in
            // Another desktop can reserve while this app awaits permission.
            do { _ = try otherInstance.reserve("id") }
            catch { XCTFail("Couldn’t reserve from the other instance: \(error)") }
            return nil
        }, resume: { _, _ in launches += 1; return nil }))
        let result = await executor.execute(key: "id", plan: resume) { resume }
        XCTAssertEqual(result, .failed(SessionPlanExecutor.pendingResumeMessage))
        XCTAssertEqual(launches, 0)
    }

    func testCorruptAndUnreadableFilesBlockResume() async throws {
        let command = SessionResumeCommand(args: ["resume", "id"], cwd: "/repo", executable: "codex")
        let resume = plan(.resume, command: command)
        for data in ["{broken", "{\"version\":2,\"pending\":[]}", "{\"version\":1,\"pending\":[\"id\",\"id\"]}"] {
            let file = try temporaryStoreFile()
            try Data(data.utf8).write(to: file)
            var launches = 0
            let executor = SessionPlanExecutor(store: SessionPendingResumeStore(file: file), operations: operations(resume: { _, _ in launches += 1; return nil }))
            let result = await executor.execute(key: "id", plan: resume) { resume }
            XCTAssertEqual(result, .failed(executor.resumeStorageError!))
            XCTAssertEqual(launches, 0)
            XCTAssertEqual(try String(contentsOf: file, encoding: .utf8), data, "Corruption must not be overwritten with an empty store")
        }
        let file = try temporaryStoreFile()
        try FileManager.default.createDirectory(at: file, withIntermediateDirectories: false)
        let executor = SessionPlanExecutor(store: SessionPendingResumeStore(file: file), operations: operations(resume: { _, _ in
            XCTFail("Unreadable storage must never launch")
            return nil
        }))
        XCTAssertNotNil(executor.resumeStorageError)
        _ = await executor.execute(key: "id", plan: resume) { resume }
    }

    func testFailedClearRetainsReservationAndBlocksAnotherResume() async {
        let command = SessionResumeCommand(args: ["resume", "id"], cwd: "/repo", executable: "codex")
        let resume = plan(.resume, command: command)
        for hasFreshOwner in [true, false] {
            let store = TestPendingResumeStore()
            var launches = 0
            let executor = SessionPlanExecutor(store: store, operations: operations(resume: { _, _ in launches += 1; return nil }))
            _ = await executor.execute(key: "id", plan: resume) { resume }
            store.writeError = CocoaError(.fileWriteNoPermission)
            if hasFreshOwner { executor.observe(key: "id", plan: plan(location: target())) }
            else { XCTAssertFalse(executor.resolvePendingResume(key: "id")) }
            XCTAssertTrue(executor.pendingResumes.contains("id"))
            let result = await executor.execute(key: "id", plan: resume) { resume }
            XCTAssertEqual(result, .failed(executor.resumeStorageError!))
            XCTAssertEqual(launches, 1)
            let reopened = SessionPlanExecutor(store: store, operations: operations())
            XCTAssertTrue(reopened.pendingResumes.contains("id"))
        }
    }

    func testFocusAndLinkDispatchesRemainRepeatableWithoutPendingResume() async {
        for expected in [plan(location: target()), plan(.openLink, url: "codex://threads/id")] {
            let executor = SessionPlanExecutor(store: TestPendingResumeStore(), operations: operations())
            for _ in 0..<2 {
                let result = await executor.execute(key: "id", plan: expected) { expected }
                if case .failed(let message) = result { XCTFail(message) }
                XCTAssertTrue(executor.pendingResumes.isEmpty)
            }
        }
    }
}

@MainActor final class TestPendingResumeStore: SessionPendingResumeStorage {
    let locationDescription = "test pending launches"
    var pending: Set<String> = []
    var readError: Error?
    var writeError: Error?
    func load() throws -> Set<String> {
        if let readError { throw readError }
        return pending
    }
    func reserve(_ key: String) throws -> (pending: Set<String>, inserted: Bool) {
        if let writeError { throw writeError }
        let inserted = pending.insert(key).inserted
        return (pending, inserted)
    }
    func remove(_ key: String) throws -> Set<String> {
        if let writeError { throw writeError }
        pending.remove(key)
        return pending
    }
}
