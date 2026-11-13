import XCTest
import Vision
import SwiftUI
import AppKit
@testable import RelayDesktop
@testable import RelayDesktopCore

@MainActor
final class AppModelTests: XCTestCase {
    private var core: FakeCore!
    private var defaults: UserDefaults!
    private var suite: String!

    override func setUp() async throws {
        core = FakeCore()
        suite = "app-model-test-\(UUID())"
        defaults = UserDefaults(suiteName: suite)
        core.on("settings.get") { _ in Self.settings(analytics: true) }
        core.on("settings.set") { params in Self.settings(analytics: params["analytics_enabled"] as? Bool ?? true) }
        core.on("uploader.status") { _ in
            ["running": true, "paused": false, "sharing_mode": "new", "directory": "/tmp/install",
             "collector_replacing": false, "collector_outdated": false,
             "delivery": ["bootstrap_complete": true, "pending_records": 0, "acknowledged_records": 4, "quarantined_records": 0, "skipped_records": 0,
                          "unqueued_changes": 0, "last_acknowledged_ms": NSNull(), "failure": NSNull(), "stopped": false],
             "sessions": ["total": 2, "shared": 1, "excluded": 1], "stage": "up_to_date", "stage_message": NSNull(),
             "needs_start": false, "journal_full": false, "upgrading": false, "stalled": false]
        }
        core.on("uploader.sessions.list") { _ in
            ["sharing_mode": "new", "sessions": [
                ["source": "codex", "session_id": "a", "title": "Fix tests", "is_agent": false, "included": true,
                 "status": "uploaded"],
                ["source": "claude", "session_id": "b", "title": "Docs", "is_agent": false, "included": false,
                 "status": "not_shared"],
            ]]
        }
        core.on("sessions.list") { _ in
            ["sessions": [["target": ["kind": "codex", "id": "a"], "title": "Fix tests", "live": true,
                           "registration": NSNull(), "suggested_name": "fix-tests"]]]
        }
        core.on("agents.list") { _ in ["agents": [], "archived": []] }
        core.on("roster.list") { _ in ["agents": [], "fetched_at_ms": 1, "unread_statuses": []] }
        core.on("claude_delivery.status") { _ in
            ["restricted": true,
             "sessions": [["id": "c-1", "agent": "review", "title": "Review", "direct": false]],
             "allowed_senders": [["address": "ada@desk", "name": "Ada"]],
             "held": [["id": "del-1", "agent": "review", "session_id": "c-1", "sender": "bo@desk",
                       "sender_name": "Bo", "text": "Please review", "held_at_ms": 1]]]
        }
        core.on("claude_delivery.restrict") { _ in [:] }
        core.on("claude_delivery.session") { _ in [:] }
        core.on("claude_delivery.sender") { _ in [:] }
        core.on("claude_delivery.approve") { _ in ["changed": true] }
        core.on("claude_delivery.dismiss") { _ in ["changed": true] }
        core.on("uploader.sessions.include") { params in ["updated": (params["session_keys"] as? [String])?.count ?? 0] }
        core.on("uploader.pause") { _ in ["paused": true] }
        core.on("uploader.sharing.set") { params in ["sharing_mode": params["mode"] ?? "new"] }
    }

    func testManualHealthCheckReadsVerifiedStatusAndReportsTransportFailure() async {
        var account = FakeCore.signedIn
        account["auth_health"] = "offline"
        core.setStatus(FakeCore.snapshot(revision: 2, account: account))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let connected = await eventually { model.isCoreConnected && model.phase == .attached }
        XCTAssertTrue(connected)
        let testCore = core!
        core.on("status.get") { _ in testCore.status }
        core.on("account.status") { _ in
            var verified = FakeCore.signedIn
            verified["workspace_mismatch"] = false
            verified["auth_health"] = "ready"
            verified["auth_checked_at"] = 100
            testCore.setStatus(FakeCore.snapshot(revision: 3, account: verified))
            return verified
        }
        await model.checkAccountHealth()
        XCTAssertEqual(model.account.authHealth, "ready")
        XCTAssertFalse(model.isCheckingAccountHealth)
        XCTAssertNil(model.accountHealthError)
        core.on("account.status") { _ in
            throw FakeCore.Failure(kind: "unavailable", message: "Health check unavailable")
        }
        await model.checkAccountHealth()
        XCTAssertFalse(model.isCheckingAccountHealth)
        XCTAssertNotNil(model.accountHealthError)
    }

    /// Uploader status and sessions arrive as `invalidated` events; the
    /// poll is a minute-long safety net, as on Linux.
    func testUploaderPollIsAMinuteLongSafetyNet() {
        XCTAssertEqual(AppModel.uploaderSafetyNet, 60_000_000_000)
    }

    func testEnforcedControlsAreRemovedAndWithheldUploadsLockTheRows() async {
        var answer: [String: Any] = [
            "running": true, "paused": false, "sharing_mode": "all",
            "collector_replacing": false, "collector_outdated": false,
            "delivery": ["bootstrap_complete": true, "pending_records": 0, "acknowledged_records": 0,
                         "quarantined_records": 0, "skipped_records": 0, "unqueued_changes": 0, "stopped": false],
            "sessions": ["total": 2, "shared": 2, "excluded": 0], "stage": "up_to_date",
            "needs_start": false, "journal_full": false, "upgrading": false, "stalled": false,
        ]
        let status = Locked(answer)
        core.on("uploader.status") { _ in status.value }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.phase == .attached && model.status != nil }
        XCTAssertTrue(ready)
        // Without enforced settings every control is the person's.
        XCTAssertNil(model.enforcedSharingMode)
        XCTAssertFalse(model.areUploadsWithheld)
        XCTAssertFalse(model.isUploadControlEnforced)
        XCTAssertTrue(model.canDeleteFromCloud)
        XCTAssertTrue(model.canTogglePause)
        XCTAssertEqual(GlobalSessionSharingSettings.presentation(
            enforced: model.enforcedSharingMode, withheld: model.areUploadsWithheld), .picker(lock: nil))

        // Settings the person keeps change nothing.
        answer["enforced_settings"] = ["upload_control": "user", "user_retroactive_delete": "true"]
        status.value = answer
        await model.refreshStatus()
        XCTAssertFalse(model.isUploadControlEnforced)
        XCTAssertTrue(model.canDeleteFromCloud)
        XCTAssertTrue(model.canTogglePause)

        // Each present setting removes its control.
        answer["enforced_settings"] = ["sharing_mode": "all", "upload_control": "operator",
                                       "user_retroactive_delete": "false", "oops_window_seconds": "900"]
        status.value = answer
        await model.refreshStatus()
        XCTAssertEqual(model.enforcedSharingMode, .all)
        XCTAssertFalse(model.areUploadsWithheld)
        XCTAssertEqual(GlobalSessionSharingSettings.presentation(
            enforced: model.enforcedSharingMode, withheld: model.areUploadsWithheld), .managed(.all))
        XCTAssertTrue(model.isUploadControlEnforced)
        XCTAssertFalse(model.canTogglePause)
        XCTAssertFalse(model.canDeleteFromCloud)
        XCTAssertEqual(OrganizationPolicy.managed, "Managed by your organization.")

        answer.removeValue(forKey: "enforced_settings")
        answer["uploads_withheld"] = true
        status.value = answer
        await model.refreshStatus()
        XCTAssertNil(model.enforcedSharingMode)
        XCTAssertTrue(model.areUploadsWithheld)
        XCTAssertEqual(GlobalSessionSharingSettings.presentation(
            enforced: model.enforcedSharingMode, withheld: model.areUploadsWithheld),
            .picker(lock: "Uploads wait until your organization's upload policy can be checked."))
    }

    func testMachineSessionTotalIgnoresPaginationFiltersAndHiddenRows() async {
        core.on("uploader.status") { _ in
            ["running": true, "paused": false, "sharing_mode": "new",
             "collector_replacing": false, "collector_outdated": false,
             "delivery": ["bootstrap_complete": true, "pending_records": 0, "acknowledged_records": 0,
                          "quarantined_records": 0, "skipped_records": 0, "unqueued_changes": 0, "stopped": false],
             "sessions": ["total": 12345, "shared": 1, "excluded": 12344], "stage": "up_to_date",
             "needs_start": false, "journal_full": false, "upgrading": false, "stalled": false]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        XCTAssertEqual(model.localSessionCountText, "Counting sessions…")
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.phase == .attached }
        XCTAssertTrue(ready)
        await model.refreshStatus()
        await model.refreshSessions()
        XCTAssertEqual(model.sessions.count, 2)
        XCTAssertEqual(model.localSessionCountText, "12345 sessions on this machine")
        model.sessionSearch = "no matching sessions"
        await model.setSessionsHidden(keys: ["claude:b"], hidden: true)
        model.page = .settings(.general)
        XCTAssertEqual(model.localSessionCountText, "12345 sessions on this machine")
    }

    func testRevealRequestsRelayRowsWithoutWaitingForHistory() async throws {
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.isCoreConnected && model.relay.sessions.count == 1 }
        XCTAssertTrue(ready)
        await model.connectionRefresh?.value
        let before = core.callCount("sessions.list")
        core.on("sessions.list") { _ in
            ["sessions": [["target": ["kind": "codex", "id": "new-thread"], "title": "Fresh reveal orchard", "live": true,
                           "registration": NSNull(), "suggested_name": "fresh-orchard"]]]
        }
        model.windowVisibilityChanged(true)
        let revealed = await eventually { model.relay.sessions.first?.target.id == "new-thread" }
        XCTAssertTrue(revealed, "Revealing the existing page must request the core's current rows")
        XCTAssertEqual(core.callCount("sessions.list"), before + 1)
        model.windowVisibilityChanged(true)
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(core.callCount("sessions.list"), before + 1, "Repeated visibility callbacks must not poll discovery")
        model.windowVisibilityChanged(false)
        model.windowVisibilityChanged(true)
        let refreshed = await eventually { self.core.callCount("sessions.list") == before + 2 }
        XCTAssertTrue(refreshed)
        model.panelDidAppear()
        let panelRefreshed = await eventually { self.core.callCount("sessions.list") == before + 3 }
        XCTAssertTrue(panelRefreshed)
        model.panelDidAppear()
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(core.callCount("sessions.list"), before + 3)
    }

    func testFullWindowRefreshesRelayOnlyQueryRowsWhileStatusIsHeld() async throws {
        let created = Locked(false)
        let tick = Locked(0)
        let row: [String: Any] = ["target": ["kind": "codex", "id": "fresh-full-window"],
            "title": "Fresh Full Window Orchard", "live": true, "registration": NSNull(), "suggested_name": "fresh-orchard"]
        core.on("sessions.list") { _ in
            var latest = row
            latest["updated_at_ms"] = tick.value
            return ["sessions": created.value ? [latest] : []]
        }
        core.on("uploader.sessions.query") { _ in
            ["sessions": [], "local_sessions": created.value ? [row] : [], "verification_pending": false,
             "revision": created.value ? 2 : 1, "unchanged": false, "candidates_scanned": 0, "next_cursor": NSNull()]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.phase == .attached && model.queriedLocalSessions != nil && model.status != nil }
        XCTAssertTrue(ready)
        let projectedRows = Locked<Int?>(nil)
        let host = NSHostingView(rootView: MainWindowView().environment(model)
            .onPreferenceChange(LocalSessionCountPreference.self) { projectedRows.value = $0 })
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1280, height: 800),
                              styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.makeKeyAndOrderFront(nil)
        defer { window.close() }
        try await Task.sleep(for: .milliseconds(150))
        let settled = await eventually { !model.isLoadingSessions }
        XCTAssertTrue(settled)
        let status = DispatchSemaphore(value: 0)
        defer { status.signal() }
        core.on("uploader.status") { _ in
            status.wait()
            throw FakeCore.Failure(kind: "internal", message: "Deliberately held status helper")
        }
        let statusReads = core.callCount("uploader.status")
        Task { await model.refreshStatus() }
        let held = await eventually { self.core.callCount("uploader.status") > statusReads }
        XCTAssertTrue(held)
        let queries = core.callCount("uploader.sessions.query")
        created.value = true
        core.publish("invalidated", ["what": "sessions"])
        let live = await eventually { model.relay.sessions.contains { $0.target.id == "fresh-full-window" } }
        XCTAssertTrue(live)
        let queried = await eventually(2) { model.queriedLocalSessions?.contains { $0.target.id == "fresh-full-window" } == true }
        XCTAssertTrue(queried, "A completed history query must not hide new relay-only sessions while status is held")
        XCTAssertGreaterThan(core.callCount("uploader.sessions.query"), queries)
        let renderDeadline = ProcessInfo.processInfo.systemUptime + 2
        let output = (ProcessInfo.processInfo.environment["REQUEST_RENDER_DIR"] ?? ProcessInfo.processInfo.environment["RELAY_NATIVE_VISIBILITY_ARTIFACTS"]).map {
            URL(fileURLWithPath: $0).appendingPathComponent("full-window-held-status.png")
        }
        // A committed query precedes the detached search projection and SwiftUI
        // publication. Let that real projection reach the full window before
        // asking Vision to inspect pixels; OCR must not delay the first paint.
        while projectedRows.value != 1 && ProcessInfo.processInfo.systemUptime < renderDeadline {
            host.layoutSubtreeIfNeeded()
            try await Task.sleep(for: .milliseconds(10))
        }
        let frame = try await NativeVisibilityTests.renderedFrame(title: "Fresh Full Window Orchard", deadline: renderDeadline, output: output,
            accepts: { $0.title("Fresh Full Window Orchard") != nil }) {
                host.layoutSubtreeIfNeeded()
                return try SessionRenderFixture.bitmap(for: host)
            }
        if let output { try NativeVisibilityTests.save(frame.bitmap, to: output) }
        XCTAssertTrue(frame.capturedAt <= renderDeadline && frame.title("Fresh Full Window Orchard") != nil,
            "Actual MainWindowView must render the fresh row within deadline (attempts \(frame.attempts)): \(frame.words.map(\.0))")
        let stable = core.callCount("uploader.sessions.query")
        tick.value = 99
        core.publish("invalidated", ["what": "sessions"])
        let timestamp = await eventually { model.relay.sessions.first?.updatedAtMs == 99 }
        XCTAssertTrue(timestamp)
        core.setStatus(FakeCore.snapshot(revision: 3, account: FakeCore.signedIn, push: ["unrelated": ["state": "live"]]))
        try await Task.sleep(for: .milliseconds(180))
        XCTAssertEqual(core.callCount("uploader.sessions.query"), stable, "Timestamp/push-only activity must not reset a query page")
    }

    func testCoreQueryPagesReplaceWorkingSetAndUnchangedRepliesKeepIt() async throws {
        let initialRead = DispatchSemaphore(value: 0)
        let backgroundRead = DispatchSemaphore(value: 0)
        defer { initialRead.signal(); backgroundRead.signal() }
        let queryRequests = Locked(0)
        core.on("uploader.sessions.query") { params in
            queryRequests.value += 1
            if queryRequests.value == 1 { initialRead.wait() }
            else if queryRequests.value == 2 { backgroundRead.wait() }
            let cursor = params["cursor"] as? String
            let unchanged = params["known_revision"] as? Int == 42
            return ["sessions": unchanged ? [] : [["source": "codex", "session_id": cursor == nil ? "first" : "old-match", "title": "CPU history", "is_agent": false, "included": true, "status": "uploaded"]],
                    "local_sessions": [], "verification_pending": false, "revision": 42, "unchanged": unchanged, "candidates_scanned": unchanged ? 0 : 200,
                    "next_cursor": cursor == nil ? "next" as Any : NSNull()]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let readingInitial = await eventually { self.core.callCount("uploader.sessions.query") == 1 && model.isLoadingSessions }
        XCTAssertTrue(readingInitial)
        // Queue a refresh while the first read is held, as startup can do.
        // Its reply leaves the first page visible while the coalesced read runs.
        await model.refreshSessions()
        initialRead.signal()
        let loaded = await eventually { model.sessions.first?.sessionId == "first" && self.core.callCount("uploader.sessions.query") == 2 }; XCTAssertTrue(loaded)
        XCTAssertTrue(model.isLoadingSessions)
        XCTAssertFalse(model.canLoadMoreSessions, "Visible first-page rows do not mean the background read is done")
        backgroundRead.signal()
        await model.connectionRefresh?.value
        let ready = await eventually {
            model.canLoadMoreSessions && model.relay.sessions.count == 1 && !model.relay.isRefreshing
        }
        XCTAssertTrue(ready)
        let beforePaging = core.callCount("uploader.sessions.query")
        await model.loadMoreSessions()
        XCTAssertEqual(core.allParams("uploader.sessions.query").dropFirst(beforePaging).first?["cursor"] as? String, "next")
        XCTAssertEqual(model.sessions.map(\.sessionId), ["old-match"])
        XCTAssertEqual(model.sessionLimit, 500)
        let rowsRevision = model.sessionRowsRevision
        let beforeUnchanged = core.callCount("uploader.sessions.query")
        await model.refreshSessions()
        let unchanged = await eventually {
            self.core.callCount("uploader.sessions.query") > beforeUnchanged
                && self.core.lastParams("uploader.sessions.query")?["known_revision"] as? Int == 42
                && !model.isLoadingSessions
        }
        XCTAssertTrue(unchanged)
        XCTAssertEqual(model.sessions.map(\.sessionId), ["old-match"])
        XCTAssertEqual(model.sessionRowsRevision, rowsRevision)
        XCTAssertTrue(core.calls.filter { $0.method == "uploader.sessions.query" }.allSatisfy { $0.params["limit"] as? Int == 200 })
        XCTAssertFalse(core.calls.contains { $0.method == "uploader.sessions.list" })
    }

    func testCoreQueryFencesResponseFromAnEarlierSearch() async throws {
        let oldReply = DispatchSemaphore(value: 0)
        defer { oldReply.signal() }
        core.on("uploader.sessions.query") { params in
            let query = params["query"] as? String ?? ""
            if query == "old" { oldReply.wait() }
            return ["sessions": [["source": "codex", "session_id": query.isEmpty ? "initial" : query, "title": query, "is_agent": false, "included": false, "status": "not_shared"]],
                    "local_sessions": [], "verification_pending": false, "revision": 1, "unchanged": false, "candidates_scanned": 1, "next_cursor": NSNull()]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let loaded = await eventually { !model.sessions.isEmpty }; XCTAssertTrue(loaded)
        let observation = ObservationProbe { model.sessions.map(\.sessionId) }
        defer { observation.stop() }
        model.sessionSearch = "old"
        let requested = await eventually { self.core.calls.contains { $0.method == "uploader.sessions.query" && $0.params["query"] as? String == "old" } }; XCTAssertTrue(requested)
        model.sessionSearch = "new"
        oldReply.signal()
        let settled = await eventually { model.sessions.first?.sessionId == "new" }; XCTAssertTrue(settled)
        XCTAssertFalse(observation.values.joined().contains("old"))
    }

    func testInitialUsageWaitsForLegacyDiagnosticsOptOut() async {
        defaults.set(false, forKey: LegacyMigration.analyticsEnabledKey)
        let testCore = core!
        core.on("telemetry.record") { [weak testCore] _ in
            XCTAssertEqual(testCore?.lastParams("settings.set")?["analytics_enabled"] as? Bool, false,
                           "Usage must not reach the core before the legacy opt-out")
            return [:]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let recorded = await eventually {
            testCore.allParams("telemetry.record").contains { $0["operation"] as? String == "page_view" }
        }
        XCTAssertTrue(recorded)
    }

    func testUsageTelemetryTracksNavigationAndDebouncesSearchWithoutContent() async throws {
        core.on("telemetry.record") { _ in [:] }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let connected = await eventually {
            model.isCoreConnected && model.phase == .attached
                && self.core.allParams("telemetry.record").contains { $0["operation"] as? String == "app_launch" }
        }
        XCTAssertTrue(connected)
        model.sessionSearch = "private query"
        model.sessionSearch = "private query revised"
        let searched = await eventually {
            self.core.allParams("telemetry.record").contains { $0["operation"] as? String == "search" }
        }
        XCTAssertTrue(searched)
        model.sessionSearch = ""
        model.page = .settings(.general) // sidebar's direct assignment
        model.showSettingsWindow(.general) // no duplicate view
        model.isComposingMessage = true
        let opened = await eventually {
            self.core.allParams("telemetry.record").contains { $0["operation"] as? String == "message_composer" }
        }
        XCTAssertTrue(opened)
        try await Task.sleep(nanoseconds: 650_000_000)
        let events = core.allParams("telemetry.record")
        let searches = events.filter { $0["operation"] as? String == "search" }
        XCTAssertEqual(searches.count, 1)
        XCTAssertEqual(searches.first?["surface"] as? String, "sessions")
        XCTAssertEqual(events.filter { $0["operation"] as? String == "settings" }.count, 1)
        XCTAssertTrue(events.contains { $0["operation"] as? String == "page_view" && $0["surface"] as? String == "sessions" })
        let payload = String(data: try JSONSerialization.data(withJSONObject: events), encoding: .utf8)!
        XCTAssertFalse(payload.contains("private"))
    }

    func testDestinationSearchAppliedBeforeNavigationIsRecorded() async throws {
        core.on("telemetry.record") { _ in [:] }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let launched = await eventually {
            self.core.allParams("telemetry.record").contains { $0["operation"] as? String == "app_launch" }
        }
        XCTAssertTrue(launched)
        model.page = .settings(.general)
        model.showLocalSessions(search: "private destination")
        let searched = await eventually {
            self.core.allParams("telemetry.record").contains { $0["operation"] as? String == "search" }
        }
        XCTAssertTrue(searched)
        try await Task.sleep(nanoseconds: 100_000_000)
        let events = core.allParams("telemetry.record").filter { $0["operation"] as? String == "search" }
        XCTAssertEqual(events.count, 1)
        XCTAssertEqual(events.first?["surface"] as? String, "sessions")
    }

    func testNavigationCancelsPendingSearchUsageEvenAfterReturning() async throws {
        core.on("telemetry.record") { _ in [:] }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let launched = await eventually {
            self.core.allParams("telemetry.record").contains { $0["operation"] as? String == "app_launch" }
        }
        XCTAssertTrue(launched)
        model.sessionSearch = "private sessions"
        model.showSettingsWindow(.general)
        model.page = .sessions // Returning must not revive the cancelled debounce.
        model.sessionSearch = "private composer transition"
        model.isComposingMessage = true
        model.isComposingMessage = false
        try await Task.sleep(nanoseconds: 650_000_000)
        XCTAssertFalse(core.allParams("telemetry.record").contains { $0["operation"] as? String == "search" })
    }

    func testMessageLinkAndSettingsRecordOnlyVisibleDestinations() async {
        core.on("telemetry.record") { _ in [:] }
        core.on("links.parse") { _ in ["kind": "message", "endpoint": "web@desk", "address": true] }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let launched = await eventually {
            self.core.allParams("telemetry.record").contains { $0["operation"] as? String == "app_launch" }
        }
        XCTAssertTrue(launched)
        let sessionsVisible = await eventually {
            self.core.allParams("telemetry.record").contains {
                $0["operation"] as? String == "page_view" && $0["surface"] as? String == "sessions"
            }
        }
        XCTAssertTrue(sessionsVisible)
        let before = core.allParams("telemetry.record").count
        model.openConnection(URL(string: "agentrelay://message?to=web%40desk")!)
        let messageVisible = await eventually {
            self.core.allParams("telemetry.record").contains {
                $0["operation"] as? String == "page_view" && $0["surface"] as? String == "messages"
            }
        }
        XCTAssertTrue(messageVisible)
        model.showSettingsWindow(.general)
        let settingsVisible = await eventually {
            self.core.allParams("telemetry.record").contains { $0["operation"] as? String == "settings" }
        }
        XCTAssertTrue(settingsVisible)
        let events = Array(core.allParams("telemetry.record").dropFirst(before))
        XCTAssertEqual(events.filter { $0["operation"] as? String == "page_view" }
            .compactMap { $0["surface"] as? String }, ["messages", "settings_general"])
        XCTAssertEqual(events.filter { $0["operation"] as? String == "message_composer" }.count, 1)
    }

    func testColdMessageLinkRecordsComposerAfterDiagnosticsMigration() async {
        let settingsGate = DispatchSemaphore(value: 0)
        defer { settingsGate.signal() }
        defaults.set(false, forKey: LegacyMigration.analyticsEnabledKey)
        core.on("settings.get") { _ in
            _ = settingsGate.wait(timeout: .now() + 5)
            return Self.settings(analytics: true)
        }
        core.on("links.parse") { _ in ["kind": "message", "endpoint": "web@desk", "address": true] }
        let testCore = core!
        core.on("telemetry.record") { [weak testCore] _ in
            XCTAssertEqual(testCore?.lastParams("settings.set")?["analytics_enabled"] as? Bool, false)
            return [:]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        model.openConnection(URL(string: "agentrelay://message?to=web%40desk")!)
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let opened = await eventually { model.isComposingMessage }
        XCTAssertTrue(opened)
        XCTAssertEqual(core.callCount("telemetry.record"), 0)
        settingsGate.signal()
        let recorded = await eventually {
            self.core.allParams("telemetry.record").contains { $0["operation"] as? String == "message_composer" }
        }
        XCTAssertTrue(recorded)
        await model.connectionRefresh?.value
        let events = core.allParams("telemetry.record")
        XCTAssertEqual(events.filter { $0["operation"] as? String == "message_composer" }.count, 1)
        XCTAssertFalse(events.contains { $0["operation"] as? String == "page_view" && $0["surface"] as? String == "sessions" })
    }

    func testAccountMenuKeepsSettingsWithoutOfferingAgentThreadCreation() async throws {
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.phase == .attached }
        XCTAssertTrue(ready)

        let updater = AppUpdater(defaults: defaults, currentVersion: "development build", terminate: {})
        let menu = HeaderAccountMenu(model: model, updater: updater,
                                     connection: WorkspaceConnection(model: model, relay: model.relay),
                                     name: "Alex Morgan", email: "alex@example.com",
                                     isShowingMenu: .constant(true), isConfirmingLogOut: .constant(false),
                                     isConfirmingWorkspaceSignOut: .constant(false))
        let host = NSHostingView(rootView: menu)
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 270, height: 320),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.orderFront(nil)
        defer { window.close() }
        try await Task.sleep(for: .milliseconds(100))
        host.layoutSubtreeIfNeeded()

        let bitmap = try SessionRenderFixture.bitmap(for: host)
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        try VNImageRequestHandler(cgImage: XCTUnwrap(bitmap.cgImage), options: [:]).perform([request])
        let text = (request.results ?? []).compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        XCTAssertTrue(text.localizedCaseInsensitiveContains("Settings"), text)
        XCTAssertTrue(text.localizedCaseInsensitiveContains("Log out"), text)
        XCTAssertFalse(text.localizedCaseInsensitiveContains("Capy"), text)
    }

    func testMenuPanelSummarizesAgentsAndUploads() async throws {
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        var sessions: [[String: Any]] = []
        for index in 0..<12 {
            var session: [String: Any] = ["is_agent": false, "included": index < 5, "status": index < 5 ? "uploaded" : "not_shared"]
            session["source"] = index % 2 == 0 ? "codex" : "claude"
            session["session_id"] = "recent-\(index)"
            session["title"] = index == 11 ? "Add signup buttons to homepage" : "Recent session \(index)"
            session["cwd"] = "/work/agentrelay.com"
            session["last_activity_ms"] = now - Int64((12 - index) * 60000)
            sessions.append(session)
        }
        core.on("uploader.sessions.list") { _ in ["sharing_mode": "new", "sessions": sessions] }
        core.on("sessions.list") { _ in
            ["sessions": [("a", true), ("b", true), ("c", false)].map { id, onRelay -> [String: Any] in
                ["target": ["kind": "codex", "id": id], "title": id, "live": true,
                 "registration": onRelay ? ["name": id, "address": "\(id)@mac", "added_by_session": false] as Any : NSNull(),
                 "suggested_name": id]
            }]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.phase == .attached && model.sessions.count == 12 }
        XCTAssertTrue(ready)
        await model.relay.refreshSessions()
        let rows = model.menuSessionRows(relay: model.relay)
        let count = { (filter: MenuSummaryFilter) in filter.count(rows) { $0.upload.map(model.uploadState) } }
        XCTAssertEqual(count(.receiving), 2)
        XCTAssertEqual(count(.localOnly), 1)
        XCTAssertEqual(count(.uploaded), 5)
        XCTAssertEqual(count(.notUploaded), rows.count - 5)
        XCTAssertEqual(model.sessionCount(for: .uploaded), 5)
        let host = NSHostingView(rootView: MenuPanelView(relay: model.relay)
            .environment(model).preferredColorScheme(.dark))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 360, height: 560),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.appearance = NSAppearance(named: .darkAqua)
        window.contentView = host
        window.orderFront(nil)
        defer { window.close() }
        try await Task.sleep(for: .milliseconds(100))
        host.layoutSubtreeIfNeeded()
        XCTAssertEqual(host.fittingSize.width, 260, accuracy: 1)
        XCTAssertLessThanOrEqual(host.fittingSize.height, 300)
        XCTAssertEqual(window.contentRect(forFrameRect: window.frame).width, 260, accuracy: 1)
        XCTAssertEqual(window.contentRect(forFrameRect: window.frame).height, host.fittingSize.height, accuracy: 1)
        if let directory = ProcessInfo.processInfo.environment["MENU_RENDER_DIR"] {
            try FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
            let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
            host.cacheDisplay(in: host.bounds, to: bitmap)
            let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
            try data.write(to: URL(fileURLWithPath: directory).appendingPathComponent("menu-summary.png"))
        }
    }

    func testMenuPanelResizesNativeWindowAcrossSignIn() async throws {
        let model = model()
        defer { model.shutdown() }
        let host = NSHostingView(rootView: MenuPanelView(relay: model.relay).environment(model))
        // Model the menu host retaining its initial native size even though
        // the SwiftUI content reports a new fitting size.
        host.sizingOptions = []
        let window = NSWindow(contentRect: NSRect(x: 100, y: 100, width: 350, height: 500),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.orderFront(nil)
        defer { window.close() }
        try await Task.sleep(for: .milliseconds(150))
        host.layoutSubtreeIfNeeded()
        let signedOutSize = window.contentRect(forFrameRect: window.frame).size
        XCTAssertEqual(signedOutSize.width, 350, accuracy: 1)
        XCTAssertGreaterThan(signedOutSize.height, 300)
        let top = window.frame.maxY
        let left = window.frame.minX

        // Sign In closes the menu before browser sign-in completes, so the
        // first summary must acquire a height even though it is still hidden.
        window.orderOut(nil)
        try await Task.sleep(for: .milliseconds(150))
        XCTAssertEqual(model.menuSummaryReads, 0)

        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        await model.bootstrapIfNeeded()
        let attached = await eventually { model.phase == .attached }
        XCTAssertTrue(attached)
        host.layoutSubtreeIfNeeded()
        try await Task.sleep(for: .milliseconds(150))
        XCTAssertGreaterThan(window.contentRect(forFrameRect: window.frame).height, 100,
                             "First sign-in while closed must not collapse the panel to an unmeasured placeholder")
        window.orderFront(nil)
        let shrank = await eventually {
            model.phase == .attached && window.contentRect(forFrameRect: window.frame).width == 260
                && window.contentRect(forFrameRect: window.frame).height <= 300
        }
        XCTAssertTrue(shrank, "The native panel, not only its SwiftUI content, must shrink after sign-in")
        XCTAssertEqual(window.frame.maxY, top, accuracy: 1, "Resizing keeps the menu's top edge anchored")
        XCTAssertEqual(window.frame.minX, left, accuracy: 1)

        core.setStatus(FakeCore.snapshot(revision: 3, account: ["state": "signed_out"]))
        let grew = await eventually {
            model.phase == .signedOut && window.contentRect(forFrameRect: window.frame).size == signedOutSize
        }
        XCTAssertTrue(grew, "Signing out must restore the larger panel without clipping it")
        XCTAssertEqual(window.frame.maxY, top, accuracy: 1)
        XCTAssertEqual(window.frame.minX, left, accuracy: 1)
    }

    func testClosedMenuPanelStopsCountingSessions() async throws {
        core.on("uploader.sessions.list") { _ in
            ["sharing_mode": "new", "sessions": (0..<4).map { index -> [String: Any] in
                ["is_agent": false, "included": index < 2, "status": index < 2 ? "uploaded" : "not_shared", "source": "codex",
                 "session_id": "recent-\(index)", "title": "Recent session \(index)", "cwd": "/work/agentrelay.com"]
            }]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.phase == .attached && model.sessions.count == 4 }
        XCTAssertTrue(ready)
        let host = NSHostingView(rootView: MenuPanelView(relay: model.relay).environment(model))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 360, height: 560),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.orderFront(nil)
        defer { window.close() }
        try await Task.sleep(for: .milliseconds(150))
        host.layoutSubtreeIfNeeded()
        let shownHeight = host.fittingSize.height
        let shownLeft = window.frame.minX
        let shownTop = window.frame.maxY
        XCTAssertGreaterThan(model.menuSummaryReads, 0)

        // Closed: the models keep changing, the panel counts nothing.
        window.orderOut(nil)
        let hidden = await eventually { model.menuSummaryReads > 0 && !window.isVisible }
        XCTAssertTrue(hidden)
        try await Task.sleep(for: .milliseconds(150))
        let readsWhileClosed = model.menuSummaryReads
        for _ in 0..<3 {
            await model.refreshSessions()
            model.windowVisibilityChanged(true)
            model.windowVisibilityChanged(false)
        }
        try await Task.sleep(for: .milliseconds(200))
        XCTAssertEqual(model.menuSummaryReads, readsWhileClosed, "A closed panel must not count sessions")
        host.layoutSubtreeIfNeeded()
        XCTAssertEqual(host.fittingSize.height, shownHeight, accuracy: 1, "The closed panel keeps its size")
        XCTAssertEqual(window.contentRect(forFrameRect: window.frame).height, shownHeight, accuracy: 1)

        // Opened again: counted afresh.
        window.orderFront(nil)
        let counted = await eventually { model.menuSummaryReads > readsWhileClosed }
        XCTAssertTrue(counted)
        XCTAssertEqual(window.contentRect(forFrameRect: window.frame).width, 260, accuracy: 1)
        XCTAssertEqual(window.contentRect(forFrameRect: window.frame).height, shownHeight, accuracy: 1)
        XCTAssertEqual(window.frame.minX, shownLeft, accuracy: 1)
        XCTAssertEqual(window.frame.maxY, shownTop, accuracy: 1)
    }

    /// Status replies whose counters move, a new snapshot revision that
    /// changes nothing shown, and edits to state other pages keep re-render
    /// none of the window's always-on views or the menu bar icon; a change to
    /// what one of them shows re-renders that one.
    func testUnrelatedModelChangesDoNotRerenderTheWindow() async throws {
        let pending = Locked(0)
        core.on("uploader.status") { _ in
            ["running": true, "paused": false, "sharing_mode": "new", "directory": "/tmp/install",
             "collector_replacing": false, "collector_outdated": false,
             "delivery": ["bootstrap_complete": true, "pending_records": pending.value, "acknowledged_records": 4, "quarantined_records": 0, "skipped_records": 0,
                          "unqueued_changes": 0, "last_acknowledged_ms": NSNull(), "failure": NSNull(), "stopped": false],
             "sessions": ["total": 2, "shared": 1, "excluded": 1], "stage": "up_to_date", "stage_message": NSNull(),
             "needs_start": false, "journal_full": false, "upgrading": false, "stalled": false]
        }
        let registered = Locked(false)
        core.on("sessions.list") { _ in
            ["sessions": [["target": ["kind": "codex", "id": "a"], "title": "Fix tests", "live": true,
                           "registration": registered.value ? ["name": "fix-tests", "address": "fix-tests@mac", "added_by_session": false] as Any : NSNull(),
                           "suggested_name": "fix-tests"]]]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.phase == .attached && model.sessions.count == 2 && model.relay.sessions.count == 1 && model.status != nil }
        XCTAssertTrue(ready)
        await model.connectionRefresh?.value

        let views = ["WorkspaceStatusBar", "HeaderAccountButton", "MenuBarLabel", "LocalSessionsView"]
        let host = NSHostingView(rootView: VStack {
            HeaderAccountButton(relay: model.relay, updater: AppUpdater.shared)
            MenuBarLabel(relay: model.relay)
            LocalSessionsView(relay: model.relay).frame(height: 300)
            WorkspaceStatusBar(relay: model.relay)
        }.environment(model))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1200, height: 500), styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.orderFront(nil)
        defer { window.close() }
        let shown = await eventually { model.sessionResults.generation > 0 }
        XCTAssertTrue(shown)
        func settle() async throws {
            try await Task.sleep(for: .milliseconds(250))
            host.layoutSubtreeIfNeeded()
        }
        try await settle()
        let before = Dictionary(uniqueKeysWithValues: views.map { ($0, BodyEvaluations.count($0)) })
        XCTAssertTrue(before.values.allSatisfy { $0 > 0 }, "every view rendered: \(before)")

        // Unrelated: delivery counters, a snapshot revision, other pages' state.
        for count in 1...5 {
            pending.value = count
            await model.refreshStatus()
        }
        core.setStatus(FakeCore.snapshot(revision: 3, account: FakeCore.signedIn))
        let revised = await eventually { model.core?.revision == 3 }
        XCTAssertTrue(revised)
        model.messageEndpoint = "@bot"
        await model.relay.refresh(.roster)
        await model.relay.refresh(.agents)
        try await settle()
        for view in views {
            XCTAssertEqual(BodyEvaluations.count(view), before[view], "\(view) re-rendered for a change it does not show")
        }

        // Related: a session goes on the relay; the footer counts it.
        let footer = BodyEvaluations.count("WorkspaceStatusBar")
        let icon = BodyEvaluations.count("MenuBarLabel")
        registered.value = true
        try await model.relay.loadSessions()
        XCTAssertEqual(model.relay.liveOnRelayCount, 1)
        try await settle()
        XCTAssertGreaterThan(BodyEvaluations.count("WorkspaceStatusBar"), footer)
        XCTAssertEqual(BodyEvaluations.count("MenuBarLabel"), icon, "the icon shows no session count")
    }

    /// Derived values are kept by the models: reading them does no work, and
    /// a change to one input notifies only the readers of what it changed.
    func testDerivedStateNotifiesOnlyForItsInputs() async throws {
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.phase == .attached && model.relay.sessions.count == 1 && model.status != nil }
        XCTAssertTrue(ready)
        await model.connectionRefresh?.value
        try await Task.sleep(for: .milliseconds(100))

        let phase = ObservationProbe { model.phase }
        let problem = ObservationProbe { model.relayProblem }
        let count = ObservationProbe { model.localSessionCountText }
        let listed = ObservationProbe { model.relay.liveOnRelayCount }
        defer { phase.stop(); problem.stop(); count.stop(); listed.stop() }
        core.setStatus(FakeCore.snapshot(revision: 3, account: FakeCore.signedIn))
        _ = await eventually { model.core?.revision == 3 }
        await model.refreshStatus()
        model.messageEndpoint = "x"
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(phase.changes, 0)
        XCTAssertEqual(problem.changes, 0)
        XCTAssertEqual(count.changes, 0)
        XCTAssertEqual(listed.changes, 0)

        core.setStatus(FakeCore.snapshot(revision: 4, account: FakeCore.signedIn).merging(["agents": ["store_error": "disk full", "delivery_error": NSNull()]]) { $1 })
        let failed = await eventually { model.relayProblem == "disk full" }
        XCTAssertTrue(failed)
        XCTAssertEqual(problem.changes, 1)
        XCTAssertEqual(phase.changes, 0)
    }

    /// With the window and the panel out of sight nothing periodic runs:
    /// the request log is not read, the Sessions table is not recomputed,
    /// and the clocks inside are told the window is hidden.
    func testHiddenWindowStopsPeriodicWork() async throws {
        core.on("diagnostics.requests") { _ in ["entries": []] }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.phase == .attached && model.sessions.count == 2 }
        XCTAssertTrue(ready)

        let shownStates = Locked<[Bool]>([])
        let host = NSHostingView(rootView: VStack {
            LocalSessionsView(relay: model.relay).frame(height: 300)
            VisibilityProbe { shownStates.value.append($0) }
        }
        .tracksWindowVisibility { model.windowVisibilityChanged($0) }
        .environment(model))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1200, height: 400), styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.orderFront(nil)
        defer { window.close() }

        // Shown on Diagnostics: the log is read every 2 s and the table is fed.
        model.page = .settings(.diagnostics)
        let reading = await eventually { self.core.callCount("diagnostics.requests") >= 2 }
        XCTAssertTrue(reading, "Diagnostics reads the log while the window shows it")
        let fed = await eventually { model.sessionResults.generation > 0 }
        XCTAssertTrue(fed)

        window.orderOut(nil)
        let hidden = await eventually { shownStates.value.last == false }
        XCTAssertTrue(hidden, "views inside learn the window is hidden")
        let reads = core.callCount("diagnostics.requests")
        let generation = model.sessionResults.generation
        core.on("uploader.sessions.list") { _ in
            ["sharing_mode": "new", "sessions": [
                ["source": "codex", "session_id": "c", "title": "New while hidden", "is_agent": false, "included": false, "status": "not_shared"],
            ]]
        }
        await model.refreshSessions()
        XCTAssertEqual(model.sessions.count, 1)
        try await Task.sleep(for: .milliseconds(2_500))
        XCTAssertEqual(core.callCount("diagnostics.requests"), reads, "a hidden window reads no request log")
        XCTAssertEqual(model.sessionResults.generation, generation, "a hidden table is not recomputed")

        window.orderFront(nil)
        let caughtUp = await eventually { model.sessionResults.generation > generation && self.core.callCount("diagnostics.requests") > reads }
        XCTAssertTrue(caughtUp, "shown again, the table and the log catch up at once")
    }

    /// An open transcript covers the table: session changes meanwhile do
    /// not rebuild it, and closing the transcript catches it up at once.
    func testOpenTranscriptStopsRebuildingTheTable() async throws {
        core.on("sessions.transcript") { _ in ["entries": [], "next_cursor": NSNull()] }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.phase == .attached && model.sessions.count == 2 }
        XCTAssertTrue(ready)
        let host = NSHostingView(rootView: LocalSessionsView(relay: model.relay).frame(height: 400).environment(model))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1200, height: 400), styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.orderFront(nil)
        defer { window.close() }
        let fed = await eventually { model.sessionResults.rows.count == 2 }
        XCTAssertTrue(fed)

        model.selectedLocalSession = model.sessionResults.rows.first
        try await Task.sleep(for: .milliseconds(200))
        let generation = model.sessionResults.generation
        core.on("uploader.sessions.list") { _ in
            ["sharing_mode": "new", "sessions": [
                ["source": "codex", "session_id": "c", "title": "New while reading", "is_agent": false, "included": false, "status": "not_shared"],
            ]]
        }
        await model.refreshSessions()
        XCTAssertEqual(model.sessions.count, 1)
        try await Task.sleep(for: .milliseconds(400))
        XCTAssertEqual(model.sessionResults.generation, generation, "the covered table is not rebuilt")

        model.selectedLocalSession = nil
        let caughtUp = await eventually { model.sessionResults.generation > generation && model.sessionResults.rows.contains { $0.sessionID == "c" } }
        XCTAssertTrue(caughtUp, "closing the transcript shows the current sessions")
    }

    func testAccountReattachmentLandsOnSessions() async {
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        XCTAssertEqual(model.page, .sessions)
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.phase == .attached }
        XCTAssertTrue(ready)
        model.showSettingsWindow(.about)

        core.setStatus(FakeCore.snapshot(revision: 3, account: ["state": "pending_approval", "phase": "waiting_for_browser"]))
        let waiting = await eventually { model.phase == .waitingForBrowser }
        XCTAssertTrue(waiting)
        core.setStatus(FakeCore.snapshot(revision: 4, account: FakeCore.signedIn))
        let reattached = await eventually { model.phase == .attached }
        XCTAssertTrue(reattached)
        XCTAssertEqual(model.page, .sessions)
    }

    func testSessionSearchClosesOnNavigationAndAccountReset() async {
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.phase == .attached }
        XCTAssertTrue(ready)
        model.showsSessionSearch = true
        model.showSettingsWindow()
        XCTAssertFalse(model.showsSessionSearch)
        model.showLocalSessions()
        XCTAssertFalse(model.showsSessionSearch)
        model.showsSessionSearch = true
        model.selectedLocalSession = LocalSessionRow.merge(uploads: [UploadSession(included: false, isAgent: false,
            sessionId: "selected", source: "codex", status: .notShared, title: "Selected")], sessions: [])[0]
        XCTAssertFalse(model.showsSessionSearch)
        model.showLocalSessions()
        model.showsSessionSearch = true
        core.setStatus(FakeCore.snapshot(revision: 3, account: ["state": "signed_out"]))
        let signedOut = await eventually { model.phase == .signedOut }
        XCTAssertTrue(signedOut)
        XCTAssertFalse(model.showsSessionSearch)
    }

    func testStaleMenuClickCannotOpenHiddenSession() {
        defaults.set(["codex:hidden"], forKey: "hiddenSessions.v1")
        let model = model()
        var opened: [String] = []
        model.registerOpenWindow { opened.append($0) }
        let row = LocalSessionRow.merge(uploads: [UploadSession(included: false, isAgent: false,
            sessionId: "hidden", source: "codex", status: .notShared, title: "Hidden")], sessions: [])[0]
        model.openSessionFromMenu(row)
        XCTAssertNil(model.selectedLocalSession)
        XCTAssertTrue(opened.isEmpty)
    }

    func testMenuBarIconTurnsOrangeWhenSomethingNeedsAttention() throws {
        XCTAssertTrue(BrandMark.menuBarImage(state: .normal).isTemplate)
        XCTAssertTrue(BrandMark.menuBarImage(state: .paused).isTemplate)
        let attention = BrandMark.menuBarImage(state: .attention, tint: .systemGreen)
        XCTAssertFalse(attention.isTemplate)
        let bitmap = try XCTUnwrap(NSBitmapImageRep(data: try XCTUnwrap(attention.tiffRepresentation)))
        let painted = (0..<bitmap.pixelsWide).flatMap { x in (0..<bitmap.pixelsHigh).compactMap { bitmap.colorAt(x: x, y: $0) } }
            .first { $0.alphaComponent == 1 }
        let pixel = try XCTUnwrap(painted?.usingColorSpace(.sRGB))
        // The warning orange in either appearance, not the Dev build's green.
        XCTAssertGreaterThan(pixel.redComponent, pixel.greenComponent + 0.2)
        XCTAssertGreaterThan(pixel.greenComponent, pixel.blueComponent)
    }

    func testAProfileBuildsMenuBarIconIsWiderForItsBadge() {
        let plain = BrandMark.menuBarImage(state: .normal)
        let badged = BrandMark.menuBarImage(state: .normal, badge: "d4vq8z")
        XCTAssertEqual(plain.size.width, BrandMark.menuBarLength)
        XCTAssertGreaterThan(badged.size.width, plain.size.width)
        XCTAssertEqual(badged.size.height, plain.size.height)
        XCTAssertTrue(badged.isTemplate)
    }

    func testMenuOpensExactSessionAndFilteredSessions() {
        let model = model()
        var opened: [String] = []
        model.registerOpenWindow { opened.append($0) }
        let row = LocalSessionRow.merge(uploads: [UploadSession(included: false, isAgent: false,
            sessionId: "menu-session", source: "codex", status: .notShared, title: "Recent")], sessions: [])[0]
        model.page = .settings(.general)
        model.openSessionFromMenu(row)
        XCTAssertEqual(model.page, .sessions)
        XCTAssertEqual(model.selectedLocalSession?.id, "codex:menu-session")
        XCTAssertFalse(model.showsSessionSearch)
        model.showSessionsFromMenu(search: MenuSummaryFilter.receiving.search)
        XCTAssertEqual(model.page, .sessions)
        XCTAssertNil(model.selectedLocalSession)
        XCTAssertEqual(model.sessionSearch, "is:active is:on-relay")
        model.showDiagnosticsFromMenu()
        XCTAssertEqual(model.page, .settings(.diagnostics))
        XCTAssertEqual(opened, [AppInfo.mainWindowID, AppInfo.mainWindowID, AppInfo.mainWindowID])
    }

    func testHidingStopsSharingWithoutDeletingAndBlocksReenableUntilUnhidden() async {
        var registered = true
        var included = true
        core.on("sessions.list") { _ in
            ["sessions": [["target": ["kind": "codex", "id": "a"], "title": "Fix tests", "live": true,
                "registration": registered ? ["name": "fix-tests", "address": "fix-tests@mac", "added_by_session": false] as Any : NSNull(), "suggested_name": "fix-tests"]]]
        }
        core.on("uploader.sessions.list") { _ in
            ["sharing_mode": "new", "sessions": [["source": "codex", "session_id": "a", "title": "Fix tests", "is_agent": false, "included": included, "status": included ? "uploaded" : "not_shared"]]]
        }
        core.on("agents.unregister") { _ in registered = false; return [:] }
        core.on("uploader.sessions.exclude") { params in
            XCTAssertFalse(registered, "Relay must stop before excluding uploads")
            XCTAssertEqual(params["session_keys"] as? [String], ["codex:a"])
            included = false
            return ["updated": 1]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { !model.sessions.isEmpty && !model.relay.sessions.isEmpty }
        XCTAssertTrue(ready)
        let session = model.relay.sessions[0]
        await model.setSessionsHidden(keys: ["codex:a"], hidden: true)
        XCTAssertEqual(core.callCount("agents.unregister"), 1)
        XCTAssertEqual(core.callCount("uploader.sessions.exclude"), 1)
        XCTAssertEqual(core.callCount("uploader.sessions.delete"), 0)
        XCTAssertTrue(model.hiddenSessionIDs.contains("codex:a"))
        XCTAssertFalse(model.sessions[0].included)
        model.showHiddenSessions = true
        let restored = AppModel(manageSystemServices: false, defaults: defaults)
        XCTAssertTrue(restored.hiddenSessionIDs.contains("codex:a"))
        XCTAssertTrue(restored.showHiddenSessions)
        let accepted = await model.includeSessions(keys: ["codex:a"])
        XCTAssertFalse(accepted)
        model.relay.requestRelay(true, for: session)
        await model.relay.register(session, as: "hidden")
        XCTAssertEqual(core.callCount("uploader.sessions.include"), 0)
        XCTAssertEqual(core.callCount("agents.register"), 0)
        await model.setSessionsHidden(keys: ["codex:a"], hidden: false)
        XCTAssertFalse(model.hiddenSessionIDs.contains("codex:a"))
        XCTAssertEqual(core.callCount("uploader.sessions.include"), 0, "Unhiding does not resume sharing")
    }

    func testUnhidingClaudeWebSessionRemovesTargetAndHistoryAliases() async {
        let target = "claude-cloud:session_019a7SvoqfvBg4Z6zbjanBat"
        let history = "claude:cse_019a7SvoqfvBg4Z6zbjanBat"
        defaults.set([target, history], forKey: "hiddenSessions.v1")
        let model = model()
        XCTAssertEqual(model.hiddenSessionIDs, [target, history])

        await model.setSessionsHidden(keys: [history], hidden: false)

        XCTAssertTrue(model.hiddenSessionIDs.isEmpty)
        XCTAssertEqual(defaults.stringArray(forKey: "hiddenSessions.v1"), [])
    }

    func testHiddenClaudeWebSessionPersistsBothAliasesOnce() {
        let targetID = "session_019a7SvoqfvBg4Z6zbjanBat"
        let target = "claude-cloud:\(targetID)"
        let history = "claude:cse_019a7SvoqfvBg4Z6zbjanBat"
        defaults.set([target], forKey: "hiddenSessions.v1")
        let model = model()
        let web = LocalSession(host: SessionHost.claudeWeb, live: true, project: nil, suggestedName: "web",
                               target: Target(id: targetID, kind: .claudeCloud), title: "Web", updatedAtMs: nil)

        model.migrateHiddenSessionIDs(for: [web])
        XCTAssertEqual(model.hiddenSessionIDs, [target, history])
        model.migrateHiddenSessionIDs(for: [web])
        XCTAssertEqual(defaults.stringArray(forKey: "hiddenSessions.v1"), [target, history])
    }

    func testFailedClaudeWebHideRollsBackBothAliases() async {
        let target = "claude-cloud:session_019a7SvoqfvBg4Z6zbjanBat"
        let history = "claude:cse_019a7SvoqfvBg4Z6zbjanBat"
        defaults.set([target], forKey: "hiddenSessions.v1")
        core.on("uploader.sessions.exclude") { _ in throw FakeCore.Failure(kind: "internal", message: "Cannot stop uploads") }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.phase == .attached }
        XCTAssertTrue(ready)

        await model.setSessionsHidden(keys: [history], hidden: true)

        XCTAssertTrue(model.hiddenSessionIDs.isEmpty)
        XCTAssertTrue(model.uploadErrors[history]?.contains("Cannot stop uploads") == true)
    }

    func testHideStopsAnExplicitRegistrationAlreadyInFlight() async {
        let registered = Locked(false)
        let held = DispatchSemaphore(value: 0)
        defer { held.signal() }
        core.on("sessions.list") { _ in
            ["sessions": [["target": ["kind": "codex", "id": "a"], "title": "Fix tests", "live": true,
                "registration": registered.value ? ["name": "fix-tests", "address": "fix-tests@mac", "added_by_session": false] as Any : NSNull(), "suggested_name": "fix-tests"]]]
        }
        core.on("agents.register") { _ in
            held.wait()
            registered.value = true
            return ["name": "fix-tests", "address": "fix-tests@mac", "already_registered": false]
        }
        core.on("agents.unregister") { _ in registered.value = false; return [:] }
        core.on("uploader.sessions.exclude") { _ in
            XCTAssertFalse(registered.value, "Hide must stop an explicit registration before excluding uploads")
            return ["updated": 1]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        _ = await eventually { !model.relay.sessions.isEmpty && !model.sessions.isEmpty }
        let session = model.relay.sessions[0]
        let registration = Task { await model.relay.register(session, as: "fix-tests") }
        _ = await eventually { core.callCount("agents.register") == 1 }
        let hide = Task { await model.setSessionsHidden(keys: ["codex:a"], hidden: true) }
        _ = await eventually { model.hidingSessionIDs.contains("codex:a") }
        held.signal()
        await registration.value
        await hide.value
        XCTAssertEqual(core.callCount("agents.unregister"), 1)
        XCTAssertFalse(registered.value)
        XCTAssertTrue(model.hiddenSessionIDs.contains("codex:a"))
    }

    func testHideSurvivesFailedListAfterExplicitRegistration() async {
        let registered = Locked(false)
        let failOnce = Locked(true)
        let held = DispatchSemaphore(value: 0)
        defer { held.signal() }
        core.on("sessions.list") { _ in
            if registered.value && failOnce.value {
                failOnce.value = false
                throw FakeCore.Failure(kind: "internal", message: "Cannot read registered sessions")
            }
            return ["sessions": [["target": ["kind": "codex", "id": "a"], "title": "Fix tests", "live": true,
                "registration": registered.value ? ["name": "fix-tests", "address": "fix-tests@mac", "added_by_session": false] as Any : NSNull(), "suggested_name": "fix-tests"]]]
        }
        core.on("agents.register") { _ in
            held.wait()
            registered.value = true
            return ["name": "fix-tests", "address": "fix-tests@mac", "already_registered": false]
        }
        core.on("agents.unregister") { _ in registered.value = false; return [:] }
        core.on("uploader.sessions.exclude") { _ in
            XCTAssertFalse(registered.value, "Hide must stop an explicit registration before excluding uploads")
            return ["updated": 1]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        _ = await eventually { !model.relay.sessions.isEmpty && !model.sessions.isEmpty }
        let session = model.relay.sessions[0]
        let registration = Task { await model.relay.register(session, as: "fix-tests") }
        _ = await eventually { core.callCount("agents.register") == 1 }
        let hide = Task { await model.setSessionsHidden(keys: ["codex:a"], hidden: true) }
        _ = await eventually { model.hidingSessionIDs.contains("codex:a") }
        held.signal()
        await registration.value
        await hide.value
        XCTAssertEqual(core.callCount("agents.unregister"), 1)
        XCTAssertFalse(registered.value)
        XCTAssertTrue(model.hiddenSessionIDs.contains("codex:a"))
    }

    func testHideWaitsForAnUploadAlreadyInFlight() async {
        let included = Locked(false)
        let held = DispatchSemaphore(value: 0)
        defer { held.signal() }
        core.on("uploader.sessions.list") { _ in
            ["sharing_mode": "selected", "sessions": [["source": "codex", "session_id": "a", "title": "Fix tests", "is_agent": false, "included": included.value, "status": included.value ? "shared" : "not_shared"]]]
        }
        core.on("uploader.sessions.include") { _ in held.wait(); included.value = true; return ["updated": 1] }
        core.on("uploader.sessions.exclude") { _ in
            XCTAssertTrue(included.value, "Exclude must run after the held include finishes")
            included.value = false
            return ["updated": 1]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        _ = await eventually { !model.sessions.isEmpty }
        let upload = Task { await model.includeSessions(keys: ["codex:a"]) }
        _ = await eventually { self.core.callCount("uploader.sessions.include") == 1 }
        let hide = Task { await model.setSessionsHidden(keys: ["codex:a"], hidden: true) }
        _ = await eventually { model.hidingSessionIDs.contains("codex:a") }
        XCTAssertEqual(core.callCount("uploader.sessions.exclude"), 0)
        held.signal()
        _ = await upload.value
        await hide.value
        XCTAssertFalse(included.value)
        XCTAssertTrue(model.hiddenSessionIDs.contains("codex:a"))
        XCTAssertEqual(core.callCount("uploader.sessions.exclude"), 1)
    }

    func testRestoredHiddenSessionIsExcludedWhenDiscoveredStillUploading() async {
        defaults.set(["codex:a"], forKey: "hiddenSessions.v1")
        let included = Locked(true)
        core.on("uploader.sessions.list") { _ in
            ["sharing_mode": "new", "sessions": [["source": "codex", "session_id": "a", "title": "Fix tests", "is_agent": false, "included": included.value, "status": included.value ? "shared" : "not_shared"]]]
        }
        core.on("uploader.sessions.exclude") { _ in included.value = false; return ["updated": 1] }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let stopped = await eventually { !included.value && model.hidingSessionIDs.isEmpty }
        XCTAssertTrue(stopped)
        XCTAssertTrue(model.hiddenSessionIDs.contains("codex:a"))
        XCTAssertFalse(model.sessions.first?.included ?? true)
    }

    func testFailedHideReturnsSessionToTableWithError() async {
        core.on("uploader.sessions.exclude") { _ in throw FakeCore.Failure(kind: "internal", message: "Cannot stop uploads") }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { !model.sessions.isEmpty }
        XCTAssertTrue(ready)
        await model.setSessionsHidden(keys: ["codex:a"], hidden: true)
        XCTAssertFalse(model.hiddenSessionIDs.contains("codex:a"))
        XCTAssertTrue(model.uploadErrors["codex:a"]?.contains("Cannot stop uploads") == true)
        XCTAssertTrue(model.hidingSessionIDs.isEmpty)
    }

    func testHiddenUploadIsExcludedOffPageOnceAndHideWinsHeldInclude() async {
        let included = Locked(false)
        let held = DispatchSemaphore(value: 0)
        defer { held.signal() }
        core.on("uploader.sessions.query") { _ in
            ["sessions": [], "local_sessions": [], "verification_pending": false, "unchanged": false, "revision": 42, "candidates_scanned": 0, "next_cursor": NSNull()]
        }
        core.on("uploader.sessions.include") { _ in held.wait(); included.value = true; return ["updated": 1] }
        core.on("uploader.sessions.exclude") { _ in included.value = false; return ["updated": 1] }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.phase == .attached }; XCTAssertTrue(ready)
        let include = Task { await model.includeSessions(keys: ["codex:off-page"]) }
        let started = await eventually { self.core.callCount("uploader.sessions.include") == 1 }; XCTAssertTrue(started)
        let hide = Task { await model.setSessionsHidden(keys: ["codex:off-page"], hidden: true) }
        let hiding = await eventually { model.hidingSessionIDs.contains("codex:off-page") }; XCTAssertTrue(hiding)
        held.signal(); _ = await include.value; await hide.value
        XCTAssertFalse(included.value)
        XCTAssertEqual(core.callCount("uploader.sessions.exclude"), 1)
        model.reconcileHiddenSessions(); model.reconcileHiddenSessions()
        await Task.yield()
        XCTAssertEqual(core.callCount("uploader.sessions.exclude"), 1, "Confirmed exclusions must not repeat on each page/poll")
    }

    func testHiddenOffPageUploadReenabledExternallyIsExcludedWithoutIdleListStorm() async throws {
        let included = Locked(true)
        let policy = Locked(10)
        core.on("uploader.sessions.query") { _ in
            ["sessions": [], "local_sessions": [], "revision": 42, "unchanged": false,
             "verification_pending": false, "candidates_scanned": 0, "next_cursor": NSNull()]
        }
        core.on("uploader.sessions.exclude") { _ in
            if included.value { included.value = false; policy.value += 1 }
            return ["updated": 1]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        await model.connectionRefresh?.value
        let ready = await eventually { model.phase == .attached && model.status != nil && !model.isLoadingSessions }
        XCTAssertTrue(ready)
        let status = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(XCTUnwrap(model.status))) as? [String: Any])
        core.on("uploader.status") { _ in
            var response = status
            response["upload_policy_revision"] = policy.value
            return response
        }
        await model.refreshStatus()
        await model.setSessionsHidden(keys: ["codex:off-page"], hidden: true)
        let hidden = await eventually { !included.value && model.hidingSessionIDs.isEmpty && !model.isLoadingSessions }
        XCTAssertTrue(hidden)
        await model.firstSessionPage() // Drain the hide-triggered presentation refresh.
        XCTAssertEqual(core.lastParams("uploader.sessions.query")?["hidden"] as? [String], ["codex:off-page"])
        // Acknowledge the app's own policy write. Repeating an exclusion is
        // idempotent, so this converges even if the receipt arrives later.
        let beforeAcknowledgment = core.callCount("uploader.sessions.exclude")
        await model.refreshStatus()
        let acknowledged = await eventually { self.core.callCount("uploader.sessions.exclude") > beforeAcknowledgment && model.hidingSessionIDs.isEmpty }
        XCTAssertTrue(acknowledged)
        let exclusions = core.callCount("uploader.sessions.exclude")
        let queries = core.callCount("uploader.sessions.query")
        included.value = true
        policy.value += 1 // Same sharing mode, counts, registration and visible page.
        await model.refreshStatus()
        let stopped = await eventually { !included.value && model.hidingSessionIDs.isEmpty }
        XCTAssertTrue(stopped, "Saved policy changes must invalidate the hidden identity's confirmation")
        XCTAssertTrue(model.hiddenSessionIDs.contains("codex:off-page"))
        XCTAssertTrue(model.sessions.isEmpty)
        let beforeSettlement = core.callCount("uploader.sessions.exclude")
        await model.refreshStatus()
        let settled = await eventually { self.core.callCount("uploader.sessions.exclude") > beforeSettlement && model.hidingSessionIDs.isEmpty }
        XCTAssertTrue(settled)
        let settledExclusions = core.callCount("uploader.sessions.exclude")
        for _ in 0..<3 { await model.refreshStatus() }
        await Task.yield()
        XCTAssertEqual(core.callCount("uploader.sessions.exclude"), settledExclusions, "Unchanged policy must not repeat exclusions")
        XCTAssertLessThanOrEqual(settledExclusions - exclusions, 2, "A changed policy and its acknowledgment must converge")
        XCTAssertEqual(core.callCount("uploader.sessions.query"), queries, "Policy-only control reads must not restart history queries")
    }

    func testNewPolicyObservedDuringHeldExclusionCannotConfirmOldWrite() async throws {
        let included = Locked(true)
        let policy = Locked(10)
        let held = DispatchSemaphore(value: 0)
        defer { held.signal() }
        let requests = Locked(0)
        core.on("uploader.sessions.query") { _ in
            ["sessions": [], "local_sessions": [], "revision": 42, "unchanged": false,
             "verification_pending": false, "candidates_scanned": 0, "next_cursor": NSNull()]
        }
        core.on("uploader.sessions.exclude") { _ in
            requests.value += 1
            if included.value { included.value = false; policy.value += 1 }
            if requests.value == 1 { held.wait() }
            return ["updated": 1]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        await model.connectionRefresh?.value
        let ready = await eventually { model.phase == .attached && model.status != nil && !model.isLoadingSessions }
        XCTAssertTrue(ready)
        let status = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(XCTUnwrap(model.status))) as? [String: Any])
        core.on("uploader.status") { _ in
            var response = status
            response["upload_policy_revision"] = policy.value
            return response
        }
        await model.refreshStatus()
        let hide = Task { await model.setSessionsHidden(keys: ["codex:off-page"], hidden: true) }
        let started = await eventually { requests.value == 1 && !included.value }
        XCTAssertTrue(started)
        included.value = true
        policy.value += 1
        await model.refreshStatus() // New policy arrives before the old write receipt.
        held.signal()
        await hide.value
        let stopped = await eventually { requests.value == 2 && !included.value && model.hidingSessionIDs.isEmpty }
        XCTAssertTrue(stopped, "A retired policy's successful receipt must not confirm a newer external include")
        XCTAssertTrue(model.hiddenSessionIDs.contains("codex:off-page"))
        XCTAssertNil(model.uploadErrors["codex:off-page"])
    }

    func testHiddenIntentSurvivesUnattachedUploaderAndReconcilesOnFutureAttach() async {
        let model = model()
        defer { model.shutdown() }
        await model.setSessionsHidden(keys: ["codex:future"], hidden: true)
        XCTAssertTrue(model.hiddenSessionIDs.contains("codex:future"))
        XCTAssertEqual(core.callCount("uploader.sessions.exclude"), 0)
        core.on("uploader.sessions.exclude") { _ in ["updated": 1] }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        await model.bootstrapIfNeeded()
        let reconciled = await eventually { self.core.callCount("uploader.sessions.exclude") == 1 && model.hidingSessionIDs.isEmpty }
        XCTAssertTrue(reconciled)
        XCTAssertTrue(model.hiddenSessionIDs.contains("codex:future"))
        XCTAssertNil(model.uploadErrors["codex:future"])
    }

    func testBusyHistoryRetainsHiddenIntentUntilExclusionCanBeSaved() async {
        core.on("uploader.sessions.exclude") { _ in throw FakeCore.Failure(kind: "busy", message: "Session history is preparing.") }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        await model.connectionRefresh?.value
        let ready = await eventually { model.phase == .attached && model.relay.sessions.count == 1 && !model.relay.isRefreshing && !model.isLoadingSessions && self.core.callCount("uploader.sessions.list") >= 2 }; XCTAssertTrue(ready)
        await model.setSessionsHidden(keys: ["codex:future"], hidden: true)
        XCTAssertTrue(model.hiddenSessionIDs.contains("codex:future"))
        XCTAssertEqual(model.uploadErrors["codex:future"], "Waiting to stop uploads…")
        await model.firstSessionPage() // cancel and drain the hide-triggered listing
        let retriedBusy = await eventually { self.core.callCount("uploader.sessions.exclude") >= 2 && model.hidingSessionIDs.isEmpty && !model.isLoadingSessions }; XCTAssertTrue(retriedBusy)
        let listings = core.callCount("uploader.sessions.list")
        core.on("uploader.sessions.exclude") { _ in ["updated": 1] }
        let reconciled = await eventually { model.uploadErrors["codex:future"] == nil && model.hidingSessionIDs.isEmpty }
        XCTAssertTrue(reconciled)
        XCTAssertEqual(core.callCount("uploader.sessions.list"), listings, "A scheduled exclusion retry must not depend on another listing")
    }

    func testConfirmedHiddenSessionStopsARegistrationRenewedOutsideTheApp() async {
        let registered = Locked(false)
        core.on("sessions.list") { _ in
            ["sessions": [["target": ["kind": "codex", "id": "future"], "title": "Hidden", "live": true,
                "registration": registered.value ? ["name": "hidden", "address": "hidden@desk", "added_by_session": false] as Any : NSNull(), "suggested_name": "hidden"]]]
        }
        core.on("agents.unregister") { _ in registered.value = false; return [:] }
        core.on("uploader.sessions.exclude") { _ in ["updated": 1] }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.relay.sessions.count == 1 }; XCTAssertTrue(ready)
        await model.setSessionsHidden(keys: ["codex:future"], hidden: true)
        XCTAssertEqual(core.callCount("uploader.sessions.exclude"), 1)
        model.reconcileHiddenSessions()
        XCTAssertEqual(core.callCount("uploader.sessions.exclude"), 1, "Unchanged denied state must stay deduplicated")
        registered.value = true
        await model.relay.refreshSessions()
        let stopped = await eventually { !registered.value && self.core.callCount("uploader.sessions.exclude") == 2 && model.hidingSessionIDs.isEmpty }
        XCTAssertTrue(stopped)
        XCTAssertTrue(model.hiddenSessionIDs.contains("codex:future"))
    }

    func testInterruptedFacetsRestartAndRetiredCompletionCannotClearReplacement() async {
        let oldReply = DispatchSemaphore(value: 0)
        let newReply = DispatchSemaphore(value: 0)
        defer { oldReply.signal(); newReply.signal() }
        let requests = Locked(0)
        core.on("uploader.sessions.query") { _ in
            ["sessions": [], "local_sessions": [], "revision": 1, "unchanged": false,
             "verification_pending": false, "candidates_scanned": 0, "next_cursor": NSNull()]
        }
        core.on("uploader.sessions.facets") { _ in
            requests.value += 1
            let request = requests.value
            if request == 1 { oldReply.wait() }
            else if request == 2 { newReply.wait() }
            return ["harnesses": ["codex"], "projects": [request == 1 ? "retired" : "current"],
                    "revision": request, "next_cursor": NSNull()]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let started = await eventually { requests.value == 1 && !model.isLoadingSessions }
        XCTAssertTrue(started)
        var preparing = FakeCore.signedIn
        preparing["state"] = "preparing"
        preparing["phase"] = "preparing_local_sessions"
        core.setStatus(FakeCore.snapshot(revision: 3, account: preparing))
        let interrupted = await eventually { model.phase == .preparingSessions }
        XCTAssertTrue(interrupted)
        core.setStatus(FakeCore.snapshot(revision: 4, account: FakeCore.signedIn))
        let restarted = await eventually { requests.value == 2 && !model.isLoadingSessions }
        XCTAssertTrue(restarted, "Retiring a connection must release its held facets task")
        oldReply.signal()
        await model.refreshSessions()
        XCTAssertNil(model.sessionFacets, "The retired completion must not publish old suggestions")
        XCTAssertEqual(requests.value, 2, "The retired completion must not clear the held replacement")
        newReply.signal()
        let loaded = await eventually { model.sessionFacets?.projects == ["current"] }
        XCTAssertTrue(loaded)
    }

    func testPagedSuggestionsWaitForCompleteFacetsAndRestartAnExpiredFacetCursor() async {
        let firstFacet = DispatchSemaphore(value: 0)
        defer { firstFacet.signal() }
        let rounds = Locked(0)
        core.on("uploader.sessions.query") { _ in
            ["archived_keys":["codex:page-only"], "sessions": [["source":"codex", "session_id":"page-only", "title":"Page", "cwd":"/partial", "project":"partial", "included":false, "is_agent":false, "status":"not_shared"]], "local_sessions":[], "revision":1, "unchanged":false, "verification_pending":false, "candidates_scanned":1, "next_cursor":NSNull()]
        }
        core.on("uploader.sessions.facets") { params in
            if params["cursor"] as? String == "expired" { throw FakeCore.Failure(kind:"busy", message:"Facet membership changed") }
            if params["cursor"] == nil {
                rounds.value += 1
                if rounds.value == 1 {
                    firstFacet.wait()
                    return ["harnesses":["codex"], "projects":[], "revision":1, "next_cursor":"expired"]
                }
                return ["harnesses":["codex","claude"], "projects":["complete"], "revision":2, "next_cursor":NSNull()]
            }
            return [:]
        }
        core.setStatus(FakeCore.snapshot(revision:2, account:FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let loaded = await eventually { model.sessions.first?.sessionId == "page-only" && rounds.value == 1 }; XCTAssertTrue(loaded)
        let partial = model.sessionSearchSource.rows(model:model, relay:model.relay)
        XCTAssertTrue(partial.harnesses.isEmpty)
        XCTAssertTrue(partial.projects.isEmpty)
        let archived = LocalSessionRow.merge(uploads:model.sessions, sessions:[])[0]
        XCTAssertEqual(SessionRowIndex(model:model, relay:model.relay).activityStatus(for:archived), "Archived", "Page snapshot metadata must label archives beyond the recent-agent window")
        firstFacet.signal()
        let complete = await eventually { model.sessionFacets?.revision == 2 }; XCTAssertTrue(complete)
        XCTAssertEqual(model.sessionSearchSource.rows(model:model, relay:model.relay).projects, ["complete"])
    }

    func testQueryArchiveSnapshotOverridesNewerRecentArchivesAndOldCoreDefaultsRemain() async {
        let metadata = Locked(true)
        let holdNextReply = DispatchSemaphore(value: 0)
        let oldReply = DispatchSemaphore(value: 0)
        let holdingReply = Locked(false)
        defer { oldReply.signal() }
        core.on("agents.list") { _ in ["agents":[], "archived":[["name":"old", "address":"old@desk", "target":["kind":"codex","id":"page-only"], "title":"Old archive", "archived_at":"2026-10-24T00:00:00Z"]]] }
        core.on("uploader.sessions.query") { _ in
            let includesMetadata = metadata.value
            if holdNextReply.wait(timeout: .now()) == .success {
                holdingReply.value = true
                oldReply.wait()
            }
            var page: [String:Any] = ["sessions":[["source":"codex","session_id":"page-only","title":"Snapshot","included":false,"is_agent":false,"status":"not_shared"]],"local_sessions":[],"revision":1,"unchanged":false,"verification_pending":false,"candidates_scanned":1,"next_cursor":NSNull()]
            if includesMetadata { page["archived_keys"] = [String]() }
            return page
        }
        core.setStatus(FakeCore.snapshot(revision:2, account:FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.relay.archivedAgents.count == 1 && model.sessions.count == 1 && !model.isLoadingSessions }; XCTAssertTrue(ready)
        let row = LocalSessionRow.merge(uploads:model.sessions, sessions:[])[0]
        XCTAssertEqual(SessionRowIndex(model:model, relay:model.relay).activityStatus(for:row), "Finished", "An explicit page snapshot is authoritative over current recent-agent state")
        holdNextReply.signal()
        let priorRefresh = Task { await model.refreshSessions() }
        let held = await eventually { holdingReply.value && model.isLoadingSessions }
        XCTAssertTrue(held, "Keep an earlier page read in flight when the next refresh is requested")
        metadata.value = false
        await model.refreshSessions()
        XCTAssertNotNil(model.queryArchivedSessionKeys, "A coalesced refresh queues another read before its result is available")
        oldReply.signal()
        await priorRefresh.value
        let fallback = await eventually { model.queryArchivedSessionKeys == nil && !model.isLoadingSessions }
        XCTAssertTrue(fallback, "Wait for the coalesced older-core response to replace the prior archive snapshot")
        XCTAssertNil(model.queryArchivedSessionKeys)
        XCTAssertEqual(SessionRowIndex(model:model, relay:model.relay).activityStatus(for:row), "Archived", "Older cores without page metadata retain the legacy classification fallback")
    }

    func testOldAccountHideFailureCannotUnhideOrAddErrorAfterAccountChanges() async {
        let held = DispatchSemaphore(value: 0)
        defer { held.signal() }
        core.on("uploader.sessions.exclude") { _ in held.wait(); throw FakeCore.Failure(kind: "internal", message: "Old account failed") }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.phase == .attached }; XCTAssertTrue(ready)
        let hide = Task { await model.setSessionsHidden(keys: ["codex:future"], hidden: true) }
        let started = await eventually { self.core.callCount("uploader.sessions.exclude") == 1 }; XCTAssertTrue(started)
        core.setStatus(FakeCore.snapshot(revision: 3, account: ["state": "signed_out"]))
        let left = await eventually { model.phase == .signedOut }; XCTAssertTrue(left)
        XCTAssertTrue(model.hidingSessionIDs.isEmpty, "Sign-out must retire in-flight hide markers")
        held.signal(); await hide.value
        XCTAssertTrue(model.hiddenSessionIDs.contains("codex:future"))
        XCTAssertNil(model.uploadErrors["codex:future"])
    }

    func testInterruptedHideReconcilesAfterSignInWithoutRetiredReplyClearingNewWork() async {
        await assertInterruptedHideReconciles(intermediateAccount: ["state": "signed_out"], phase: .signedOut)
    }

    func testInterruptedHideReconcilesAfterPreparationWithoutRetiredReplyClearingNewWork() async {
        var preparing = FakeCore.signedIn
        preparing["state"] = "preparing"
        preparing["phase"] = "preparing_local_sessions"
        await assertInterruptedHideReconciles(intermediateAccount: preparing, phase: .preparingSessions)
    }

    private func assertInterruptedHideReconciles(intermediateAccount: [String: Any], phase: AppModel.Phase) async {
        let oldReply = DispatchSemaphore(value: 0)
        let newReply = DispatchSemaphore(value: 0)
        let oldHandlerStarted = Locked(false)
        defer { oldReply.signal(); newReply.signal() }
        core.on("uploader.sessions.exclude") { _ in
            oldHandlerStarted.value = true
            oldReply.wait()
            throw FakeCore.Failure(kind: "internal", message: "Retired account failed")
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        await model.connectionRefresh?.value
        let ready = await eventually { model.phase == .attached && !model.isLoadingSessions }; XCTAssertTrue(ready)
        let oldHide = Task { await model.setSessionsHidden(keys: ["codex:future"], hidden: true) }
        let started = await eventually { oldHandlerStarted.value }
        guard started else {
            XCTFail("The old exclusion handler must be held before replacing it")
            return
        }
        core.setStatus(FakeCore.snapshot(revision: 3, account: intermediateAccount))
        let interrupted = await eventually { model.phase == phase }; XCTAssertTrue(interrupted)
        core.on("uploader.sessions.exclude") { _ in newReply.wait(); return ["updated": 1] }
        core.setStatus(FakeCore.snapshot(revision: 4, account: FakeCore.signedIn))
        let retried = await eventually { self.core.callCount("uploader.sessions.exclude") == 2 }; XCTAssertTrue(retried, "Reattachment must retry persisted hide intent")
        XCTAssertTrue(model.hidingSessionIDs.contains("codex:future"))
        oldReply.signal()
        await oldHide.value
        XCTAssertTrue(model.hidingSessionIDs.contains("codex:future"), "A retired completion must not remove the new hide marker")
        XCTAssertTrue(model.hiddenSessionIDs.contains("codex:future"))
        XCTAssertNil(model.uploadErrors["codex:future"])
        newReply.signal()
        let finished = await eventually { model.hidingSessionIDs.isEmpty }; XCTAssertTrue(finished)
        XCTAssertEqual(core.callCount("uploader.sessions.exclude"), 2)
        await model.setSessionsHidden(keys: ["codex:future"], hidden: false)
        XCTAssertFalse(model.hiddenSessionIDs.contains("codex:future"), "Unhide must remain usable after reattachment")
    }

    func testWorkspaceHeaderUsesUploadWorkspaceInsteadOfRelayWorkspace() async {
        var account = FakeCore.signedIn
        account["workspace_name"] = "Upload workspace"
        account["workspace_id"] = "0f8fad5b-d9cb-469f-a165-70867728950e"
        var snapshot = FakeCore.snapshot(revision: 2, account: account)
        var relay = snapshot["workspace"] as? [String: Any] ?? [:]
        relay["workspace_name"] = "Another relay"
        snapshot["workspace"] = relay
        core.setStatus(snapshot)
        let model = model()
        await model.bootstrapIfNeeded()
        let ready = await eventually { model.workspaceName == "Upload workspace" }
        XCTAssertTrue(ready)
        model.shutdown()
    }

    func testWorkspaceHeaderDoesNotDisplayAnIDWhileNameIsUnavailable() async {
        var account = FakeCore.signedIn
        account["workspace_name"] = "  "
        account["workspace_id"] = "0f8fad5b-d9cb-469f-a165-70867728950e"
        core.setStatus(FakeCore.snapshot(revision: 2, account: account))
        let model = model()
        await model.bootstrapIfNeeded()
        let ready = await eventually { model.workspaceID != nil }
        XCTAssertTrue(ready)
        XCTAssertEqual(model.workspaceName, "Workspace")
        model.shutdown()
    }

    override func tearDown() async throws {
        core.stop()
        defaults.removePersistentDomain(forName: suite)
    }

    nonisolated static func settings(analytics: Bool) -> [String: Any] {
        ["allow_self_registration": true, "auto_activate": false, "direct_delivery": false,
         "direct_delivery_managed": NSNull(), "analytics_enabled": analytics, "contractor_access": false]
    }

    func testStalledHandshakeShowsRecoveryThenClearsOnReady() async {
        let held = DispatchSemaphore(value: 0)
        defer { held.signal() }
        core.beforeReply = { method in if method == "subscribe" { held.wait() } }
        let service = FakeService(status: .enabled)
        defaults.set(service.binaryIdentity, forKey: AppModel.registeredCoreKey)
        let model = AppModel(client: CoreClient(socketPath: core.path), service: service,
                             bundledCoreVersion: "1.0.0", startupTimeout: 100_000_000,
                             manageSystemServices: false, defaults: defaults)
        defer { model.shutdown() }
        await model.bootstrapIfNeeded()
        let timedOut = await eventually(1) { model.coreProblem == StartupPresentation.timeoutMessage }
        XCTAssertTrue(timedOut)
        XCTAssertEqual(model.phase, .launching)
        held.signal()
        let ready = await eventually { model.phase == .signedOut && model.coreProblem == nil }
        XCTAssertTrue(ready)
    }

    func testCoreRestartShowsReconnectingUntilTheNewSnapshotArrives() async {
        let model = model()
        defer { model.shutdown() }
        await model.bootstrapIfNeeded()
        let connected = await eventually { model.isCoreConnected && model.core != nil }
        XCTAssertTrue(connected)
        let path = core.path
        core.stop()
        let disconnected = await eventually { !model.isCoreConnected }
        XCTAssertTrue(disconnected)
        XCTAssertEqual(WorkspaceConnection(model: model, relay: model.relay).label, "Reconnecting")
        XCTAssertEqual(WorkspaceConnection(model: model, relay: model.relay).help,
                       "Agent Relay is reconnecting to its background service.")
        core = FakeCore(path: path, pid: 4243)
        let recovered = await eventually { model.isCoreConnected && model.core?.daemon.pid == 4243 }
        XCTAssertTrue(recovered)
        XCTAssertNotEqual(WorkspaceConnection(model: model, relay: model.relay).label, "Reconnecting")
    }

    func testCoreRestartReleasesInterruptedUploaderAction() async throws {
        let held = DispatchSemaphore(value: 0)
        defer { held.signal() }
        core.on("uploader.pause") { _ in held.wait(); return ["paused": true] }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        defer { model.shutdown() }
        await model.bootstrapIfNeeded()
        let attached = await eventually { model.phase == .attached && model.status != nil }
        XCTAssertTrue(attached)
        let overviewReloads = model.relay.overviewReloads
        // The replacement must report paused for Resume to change its state.
        var restartedStatus = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(XCTUnwrap(model.status))) as? [String: Any])
        restartedStatus["paused"] = true
        let pausedStatus = restartedStatus
        let pause = Task { await model.setPaused(true) }
        let started = await eventually { model.isWorking && self.core.callCount("uploader.pause") == 1 }
        XCTAssertTrue(started)
        let path = core.path
        core.stop()
        let disconnected = await eventually { !model.isCoreConnected && !model.isWorking }
        XCTAssertTrue(disconnected)
        core = FakeCore(path: path, pid: 4243)
        core.on("uploader.status") { _ in pausedStatus }
        core.on("uploader.resume") { _ in ["paused": false] }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn, pid: 4243))
        let reconnected = await eventually { model.isCoreConnected && model.core?.daemon.pid == 4243 && model.status?.paused == true }
        XCTAssertTrue(reconnected)
        XCTAssertFalse(model.pausedWanted.isPending, "restarting clears the interrupted optimistic choice")
        XCTAssertEqual(model.relay.overviewReloads, overviewReloads + 1)
        await model.setPaused(false)
        XCTAssertEqual(core.callCount("uploader.resume"), 1)
        held.signal()
        await pause.value
    }

    /// A Retry the core never received, because it could not be reached,
    /// does not leave "reconnecting" on screen: once the core answers, the
    /// message clears and the retry is sent, without another click.
    func testARetryTheCoreNeverReceivedClearsAndIsSentOnceItAnswers() async {
        core.on("uploader.retry") { _ in ["retried": true] }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        defer { model.shutdown() }
        await model.bootstrapIfNeeded()
        let attached = await eventually { model.phase == .attached && model.status != nil }
        XCTAssertTrue(attached)
        let path = core.path
        core.stop()
        let disconnected = await eventually { !model.isCoreConnected }
        XCTAssertTrue(disconnected)
        await model.retryUploads()
        XCTAssertEqual(model.uploadActionError, CoreError.reconnecting.message)

        core = FakeCore(path: path, pid: 4243)
        core.on("uploader.retry") { _ in ["retried": true] }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn, pid: 4243))
        let recovered = await eventually(10) {
            model.isCoreConnected && model.uploadActionError == nil && self.core.callCount("uploader.retry") == 1
                && model.pendingUploadAction == nil
        }
        XCTAssertTrue(recovered)
        // Sent once: later answers send nothing more.
        await model.refreshStatus()
        XCTAssertEqual(core.callCount("uploader.retry"), 1)

        // A failure from a retry that reached the core keeps its message.
        core.on("uploader.retry") { _ in throw FakeCore.Failure(kind: "internal", message: "The upload service refused.") }
        await model.retryUploads()
        XCTAssertEqual(model.uploadActionError, "The upload service refused.")
        await model.refreshStatus()
        XCTAssertEqual(model.uploadActionError, "The upload service refused.")
        XCTAssertEqual(core.callCount("uploader.retry"), 2)
    }

    func testOnlyAFailureToReachTheCoreReadsAsNeverSent() {
        XCTAssertTrue(CoreError.neverSent(CoreError.reconnecting.message))
        XCTAssertTrue(CoreError.neverSent(CoreError.starting.message))
        XCTAssertFalse(CoreError.neverSent(CoreError.interrupted.message))
        XCTAssertFalse(CoreError.neverSent(CoreError.closed.message))
        XCTAssertFalse(CoreError.neverSent("The upload service refused."))
    }

    func testCachedHistoryAppearsWhileUploaderStatusIsHeld() async {
        let held = DispatchSemaphore(value: 0)
        defer { for _ in 0..<4 { held.signal() } }
        core.on("uploader.status") { _ in held.wait(); return [:] }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        defer { model.shutdown() }
        await model.bootstrapIfNeeded()
        let visible = await eventually(0.5) { !model.sessions.isEmpty }
        XCTAssertTrue(visible, "Cached history must not wait for uploader status")
    }

    func testSignInAndStatusEventsDoNotWaitForSessionDiscovery() async {
        let held = DispatchSemaphore(value: 0)
        defer { held.signal() }
        core.on("sessions.list") { _ in
            held.wait()
            return ["sessions": []]
        }
        let model = model()
        defer { model.shutdown() }
        await model.bootstrapIfNeeded()
        let listing = await eventually { self.core.callCount("sessions.list") > 0 }
        XCTAssertTrue(listing)
        let ready = await eventually(0.5) { model.phase == .signedOut }
        XCTAssertTrue(ready, "Sign-in must render while discovery is still held")
        core.setStatus(FakeCore.snapshot(revision: 2, account: ["state": "pending_approval"]))
        let updated = await eventually(0.5) { model.phase == .waitingForBrowser }
        XCTAssertTrue(updated, "Status events must keep flowing during discovery")
    }

    func testRequestFailureLinksToExactEntryAndSurvivesLogEviction() async {
        let entry: [String: Any] = ["id": "failure-42", "started_at_ms": 123, "service": "Relaycast",
            "method": "GET", "endpoint": "https://cast.agentrelay.com/v1/workspace",
            "request": "null", "response": "workspace unavailable", "status": 503,
            "duration_ms": 40, "failed": true]
        core.on("diagnostics.requests") { _ in ["entries": [entry]] }
        let model = model()
        await model.refreshRequestLog()
        XCTAssertTrue(model.requestEntries.isEmpty, "Entries publish only while Diagnostics shows them")
        XCTAssertEqual(model.requestFailure?.id, "failure-42")
        model.openFailedRequest()
        XCTAssertEqual(model.page, .settings(.diagnostics))
        XCTAssertEqual(model.selectedRequest?.id, "failure-42")
        XCTAssertNil(model.requestFailure)
        await model.refreshRequestLog()
        XCTAssertEqual(model.requestEntries.first?.status, 503)
        XCTAssertNil(model.requestFailure, "Reading the same failure should not notify again")
        core.on("diagnostics.requests") { _ in ["entries": []] }
        await model.refreshRequestLog()
        XCTAssertTrue(model.requestEntries.isEmpty)
        XCTAssertEqual(model.selectedRequest?.response, "workspace unavailable")
    }

    func testIdenticalRefreshesPreservePublishedDataAndSearchIndex() async throws {
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.sessions.count == 2 && model.relay.sessions.count == 1 && model.status != nil && !model.isLoadingSessions }
        XCTAssertTrue(ready)
        // Settle all initial refreshes before subscribing.
        await model.refreshStatus()
        await model.refreshSessions()
        try await model.relay.loadSessions()
        let cache = model.sessionSearchSource
        _ = cache.snapshot(model: model, relay: model.relay)
        let builds = (cache.rowsBuildCount, cache.metadataBuildCount)
        let publications = ObservationProbe {
            _ = (model.status, model.sessions, model.previewSessionIDs,
                 model.relay.sessions, model.relay.roster, model.relay.archivedAgents)
        }
        defer { publications.stop() }
        for _ in 0..<100 {
            await model.refreshStatus()
            await model.refreshSessions()
            try await model.relay.loadSessions()
            await model.relay.refresh(.roster)
            await model.relay.refresh(.agents)
            _ = cache.snapshot(model: model, relay: model.relay)
        }
        XCTAssertEqual(publications.changes, 0, "No-op responses must not republish catalog/status data; loading feedback remains independent")
        XCTAssertEqual(cache.rowsBuildCount, builds.0)
        XCTAssertEqual(cache.metadataBuildCount, builds.1)
    }

    func testDiagnosticCoreRevisionsKeepSearchMetadataButPushChangesRebuildIt() async throws {
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.sessions.count == 2 && model.relay.sessions.count == 1 && model.status != nil }
        XCTAssertTrue(ready)
        let cache = model.sessionSearchSource
        _ = cache.snapshot(model: model, relay: model.relay)
        let builds = (cache.rowsBuildCount, cache.metadataBuildCount)
        for revision in 3..<103 {
            let snapshot = try JSONDecoder().decode(Status.self, from: JSONSerialization.data(withJSONObject:
                FakeCore.snapshot(revision: revision, account: FakeCore.signedIn)))
            model.handle(.status(snapshot))
            _ = cache.snapshot(model: model, relay: model.relay)
        }
        XCTAssertEqual(model.core?.revision, 102, "Status freshness and event ordering must survive metadata reuse")
        XCTAssertEqual(cache.rowsBuildCount, builds.0)
        XCTAssertEqual(cache.metadataBuildCount, builds.1)
        let changed = try JSONDecoder().decode(Status.self, from: JSONSerialization.data(withJSONObject:
            FakeCore.snapshot(revision: 103, account: FakeCore.signedIn, push: ["fix-tests": ["state": "live"]])))
        model.handle(.status(changed))
        _ = cache.snapshot(model: model, relay: model.relay)
        XCTAssertEqual(cache.metadataBuildCount, builds.1 + 1)
        XCTAssertEqual(cache.rowsBuildCount, builds.0)
    }

    func testDeliveryCounterRefreshKeepsSearchMetadataAndPublishesFreshStatus() async throws {
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.status != nil && model.sessions.count == 2 && model.relay.sessions.count == 1 }
        XCTAssertTrue(ready)
        var reply = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(XCTUnwrap(model.status))) as? [String: Any])
        var delivery = try XCTUnwrap(reply["delivery"] as? [String: Any])
        delivery["acknowledged_records"] = 42
        reply["delivery"] = delivery
        let response = reply
        core.on("uploader.status") { _ in response }
        let cache = model.sessionSearchSource
        _ = cache.snapshot(model: model, relay: model.relay)
        let builds = (cache.rowsBuildCount, cache.metadataBuildCount)
        await model.refreshStatus()
        XCTAssertEqual(model.status?.delivery.acknowledgedRecords, 42)
        _ = cache.snapshot(model: model, relay: model.relay)
        XCTAssertEqual(cache.rowsBuildCount, builds.0)
        XCTAssertEqual(cache.metadataBuildCount, builds.1)
    }

    func testUnchangedStatusStillClearsRefreshFailure() async throws {
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.status != nil && model.sessions.count == 2 }
        XCTAssertTrue(ready)
        let status = try XCTUnwrap(model.status)
        let reply = try JSONSerialization.jsonObject(with: JSONEncoder().encode(status))
        core.on("uploader.status") { _ in throw FakeCore.Failure(kind: "internal", message: "Transient refresh failure") }
        await model.refreshStatus()
        XCTAssertNotNil(model.lastError)
        core.on("uploader.status") { _ in reply }
        await model.refreshStatus()
        XCTAssertNil(model.lastError, "An equal successful result must still clear a preceding error")
        XCTAssertEqual(model.status, status)
    }

    func testSearchSourceCacheReusesThousandSessionIndexAcrossHundredQueryEdits() async {
        core.on("uploader.sessions.list") { _ in Self.history(1_000) }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.sessions.count == 1_000 && !model.relay.sessions.isEmpty }
        XCTAssertTrue(ready)
        let cache = model.sessionSearchSource
        let source = cache.snapshot(model: model, relay: model.relay)
        XCTAssertEqual(source.rows.count, 1_001)
        let builds = (cache.rowsBuildCount, cache.metadataBuildCount)
        for number in 0..<100 {
            model.sessionSearch = "title:\"Session \(number)\""
            model.showsSessionSearch.toggle()
            let input = cache.snapshot(model: model, relay: model.relay)
            let result = SessionSearchSnapshot(source: input, query: model.sessionSearch, sort: model.sessionSort).results()
            XCTAssertTrue(result.rows.contains { $0.title == "Session \(number)" })
            XCTAssertEqual(cache.rows(model: model, relay: model.relay).harnesses, ["codex"])
            _ = cache.rowIndex(model: model, relay: model.relay).state(for: source.rows[0])
        }
        XCTAssertEqual(cache.rowsBuildCount, builds.0, "100 settled queries, header renders and table lookups must perform zero merges")
        XCTAssertEqual(cache.metadataBuildCount, builds.1, "Query edits must perform zero metadata/index rebuilds")
    }

    func testSearchSourceCacheTracksUploadAndLiveSessionChangesAndSignOut() async {
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.sessions.count == 2 && model.relay.sessions.count == 1 }
        XCTAssertTrue(ready)
        let cache = model.sessionSearchSource
        XCTAssertEqual(cache.snapshot(model: model, relay: model.relay).rows.count, 2)
        let builds = cache.rowsBuildCount
        core.on("uploader.sessions.list") { _ in
            ["sharing_mode": "new", "sessions": [["source": "claude", "session_id": "new", "title": "Fresh", "cwd": "/work/new-project", "is_agent": false, "included": false, "status": "not_shared"]]]
        }
        await model.refreshSessions()
        // A bootstrap read may still be in flight. The explicit refresh then
        // queues another read rather than waiting for it to finish.
        let refreshed = await eventually { model.sessions.count == 1 && model.sessions.first?.title == "Fresh" && !model.isLoadingSessions }
        XCTAssertTrue(refreshed)
        let changed = cache.snapshot(model: model, relay: model.relay)
        XCTAssertEqual(cache.rowsBuildCount, builds + 1)
        XCTAssertTrue(changed.rows.contains { $0.title == "Fresh" })
        XCTAssertEqual(cache.rows(model: model, relay: model.relay).projects, ["new-project"])
        core.on("sessions.list") { _ in ["sessions": []] }
        await model.relay.refreshSessions()
        XCTAssertEqual(cache.snapshot(model: model, relay: model.relay).rows.map(\.title), ["Fresh"])
        XCTAssertEqual(cache.rowsBuildCount, builds + 2)
        core.setStatus(FakeCore.snapshot(revision: 3, account: ["state": "signed_out"]))
        let signedOut = await eventually { model.phase == .signedOut && model.sessions.isEmpty }
        XCTAssertTrue(signedOut)
        XCTAssertTrue(cache.snapshot(model: model, relay: model.relay).rows.isEmpty)
        XCTAssertTrue(cache.rows(model: model, relay: model.relay).projects.isEmpty)
    }

    func testSearchSourceCacheTracksRosterArchiveAndCorePresenceWithoutRemerging() async {
        core.on("sessions.list") { _ in
            ["sessions": [["target": ["kind": "codex", "id": "a"], "title": "Fix tests", "live": true,
                "registration": ["name": "fix-tests", "address": "fix-tests@mac", "added_by_session": false], "suggested_name": "fix-tests"]]]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.sessions.count == 2 && model.relay.sessions.count == 1 }
        XCTAssertTrue(ready)
        let cache = model.sessionSearchSource
        XCTAssertEqual(cache.snapshot(model: model, relay: model.relay).metadata["codex:a"]?.status, "Active")
        let builds = cache.rowsBuildCount
        core.on("roster.list") { _ in
            var agent = Self.rosterAgent("idle")
            agent["name"] = "fix-tests"
            return ["agents": [agent], "fetched_at_ms": 1, "unread_statuses": []]
        }
        await model.relay.refresh(.roster)
        XCTAssertEqual(cache.snapshot(model: model, relay: model.relay).metadata["codex:a"]?.status, "Idle")
        core.on("agents.list") { _ in
            ["agents": [], "archived": [["address": "fix-tests@mac", "archived_at": "2026-10-10T12:00:00Z", "name": "fix-tests", "title": "Fix tests", "target": ["kind": "codex", "id": "a"]]]]
        }
        await model.relay.refresh(.agents)
        XCTAssertEqual(cache.snapshot(model: model, relay: model.relay).metadata["codex:a"]?.status, "Archived")
        let metadataBuilds = cache.metadataBuildCount
        core.setStatus(FakeCore.snapshot(revision: 3, account: FakeCore.signedIn, push: ["fix-tests": ["state": "live"]]))
        let online = await eventually { model.core?.push["fix-tests"]?.isLive == true }
        XCTAssertTrue(online)
        let row = cache.snapshot(model: model, relay: model.relay).metadata["codex:a"]!.row
        XCTAssertEqual(cache.rowIndex(model: model, relay: model.relay).state(for: row).presence, .online)
        XCTAssertGreaterThan(cache.metadataBuildCount, metadataBuilds)
        XCTAssertEqual(cache.rowsBuildCount, builds, "Roster, archives and push metadata must not redo the merge")
        model.relay.accountChanged()
        XCTAssertTrue(model.relay.roster.isEmpty)
        _ = cache.snapshot(model: model, relay: model.relay)
        XCTAssertGreaterThan(cache.metadataBuildCount, metadataBuilds + 1)
    }

    func testSearchSourceCacheTracksOptimisticUploadAndRelayMetadata() async {
        let includeGate = DispatchSemaphore(value: 0)
        let deleteGate = DispatchSemaphore(value: 0)
        let registerGate = DispatchSemaphore(value: 0)
        defer { includeGate.signal(); deleteGate.signal(); registerGate.signal() }
        core.on("uploader.sessions.include") { _ in
            includeGate.wait()
            throw FakeCore.Failure(kind: "unavailable", message: "Exercise rollback")
        }
        core.on("uploader.sessions.delete") { _ in
            deleteGate.wait()
            throw FakeCore.Failure(kind: "unavailable", message: "Exercise rollback")
        }
        core.on("agents.register") { _ in
            registerGate.wait()
            throw FakeCore.Failure(kind: "conflict", message: "Exercise rollback")
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.sessions.count == 2 && model.relay.sessions.count == 1 }
        guard ready else { XCTFail("FakeCore sessions did not load"); return }
        let cache = model.sessionSearchSource
        XCTAssertEqual(cache.snapshot(model: model, relay: model.relay).metadata["claude:b"]?.uploadState, .local)
        let include = Task { await model.includeSessions(keys: ["claude:b"]) }
        let requesting = await eventually { model.requestingUploads.contains("claude:b") }
        XCTAssertTrue(requesting)
        let pendingUpload = cache.snapshot(model: model, relay: model.relay).metadata["claude:b"]
        XCTAssertEqual(pendingUpload?.uploadState, .preparing)
        XCTAssertEqual(pendingUpload?.uploadEnabled, true)
        includeGate.signal()
        _ = await include.value
        XCTAssertEqual(cache.snapshot(model: model, relay: model.relay).metadata["claude:b"]?.uploadState, .local)

        XCTAssertEqual(cache.snapshot(model: model, relay: model.relay).metadata["codex:a"]?.uploadEnabled, true)
        let deletion = Task { await model.excludeSessions(keys: ["codex:a"]) }
        let stopping = await eventually { model.stoppingUploads.contains("codex:a") }
        XCTAssertTrue(stopping)
        XCTAssertEqual(cache.snapshot(model: model, relay: model.relay).metadata["codex:a"]?.uploadEnabled, false)
        deleteGate.signal()
        await deletion.value
        XCTAssertEqual(cache.snapshot(model: model, relay: model.relay).metadata["codex:a"]?.uploadEnabled, true)

        let session = model.relay.sessions[0]
        let registration = Task { await model.relay.register(session, as: "fix-tests") }
        let busy = await eventually { model.relay.busyTargets.contains(session.id) }
        XCTAssertTrue(busy)
        XCTAssertNil(model.relay.wanted[session.id], "An explicit name registration changes busy state alone")
        XCTAssertEqual(cache.snapshot(model: model, relay: model.relay).metadata["codex:a"]?.onRelay, true)
        // While busy, requesting a different state only changes wanted. Both
        // dependencies independently invalidate the cached sort metadata.
        model.relay.requestRelay(false, for: session)
        XCTAssertEqual(cache.snapshot(model: model, relay: model.relay).metadata["codex:a"]?.onRelay, false)
        registerGate.signal()
        await registration.value
        XCTAssertFalse(model.relay.busyTargets.contains(session.id))
        XCTAssertEqual(cache.snapshot(model: model, relay: model.relay).metadata["codex:a"]?.onRelay, false)
    }

    func testSearchSourceCacheTracksUploaderStatusWithoutRemerging() async {
        core.on("uploader.sessions.list") { _ in
            ["sharing_mode": "new", "sessions": [["source": "codex", "session_id": "a", "title": "Fix tests", "is_agent": false, "included": true, "status": "queued"]]]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.sessions.count == 1 && model.status != nil }
        guard ready else { XCTFail("FakeCore uploader did not load"); return }
        let cache = model.sessionSearchSource
        XCTAssertEqual(cache.snapshot(model: model, relay: model.relay).metadata["codex:a"]?.uploadState, .queued)
        let builds = cache.rowsBuildCount
        core.on("uploader.status") { _ in
            ["running": true, "paused": true, "sharing_mode": "new", "directory": "/tmp/install",
             "collector_replacing": false, "collector_outdated": false,
             "delivery": ["bootstrap_complete": true, "pending_records": 0, "acknowledged_records": 4, "quarantined_records": 0, "skipped_records": 0,
                          "unqueued_changes": 0, "last_acknowledged_ms": NSNull(), "failure": NSNull(), "stopped": false],
             "sessions": ["total": 1, "shared": 1, "excluded": 0], "stage": "up_to_date", "stage_message": NSNull(),
             "needs_start": false, "journal_full": false, "upgrading": false, "stalled": false]
        }
        await model.refreshStatus()
        XCTAssertEqual(cache.snapshot(model: model, relay: model.relay).metadata["codex:a"]?.uploadState, .paused)
        XCTAssertEqual(cache.rowsBuildCount, builds)
    }

    func testSearchSourceCacheClearsPreviewMetadataWithoutChangingRows() async {
        core.on("uploader.sessions.preview") { _ in Self.history(1) }
        core.on("uploader.sessions.list") { _ in Self.history(1) }
        core.setStatus(FakeCore.snapshot(revision: 2, account: [
            "state": "preparing", "phase": "preparing_local_sessions", "person": ["id": "user_1", "email": "ada@example.com"]
        ]))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.previewSessionIDs.contains("codex:history-0") }
        guard ready else { XCTFail("FakeCore preview did not load"); return }
        let cache = model.sessionSearchSource
        XCTAssertEqual(cache.snapshot(model: model, relay: model.relay).metadata["codex:history-0"]?.uploadState, .discovering)
        let rowsRevision = model.sessionRowsRevision
        core.setStatus(FakeCore.snapshot(revision: 3, account: FakeCore.signedIn))
        let attached = await eventually { model.phase == .attached && model.previewSessionIDs.isEmpty }
        XCTAssertTrue(attached)
        XCTAssertEqual(model.sessionRowsRevision, rowsRevision, "Identical uploaded rows must not hide a missing preview-metadata invalidation")
        XCTAssertEqual(cache.snapshot(model: model, relay: model.relay).metadata["codex:history-0"]?.uploadState, .local)
    }

    private func model(service: FakeService = FakeService(status: .enabled), version: String? = "1.0.0", unregisterSettle: UInt64 = 0,
                       startupTimeout: UInt64 = 20_000_000_000,
                       coreVersionRecheckDelay: UInt64 = 30_000_000_000) -> AppModel {
        AppModel(client: CoreClient(socketPath: core.path), service: service, bundledCoreVersion: version, unregisterSettle: unregisterSettle,
                 startupTimeout: startupTimeout, coreVersionRecheckDelay: coreVersionRecheckDelay,
                 siteURL: "https://agentrelay.com", manageSystemServices: false, defaults: defaults)
    }

    nonisolated private static func history(_ count: Int) -> [String: Any] {
        ["sharing_mode": "new", "sessions": (0..<count).map { index in
            ["source": "codex", "session_id": "history-\(index)", "title": "Session \(index)",
             "is_agent": false, "included": false, "status": "not_shared"] as [String: Any]
        }]
    }

    private func scrollView(in view: NSView) -> NSScrollView? {
        // The toolbar also contains a horizontal scroll view for filter chips.
        if let scroll = view as? NSScrollView, scroll.hasVerticalScroller { return scroll }
        return view.subviews.lazy.compactMap { self.scrollView(in: $0) }.first
    }

    func testSessionListEndIgnoresAnUnmeasuredLazyStack() {
        let row = SessionListEndMeasurement.rowHeight
        XCTAssertFalse(SessionListEndMeasurement.isNearEnd(
            contentHeight: 9 * row, contentMaxY: 9 * row, viewportHeight: 600, renderedRows: 500),
            "A footer placed after the first realized rows is not the end of the list")
        XCTAssertFalse(SessionListEndMeasurement.isNearEnd(
            contentHeight: 500 * row - 1, contentMaxY: 600, viewportHeight: 600, renderedRows: 500),
            "A measurement one point short of the rendered rows is still incomplete")
        XCTAssertFalse(SessionListEndMeasurement.isNearEnd(
            contentHeight: 500 * row, contentMaxY: 500 * row, viewportHeight: 600, renderedRows: 500),
            "The first viewport stays on the first page")
        XCTAssertFalse(SessionListEndMeasurement.isNearEnd(
            contentHeight: 500 * row, contentMaxY: 600 + SessionListEndMeasurement.nearEndSlack + 1,
            viewportHeight: 600, renderedRows: 500),
            "The end just outside the slack must not page")
        XCTAssertTrue(SessionListEndMeasurement.isNearEnd(
            contentHeight: 500 * row, contentMaxY: 600, viewportHeight: 600, renderedRows: 500),
            "Scrolling the measured list to its end loads the next page")
        XCTAssertTrue(SessionListEndMeasurement.isNearEnd(
            contentHeight: 500 * row, contentMaxY: 600 + SessionListEndMeasurement.nearEndSlack,
            viewportHeight: 600, renderedRows: 500),
            "The end within the same slack still loads the next page")
        XCTAssertTrue(SessionListEndMeasurement.isNearEnd(
            contentHeight: 120, contentMaxY: 120, viewportHeight: 600, renderedRows: 0),
            "A short filtered list is at its end and must keep paging")
        XCTAssertFalse(SessionListEndMeasurement.isNearEnd(
            contentHeight: 0, contentMaxY: 0, viewportHeight: 600, renderedRows: 500),
            "A missing coordinate space must not page")
    }

    func testSessionViewportLoadsOnlyWhenScrolledNearEnd() async throws {
        // The independent initial Relay refresh can add a row after the upload
        // list is already laid out. Hold that transition to exercise the lazy
        // stack's reconciliation before making a native end-scroll gesture.
        let initialRelayRead = DispatchSemaphore(value: 0)
        defer { initialRelayRead.signal() }
        let relayRequested = Locked(false)
        core.on("sessions.list") { _ in
            if !relayRequested.value {
                relayRequested.value = true
                initialRelayRead.wait()
            }
            return ["sessions": [["target": ["kind": "codex", "id": "a"], "title": "Fix tests", "live": true,
                                   "registration": NSNull(), "suggested_name": "fix-tests"]]]
        }
        core.on("uploader.sessions.list") { params in
            Self.history(min(params["limit"] as? Int ?? 500, 503))
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        let ready = await eventually { model.canLoadMoreSessions }
        XCTAssertTrue(ready)
        await model.connectionRefresh?.value
        let initialRefreshStarted = await eventually {
            relayRequested.value && model.relay.isRefreshing && model.canLoadMoreSessions
        }
        XCTAssertTrue(initialRefreshStarted)
        XCTAssertTrue(model.canLoadMoreSessions, "Connection refresh can finish while the independent initial Relay refresh is held")
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1200, height: 600),
                              styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        let host = NSHostingView(rootView: LocalSessionsView(relay: model.relay).environment(model))
        window.contentView = host
        window.orderFront(nil)
        defer { window.close(); model.shutdown() }
        let laidOut = await eventually {
            guard let scroll = self.scrollView(in: host) else { return false }
            return model.canLoadMoreSessions
                && (scroll.documentView?.bounds.height ?? 0) >= CGFloat(model.sessions.count) * SessionListEndMeasurement.rowHeight
                && (scroll.documentView?.bounds.height ?? 0) > scroll.contentView.bounds.height
        }
        XCTAssertTrue(laidOut)
        XCTAssertEqual(model.sessionLimit, 500, "The first viewport must not load all history")
        initialRelayRead.signal()
        let inputsSettled = await eventually {
            model.relay.sessions.count == 1 && !model.relay.isRefreshing && !model.isLoadingSessions
                && self.core.calls.filter { $0.method == "uploader.sessions.list" }.count >= 2
        }
        XCTAssertTrue(inputsSettled, "Both independent initial reads must finish before the end-scroll gesture")
        let finalRowsLaidOut = await eventually {
            guard let scroll = self.scrollView(in: host) else { return false }
            let rendered = model.sessionSearchSource.rows(model: model, relay: model.relay).values.count
            return rendered == 501 && (scroll.documentView?.bounds.height ?? 0) >= CGFloat(rendered) * SessionListEndMeasurement.rowHeight
        }
        XCTAssertTrue(finalRowsLaidOut, "Layout must include the Relay-only row added by the held initial refresh")
        XCTAssertEqual(model.sessionLimit, 500, "Settling initial sources at the top must not fetch older history")
        let scroll = try XCTUnwrap(scrollView(in: host))
        let document = try XCTUnwrap(scroll.documentView)
        let loaded = await eventually {
            if model.sessions.count == 503 { return true }
            guard model.canLoadMoreSessions else { return false }
            // LazyVStack corrects its estimated extent as rows are realized.
            // Continue the native end-scroll gesture until the current, fully
            // measured document end is reached, rather than an earlier estimate.
            let end = max(0, document.bounds.maxY - scroll.contentView.bounds.height)
            scroll.contentView.scroll(to: NSPoint(x: 0, y: end))
            scroll.reflectScrolledClipView(scroll.contentView)
            return false
        }
        XCTAssertTrue(loaded, "Scrolling to the end must fetch older sessions without a button; document=\(document.bounds), clip=\(scroll.contentView.bounds), loading=\(model.isLoadingSessions), canPage=\(model.canLoadMoreSessions)")
        XCTAssertEqual(model.sessionLimit, 1000)
        XCTAssertTrue(core.calls.contains { $0.method == "uploader.sessions.list" && $0.params["limit"] as? Int == 1000 },
                      "Reaching the final viewport must issue the next legacy page request")
    }

    func testFilteredViewportLoadsOlderSessionsAutomatically() async {
        core.on("uploader.sessions.list") { params in
            Self.history(min(params["limit"] as? Int ?? 500, 503))
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        let ready = await eventually { model.canLoadMoreSessions }
        XCTAssertTrue(ready)
        model.sessionSearch = "id:history-502"
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1200, height: 600),
                              styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = NSHostingView(rootView: LocalSessionsView(relay: model.relay).environment(model))
        window.orderFront(nil)
        defer { window.close(); model.shutdown() }
        let loaded = await eventually { model.sessions.count == 503 }
        XCTAssertTrue(loaded, "An empty filtered viewport must fetch older matching sessions without a button")
        XCTAssertEqual(model.sessionLimit, 1000)
    }

    func testPaginationCoalescesRequestsAndStopsAtEnd() async {
        let held = DispatchSemaphore(value: 0)
        defer { held.signal() }
        core.on("uploader.sessions.list") { params in
            let limit = params["limit"] as? Int ?? 500
            if limit == 1000 { held.wait() }
            return Self.history(min(limit, 503))
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.canLoadMoreSessions }; XCTAssertTrue(ready)
        let first = Task {
            while !model.canLoadMoreSessions { await Task.yield() }
            await model.loadMoreSessions()
        }
        let loading = await eventually { self.core.calls.contains { $0.method == "uploader.sessions.list" && $0.params["limit"] as? Int == 1000 } }
        XCTAssertTrue(loading)
        await model.loadMoreSessions()
        XCTAssertEqual(core.calls.filter { $0.method == "uploader.sessions.list" && $0.params["limit"] as? Int == 1000 }.count, 1)
        held.signal(); await first.value
        XCTAssertEqual(model.sessions.count, 503)
        XCTAssertEqual(model.sessionLimit, 1000)
        XCTAssertFalse(model.canLoadMoreSessions)
        await model.loadMoreSessions()
        XCTAssertEqual(model.sessionLimit, 1000)
    }

    func testFailedPaginationRecoversOnAutomaticRefresh() async {
        core.on("uploader.sessions.list") { _ in Self.history(500) }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        let ready = await eventually { model.canLoadMoreSessions }
        XCTAssertTrue(ready)
        core.on("uploader.sessions.list") { _ in
            throw FakeCore.Failure(kind: "internal", message: "Temporary outage")
        }
        await model.loadMoreSessions()
        XCTAssertEqual(model.sessions.count, 500)
        XCTAssertNotNil(model.sessionsError)
        XCTAssertFalse(model.canLoadMoreSessions)
        core.on("uploader.sessions.list") { params in
            Self.history(min(params["limit"] as? Int ?? 500, 503))
        }
        model.windowVisibilityChanged(true)
        await model.refreshVisibleState()
        XCTAssertEqual(model.sessions.count, 503)
        XCTAssertNil(model.sessionsError)
        XCTAssertEqual(model.sessionLimit, 1000)
        model.shutdown()
    }

    func testLocalSessionLanding() async throws {
        let model = model()
        XCTAssertEqual(model.page, .sessions)
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        await model.bootstrapIfNeeded()
        await model.refreshVisibleState()
        let loaded = await eventually { model.sessions.count == 2 && model.relay.sessions.count == 1 }
        XCTAssertTrue(loaded)
        let row = try XCTUnwrap(LocalSessionRow.merge(uploads: model.sessions, sessions: model.relay.sessions).first)
        model.selectedLocalSession = row
        model.sessionSearch = "Fix tests"
        model.page = .settings(.account)
        model.showLocalSessions()
        XCTAssertEqual(model.page, .sessions)
        XCTAssertNil(model.selectedLocalSession)
        XCTAssertEqual(model.sessionSearch, "Fix tests")
        model.selectedLocalSession = row
        model.page = .settings(.account)
        model.showLocalSessions(search: "is:active is:on-relay")
        XCTAssertEqual(model.page, .sessions)
        XCTAssertNil(model.selectedLocalSession)
        XCTAssertEqual(model.sessionSearch, "is:active is:on-relay")
        model.showDiagnosticsFromMenu()
        XCTAssertEqual(model.page, .settings(.diagnostics))
        model.shutdown()
    }

    func testRegistersTheCoreWhenItIsNotRegistered() async {
        let service = FakeService(status: .notRegistered)
        let model = model(service: service)
        await model.bootstrapIfNeeded()
        XCTAssertEqual(service.registered, 1)
        XCTAssertFalse(model.coreNeedsApproval)
        model.shutdown()
    }

    func testRecoveryLetsAFreshlyRegisteredCoreStartWithoutAnotherRestart() async {
        let service = FakeService(status: .notRegistered)
        let model = model(service: service)
        defer { model.shutdown() }

        await model.retryBackgroundService()

        XCTAssertEqual(service.registered, 1)
        XCTAssertEqual(service.unregistered, 0)
        XCTAssertEqual(service.spawnRepairChecks, 0)
        XCTAssertEqual(service.status, .enabled)
    }

    func testRecoveryDoesNotRestartTheCoreTwiceAfterABinaryUpdate() async {
        let service = FakeService(status: .enabled)
        defaults.set("old-build", forKey: AppModel.registeredCoreKey)
        let model = model(service: service)
        defer { model.shutdown() }

        await model.retryBackgroundService()

        XCTAssertEqual(service.registered, 1)
        XCTAssertEqual(service.unregistered, 1)
        XCTAssertEqual(service.spawnRepairChecks, 0)
    }

    func testRequiresAnInstalledAppBeforeRegisteringTheCore() async {
        let service = FakeService(status: .notRegistered)
        service.installationProblem = "Move Agent Relay to Applications before opening it."
        let model = model(service: service)
        await model.bootstrapIfNeeded()
        XCTAssertEqual(service.registered, 0)
        XCTAssertEqual(model.coreProblem, service.installationProblem)
        XCTAssertTrue(model.coreNeedsInstallation)
        XCTAssertFalse(model.isCoreConnected)
        XCTAssertTrue(core.calls.isEmpty)
        await model.retryBackgroundService()
        XCTAssertEqual(service.registered, 0)
        XCTAssertEqual(service.unregistered, 0)
        model.stopBackgroundService()
        XCTAssertEqual(service.unregistered, 0)
        model.shutdown()
        XCTAssertEqual(service.unregistered, 0)
    }

    func testOnlyApplicationsLocationsCanRegisterTheBundledAgent() {
        let home = URL(fileURLWithPath: "/Users/example", isDirectory: true)
        XCTAssertTrue(LaunchAgentService.isInstalledInApplications(
            URL(fileURLWithPath: "/Applications/Agent Relay.app"), homeDirectory: home))
        XCTAssertTrue(LaunchAgentService.isInstalledInApplications(
            URL(fileURLWithPath: "/Users/example/Applications/Agent Relay.app"), homeDirectory: home))
        XCTAssertFalse(LaunchAgentService.isInstalledInApplications(
            URL(fileURLWithPath: "/Volumes/Agent Relay/Agent Relay.app"), homeDirectory: home))
        XCTAssertFalse(LaunchAgentService.isInstalledInApplications(
            URL(fileURLWithPath: "/Users/example/Downloads/Agent Relay.app"), homeDirectory: home))
    }

    func testMovingAnUnchangedAppChangesItsCoreRegistrationIdentity() {
        let date = Date(timeIntervalSince1970: 1_800_000_000)
        let first = LaunchAgentService.binaryIdentity(
            digest: "same-helper", modified: date,
            bundleURL: URL(fileURLWithPath: "/Users/example/Applications/Agent Relay.app"))
        let moved = LaunchAgentService.binaryIdentity(
            digest: "same-helper", modified: date,
            bundleURL: URL(fileURLWithPath: "/Applications/Agent Relay.app"))
        XCTAssertNotEqual(first, moved)
    }

    func testRegistersAgainForANewCoreBinary() async {
        let service = FakeService(status: .enabled)
        defaults.set("build-1", forKey: AppModel.registeredCoreKey)
        let model = model(service: service)
        await model.ensureService()
        XCTAssertEqual(service.unregistered, 0)
        service.binaryIdentity = "build-2"
        await model.ensureService()
        XCTAssertEqual(service.unregistered, 1)
        XCTAssertEqual(service.registered, 1)
        XCTAssertEqual(defaults.string(forKey: AppModel.registeredCoreKey), "build-2")
        await model.ensureService()
        XCTAssertEqual(service.registered, 1)
    }

    func testFallbackUpdateUsesReloadWithoutUnregisteringOrChangingPreferences() async {
        let service = FakeService(status: .enabled)
        service.handlesUpdateReload = true
        defaults.set("old-build", forKey: AppModel.registeredCoreKey)
        defaults.set(true, forKey: AppModel.keepsAgentsConnectedKey)
        let model = model(service: service)
        await model.ensureService()
        XCTAssertEqual(service.updateReloads, 1)
        XCTAssertEqual(service.unregistered, 0)
        XCTAssertEqual(service.registered, 0)
        XCTAssertEqual(defaults.string(forKey: AppModel.registeredCoreKey), "build-1")
        XCTAssertTrue(model.keepsAgentsConnectedAfterQuit)
        await model.ensureService()
        XCTAssertEqual(service.updateReloads, 1, "The next service check must not repeat the successful handoff")
    }

    func testFailedFallbackReloadDoesNotRecordTheNewRevisionAndCanRetry() async {
        let service = FakeService(status: .enabled)
        service.handlesUpdateReload = true
        service.updateReloadError = FakeCore.Failure(kind: "unavailable", message: "Temporary bootstrap failure")
        defaults.set("old-build", forKey: AppModel.registeredCoreKey)
        let model = model(service: service)
        await model.ensureService()
        XCTAssertEqual(defaults.string(forKey: AppModel.registeredCoreKey), "old-build")
        XCTAssertEqual(service.unregistered, 0)
        service.updateReloadError = nil
        await model.ensureService()
        XCTAssertEqual(service.updateReloads, 2)
        XCTAssertEqual(defaults.string(forKey: AppModel.registeredCoreKey), "build-1")
    }

    func testCancelledBinaryReplacementRestoresRegistration() async {
        let service = FakeService(status: .enabled)
        let model = model(service: service, unregisterSettle: 60_000_000_000)
        let repair = Task { await model.ensureService() }
        let unregistered = await eventually { service.unregistered == 1 }
        XCTAssertTrue(unregistered)
        repair.cancel()
        await repair.value
        XCTAssertEqual(service.status, .enabled)
        XCTAssertEqual(service.registered, 1)
        XCTAssertEqual(defaults.string(forKey: AppModel.registeredCoreKey), "build-1")
    }

    func testCancelledRecoveryRestoresRegistrationWithoutStartingAnotherRepair() async {
        // Cover cancellation in both ensureService and the refusal repair itself.
        for binaryChanged in [false, true] {
            let service = FakeService(status: .enabled)
            defaults.set(binaryChanged ? "old-build" : "build-1", forKey: AppModel.registeredCoreKey)
            let model = model(service: service, unregisterSettle: 60_000_000_000)
            let repair = Task { await model.retryBackgroundService() }
            let unregistered = await eventually { service.unregistered == 1 }
            XCTAssertTrue(unregistered)
            repair.cancel()
            await repair.value
            XCTAssertEqual(service.status, .enabled)
            XCTAssertEqual(service.registered, 1)
            XCTAssertEqual(service.unregistered, 1)
            XCTAssertFalse(model.isRecoveringCore)
        }
    }

    func testShutdownRestoresRegistrationBeforeReturning() async {
        let service = FakeService(status: .enabled)
        defaults.set("build-1", forKey: AppModel.registeredCoreKey)
        let model = model(service: service, unregisterSettle: 60_000_000_000)
        let repair = Task { await model.retryBackgroundService() }
        let unregistered = await eventually { service.unregistered == 1 }
        XCTAssertTrue(unregistered)
        model.shutdown()
        // applicationWillTerminate cannot wait for an async repair to resume.
        XCTAssertEqual(service.status, .enabled)
        XCTAssertEqual(service.registered, 1)
        repair.cancel()
        await repair.value
        XCTAssertEqual(service.registered, 1)
    }

    func testExplicitStopDuringRecoveryDoesNotRestoreRegistration() async {
        let service = FakeService(status: .enabled)
        defaults.set("build-1", forKey: AppModel.registeredCoreKey)
        let model = model(service: service, unregisterSettle: 60_000_000_000)
        let repair = Task { await model.retryBackgroundService() }
        let unregistered = await eventually { service.unregistered == 1 }
        XCTAssertTrue(unregistered)
        model.stopBackgroundService()
        model.shutdown()
        repair.cancel()
        await repair.value
        XCTAssertEqual(service.status, .notRegistered)
        XCTAssertEqual(service.registered, 0)
    }

    func testQuittingLeavesTheCoreRunningByDefault() async {
        let service = FakeService(status: .enabled)
        defaults.set("build-1", forKey: AppModel.registeredCoreKey)
        let model = model(service: service)
        XCTAssertTrue(model.keepsAgentsConnectedAfterQuit)
        await model.bootstrapIfNeeded()
        model.shutdown()
        XCTAssertEqual(service.unregistered, 0)
        XCTAssertEqual(service.status, .enabled)
    }

    func testQuittingStopsTheCoreUntilTheAppOpensAgainWhenChosen() async {
        let service = FakeService(status: .enabled)
        defaults.set("build-1", forKey: AppModel.registeredCoreKey)
        let first = model(service: service)
        first.setKeepsAgentsConnectedAfterQuit(false)
        await first.bootstrapIfNeeded()
        first.shutdown()
        XCTAssertEqual(service.unregistered, 1)
        XCTAssertEqual(service.status, .notRegistered)

        // The choice is remembered, and opening the app registers the core again.
        let second = model(service: service)
        XCTAssertFalse(second.keepsAgentsConnectedAfterQuit)
        await second.bootstrapIfNeeded()
        XCTAssertEqual(service.registered, 1)
        XCTAssertEqual(service.status, .enabled)
        second.shutdown()
    }

    func testAsksBeforeQuittingOnlyWhenQuittingTakesSignedInAgentsOffline() async {
        let model = model()
        model.setKeepsAgentsConnectedAfterQuit(false)
        XCTAssertFalse(model.asksBeforeQuitting, "nothing is on the relay before sign-in")

        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.phase == .attached }
        XCTAssertTrue(ready)
        XCTAssertTrue(model.asksBeforeQuitting)

        model.setKeepsAgentsConnectedAfterQuit(true)
        XCTAssertFalse(model.asksBeforeQuitting)
    }

    func testQuitChoicesOnlyKeepAgentsConnectedWhenRequested() async {
        let model = model()
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.phase == .attached }
        XCTAssertTrue(ready)

        for choice in [NSApplication.ModalResponse.cancel, .alertSecondButtonReturn, .alertFirstButtonReturn] {
            model.setKeepsAgentsConnectedAfterQuit(false)
            var asked = 0
            let delegate = AppDelegate(model: model, confirmQuit: { asked += 1; return choice })
            let reply = delegate.applicationShouldTerminate(NSApplication.shared)
            XCTAssertEqual(asked, 1)
            XCTAssertEqual(reply, choice == .cancel ? .terminateCancel : .terminateNow)
            XCTAssertEqual(model.keepsAgentsConnectedAfterQuit, choice == .alertFirstButtonReturn)
        }
    }

    func testWaitsForLoginItemsApproval() async {
        let service = FakeService(status: .requiresApproval)
        let model = model(service: service)
        await model.bootstrapIfNeeded()
        XCTAssertEqual(service.registered, 0)
        XCTAssertTrue(model.coreNeedsApproval)
        model.openLoginItemsSettings()
        XCTAssertEqual(service.openedSettings, 1)
        model.shutdown()
    }

    func testRendersTheSnapshotAndFollowsTheAccount() async {
        let model = model()
        await model.bootstrapIfNeeded()
        let launched = await eventually { model.phase == .signedOut }
        XCTAssertTrue(launched)
        XCTAssertTrue(model.isCoreConnected)

        core.setStatus(FakeCore.snapshot(revision: 2, account: ["state": "pending_approval", "phase": "waiting_for_browser"]))
        let waiting = await eventually { model.phase == .waitingForBrowser }
        XCTAssertTrue(waiting)

        core.setStatus(FakeCore.snapshot(revision: 3, account: FakeCore.signedIn))
        let attached = await eventually { model.phase == .attached && model.status != nil && model.sessions.count == 2 }
        XCTAssertTrue(attached)
        XCTAssertEqual(model.accountEmail, "ada@example.com")
        XCTAssertEqual(model.statusViewModel?.state, .upToDate)
        XCTAssertEqual(model.sessionUploadSummary?.headline, "Uploads up to date")
        XCTAssertEqual(model.uploadState(for: model.sessions[0]), .uploaded)
        XCTAssertEqual(model.uploadState(for: model.sessions[1]), .local)

        // An older revision never replaces a newer one.
        core.setStatus(FakeCore.snapshot(revision: 2, account: ["state": "signed_out"]))
        try? await Task.sleep(nanoseconds: 200_000_000)
        XCTAssertEqual(model.phase, .attached)

        core.setStatus(FakeCore.snapshot(revision: 4, account: ["state": "error", "error": "The approval expired."]))
        let failed = await eventually { model.phase == .signedOut }
        XCTAssertTrue(failed)
        XCTAssertEqual(model.signInError, "The approval expired.")
        XCTAssertNil(model.status)
        model.shutdown()
    }

    func testSettingsInvalidationsAreCountedForRowsThatReadTheirOwnStatus() async {
        let model = model()
        await model.bootstrapIfNeeded()
        _ = await eventually { self.core.callCount("settings.get") >= 1 }
        let reads = core.callCount("settings.get")
        XCTAssertEqual(model.settingsInvalidations, 0)
        core.publish("invalidated", ["what": "settings"])
        let counted = await eventually { model.settingsInvalidations == 1 }
        XCTAssertTrue(counted, "the Agent Relay tools row restarts its read on this")
        let reread = await eventually { self.core.callCount("settings.get") > reads }
        XCTAssertTrue(reread)
        model.shutdown()
    }

    func testInvalidationsRefetchTheirCollection() async {
        let model = model()
        await model.bootstrapIfNeeded()
        _ = await eventually { self.core.callCount("agents.list") >= 1 }
        let before = core.callCount("agents.list")
        core.publish("invalidated", ["what": "agents"])
        let refetched = await eventually { self.core.callCount("agents.list") > before }
        XCTAssertTrue(refetched)
        model.shutdown()
    }

    nonisolated static func rosterAgent(_ liveness: String) -> [String: Any] {
        ["name": "bot", "address": "bot@box", "kind": "agent", "where": "other_desktop",
         "status": liveness == "live" ? "active" : "idle", "liveness": liveness, "last_seen_ms": 1,
         "description": "", "host": NSNull(), "owner_hash": NSNull(), "session_id": NSNull(), "is_self": false]
    }

    nonisolated static func recipient(_ liveness: String, message: String?, known: Bool = true) -> [String: Any] {
        guard known else { return ["known": false, "agent": NSNull(), "message": NSNull()] }
        return ["known": true, "agent": rosterAgent(liveness), "message": message ?? NSNull()]
    }

    func testTheComposerShowsWhatRelaycastHoldsForTheMessage() async {
        let message = Locked<String?>("queued")
        core.on("roster.get") { params in
            Self.recipient("live", message: params["message_id"] == nil ? nil : message.value)
        }
        core.on("messages.send") { params in
            ["message_id": "m1", "conversation_id": "c1", "to": params["to"] ?? NSNull(),
             "recipient": Self.recipient("live", message: "queued")]
        }
        core.on("messages.history") { _ in ["identity": "ada", "messages": []] }
        let model = model()
        await model.bootstrapIfNeeded()
        let relay = model.relay
        relay.openComposer("@bot")

        let error = await relay.sendMessage(to: "@bot", text: "status?")
        XCTAssertNil(error)
        // Live, but Relaycast still holds it: never shown as delivered.
        XCTAssertEqual(relay.deliveryNote(at: "bot")?.text, "Queued — not yet handed to the agent")

        message.value = "delivered"
        core.publish("invalidated", ["what": "sessions"])
        let delivered = await eventually { relay.deliveryNote(at: "bot")?.text == "Delivered" }
        XCTAssertTrue(delivered)
        XCTAssertEqual(core.lastParams("roster.get")?["message_id"] as? String, "m1")

        // A read that cannot tell keeps what Relaycast last reported.
        message.value = nil
        await relay.lookUpRecipient("bot")
        XCTAssertEqual(relay.deliveryNote(at: "bot")?.text, "Delivered")

        // Writing the next message gives the note way to the recipient.
        relay.forgetLastSend()
        XCTAssertNil(relay.deliveryNote(at: "bot"))
        relay.closeComposer()
        XCTAssertNil(relay.recipient(at: "bot"))
        let reads = core.callCount("roster.get")
        core.publish("invalidated", ["what": "sessions"])
        try? await Task.sleep(nanoseconds: 300_000_000)
        XCTAssertEqual(core.callCount("roster.get"), reads, "a closed composer follows nothing")
        model.shutdown()
    }

    func testChangingToRetiresTheLastSendAndItsFollowUp() async {
        core.on("roster.get") { params in
            let offline = params["to"] as? String == "sleeper"
            var agent = Self.rosterAgent(offline ? "offline" : "live")
            agent["name"] = params["to"] ?? "bot"
            return ["known": true, "agent": agent,
                    "message": params["message_id"] == nil ? NSNull() : "queued"] as [String: Any]
        }
        core.on("messages.send") { _ in
            ["message_id": "m1", "conversation_id": "c1", "to": "bot", "recipient": Self.recipient("live", message: "queued")]
        }
        core.on("messages.history") { _ in ["identity": "ada", "messages": []] }
        let model = model()
        await model.bootstrapIfNeeded()
        let relay = model.relay
        relay.openComposer("@bot")

        _ = await relay.sendMessage(to: "@bot", text: "status?")
        XCTAssertNotNil(relay.deliveryNote(at: "bot"))
        // To changes to an agent that is not running before the follow-up fires.
        relay.composerShows("@sleeper")
        XCTAssertNil(relay.deliveryNote(at: "bot"), "the last send is retired")
        await relay.lookUpRecipient("@sleeper")
        let reads = core.callCount("roster.get")
        try? await Task.sleep(nanoseconds: 2_400_000_000)
        XCTAssertEqual(core.callCount("roster.get"), reads, "the retired send's follow-up never reads")
        XCTAssertEqual(relay.recipient(at: "sleeper")?.agent?.name, "sleeper")
        XCTAssertEqual(relay.recipient(at: "sleeper").flatMap { MessageDelivery.warning(for: $0) },
                       "@sleeper is not running right now. Queue anyway?")
        // Invalidations follow the new recipient only.
        core.publish("invalidated", ["what": "sessions"])
        let followed = await eventually { self.core.callCount("roster.get") > reads }
        XCTAssertTrue(followed)
        XCTAssertEqual(core.lastParams("roster.get")?["to"] as? String, "sleeper")
        model.shutdown()
    }

    func testAClosedComposerFollowsNothing() async {
        let held = DispatchSemaphore(value: 0)
        let holding = Locked(true)
        core.on("roster.get") { _ in
            if holding.value { held.wait() }
            return Self.recipient("offline", message: nil)
        }
        let model = model()
        await model.bootstrapIfNeeded()
        let relay = model.relay
        relay.openComposer("@bot")
        // A lookup is in flight when the sheet closes.
        let inFlight = Task { await relay.lookUpRecipient("@bot") }
        let started = await eventually { self.core.callCount("roster.get") == 1 }
        XCTAssertTrue(started)
        relay.closeComposer()
        holding.value = false
        held.signal()
        await inFlight.value
        XCTAssertNil(relay.recipient(at: "bot"), "the answer for a closed composer is dropped")
        // The sheet's task firing after close is only a read, and reads nothing.
        await relay.lookUpRecipient("@bot")
        for what in ["sessions", "roster", "messages"] {
            core.publish("invalidated", ["what": what])
        }
        try? await Task.sleep(nanoseconds: 400_000_000)
        XCTAssertEqual(core.callCount("roster.get"), 1, "nothing is followed after close")
        model.shutdown()
    }

    func testAnotherWorkspaceSaysNothingAboutTheRecipient() async {
        core.on("roster.get") { _ in Self.recipient("live", message: nil, known: false) }
        core.on("messages.send") { _ in
            ["message_id": "m1", "conversation_id": "c1", "to": "alice",
             "recipient": Self.recipient("live", message: nil, known: false)]
        }
        core.on("messages.history") { _ in ["identity": "ada", "messages": []] }
        let model = model()
        await model.bootstrapIfNeeded()
        let relay = model.relay
        relay.openComposer("alice")
        await relay.lookUpRecipient("alice")
        let unknown = relay.recipient(at: "alice")
        XCTAssertEqual(unknown?.known, false)
        XCTAssertNil(unknown.flatMap { MessageDelivery.warning(for: $0) })
        XCTAssertFalse(MessageDelivery.queues(unknown))
        _ = await relay.sendMessage(to: "alice", text: "hi")
        XCTAssertEqual(relay.deliveryNote(at: "alice")?.text, "Sent — delivery unknown")
        model.shutdown()
    }

    func testASendToNoSuchAgentSaysSo() async {
        core.on("messages.send") { _ in throw FakeCore.Failure(kind: "not_found", message: "No agent is at that address.") }
        let model = model()
        await model.bootstrapIfNeeded()
        model.relay.openComposer("ghost@box")
        let error = await model.relay.sendMessage(to: "ghost@box", text: "hello")
        XCTAssertEqual(error, "No such agent")
        XCTAssertEqual(model.relay.recipient(at: "ghost@box").map { MessageDelivery.warning(for: $0) },
                       "No agent has this name or address.")
        XCTAssertEqual(core.lastParams("messages.send")?["address"] as? String, "ghost@box")
        model.shutdown()
    }

    func testCarriesAnOptOutOverOnceAndForgetsTheOldKeys() async {
        defaults.set(false, forKey: LegacyMigration.analyticsEnabledKey)
        defaults.set("old-id", forKey: LegacyMigration.analyticsIdentityKey)
        defaults.set("device", forKey: LegacyMigration.deviceIDKey)
        defaults.set(true, forKey: LegacyMigration.loginItemConfiguredKey)
        defaults.set("digest", forKey: LegacyMigration.incomingRecoveryPrefix + "/x/shares.json")
        let model = model()
        await model.bootstrapIfNeeded()
        let migrated = await eventually { model.settings?.analyticsEnabled == false }
        XCTAssertTrue(migrated)
        XCTAssertEqual(core.callCount("settings.set"), 1)
        XCTAssertEqual(core.lastParams("settings.set")?["analytics_enabled"] as? Bool, false)
        for key in [LegacyMigration.analyticsEnabledKey, LegacyMigration.analyticsIdentityKey,
                    LegacyMigration.deviceIDKey, LegacyMigration.loginItemConfiguredKey,
                    LegacyMigration.incomingRecoveryPrefix + "/x/shares.json"] {
            XCTAssertNil(defaults.object(forKey: key), key)
        }
        model.shutdown()

        let again = self.model()
        await again.bootstrapIfNeeded()
        _ = await eventually { again.settings != nil }
        XCTAssertEqual(core.callCount("settings.set"), 1)
        again.shutdown()
    }

    func testRestartsACoreOfAnotherVersion() async {
        let model = model(version: "2.0.0")
        await model.bootstrapIfNeeded()
        let asked = await eventually { self.core.callCount("daemon.shutdown") == 1 }
        XCTAssertTrue(asked)
        // The core asked to stop still answers while it exits; the app
        // never connects to it.
        let reconnected = await eventually(3) { self.core.callCount("subscribe") >= 3 }
        XCTAssertTrue(reconnected)
        XCTAssertFalse(model.isCoreConnected)
        // launchd's replacement is connected to, and asked nothing.
        core.pid += 1
        let connected = await eventually { model.isCoreConnected && model.phase == .signedOut }
        XCTAssertTrue(connected)
        XCTAssertEqual(core.callCount("daemon.shutdown"), 1)
        model.shutdown()
    }

    func testVersionMismatchDoesNotRestartEveryReplacementProcess() async {
        core.on("daemon.shutdown") { [core] _ in
            core!.pid += 1
            return ["stopping": true]
        }
        let model = model(version: "incorrect-build-metadata")
        await model.bootstrapIfNeeded()
        let connected = await eventually { model.phase == .signedOut }
        XCTAssertTrue(connected, "A bridge-compatible replacement must reach sign-in")
        XCTAssertEqual(core.callCount("daemon.shutdown"), 1)
        model.shutdown()
    }

    /// A replacement still on the old build is rechecked later. When that
    /// recheck is cancelled after its matching hello completed and before it
    /// writes the stop, the core is left usable: not retired, not counted as
    /// restarted, and connected to again. An implementation that counted the
    /// matching hello as a stop would keep the pid retired and fail here.
    func testARecheckCancelledBeforeItsStopLeavesTheCoreUsable() async {
        core.on("daemon.shutdown") { [core] _ in
            core!.pid += 1
            return ["stopping": true]
        }
        let model = model(version: "2.0.0", coreVersionRecheckDelay: 200_000_000)
        let fake = core!
        let reached = OneShot()
        let cancelled = OneShot()
        // Runs in the recheck's task once the matching hello has completed.
        // Closing the event stream makes connectLoop cancel the recheck; the
        // hook returns only once that cancellation has reached the task, so
        // the stop is never written. Later rechecks of the reattached core
        // hold the same way until the model shuts down.
        model.client.afterGuardedShutdownHello = {
            let first = reached.fire()
            if first { fake.dropSubscribers() }
            let deadline = Date().addingTimeInterval(5)
            while !Task.isCancelled && Date() < deadline {
                try? await Task.sleep(nanoseconds: 10_000_000)
            }
            if first && Task.isCancelled { _ = cancelled.fire() }
        }
        await model.bootstrapIfNeeded()
        let attached = await eventually { model.isCoreConnected && model.phase == .signedOut }
        XCTAssertTrue(attached)
        XCTAssertEqual(core.callCount("daemon.shutdown"), 1)
        let replacement = core.pid
        let subscribes = core.callCount("subscribe")
        let recheckCancelled = await eventually { cancelled.fired }
        XCTAssertTrue(reached.fired, "the recheck must complete its matching hello")
        XCTAssertTrue(recheckCancelled, "closing the event stream must cancel the recheck")
        // The same core, subscribed to again.
        let reattached = await eventually {
            self.core.callCount("subscribe") > subscribes && model.isCoreConnected
                && !model.retiredCorePIDs.contains(replacement)
        }
        XCTAssertTrue(reattached, "the core a cancelled recheck checked must stay usable")
        XCTAssertEqual(core.pid, replacement)
        XCTAssertEqual(core.callCount("daemon.shutdown"), 1)
        XCTAssertEqual(model.coreVersionRestarts["1.0.0"], 1)
        model.shutdown()
    }

    /// A recheck whose stop was sent counts as a stop even when the core
    /// exits without answering it.
    func testARecheckWhoseStopReplyIsLostStillCountsAsAStop() async {
        core.on("daemon.shutdown") { [core] _ in
            core!.pid += 1
            return ["stopping": true]
        }
        let model = model(version: "2.0.0", coreVersionRecheckDelay: 200_000_000)
        await model.bootstrapIfNeeded()
        let attached = await eventually { model.isCoreConnected && model.phase == .signedOut }
        XCTAssertTrue(attached)
        let replacement = core.pid
        // The replacement exits on the recheck's stop, and its reply is lost.
        core.on("daemon.shutdown") { [core] _ in
            core!.dropConnections()
            return ["stopping": true]
        }
        let asked = await eventually { self.core.callCount("daemon.shutdown") == 2 }
        XCTAssertTrue(asked)
        let counted = await eventually { model.coreVersionRestarts["1.0.0"] == 2 }
        XCTAssertTrue(counted, "a stop that was sent counts, though its reply was lost")
        XCTAssertTrue(model.retiredCorePIDs.contains(replacement))
        model.shutdown()
    }

    func testManualRecoveryRegistersTheServiceAgain() async {
        let service = FakeService(status: .enabled)
        defaults.set("build-1", forKey: AppModel.registeredCoreKey)
        let model = model(service: service)
        await model.retryBackgroundService()
        XCTAssertEqual(service.unregistered, 1)
        XCTAssertEqual(service.registered, 1)
        XCTAssertFalse(model.isRecoveringCore)
        XCTAssertNil(model.coreProblem)
    }

    func testBrokenBundleProgramUsesFallbackWithoutReregistering() async {
        let service = FakeService(status: .enabled)
        service.repairsSpawnFailure = true
        defaults.set("build-1", forKey: AppModel.registeredCoreKey)
        let model = model(service: service)
        await model.retryBackgroundService()
        XCTAssertEqual(service.unregistered, 0)
        XCTAssertEqual(service.registered, 0)
        XCTAssertFalse(model.isRecoveringCore)
        XCTAssertNil(model.coreProblem)
    }

    func testFallbackApprovalIsShownImmediatelyAfterRecovery() async {
        let service = FakeService(status: .enabled)
        service.repairsSpawnFailure = true
        service.spawnRepairStatus = .requiresApproval
        defaults.set("build-1", forKey: AppModel.registeredCoreKey)
        let model = model(service: service)
        await model.retryBackgroundService()
        XCTAssertTrue(model.coreNeedsApproval)
        XCTAssertTrue(model.coreProblem?.contains("Allow Agent Relay") == true)
        XCTAssertEqual(service.registered, 0)
    }

    func testBrokenBundleProgramIsCheckedBeforeGeneralRecovery() async {
        core.stop()
        let service = FakeService(status: .enabled)
        service.repairsSpawnFailure = true
        defaults.set("build-1", forKey: AppModel.registeredCoreKey)
        let model = model(service: service)
        await model.bootstrapIfNeeded()
        let checked = await eventually(3) { service.spawnRepairChecks > 0 }
        XCTAssertTrue(checked)
        try? await Task.sleep(for: .milliseconds(150))
        XCTAssertEqual(service.unregistered, 0)
        XCTAssertEqual(service.registered, 0)
        model.shutdown()
    }

    func testRecognizesOnlyTheBrokenBundleProgramFailure() {
        let broken = """
        program identifier = Contents/Helpers/Agent Relay Helper (mode: 2)
        last exit code = 78: EX_CONFIG
        job state = spawn failed
        """
        XCTAssertTrue(LaunchAgentService.hasBrokenBundleProgram(broken))
        XCTAssertFalse(LaunchAgentService.hasBrokenBundleProgram(
            broken.replacingOccurrences(of: "78: EX_CONFIG", with: "1")))
        XCTAssertFalse(LaunchAgentService.hasBrokenBundleProgram(
            broken.replacingOccurrences(of: "78: EX_CONFIG", with: "780: OTHER")))
        XCTAssertFalse(LaunchAgentService.hasBrokenBundleProgram(
            broken.replacingOccurrences(of: "spawn failed", with: "running")))
    }

    func testFailedFallbackShowsARelevantStartupError() async {
        let service = FakeService(status: .enabled)
        service.spawnRepairError = NSError(domain: "com.agentrelay.desktop.launch-agent", code: 1)
        defaults.set("build-1", forKey: AppModel.registeredCoreKey)
        let model = model(service: service)
        await model.retryBackgroundService()
        XCTAssertTrue(model.coreProblem?.contains("backup background service") == true)
        XCTAssertEqual(service.unregistered, 0)
    }

    func testRecoveryPreservesRequiredApproval() async {
        let service = FakeService(status: .requiresApproval)
        let model = model(service: service)
        await model.retryBackgroundService()
        XCTAssertTrue(model.coreNeedsApproval)
        XCTAssertNotNil(model.coreProblem)
        XCTAssertEqual(service.unregistered, 0)
        XCTAssertEqual(service.registered, 0)
    }

    func testMissingServiceHasActionableStartupError() async {
        let model = model(service: FakeService(status: .notFound))
        await model.ensureService()
        XCTAssertNotNil(model.coreProblem)
    }

    func testRecoveryShowsApprovalRequiredByNewRegistration() async {
        let service = FakeService(status: .enabled)
        service.registrationStatus = .requiresApproval
        defaults.set("build-1", forKey: AppModel.registeredCoreKey)
        let model = model(service: service)
        await model.retryBackgroundService()
        XCTAssertTrue(model.coreNeedsApproval)
        XCTAssertNotNil(model.coreProblem)
        XCTAssertFalse(model.isRecoveringCore)
    }

    func testFailedRegistrationIsVisibleAndCanBeRetried() async {
        let service = FakeService(status: .notRegistered)
        service.registrationError = NSError(domain: "test", code: 1)
        let model = model(service: service)
        await model.ensureService()
        XCTAssertNotNil(model.coreProblem)
        service.registrationError = nil
        await model.retryBackgroundService()
        XCTAssertEqual(service.status, .enabled)
        XCTAssertFalse(model.isRecoveringCore)
    }

    func testWorkspacePickerStaysOnWelcomeAndSendsSelectedID() async {
        core.on("account.select_workspace") { _ in [:] }
        core.setStatus(FakeCore.snapshot(revision: 2, account: [
            "state": "preparing", "phase": "selecting_workspace",
            "person": ["id": "u", "email": "ada@example.com"],
            "workspace_choices": [
                ["id": "a", "name": "Alpha", "organization_name": "Company"],
                ["id": "b", "name": "Beta", "organization_name": "Personal"]
            ]
        ]))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.account.workspaceChoices?.count == 2 }
        XCTAssertTrue(ready)
        XCTAssertEqual(model.phase, .signingIn)
        XCTAssertEqual(model.account.phase, "selecting_workspace")
        if let path = ProcessInfo.processInfo.environment["WORKSPACE_PICKER_RENDER_PATH"] {
            let host = NSHostingView(rootView: SignInView().environment(model))
            let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 980, height: 650), styleMask: [.borderless], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            window.contentView = host
            defer { window.close() }
            try? await Task.sleep(for: .milliseconds(100))
            host.layoutSubtreeIfNeeded()
            if let bitmap = host.bitmapImageRepForCachingDisplay(in: host.bounds) {
                host.cacheDisplay(in: host.bounds, to: bitmap)
                try? bitmap.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
            }
        }
        model.selectWorkspace("b")
        let selected = await eventually { self.core.callCount("account.select_workspace") == 1 }
        XCTAssertTrue(selected)
        XCTAssertEqual(core.lastParams("account.select_workspace")?["workspace_id"] as? String, "b")
        XCTAssertEqual(core.callCount("account.sign_in"), 0)
    }

    func testSignInRequestsGoogleAccountSelection() async {
        core.on("account.sign_in") { [core] _ in
            core!.setStatus(FakeCore.snapshot(revision: 5, account: ["state": "pending_approval", "phase": "waiting_for_browser"]))
            return ["state": "pending_approval", "phase": "waiting_for_browser",
             "verification_url": "https://agentrelay.com/login", "verification_url_complete": "https://agentrelay.com/login?s=1"]
        }
        let model = model()
        await model.bootstrapIfNeeded()
        _ = await eventually { model.phase == .signedOut }
        model.signIn()
        let asked = await eventually { model.signInLink == "https://agentrelay.com/login?s=1" && model.phase == .waitingForBrowser }
        XCTAssertTrue(asked)
        let params = core.lastParams("account.sign_in")
        XCTAssertEqual(params?["flow"] as? String, "browser")
        XCTAssertEqual(params?["sharing"] as? String, "selected")
        // The site is the core's own build's; the request names none.
        XCTAssertNil(params?["site_url"])
        model.shutdown()
    }

    func testContinueAnywayStillRequestsGoogleAccountSelection() async {
        let model = model()
        await model.bootstrapIfNeeded()
        _ = await eventually { model.phase == .signedOut }
        model.signIn(acknowledgeUninspectedSchedules: true)
        let asked = await eventually { self.core.callCount("account.sign_in") == 1 }
        XCTAssertTrue(asked)
        XCTAssertEqual(core.lastParams("account.sign_in")?["flow"] as? String, "browser")
        XCTAssertEqual(core.lastParams("account.sign_in")?["acknowledge_uninspected_schedules"] as? Bool, true)
        model.shutdown()
    }

    func testProviderReconsentStartsBrowserRenewalWhileAccountIsHealthy() async {
        let connected = FakeCore.signedIn.merging(["auth_health": "ready"]) { $1 }
        core.setStatus(FakeCore.snapshot(revision: 2, account: connected))
        core.on("account.sign_in") { _ in
            connected.merging(["state": "pending_approval", "phase": "waiting_for_browser",
                               "verification_url_complete": "https://agentrelay.com/login?consent=1"]) { $1 }
        }
        let model = model()
        defer { model.shutdown() }
        await model.bootstrapIfNeeded()
        _ = await eventually { model.phase == .attached }
        model.recoverProviderAccount(state: "signed_out")
        let requested = await eventually { model.signInLink == "https://agentrelay.com/login?consent=1" }
        XCTAssertTrue(requested)
        XCTAssertEqual(core.lastParams("account.sign_in")?["flow"] as? String, "browser")
        XCTAssertEqual(core.callCount("account.sign_out"), 0)
        XCTAssertEqual(model.phase, .attached)
    }

    func testAConnectedAccountWithoutASessionAsksToSignInAgain() async {
        let stale = FakeCore.signedIn.merging(["needs_sign_in": true]) { $1 }
        core.setStatus(FakeCore.snapshot(revision: 2, account: stale))
        let pending = stale.merging(["state": "pending_approval", "phase": "waiting_for_browser"]) { $1 }
        core.on("account.sign_in") { [core] _ in
            core!.setStatus(FakeCore.snapshot(revision: 3, account: pending))
            return pending.merging(["verification_url": "https://agentrelay.com/login",
                                    "verification_url_complete": "https://agentrelay.com/login?s=1"]) { $1 }
        }
        let model = model()
        await model.bootstrapIfNeeded()
        let asking = await eventually { model.phase == .attached && model.needsSignIn }
        XCTAssertTrue(asking)

        model.renewSignIn()
        let waiting = await eventually { model.renewingSignIn && model.signInLink == "https://agentrelay.com/login?s=1" }
        XCTAssertTrue(waiting)
        XCTAssertEqual(core.lastParams("account.sign_in")?["flow"] as? String, "browser")
        // The browser approval keeps the connected view and the notice.
        XCTAssertEqual(model.phase, .attached)
        XCTAssertTrue(model.needsSignIn)

        core.setStatus(FakeCore.snapshot(revision: 4, account: FakeCore.signedIn))
        let renewed = await eventually { !model.renewingSignIn && !model.needsSignIn }
        XCTAssertTrue(renewed)
        XCTAssertEqual(model.phase, .attached)
        model.shutdown()
    }

    func testARenewalEndsEvenWhenItsApprovalSnapshotWasSkipped() async {
        let stale = FakeCore.signedIn.merging(["needs_sign_in": true]) { $1 }
        core.setStatus(FakeCore.snapshot(revision: 2, account: stale))
        let pending = stale.merging(["state": "pending_approval", "phase": "waiting_for_browser"]) { $1 }
        // The core answers with its approval running, but this app never
        // sees the approval's own snapshot.
        core.on("account.sign_in") { _ in
            pending.merging(["verification_url": "https://agentrelay.com/login",
                             "verification_url_complete": "https://agentrelay.com/login?s=1"]) { $1 }
        }
        let model = model()
        await model.bootstrapIfNeeded()
        let asking = await eventually { model.phase == .attached && model.needsSignIn }
        XCTAssertTrue(asking)

        model.renewSignIn()
        let waiting = await eventually { model.renewingSignIn && model.signInLink == "https://agentrelay.com/login?s=1" }
        XCTAssertTrue(waiting)
        // The renewal failed; the next snapshot still asks to sign in again.
        core.setStatus(FakeCore.snapshot(revision: 3, account: stale))
        let ended = await eventually { !model.renewingSignIn }
        XCTAssertTrue(ended)
        XCTAssertTrue(model.needsSignIn)
        XCTAssertEqual(model.phase, .attached)
        model.shutdown()
    }

    /// The core found Claude Code and Grok, not yet connected.
    private func toolsFound() {
        core.on("settings.get") { _ in
            Self.settings(analytics: true).merging(["detected_tools": ["Claude Code", "Grok"]]) { $1 }
        }
        core.on("account.sign_in") { _ in ["state": "signed_out"] }
    }

    func testSignInLinkOpensOnceAcrossReplyAndPreparingEvents() throws {
        for provider in ["google", "microsoft"] {
            var opened: [URL] = []
            let model = AppModel(client: CoreClient(socketPath: core.path), manageSystemServices: false,
                                 defaults: defaults, openSignInBrowser: { opened.append($0); return true })
            defer { model.shutdown() }
            let link = "https://agentrelay.com/cloud/api/auth/\(provider)/start?next=attempt-1"
            let report = try JSONDecoder().decode(SignInReport.self, from: JSONSerialization.data(withJSONObject:
                ["state": "pending_approval", "verification_url_complete": link]))
            // The reply's URL reaches the UI before the subscription catches
            // up with the preparing snapshot emitted earlier by the core.
            model.handle(.signIn(report))
            let preparing = try JSONDecoder().decode(Status.self, from: JSONSerialization.data(withJSONObject:
                FakeCore.snapshot(revision: 2, account: ["state": "preparing", "phase": "checking_existing_login"])))
            model.handle(.status(preparing))
            model.handle(.signIn(report))
            XCTAssertEqual(opened.map(\.absoluteString), [link])
            model.openApprovalPageAgain()
            XCTAssertEqual(opened.count, 2, "Explicitly reopening the browser remains available")
            model.handle(.signIn(report))
            XCTAssertEqual(opened.count, 2, "A repeated event must not duplicate the explicit reopen")
            let next = try JSONDecoder().decode(SignInReport.self, from: JSONSerialization.data(withJSONObject:
                ["state": "pending_approval", "verification_url_complete": link + "-new"]))
            model.handle(.signIn(next))
            model.handle(.signIn(report))
            XCTAssertEqual(opened.count, 3, "A new attempt opens, and a late old event does not reopen its URL")
        }
    }

    func testSignInSendsTheChosenProviderWithoutChangingToolSettings() async {
        for provider in [SignInProvider.google, .microsoft] {
            toolsFound()
            let model = model()
            await model.bootstrapIfNeeded()
            defer { model.shutdown() }
            let ready = await eventually { model.phase == .signedOut }
            XCTAssertTrue(ready)
            let calls = core.callCount("account.sign_in")
            model.signIn(provider: provider)
            let started = await eventually { self.core.callCount("account.sign_in") == calls + 1 }
            XCTAssertTrue(started)
            XCTAssertEqual(core.lastParams("account.sign_in")?["provider"] as? String, provider.rawValue)
            XCTAssertEqual(core.lastParams("account.sign_in")?["flow"] as? String, "browser")
            XCTAssertEqual(core.callCount("settings.set"), 0)
        }
    }

    func testASelfHostLoginConfigOpensTheSitesOwnSignInPage() async {
        core.on("account.login_config") { _ in
            ["available": true, "email_link": true, "password": true,
             "logo_url": "https://cdn.example.com/logo.png", "org_name": "Example Org",
             "methods_line": "Sign in with an email link or a password in your browser."]
        }
        toolsFound()
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        _ = await eventually { model.phase == .signedOut }
        let loaded = await eventually { model.siteLoginConfig != nil }
        XCTAssertTrue(loaded)
        XCTAssertEqual(model.siteLoginConfig?.logoUrl, "https://cdn.example.com/logo.png")
        XCTAssertEqual(model.siteLoginConfig?.orgName, "Example Org")
        XCTAssertEqual(model.siteLoginConfig?.methodsLine, "Sign in with an email link or a password in your browser.")
        model.signIn()
        let started = await eventually { self.core.callCount("account.sign_in") == 1 }
        XCTAssertTrue(started)
        XCTAssertNil(core.lastParams("account.sign_in")?["provider"], "The site's own page names no provider")
        XCTAssertEqual(core.lastParams("account.sign_in")?["flow"] as? String, "browser")
        XCTAssertEqual(core.callCount("account.login_config"), 1, "Asked once while signed out")
    }

    func testWithoutALoginConfigSignInKeepsTheChosenProvider() async {
        core.on("account.login_config") { _ in ["available": false, "email_link": false, "password": false] }
        toolsFound()
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        _ = await eventually { model.phase == .signedOut }
        let asked = await eventually { self.core.callCount("account.login_config") == 1 }
        XCTAssertTrue(asked)
        XCTAssertNil(model.siteLoginConfig)
        model.signIn(provider: .microsoft)
        let started = await eventually { self.core.callCount("account.sign_in") == 1 }
        XCTAssertTrue(started)
        XCTAssertEqual(core.lastParams("account.sign_in")?["provider"] as? String, "microsoft")
    }

    func testAFailedLoginConfigCallAsksAgain() async {
        core.on("account.login_config") { _ in
            throw FakeCore.Failure(kind: "cloud_failed", message: "The site's sign-in methods could not be read.")
        }
        toolsFound()
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        _ = await eventually { model.phase == .signedOut }
        let failed = await eventually { self.core.callCount("account.login_config") == 1 }
        XCTAssertTrue(failed)
        XCTAssertNil(model.siteLoginConfig, "The default methods show this time")
        core.on("account.login_config") { _ in
            ["available": true, "email_link": false, "password": true, "org_name": "Example Org",
             "methods_line": "Sign in with a password in your browser."]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: ["state": "signed_out"]))
        let loaded = await eventually { model.siteLoginConfig != nil }
        XCTAssertTrue(loaded, "The next signed-out snapshot asks again")
        XCTAssertGreaterThanOrEqual(core.callCount("account.login_config"), 2)
        XCTAssertEqual(model.siteLoginConfig?.orgName, "Example Org")
    }

    func testASiteLogoRefusesRedirectsAndAnswersOverOneMebibyte() async throws {
        XCTAssertTrue(SiteLogo.accepts(status: 200, expectedLength: Int64(SiteLogo.limit)))
        XCTAssertTrue(SiteLogo.accepts(status: 200, expectedLength: -1), "An unannounced length is counted while read")
        for redirect in [301, 302, 303, 307, 308] {
            XCTAssertFalse(SiteLogo.accepts(status: redirect, expectedLength: 10), "\(redirect)")
        }
        XCTAssertFalse(SiteLogo.accepts(status: 404, expectedLength: 10))
        XCTAssertFalse(SiteLogo.accepts(status: 200, expectedLength: Int64(SiteLogo.limit) + 1))
        func stream(_ count: Int) -> AsyncStream<UInt8> {
            AsyncStream { continuation in
                for _ in 0..<count { continuation.yield(7) }
                continuation.finish()
            }
        }
        let fits = try await SiteLogo.collect(stream(SiteLogo.limit))
        XCTAssertEqual(fits?.count, SiteLogo.limit)
        let over = try await SiteLogo.collect(stream(SiteLogo.limit + 1))
        XCTAssertNil(over, "A logo past 1 MiB is dropped")
    }

    func testSignInButtonsAreWordedAsOnGTK() async {
        XCTAssertEqual(ProviderSignInButton.label(.google), "Continue with Google")
        XCTAssertEqual(ProviderSignInButton.label(.microsoft), "Continue with Microsoft")
        XCTAssertEqual(ProviderSignInButton.label(nil), "Continue in browser")
        XCTAssertEqual(ProviderSignInButton.label(.microsoft, name: "Contoso Microsoft"), "Continue with Contoso Microsoft")
        XCTAssertEqual(ProviderSignInButton.label(nil, name: "Contoso"), "Continue in browser")
        let plain = await SiteLogo.load("http://example.com/logo.png")
        XCTAssertNil(plain, "A logo loads only over https")
    }

    func testAConnectionLinkKeepsItsWorkspaceOnTheFirstSignIn() async {
        core.on("links.parse") { _ in
            ["kind": "connect", "site_url": "https://agentrelay.com", "account_id": "a",
             "workspace_id": "0f8fad5b-d9cb-469f-a165-70867728950e"]
        }
        toolsFound()
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        _ = await eventually { model.phase == .signedOut }
        model.openConnection(URL(string: "agentrelay://connect?x=1")!)
        let started = await eventually { self.core.callCount("account.sign_in") == 1 }
        XCTAssertTrue(started)
        XCTAssertEqual(core.lastParams("account.sign_in")?["workspace"] as? String, "0f8fad5b-d9cb-469f-a165-70867728950e")
    }

    func testARefusedLinkShowsTheCoresReasonWithTheRemedyForAnotherSite() async {
        core.on("links.parse") { _ in
            throw FakeCore.Failure(kind: "invalid_link", message: "That link connects to https://other.example, but this app is built for https://agentrelay.com.")
        }
        toolsFound()
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        _ = await eventually { model.phase == .signedOut }
        model.openConnection(URL(string: "agentrelay://connect?x=1")!)
        let refused = await eventually { model.lastError != nil }
        XCTAssertTrue(refused)
        XCTAssertEqual(model.lastError, "That link is for another Agent Relay site. Open it with the Agent Relay app for other.example, or ask for a link for this app's site (https://agentrelay.com).")
        XCTAssertEqual(core.callCount("account.sign_in"), 0)
        XCTAssertNil(model.developmentBadge, "A production core shows no Development badge")
    }

    func testASuccessfulConnectionLinkClearsThePreviousErrorWithoutSigningInAgain() async {
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        core.on("links.parse") { _ in
            throw FakeCore.Failure(kind: "invalid_params", message: "That is not an Agent Relay link this app opens.")
        }
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let attached = await eventually { model.phase == .attached }
        XCTAssertTrue(attached)
        model.openConnection(URL(string: "agentrelay://connect?account=user_1")!)
        let refused = await eventually { model.lastError != nil }
        XCTAssertTrue(refused)
        XCTAssertEqual(model.lastError, "That is not an Agent Relay link this app opens.")
        XCTAssertEqual(model.linkError, "That is not an Agent Relay link this app opens.")
        XCTAssertTrue(DesktopDiagnostics.report(DesktopDiagnostics.sections(for: model))
            .contains("Last link failure: That is not an Agent Relay link this app opens."))

        core.on("links.parse") { _ in
            ["kind": "connect", "site_url": "https://agentrelay.com", "account_id": "user_1",
             "workspace_id": "0f8fad5b-d9cb-469f-a165-70867728950e"]
        }
        model.openConnection(URL(string: "agentrelay://connect?site=https%3A%2F%2Fagentrelay.com&account=user_1&workspace=0f8fad5b-d9cb-469f-a165-70867728950e")!)
        let cleared = await eventually { model.lastError == nil }
        XCTAssertTrue(cleared)
        XCTAssertNil(model.linkError)
        XCTAssertEqual(model.phase, .attached)
        XCTAssertEqual(core.callCount("account.sign_in"), 0)
        XCTAssertEqual(core.callCount("account.sign_out"), 0)
    }

    /// A link the core could not be reached for is not a refusal: it is
    /// tried once more after a short delay, with the core still attached
    /// and no new snapshot, then says Agent Relay is still starting, and
    /// Diagnostics keeps why (as on Linux and Windows).
    func testALinkTheCoreCannotBeReachedForIsTriedOnceMoreThenSaysItIsStillStarting() async {
        let cause = "Agent Relay is reconnecting to its background service. Try again in a moment."
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        core.on("links.parse") { _ in
            throw FakeCore.Failure(kind: "core_unreachable", message: cause)
        }
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let attached = await eventually { model.phase == .attached }
        XCTAssertTrue(attached)
        model.openConnection(URL(string: "agentrelay://connect?account=user_1")!)
        // The client tries the read again for a while before giving up.
        let held = await eventually(15) { model.linkError != nil }
        XCTAssertTrue(held)
        XCTAssertEqual(model.linkError, cause)
        XCTAssertNil(model.lastError)
        let firstParse = core.callCount("links.parse")
        // No snapshot and no reconnect: the retry goes out on its own.
        let failed = await eventually(15) { model.lastError != nil }
        XCTAssertTrue(failed)
        XCTAssertTrue(model.isCoreConnected)
        XCTAssertGreaterThan(core.callCount("links.parse"), firstParse)
        XCTAssertEqual(model.lastError, "Agent Relay is still starting. Open the link again in a moment.")
        XCTAssertEqual(model.lastError, AppModel.linkStillStarting)
        XCTAssertEqual(model.linkError, cause)
        XCTAssertFalse(core.allParams("telemetry.record").contains { $0["failure"] as? String == "invalid_connection" })
        XCTAssertTrue(DesktopDiagnostics.report(DesktopDiagnostics.sections(for: model))
            .contains("Last link failure: \(cause)"))
    }

    /// A link held while the core stays away past the startup deadline is
    /// dropped and says Agent Relay is still starting, with the cause in
    /// Diagnostics; a core that comes back later does not open it (as on
    /// Linux and Windows).
    func testALinkWhileTheCoreStaysDownGivesUpAfterTheDeadline() async {
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model(startupTimeout: 1_000_000_000)
        defer { model.shutdown() }
        await model.bootstrapIfNeeded()
        let attached = await eventually { model.isCoreConnected && model.phase == .attached }
        XCTAssertTrue(attached)
        let path = core.path
        core.stop()
        let disconnected = await eventually { !model.isCoreConnected }
        XCTAssertTrue(disconnected)
        let link = URL(string: "agentrelay://connect?site=https%3A%2F%2Fagentrelay.com&account=user_1&workspace=0f8fad5b-d9cb-469f-a165-70867728950e")!
        model.openConnection(link)
        // The same link opened again is not held twice.
        model.openConnection(link)
        XCTAssertNil(model.lastError)
        XCTAssertNil(model.linkError)
        let gaveUp = await eventually(5) { model.lastError == AppModel.linkStillStarting }
        XCTAssertTrue(gaveUp)
        let cause = "Agent Relay's background service was not connected within 1 s."
        XCTAssertEqual(AppModel.linkHoldTimedOut(1_000_000_000), cause)
        XCTAssertEqual(model.linkError, cause)
        XCTAssertTrue(DesktopDiagnostics.report(DesktopDiagnostics.sections(for: model))
            .contains("Last link failure: \(cause)"))
        core = FakeCore(path: path, pid: 4243)
        core.on("links.parse") { _ in
            ["kind": "connect", "site_url": "https://agentrelay.com", "account_id": "user_1",
             "workspace_id": "0f8fad5b-d9cb-469f-a165-70867728950e"]
        }
        core.setStatus(FakeCore.snapshot(revision: 3, account: FakeCore.signedIn, pid: 4243))
        let reconnected = await eventually { model.isCoreConnected && model.phase == .attached }
        XCTAssertTrue(reconnected)
        try? await Task.sleep(nanoseconds: 500_000_000)
        XCTAssertEqual(core.callCount("links.parse"), 0)
    }

    /// Before the first snapshot the core may answer other calls, so the
    /// timed-out link is recorded as `timed_out`.
    func testALinkBeforeTheFirstSnapshotGivesUpAfterTheDeadline() async {
        let held = DispatchSemaphore(value: 0)
        defer { held.signal() }
        core.beforeReply = { method in if method == "subscribe" { held.wait() } }
        core.on("telemetry.record") { _ in [:] }
        let service = FakeService(status: .enabled)
        defaults.set(service.binaryIdentity, forKey: AppModel.registeredCoreKey)
        let model = model(service: service, startupTimeout: 1_000_000_000)
        defer { model.shutdown() }
        await model.bootstrapIfNeeded()
        model.openConnection(URL(string: "agentrelay://connect?account=user_1")!)
        let gaveUp = await eventually(5) { model.lastError == AppModel.linkStillStarting }
        XCTAssertTrue(gaveUp)
        let cause = "Agent Relay's background service was not connected within 1 s."
        XCTAssertEqual(model.linkError, cause)
        let recorded = await eventually {
            self.core.allParams("telemetry.record").contains {
                $0["operation"] as? String == "connection_link" && $0["failure"] as? String == "timed_out"
            }
        }
        XCTAssertTrue(recorded)
        XCTAssertTrue(DesktopDiagnostics.report(DesktopDiagnostics.sections(for: model))
            .contains("Last link failure: \(cause)"))
        XCTAssertEqual(core.callCount("links.parse"), 0)
        held.signal()
        let ready = await eventually { model.phase == .signedOut }
        XCTAssertTrue(ready)
        try? await Task.sleep(nanoseconds: 500_000_000)
        XCTAssertEqual(core.callCount("links.parse"), 0)
    }

    func testALinkWhileTheCoreIsDownWaitsForTheReconnect() async {
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        defer { model.shutdown() }
        await model.bootstrapIfNeeded()
        let attached = await eventually { model.isCoreConnected && model.phase == .attached }
        XCTAssertTrue(attached)
        let path = core.path
        core.stop()
        let disconnected = await eventually { !model.isCoreConnected }
        XCTAssertTrue(disconnected)
        // The account view keeps its snapshot; the link still waits.
        model.openConnection(URL(string: "agentrelay://connect?site=https%3A%2F%2Fagentrelay.com&account=user_1&workspace=0f8fad5b-d9cb-469f-a165-70867728950e")!)
        XCTAssertNil(model.lastError)
        core = FakeCore(path: path, pid: 4243)
        core.on("links.parse") { _ in
            ["kind": "connect", "site_url": "https://agentrelay.com", "account_id": "user_1",
             "workspace_id": "0f8fad5b-d9cb-469f-a165-70867728950e"]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn, pid: 4243))
        let parsed = await eventually { model.isCoreConnected && self.core.callCount("links.parse") == 1 }
        XCTAssertTrue(parsed)
        // The link was held and parsed, not refused. The restarted core
        // answers other reads on its own terms (its uploader.status is not
        // the one setUp gave the first core), so lastError may carry one of
        // those; only a link failure would be this test's.
        XCTAssertNil(model.linkError)
        XCTAssertNotEqual(model.lastError, AppModel.linkStillStarting)
        XCTAssertNotEqual(model.lastError, AppModel.notALinkRefusal)
        XCTAssertFalse(model.lastError?.contains("another Agent Relay site") ?? false)
        XCTAssertFalse(model.lastError?.contains("another account or workspace") ?? false)
        XCTAssertEqual(model.phase, .attached)
    }

    func testSignInDoesNotWaitForToolSettings() async {
        core.on("settings.get") { _ in throw FakeCore.Failure(kind: "internal", message: "The core is busy.") }
        core.on("account.sign_in") { _ in ["state": "signed_out"] }
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        _ = await eventually { model.phase == .signedOut }
        XCTAssertNil(model.settings)
        model.signIn(provider: .microsoft)
        let started = await eventually { self.core.callCount("account.sign_in") == 1 }
        XCTAssertTrue(started)
        XCTAssertEqual(core.lastParams("account.sign_in")?["provider"] as? String, "microsoft")
    }

    func testUploadActionsGoThroughTheCore() async {
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        _ = await eventually { model.sessions.count == 2 }
        let included = await model.includeSessions(keys: ["claude:b"])
        XCTAssertTrue(included)
        XCTAssertEqual(core.lastParams("uploader.sessions.include")?["session_keys"] as? [String], ["claude:b"])
        await model.setPaused(true)
        XCTAssertEqual(core.callCount("uploader.pause"), 1)
        await model.setSharingMode(.all)
        XCTAssertEqual(core.lastParams("uploader.sharing.set")?["mode"] as? String, "all")
        model.shutdown()
    }

    func testNameConflictCanBeRetriedWithAnExplicitName() async {
        core.on("agents.register") { params in
            guard params["name"] as? String == "my-new-name" else {
                throw FakeCore.Failure(kind: "conflict", message: "Name already taken")
            }
            return ["name": "my-new-name", "address": "my-new-name@mac", "already_registered": false]
        }
        let model = model()
        await model.bootstrapIfNeeded()
        _ = await eventually { !model.relay.sessions.isEmpty }
        let session = model.relay.sessions[0]
        model.relay.requestRelay(true, for: session)
        _ = await eventually { model.relay.errors[session.id] != nil }
        XCTAssertEqual(model.relay.receiveMessagesError, "Name already taken")
        XCTAssertEqual(model.relay.presence(of: session), .off)
        await model.relay.register(session, as: "  @@My-New-Name@  ")
        XCTAssertNil(model.relay.errors[session.id])
        XCTAssertEqual(core.lastParams("agents.register")?["name"] as? String, "my-new-name")
        XCTAssertFalse(model.relay.busyTargets.contains(session.id))
        model.shutdown()
    }

    func testReceiveMessagesKeepsAcknowledgedRegistrationWhenRefreshFails() async {
        let model = model()
        await model.bootstrapIfNeeded()
        _ = await eventually { !model.relay.sessions.isEmpty }
        let session = model.relay.sessions[0]
        core.on("agents.register") { [core] _ in
            core!.on("sessions.list") { _ in
                throw FakeCore.Failure(kind: "internal", message: "Cannot refresh sessions")
            }
            return ["name": "receive-test", "address": "receive-test@mac", "already_registered": false]
        }
        model.relay.requestRelay(true, for: session)
        XCTAssertEqual(model.relay.presence(of: session), .joining)
        let failed = await eventually { model.relay.receiveMessagesError != nil }
        XCTAssertTrue(failed)
        XCTAssertEqual(model.relay.receiveMessagesError, "Cannot refresh sessions")
        XCTAssertEqual(model.relay.sessions.first { $0.id == session.id }?.registration?.name, "receive-test")
        XCTAssertFalse(model.relay.busyTargets.contains(session.id))
        model.relay.requestRelay(false, for: model.relay.sessions.first { $0.id == session.id }!)
        let released = await eventually { self.core.callCount("agents.unregister") == 1 }
        XCTAssertTrue(released)
        XCTAssertEqual(core.lastParams("agents.unregister")?["name"] as? String, "receive-test")
        model.shutdown()
    }

    func testReceiveMessagesReportsAnUnchangedRegistrationWithoutRepeating() async {
        let model = model()
        await model.bootstrapIfNeeded()
        _ = await eventually { !model.relay.sessions.isEmpty }
        let session = model.relay.sessions[0]
        core.on("agents.register") { _ in
            ["name": "receive-test", "address": "receive-test@mac", "already_registered": false]
        }
        model.relay.requestRelay(true, for: session)
        let failed = await eventually { model.relay.receiveMessagesError != nil }
        XCTAssertTrue(failed)
        XCTAssertEqual(model.relay.receiveMessagesError, "Receive messages did not change. Refresh the session and try again.")
        XCTAssertEqual(core.callCount("agents.register"), 1)
        XCTAssertFalse(model.relay.busyTargets.contains(session.id))
        model.shutdown()
    }

    func testReceiveMessagesRetiresReadsBeforeAcknowledgement() async throws {
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        _ = await eventually { !model.relay.sessions.isEmpty }
        let session = try XCTUnwrap(model.relay.sessions.first)
        let oldSessions: [String: Any] = ["sessions": try JSONSerialization.jsonObject(with: JSONEncoder().encode(model.relay.sessions))]
        let holdNext = DispatchSemaphore(value: 1)
        let release = DispatchSemaphore(value: 0)
        let held = Locked(false)
        defer { release.signal() }
        core.on("sessions.list") { _ in
            if holdNext.wait(timeout: .now()) == .success {
                held.value = true
                release.wait()
                return oldSessions
            }
            throw FakeCore.Failure(kind: "internal", message: "Cannot refresh sessions")
        }
        let staleRead = Task { try await model.relay.loadSessions() }
        let reading = await eventually { held.value }
        XCTAssertTrue(reading)
        core.on("agents.register") { _ in
            ["name": "acknowledged", "address": "acknowledged@mac", "already_registered": false]
        }
        model.relay.requestRelay(true, for: session)
        let failed = await eventually { model.relay.receiveMessagesError != nil }
        XCTAssertTrue(failed)
        release.signal()
        try await staleRead.value
        let current = try XCTUnwrap(model.relay.sessions.first { $0.id == session.id })
        XCTAssertEqual(current.registration?.name, "acknowledged")
        model.relay.requestRelay(false, for: current)
        let released = await eventually { self.core.callCount("agents.unregister") == 1 }
        XCTAssertTrue(released)
        XCTAssertEqual(core.lastParams("agents.unregister")?["name"] as? String, "acknowledged")
    }

    func testSwitchingAManagedPickerSessionRegistersItsExactTarget() async {
        var registered = false
        core.on("agents.register") { [core] params in
            let target = params["target"] as? [String: Any]
            XCTAssertEqual(target?["kind"] as? String, "opencode")
            XCTAssertEqual(target?["id"] as? String, "ses_exact")
            registered = true
            // The push channel's state arrives on the snapshot, not the list.
            core!.setStatus(FakeCore.snapshot(revision: 9, account: ["state": "signed_out"], push: ["managed-opencode": ["state": "live"]]))
            return ["name": "managed-opencode", "address": "managed-opencode@mac", "already_registered": false]
        }
        core.on("sessions.list") { _ in
            ["sessions": [["target": ["kind": "opencode", "id": "ses_exact"], "title": "OpenCode ses_exact", "live": true,
                           "directory": "/src/project",
                           "registration": registered ? ["name": "managed-opencode", "address": "managed-opencode@mac",
                                                         "added_by_session": false, "push": ["state": "live"]] as Any : NSNull(),
                           "suggested_name": "managed-opencode"]]]
        }
        let model = model()
        await model.bootstrapIfNeeded()
        let listed = await eventually { model.relay.sessions.count == 1 }
        XCTAssertTrue(listed)
        let session = model.relay.sessions[0]
        XCTAssertEqual(session.target, Target(id: "ses_exact", kind: .opencode))
        XCTAssertEqual(session.target.uploadKey, "opencode:ses_exact")
        XCTAssertEqual(model.relay.presence(of: session), .off)
        model.relay.requestRelay(true, for: session)
        XCTAssertEqual(model.relay.presence(of: session), .joining)
        let online = await eventually { model.relay.sessions.first.map { model.relay.presence(of: $0) } == .online }
        XCTAssertTrue(online)
        XCTAssertEqual(core.callCount("agents.register"), 1)
        model.shutdown()
    }

    func testLinksBeforeTheFirstSnapshotWaitForIt() async {
        core.on("links.parse") { _ in
            ["kind": "connect", "site_url": "https://agentrelay.com", "account_id": "a",
             "workspace_id": "0f8fad5b-d9cb-469f-a165-70867728950e"]
        }
        core.on("account.sign_in") { _ in ["state": "pending_approval"] }
        let model = model()
        model.openConnection(URL(string: "agentrelay://connect?x=1")!)
        XCTAssertEqual(core.callCount("links.parse"), 0)
        await model.bootstrapIfNeeded()
        let signedIn = await eventually { self.core.callCount("account.sign_in") == 1 }
        XCTAssertTrue(signedIn)
        XCTAssertEqual(core.lastParams("account.sign_in")?["workspace"] as? String, "0f8fad5b-d9cb-469f-a165-70867728950e")
        model.shutdown()
    }

    // MARK: Changes shown before the core confirms them

    func testTheUploadPolicyMovesAtOnceAndFallsBackWhenTheChangeFails() async {
        let gate = DispatchSemaphore(value: 0)
        defer { gate.signal() }
        core.on("uploader.sharing.set") { _ in
            gate.wait()
            throw FakeCore.Failure(kind: "unavailable", message: "The uploader is restarting.")
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        _ = await eventually { model.status != nil }
        XCTAssertEqual(model.sharingMode, .new)
        let change = Task { await model.setSharingMode(.all) }
        let moved = await eventually { model.sharingMode == .all && model.isChangingSharingMode }
        XCTAssertTrue(moved)
        XCTAssertTrue(model.isSessionTableReadOnly, "the table follows the policy asked for")
        gate.signal()
        await change.value
        XCTAssertEqual(model.sharingMode, .new)
        XCTAssertFalse(model.isChangingSharingMode)
        XCTAssertEqual(model.sessionActionError, "The uploader is restarting.")
    }

    func testPausingShowsAtOnceAndTheLastClickWins() async {
        let gate = DispatchSemaphore(value: 0)
        defer { gate.signal() }
        let paused = Locked(false)
        core.on("uploader.pause") { _ in gate.wait(); paused.value = true; return ["paused": true] }
        core.on("uploader.resume") { _ in paused.value = false; return ["paused": false] }
        core.on("uploader.status") { _ in
            ["running": true, "paused": paused.value, "sharing_mode": "new", "directory": "/tmp/install",
             "collector_replacing": false, "collector_outdated": false,
             "delivery": ["bootstrap_complete": true, "pending_records": 0, "acknowledged_records": 4, "quarantined_records": 0, "skipped_records": 0,
                          "unqueued_changes": 0, "last_acknowledged_ms": NSNull(), "failure": NSNull(), "stopped": false],
             "sessions": ["total": 2, "shared": 1, "excluded": 1], "stage": paused.value ? "paused" : "up_to_date", "stage_message": NSNull(),
             "needs_start": false, "journal_full": false, "upgrading": false, "stalled": false]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        _ = await eventually { model.status != nil }
        XCTAssertFalse(model.isPaused)
        let pause = Task { await model.setPaused(true) }
        let shown = await eventually { model.isPaused }
        XCTAssertTrue(shown, "paused from the click")
        XCTAssertTrue(model.canTogglePause, "the button stays usable for a later click")
        // Resume clicked while the pause is still on its way: it is sent
        // once the pause has answered, and the window shows resumed.
        await model.setPaused(false)
        XCTAssertFalse(model.isPaused)
        gate.signal()
        await pause.value
        let resumed = await eventually { self.core.callCount("uploader.resume") == 1 && !model.isPaused }
        XCTAssertTrue(resumed)
        XCTAssertFalse(paused.value)
    }

    func testASettingHoldsTheChangeUntilTheCoreAnswersAndFallsBackOnFailure() async {
        let gate = DispatchSemaphore(value: 0)
        defer { gate.signal() }
        core.on("settings.set") { _ in
            gate.wait()
            throw FakeCore.Failure(kind: "io", message: "The settings file is read-only.")
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        _ = await eventually { model.settings != nil }
        XCTAssertTrue(model.analyticsEnabled)
        let change = Task { await model.setSetting(SettingsChange(analyticsEnabled: false)) }
        let held = await eventually { !model.analyticsEnabled && model.isSettingPending("analytics_enabled") }
        XCTAssertTrue(held, "the switch holds the position asked for")
        XCTAssertFalse(model.isSettingPending("allow_self_registration"))
        gate.signal()
        let failure = await change.value
        XCTAssertEqual(failure, "The settings file is read-only.")
        XCTAssertTrue(model.analyticsEnabled, "a failed change falls back to the core's value")
        XCTAssertFalse(model.isSettingPending("analytics_enabled"))
        XCTAssertEqual(model.settingErrors["analytics_enabled"], "The settings file is read-only.")
        // Changing it again clears the row's error and succeeds.
        core.on("settings.set") { params in Self.settings(analytics: params["analytics_enabled"] as? Bool ?? true) }
        let retried = await model.setSetting(SettingsChange(analyticsEnabled: false))
        XCTAssertNil(retried)
        XCTAssertFalse(model.analyticsEnabled)
        XCTAssertNil(model.settingErrors["analytics_enabled"])
    }

    func testAnOlderSettingsWriteIsKeptWhenANewerOneFails() async {
        let gate = DispatchSemaphore(value: 0)
        defer { gate.signal() }
        core.on("settings.set") { params in
            if params["analytics_enabled"] != nil {
                gate.wait()
                return Self.settings(analytics: false)
            }
            throw FakeCore.Failure(kind: "io", message: "The settings file is read-only.")
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        _ = await eventually { model.settings != nil }
        let first = Task { await model.setSetting(SettingsChange(analyticsEnabled: false)) }
        _ = await eventually { model.isSettingPending("analytics_enabled") }
        // A newer change fails while the first is still on its way.
        let failure = await model.setSetting(SettingsChange(allowSelfRegistration: false))
        XCTAssertNotNil(failure)
        XCTAssertEqual(model.settings?.allowSelfRegistration, true)
        XCTAssertFalse(model.analyticsEnabled, "the first change still shows")
        gate.signal()
        let firstOutcome = await first.value
        XCTAssertNil(firstOutcome)
        XCTAssertFalse(model.analyticsEnabled, "the first change's answer is kept although a newer one started")
        XCTAssertFalse(model.isSettingPending("analytics_enabled"))
    }

    func testARenameShowsTheNewNameAtOnceAndTheOldOneAgainWhenItFails() async {
        let gate = DispatchSemaphore(value: 0)
        defer { gate.signal() }
        core.on("sessions.list") { _ in
            ["sessions": [["target": ["kind": "codex", "id": "a"], "title": "Fix tests", "live": true,
                           "registration": ["name": "fix-tests", "address": "fix-tests@mac", "added_by_session": false],
                           "suggested_name": "fix-tests"]]]
        }
        core.on("agents.list") { _ in
            ["agents": [["name": "fix-tests", "address": "fix-tests@mac", "target": ["kind": "codex", "id": "a"], "title": "Fix tests",
                         "registered_at": "2026-10-10T12:00:00Z", "added_by_session": false, "reply_with_final_answer": NSNull(),
                         "webhook": NSNull(), "integrations": [], "push": NSNull(), "delivery_error": NSNull()]], "archived": []]
        }
        core.on("agents.rename") { _ in
            gate.wait()
            throw FakeCore.Failure(kind: "conflict", message: "That name is taken.")
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let ready = await eventually { model.relay.agents.count == 1 && model.relay.sessions.first?.registration != nil }
        guard ready else { XCTFail("the agent did not load"); return }
        let agent = model.relay.agents[0]
        let rename = Task { await model.relay.rename(agent, to: "helper") }
        let shown = await eventually { model.relay.agents.first?.displayName == "helper" }
        XCTAssertTrue(shown, "the new name from the click")
        XCTAssertEqual(model.relay.sessions.first?.registration?.displayName, "helper")
        gate.signal()
        let outcome = await rename.value
        XCTAssertEqual(outcome, .failed("That name is taken."))
        XCTAssertEqual(model.relay.agents.first?.displayName, "fix-tests")
        XCTAssertEqual(model.relay.sessions.first?.registration?.displayName, "fix-tests")
    }

    func testAnExplicitRegistrationShowsTheSessionOnTheRelayAtOnce() async {
        let gate = DispatchSemaphore(value: 0)
        defer { gate.signal() }
        core.on("agents.register") { _ in
            gate.wait()
            throw FakeCore.Failure(kind: "conflict", message: "Name already taken")
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        _ = await eventually { !model.relay.sessions.isEmpty }
        let session = model.relay.sessions[0]
        XCTAssertNil(session.registration)
        let registration = Task { await model.relay.register(session, as: "helper") }
        let shown = await eventually { model.relay.sessions.first?.registration?.name == "helper" }
        XCTAssertTrue(shown, "on the relay under the name from the click")
        XCTAssertTrue(model.relay.presence(of: model.relay.sessions[0]).isOn)
        gate.signal()
        await registration.value
        XCTAssertNil(model.relay.sessions.first?.registration)
        XCTAssertEqual(model.relay.errors[session.id], "Name already taken")
    }

    func testASentMessageShowsAsOnItsWayUntilTheHistoryListsIt() async {
        let gate = DispatchSemaphore(value: 0)
        defer { gate.signal() }
        let sent = Locked(false)
        core.on("roster.get") { _ in Self.recipient("live", message: nil) }
        core.on("messages.send") { params in
            gate.wait()
            sent.value = true
            return ["message_id": "m1", "conversation_id": "c1", "to": params["to"] ?? NSNull(),
                    "recipient": Self.recipient("live", message: "queued")]
        }
        core.on("messages.history") { _ in
            ["identity": "ada", "messages": sent.value
                ? [["id": "m1", "sender": "ada", "sender_id": NSNull(), "sender_address": NSNull(), "text": "status?",
                    "created_at": NSNull(), "thread_id": NSNull(), "mine": true]]
                : []]
        }
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        let relay = model.relay
        relay.openComposer("@bot")
        await relay.loadConversation("@bot")
        let send = Task { await relay.sendMessage(to: "@bot", text: "status?") }
        let echoed = await eventually { relay.shownHistory(at: "@bot")?.messages.count == 1 }
        XCTAssertTrue(echoed)
        let echo = relay.shownHistory(at: "@bot")?.messages.first
        XCTAssertEqual(echo?.text, "status?")
        XCTAssertEqual(echo.map(PendingMessages.isPending), true)
        gate.signal()
        let sendError = await send.value
        XCTAssertNil(sendError)
        let listed = relay.shownHistory(at: "@bot")?.messages ?? []
        XCTAssertEqual(listed.map(\.id), ["m1"], "the history's own message replaces the echo")

        // A failed send shows nothing as on its way.
        core.on("messages.send") { _ in throw FakeCore.Failure(kind: "unavailable", message: "Relaycast is unreachable.") }
        let refused = await relay.sendMessage(to: "@bot", text: "again")
        XCTAssertEqual(refused, "Relaycast is unreachable.")
        XCTAssertEqual(relay.shownHistory(at: "@bot")?.messages.map(\.id), ["m1"])
    }

    func testClaudeDeliveryChangesShowAtOnceAndFallBackWhenTheyFail() async {
        let gate = DispatchSemaphore(value: 0)
        defer { gate.signal() }
        core.on("claude_delivery.restrict") { _ in
            gate.wait()
            throw FakeCore.Failure(kind: "io", message: "Could not save.")
        }
        let model = model()
        await model.claudeDelivery.refresh()
        XCTAssertEqual(model.claudeDelivery.status?.restricted, true)
        let change = Task { await model.claudeDelivery.restrict(false) }
        let shown = await eventually { model.claudeDelivery.status?.restricted == false }
        XCTAssertTrue(shown)
        gate.signal()
        await change.value
        XCTAssertEqual(model.claudeDelivery.status?.restricted, true)
        XCTAssertEqual(model.claudeDelivery.error, "Could not save.")
        await model.claudeDelivery.refresh()
        XCTAssertNil(model.claudeDelivery.error, "a later explicit successful read clears the action failure")
        // Always allow: the sender joins the list and the message leaves it at once.
        let heldGate = DispatchSemaphore(value: 0)
        defer { heldGate.signal() }
        core.on("claude_delivery.sender") { _ in heldGate.wait(); return [:] }
        guard let message = model.claudeDelivery.status?.held.first else { XCTFail("no held message"); return }
        let allow = Task { await model.claudeDelivery.alwaysAllow(message) }
        let allowed = await eventually {
            model.claudeDelivery.status?.held.isEmpty == true
                && model.claudeDelivery.status?.allowedSenders.map(\.address) == ["ada@desk", "bo@desk"]
        }
        XCTAssertTrue(allowed)
        heldGate.signal()
        await allow.value
    }

    func testClaudeDeliveryActionErrorSurvivesAFailedReconciliationRead() async {
        let model = model()
        await model.claudeDelivery.refresh()
        core.on("claude_delivery.restrict") { _ in
            throw FakeCore.Failure(kind: "io", message: "Could not save.")
        }
        core.on("claude_delivery.status") { _ in
            throw FakeCore.Failure(kind: "io", message: "Could not read choices.")
        }
        await model.claudeDelivery.restrict(false)
        XCTAssertEqual(model.claudeDelivery.status?.restricted, true, "the failed optimistic change rolls back")
        XCTAssertEqual(model.claudeDelivery.error, "Could not save.", "reconciliation keeps the action's failure")
        await model.claudeDelivery.refresh()
        XCTAssertEqual(model.claudeDelivery.error, "Could not read choices.", "an explicit read reports its own failure")
    }

    func testLoggingOutShowsTheAccountLeavingUntilTheSnapshotNoLongerShowsIt() async {
        let gate = DispatchSemaphore(value: 0)
        defer { gate.signal() }
        core.on("account.sign_out") { [core] _ in
            gate.wait()
            core!.setStatus(FakeCore.snapshot(revision: 3, account: ["state": "signed_out"]))
            return [:]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        _ = await eventually { model.phase == .attached }
        XCTAssertTrue(model.canLogOut)
        let logOut = Task { await model.disconnect() }
        let leaving = await eventually { model.signingOut }
        XCTAssertTrue(leaving)
        XCTAssertFalse(model.canLogOut, "no second log-out while one runs")
        gate.signal()
        await logOut.value
        let gone = await eventually { model.phase == .signedOut && !model.signingOut }
        XCTAssertTrue(gone)
    }

    func testSigningInStartsOnceUntilTheSnapshotShowsTheAttempt() async {
        let gate = DispatchSemaphore(value: 0)
        defer { gate.signal() }
        core.on("account.sign_in") { [core] _ in
            gate.wait()
            core!.setStatus(FakeCore.snapshot(revision: 5, account: ["state": "pending_approval", "phase": "waiting_for_browser"]))
            return ["state": "pending_approval", "phase": "waiting_for_browser",
                    "verification_url": "https://agentrelay.com/login", "verification_url_complete": "https://agentrelay.com/login?s=1"]
        }
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        _ = await eventually { model.phase == .signedOut }
        model.signIn()
        XCTAssertTrue(model.startingSignIn)
        model.signIn()
        gate.signal()
        let asked = await eventually { model.phase == .waitingForBrowser && !model.startingSignIn }
        XCTAssertTrue(asked)
        XCTAssertEqual(core.callCount("account.sign_in"), 1, "a second click while the first starts sends nothing")
    }

    func testConfirmedCloudDeleteStillRunsAfterHideExcludesUpload() async {
        let included = Locked(true)
        core.on("uploader.sessions.list") { _ in
            ["sharing_mode": "selected", "sessions": [["source": "codex", "session_id": "a", "title": "Fix tests", "is_agent": false, "included": included.value, "status": included.value ? "shared" : "not_shared"]]]
        }
        core.on("uploader.sessions.exclude") { _ in included.value = false; return ["updated": 1] }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        defer { model.shutdown() }
        _ = await eventually { !model.sessions.isEmpty }
        await model.setSessionsHidden(keys: ["codex:a"], hidden: true)
        XCTAssertEqual(core.callCount("uploader.sessions.exclude"), 1)
        XCTAssertFalse(model.sessions.first { $0.id == "codex:a" }?.included ?? true)
        XCTAssertEqual(core.callCount("uploader.sessions.delete"), 0, "Hide only excludes uploads")
        await model.excludeSessions(keys: ["codex:a"])
        XCTAssertEqual(core.callCount("uploader.sessions.delete"), 1, "Confirmed deletion must remove the Cloud copy even after Hide excluded uploads")
    }

    func testUploadTogglesAreIndependentAndLatestClickWins() async {
        let included = Locked(Set(["codex:a"]))
        let held = DispatchSemaphore(value: 0)
        defer { held.signal() }
        core.on("uploader.sessions.list") { _ in
            ["sharing_mode": "selected", "sessions": ["codex:a", "claude:b"].map { key in
                ["source": String(key.split(separator: ":")[0]), "session_id": String(key.split(separator: ":")[1]),
                 "title": key, "is_agent": false, "included": included.value.contains(key),
                 "status": included.value.contains(key) ? "shared" : "not_shared"] as [String: Any]
            }]
        }
        core.on("uploader.sessions.delete") { params in
            held.wait()
            included.value = included.value.subtracting(params["session_keys"] as? [String] ?? [])
            return ["updated": 1]
        }
        core.on("uploader.sessions.include") { params in
            included.value = included.value.union(params["session_keys"] as? [String] ?? [])
            return ["updated": 1]
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        _ = await eventually { model.sessions.count == 2 }
        let off = Task { await model.excludeSessions(keys: ["codex:a"]) }
        let started = await eventually { self.core.callCount("uploader.sessions.delete") == 1 }
        XCTAssertTrue(started)
        XCTAssertTrue(model.stoppingUploads.contains("codex:a"))
        XCTAssertFalse(model.isWorking)
        XCTAssertTrue(model.canSelectSession(model.sessions[1]))
        let second = Task { await model.includeSessions(keys: ["claude:b"]) }
        let independent = await eventually { self.core.callCount("uploader.sessions.include") == 1 }
        XCTAssertTrue(independent, "Another row must reach the core while the first request is held")
        let on = Task { await model.includeSessions(keys: ["codex:a"]) }
        _ = await eventually { model.requestingUploads.contains("codex:a") }
        XCTAssertFalse(model.stoppingUploads.contains("codex:a"))
        XCTAssertEqual(core.callCount("uploader.sessions.include"), 1, "The same row must serialize opposite requests")
        held.signal()
        await off.value
        _ = await on.value
        _ = await second.value
        XCTAssertEqual(included.value, Set(["codex:a", "claude:b"]))
        XCTAssertTrue(model.sessions.allSatisfy(\.included))
        XCTAssertTrue(model.requestingUploads.isEmpty)
        XCTAssertTrue(model.stoppingUploads.isEmpty)
        XCTAssertTrue(model.uploadErrors.isEmpty)
        model.shutdown()
    }

    func testFailedOptimisticUploadRollsBackOnlyThatRow() async {
        let held = DispatchSemaphore(value: 0)
        defer { held.signal() }
        core.on("uploader.sessions.include") { _ in
            held.wait()
            throw FakeCore.Failure(kind: "unavailable", message: "Upload service unavailable")
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        _ = await eventually { model.sessions.count == 2 }
        let change = Task { await model.includeSessions(keys: ["claude:b"]) }
        _ = await eventually { model.requestingUploads.contains("claude:b") }
        XCTAssertFalse(model.isWorking)
        held.signal()
        let accepted = await change.value
        XCTAssertFalse(accepted)
        XCTAssertFalse(model.sessions.first { $0.id == "claude:b" }!.included)
        XCTAssertTrue(model.sessions.first { $0.id == "codex:a" }!.included)
        XCTAssertEqual(model.uploadErrors["claude:b"], "Upload service unavailable")
        XCTAssertTrue(model.requestingUploads.isEmpty)
        model.shutdown()
    }

    func testReenableReconcilesAnUncertainCloudDeletion() async {
        let held = DispatchSemaphore(value: 0)
        defer { held.signal() }
        core.on("uploader.sessions.delete") { _ in
            held.wait()
            throw FakeCore.Failure(kind: "unavailable", message: "Deletion reply lost")
        }
        core.on("uploader.sessions.include") { _ in ["updated": 1] }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        _ = await eventually { model.sessions.count == 2 }
        let off = Task { await model.excludeSessions(keys: ["codex:a"]) }
        _ = await eventually { self.core.callCount("uploader.sessions.delete") == 1 }
        let on = Task { await model.includeSessions(keys: ["codex:a"]) }
        _ = await eventually { model.requestingUploads.contains("codex:a") }
        held.signal()
        await off.value
        _ = await on.value
        XCTAssertEqual(core.callCount("uploader.sessions.include"), 1, "The server may have deleted the copy even though local state still says included")
        XCTAssertTrue(model.uploadErrors.isEmpty)
        model.shutdown()
    }

    func testCloudDeletionFailureAndStaleConfirmationDoNotChangeSelection() async {
        core.on("uploader.sessions.delete") { _ in
            throw FakeCore.Failure(kind: "unavailable", message: "Cloud deletion unavailable")
        }
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let model = model()
        await model.bootstrapIfNeeded()
        _ = await eventually { model.sessions.count == 2 }
        await model.excludeSessions(keys: ["codex:a"], confirmedAt: model.uploadConnectionRevision - 1)
        XCTAssertEqual(core.callCount("uploader.sessions.delete"), 0, "An old workspace's confirmation must not delete this workspace's copy")
        await model.excludeSessions(keys: ["codex:a"], confirmedAt: model.uploadConnectionRevision)
        XCTAssertEqual(core.callCount("uploader.sessions.delete"), 1)
        XCTAssertEqual(core.callCount("uploader.sessions.exclude"), 0, "Deletion cannot silently fall back to stop-only")
        XCTAssertTrue(model.sessions.first { $0.id == "codex:a" }!.included)
        XCTAssertEqual(model.uploadErrors["codex:a"], "Cloud deletion unavailable")
        XCTAssertTrue(model.stoppingUploads.isEmpty)
        XCTAssertFalse(model.isWorking)
        model.shutdown()
    }

    func testClaudeDeliveryChoicesUseTheCorePolicy() async {
        let model = model()
        await model.claudeDelivery.refresh()
        XCTAssertEqual(model.claudeDelivery.status?.held.first?.senderName, "Bo")
        await model.claudeDelivery.restrict(false)
        XCTAssertEqual(core.lastParams("claude_delivery.restrict")?["restricted"] as? Bool, false)
        await model.claudeDelivery.setSession("c-1", direct: true)
        XCTAssertEqual(core.lastParams("claude_delivery.session")?["session_id"] as? String, "c-1")
        await model.claudeDelivery.alwaysAllow(model.claudeDelivery.status!.held[0])
        XCTAssertEqual(core.lastParams("claude_delivery.sender")?["sender"] as? String, "bo@desk")
        XCTAssertEqual(core.lastParams("claude_delivery.approve")?["id"] as? String, "del-1")
    }

    func testSpawnClaimRetryAsksTheCoreAgain() async {
        core.on("spawn.claim.retry") { _ in [:] }
        let model = model()
        await model.retrySpawnClaim()
        XCTAssertNotNil(core.lastParams("spawn.claim.retry"))
        XCTAssertNil(model.lastError)
    }

    func testTheTeammatesToggleSendsItsValueExplicitly() async {
        let model = model()
        _ = await model.setSetting(SettingsChange(spawnAllowTeammates: false))
        XCTAssertEqual(core.lastParams("settings.set")?["spawn_allow_teammates"] as? Bool, false)
        _ = await model.setSetting(SettingsChange(spawnAllowTeammates: true))
        XCTAssertEqual(core.lastParams("settings.set")?["spawn_allow_teammates"] as? Bool, true)
    }

    func testClaudeDeliveryRefreshClearsAnEarlierError() async {
        let model = model()
        core.on("claude_delivery.status") { _ in
            throw FakeCore.Failure(kind: "store_failed", message: "Could not read choices.")
        }
        await model.claudeDelivery.refresh()
        XCTAssertNotNil(model.claudeDelivery.error)

        core.on("claude_delivery.status") { _ in
            ["restricted": false, "sessions": [], "allowed_senders": [], "held": []]
        }
        await model.claudeDelivery.refresh()
        XCTAssertNil(model.claudeDelivery.error)
        XCTAssertEqual(model.claudeDelivery.status?.restricted, false)
    }

    func testUploadActionsRefuseWhenTheyCannotRun() async {
        let model = model()
        await model.bootstrapIfNeeded()
        _ = await eventually { model.phase == .signedOut }
        let included = await model.includeSessions(keys: ["codex:a"])
        XCTAssertFalse(included)
        XCTAssertNotNil(model.sessionActionError)
        XCTAssertEqual(core.callCount("uploader.sessions.include"), 0)
        model.shutdown()
    }
}

/// A value a fake core handler reads while the test changes it.
final class Locked<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var stored: Value
    init(_ value: Value) { stored = value }
    var value: Value {
        get { lock.withLock { stored } }
        set { lock.withLock { stored = newValue } }
    }
}

/// A count the fake core's handler threads and the test share.
private final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    var value: Int { lock.withLock { count } }
    func increment() { lock.withLock { count += 1 } }
    func set(_ value: Int) { lock.withLock { count = value } }
}
