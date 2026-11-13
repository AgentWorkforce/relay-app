import AppKit
import Darwin
import SwiftUI
import Vision
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

/// Actual Core + production LocalSessions + normal CoreClient + laid-out
/// MainWindowView. The helper owns an empty home and fake local transports only.
@MainActor final class NativeVisibilityTests: XCTestCase {
    func testIsolatedHostExitPreservesErrorAndCleansItsHome() async throws {
        let script = URL(fileURLWithPath: "/tmp/native-host-exit-\(UUID().uuidString)")
        try "#!/bin/sh\nprintf '{\"ready\":true}\\n'\nexit 2\n".write(to: script, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: script.path)
        defer { try? FileManager.default.removeItem(at: script) }
        let fixture = try NativeVisibilityHost(executable: script.path)
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertThrowsError(try fixture.command(["op": "seed"]))
        fixture.stop()
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.directory.path))
    }

    func testForcedHelperExitReapsItsOwnedChild() async throws {
        guard let executable = ProcessInfo.processInfo.environment["RELAY_NATIVE_VISIBILITY_HOST"] else {
            throw XCTSkip("Run explicitly with the built isolated native host; a skip is not acceptance evidence")
        }
        let fixture = try NativeVisibilityHost(executable: executable)
        let child = try fixture.command(["op": "spawn_cleanup_child"])
        let pid = try XCTUnwrap(child["pid"] as? Int32 ?? (child["pid"] as? Int).map(Int32.init))
        XCTAssertThrowsError(try fixture.command(["op": "exit_for_cleanup_test"])) { error in
            XCTAssertEqual((error as NSError).domain, "NativeVisibilityHostEOF")
        }
        fixture.stop()
        let gone = await eventually(2) { kill(pid, 0) != 0 && errno == ESRCH }
        XCTAssertTrue(gone, "Forced helper failure must reap its isolated child group")
    }

    func testProductionDiscoveryReachesRenderedAgentsAndExactLocalTransport() async throws {
        guard let executable = ProcessInfo.processInfo.environment["RELAY_NATIVE_VISIBILITY_HOST"] else {
            throw XCTSkip("Run explicitly with the built isolated RELAY_NATIVE_VISIBILITY_HOST; a skip is not acceptance evidence")
        }
        let requested = ProcessInfo.processInfo.environment["RELAY_NATIVE_VISIBILITY_CASE"]
        let allCases: [(String, String)] = [
            ("claude-terminal", "already-open"), ("claude-terminal", "open-after-create"),
            ("claude-terminal", "hidden-reveal"), ("claude-terminal", "blocked-other-provider"),
            ("codex", "already-open"), ("codex", "open-after-create"), ("codex", "hidden-reveal")
        ]
        let cases = allCases.filter { requested == nil || requested == "\($0.0)/\($0.1)" }
        XCTAssertFalse(cases.isEmpty, "Unknown native visibility case selector")
        for (kind, mode) in cases { try await exercise(executable: executable, kind: kind, mode: mode) }
    }

    func testProductionBackfillAndAfterIdleStillRenderBothProviders() async throws {
        guard let executable = ProcessInfo.processInfo.environment["RELAY_NATIVE_VISIBILITY_HOST"],
              ProcessInfo.processInfo.environment["RELAY_NATIVE_VISIBILITY_PROBE"] != nil else {
            throw XCTSkip("Run explicitly with built native host and immutable collector; a skip is not acceptance evidence")
        }
        let requested = ProcessInfo.processInfo.environment["RELAY_NATIVE_VISIBILITY_CASE"]
        let allCases: [(String, String)] = [
            ("claude-terminal", "backfill-held-status"), ("claude-terminal", "after-idle-backoff"),
            ("codex", "backfill-held-status"), ("codex", "after-idle-backoff")
        ]
        let cases = allCases.filter { requested == nil || requested == "\($0.0)/\($0.1)" }
        XCTAssertFalse(cases.isEmpty, "Unknown native history case selector")
        for (kind, mode) in cases {
            try await exercise(executable: executable, kind: kind, mode: mode)
        }
    }

    private func exercise(executable: String, kind: String, mode: String) async throws {
        let history = mode == "backfill-held-status" || mode == "after-idle-backoff"
        let fixture = try NativeVisibilityHost(executable: executable, probe: history ? ProcessInfo.processInfo.environment["RELAY_NATIVE_VISIBILITY_PROBE"] : nil)
        defer { fixture.stop() }
        let suite = "native-visibility-\(UUID())"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let client = CoreClient(socketPath: fixture.socket)
        let model = AppModel(client: client, service: FakeService(status: .enabled), bundledCoreVersion: nil,
                             manageSystemServices: false, defaults: defaults)
        defer { model.shutdown() }
        let id = UUID().uuidString.lowercased()
        let title = kind == "codex" ? "Fresh Codex Orchard" : "Fresh Claude Orchard"
        let projectedRows = Locked<Int?>(nil)
        let host = NSHostingView(rootView: MainWindowView().environment(model)
            .onPreferenceChange(LocalSessionCountPreference.self) { projectedRows.value = $0 })
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1280, height: 800),
                              styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        defer { window.close() }
        if mode != "open-after-create" {
            await model.bootstrapIfNeeded()
            let connected = await eventually { model.isCoreConnected && model.relay.workspace != nil }
            XCTAssertTrue(connected)
            window.makeKeyAndOrderFront(nil)
            model.windowVisibilityChanged(true)
            let queried = await eventually(10) { model.queriedLocalSessions != nil && !model.isLoadingSessions }
            XCTAssertTrue(queried, "Freeze a successful production query page before creating the new thread")
            try await Task.sleep(for: .milliseconds(100))
        }
        if mode == "hidden-reveal" {
            window.orderOut(nil)
            model.windowVisibilityChanged(false)
        }
        var seededHistoryRows = 0
        if history {
            let busy = try fixture.command(["op": "backfill"])
            // Each configured backfill JSONL is one session; derive the cue
            // from the actual isolated input rather than duplicating its count.
            let files = try XCTUnwrap(FileManager.default.enumerator(
                at: fixture.directory.appendingPathComponent(".claude/projects"), includingPropertiesForKeys: nil))
            seededHistoryRows = files.compactMap { $0 as? URL }.filter { $0.pathExtension == "jsonl" }.count
            XCTAssertGreaterThan(seededHistoryRows, 0)
            XCTAssertGreaterThan((busy["pending_records"] as? Int ?? 0) + (busy["unqueued_changes"] as? Int ?? 0), 0)
            var held = false
            for _ in 0..<100 {
                let state = try fixture.command(["op": "history_state"])
                if state["status_enrichment_held"] as? Bool == true {
                    XCTAssertEqual(state["actual_status_helper_succeeded"] as? Bool, true)
                    held = true; break
                }
                try await Task.sleep(for: .milliseconds(100))
            }
            XCTAssertTrue(held, "Real status helper must have completed and be held while discovery renders")
            if mode == "after-idle-backoff" {
                let quiet = try fixture.command(["op": "history_quiet"])
                XCTAssertEqual(quiet["quiet_seconds"] as? Int, 65)
            }
        }
        if history {
            let queried = await eventually(10) { model.queriedLocalSessions != nil && !model.isLoadingSessions }
            XCTAssertTrue(queried, "History page must finish while status remains held, before new thread creation")
        }
        if mode == "blocked-other-provider" { _ = try fixture.command(["op": "lock_codex"]) }
        let created = ProcessInfo.processInfo.systemUptime
        _ = try fixture.command(["op": "seed", "kind": kind, "id": id, "title": title])
        // This observes the production provider result, not core publication.
        let discovered = try fixture.command(["op": "discover", "kind": kind])
        XCTAssertTrue((discovered["ids"] as? [String] ?? []).contains(id))
        let providerRows = try XCTUnwrap(discovered["sessions"] as? [[String: Any]])
        XCTAssertEqual(providerRows.first(where: { ($0["target"] as? [String: Any])?["id"] as? String == id })?["live"] as? Bool, true,
                       "Fixture must supply actual trusted process evidence before expecting an Agents row")
        if mode == "open-after-create" { await model.bootstrapIfNeeded() }
        let opened = ProcessInfo.processInfo.systemUptime
        window.makeKeyAndOrderFront(nil)
        model.windowVisibilityChanged(true)
        let visible = await eventually(mode == "blocked-other-provider" ? 7 : 10) { model.relay.sessions.contains { $0.target.id == id } }
        XCTAssertTrue(visible, "\(kind) \(mode): production core invalidation must reach an already-open native client")
        fixture.record("core_visible", ["id": id, "actual_title": model.relay.sessions.first(where: { $0.target.id == id })?.title ?? "missing", "creation_to_ms": (ProcessInfo.processInfo.systemUptime-created)*1000])
        let committed = await eventually(2) { model.queriedLocalSessions?.contains { $0.target.id == id } == true }
        fixture.record("primary_query_committed", ["id": id, "committed": committed,
            "local_count": model.queriedLocalSessions?.count ?? -1, "loading": model.isLoadingSessions,
            "error": model.sessionsError ?? "", "creation_to_ms": (ProcessInfo.processInfo.systemUptime-created)*1000])
        XCTAssertTrue(committed, "Actual query page must admit the new relay-only target before native rendering")
        let visibilityDeadline = created + (mode == "blocked-other-provider" ? 7 : 10)
        let imageURL = ProcessInfo.processInfo.environment["RELAY_NATIVE_VISIBILITY_ARTIFACTS"].map {
            URL(fileURLWithPath: $0).appendingPathComponent("\(kind)-\(mode)-agents.png")
        }
        // Publication is only a readiness hint: the retained exact row and
        // its same-row Off pixels below still decide acceptance.
        let expectedRows = seededHistoryRows + 1
        var projectionReadyAt: TimeInterval?
        let frame = try await renderedRowFrame(title: title, deadline: visibilityDeadline, output: imageURL,
            readiness: {
                host.layoutSubtreeIfNeeded()
                guard projectedRows.value == expectedRows else { return false }
                projectionReadyAt = ProcessInfo.processInfo.systemUptime
                return true
            }) {
                host.layoutSubtreeIfNeeded()
                return try SessionRenderFixture.bitmap(for: host)
            }
        fixture.record("native_frame_observed", ["id": id, "attempt_count": frame.attempts,
            "expected_projected_rows": expectedRows,
            "projection_ready_to_ms": projectionReadyAt.map { ($0-created)*1000 } ?? -1,
            "pixel_capture_to_ms": (frame.capturedAt-created)*1000,
            "recognition_complete_to_ms": (ProcessInfo.processInfo.systemUptime-created)*1000,
            "exact_row_match": frame.control(title: title) != nil, "recognized": frame.words.map(\.0),
            "recognized_boxes": frame.words.map { ["text": $0.0, "global_bounds": [$0.1.minX, $0.1.minY, $0.1.width, $0.1.height]] }])
        if let imageURL { try Self.save(frame.bitmap, to: imageURL) }
        XCTAssertFalse(model.sessions.contains { $0.sessionId == id }, "Fresh row must be relay-only before any transcript/history write")
        XCTAssertLessThanOrEqual(frame.capturedAt, visibilityDeadline, "Exact row/control pixels must exist within the original visibility deadline")
        guard frame.capturedAt <= visibilityDeadline else { return }
        let rect = try XCTUnwrap(frame.control(title: title), "Exact rendered title and same-row Messages Off control missing from one frame: \(frame.words.map(\.0))")
        if mode == "backfill-held-status" {
            let state = try fixture.command(["op": "history_state"])
            XCTAssertEqual(state["status_enrichment_held"] as? Bool, true)
            let busy = state["busy"] as? [String: Any] ?? [:]
            XCTAssertGreaterThan((busy["pending_records"] as? Int ?? 0) + (busy["unqueued_changes"] as? Int ?? 0), 0,
                                 "Native row must render during proven real backfill, before releasing receiver")
            XCTAssertEqual(busy["acknowledged_records"] as? Int, 0, "Held HTTP batch must not be persisted as acknowledged")
        }
        fixture.record("native_rendered", ["id": id, "mode": mode, "creation_to_ms": (ProcessInfo.processInfo.systemUptime-created)*1000,
                                           "open_to_ms": (ProcessInfo.processInfo.systemUptime-opened)*1000])
        // The primary Sessions table title opens the transcript. Operate
        // its actual Messages control on the same uniquely identified row.
        let point = NSPoint(x: rect.midX * host.bounds.width, y: rect.midY * host.bounds.height)
        for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
            window.sendEvent(try XCTUnwrap(NSEvent.mouseEvent(with: type, location: point,
                modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
                context: nil, eventNumber: 0, clickCount: 1, pressure: type == .leftMouseDown ? 1 : 0)))
        }
        let registered = await eventually(mode == "blocked-other-provider" ? 1 : 10) { model.relay.sessions.first(where: { $0.target.id == id })?.registration != nil }
        XCTAssertTrue(registered, "The rendered row must register its exact target")
        let row = try XCTUnwrap(model.relay.sessions.first { $0.target.id == id })
        let registration = try XCTUnwrap(row.registration)
        XCTAssertEqual(row.target.kind.rawValue, kind)
        fixture.record("registered", ["id": id, "address": registration.address, "creation_to_ms": (ProcessInfo.processInfo.systemUptime-created)*1000])
        if mode == "blocked-other-provider" { _ = try fixture.command(["op": "unlock"]) }
        if history { _ = try fixture.command(["op": "history_new_path", "id": id, "kind": kind]) }
        if mode == "backfill-held-status" { _ = try fixture.command(["op": "history_release"]) }
        if kind == "claude-terminal" {
            let negative = try fixture.command(["op": "negatives", "id": id])
            XCTAssertEqual(negative.count, 4, "Push Live must not satisfy missing key/protocol/socket/process authority")
        }
        let message = "Orchard exact transport \(id)"
        let sent = try fixture.command(["op": "send", "name": registration.name, "text": message])
        let messageID = try XCTUnwrap(sent["message_id"] as? String)
        if kind == "claude-terminal" {
            let approved = try fixture.command(["op": "resolve_fixture_consent", "delivery_id": try XCTUnwrap(sent["delivery_id"] as? String)])
            XCTAssertTrue(approved["approved"] as? Bool == true || approved["already_delivered"] as? Bool == true)
        }
        let receiptArrived = await eventually(10) {
            (try? String(contentsOf: fixture.directory.appendingPathComponent("stages.jsonl")))?.contains(messageID) == true
        }
        XCTAssertTrue(receiptArrived, "Receiver frame consumption is separate from core transport-write settlement")
        let stages = try String(contentsOf: fixture.directory.appendingPathComponent("stages.jsonl"))
        let records = try stages.split(separator: "\n").map { try JSONSerialization.jsonObject(with: Data($0.utf8)) as! [String: Any] }
        let receipts = records.filter { $0["stage"] as? String == "transport_accepted" }.compactMap { $0["value"] as? [String: Any] }
        let receipt = try XCTUnwrap(receipts.first { $0["id"] as? String == id }, "Controlled receiver must identify the exact provider target")
        XCTAssertEqual(receipts.count, 1, "One exact local handoff; no duplicate delivery")
        if kind == "claude-terminal" {
            let auth = try XCTUnwrap(receipt["auth"] as? [String: Any])
            XCTAssertEqual(auth["type"] as? String, "auth")
            XCTAssertEqual(auth["token"] as? String, "fake-isolated-inbox-token")
            let frame = try XCTUnwrap(receipt["message"] as? [String: Any])
            XCTAssertEqual(frame["msg_id"] as? String, messageID)
            XCTAssertTrue(((frame["message"] as? [String: Any])?["content"] as? String ?? "").contains(message))
        } else {
            XCTAssertEqual(receipt["message_id"] as? String, messageID)
            XCTAssertTrue((receipt["text"] as? String ?? "").contains(message))
        }
        if history {
            let captured = try fixture.command(["op": "history_capture", "id": id])
            XCTAssertEqual(captured["marker"] as? String, "native-new-\(id)")
            fixture.record("native_history_ack_observed", ["id": id, "creation_to_ms": (ProcessInfo.processInfo.systemUptime-created)*1000])
        }
        if let output = ProcessInfo.processInfo.environment["RELAY_NATIVE_VISIBILITY_ARTIFACTS"] {
            let destination = URL(fileURLWithPath: output)
            try FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
            try FileManager.default.copyItem(at: fixture.directory.appendingPathComponent("stages.jsonl"),
                to: destination.appendingPathComponent("\(kind)-\(mode)-\(id).jsonl"))
        }
    }

    struct RenderedFrame {
        let bitmap: NSBitmapImageRep
        let words: [(String, CGRect)]
        let capturedAt: TimeInterval
        let attempts: Int
        func title(_ title: String) -> CGRect? {
            words.first { $0.0.trimmingCharacters(in: .whitespacesAndNewlines) == title }?.1
        }
        func control(title: String) -> CGRect? {
            guard let label = self.title(title) else { return nil }
            return words.first { $0.0.split(separator: " ").last == "Off" && $0.1.minX > label.maxX && abs($0.1.midY-label.midY) < 0.03 }?.1
        }
    }
    enum RenderObservationError: Error { case projectionDeadline }
    private func renderedRowFrame(title: String, deadline: TimeInterval, output: URL? = nil,
                                  readiness: (() -> Bool)? = nil,
                                  capture: () throws -> NSBitmapImageRep) async throws -> RenderedFrame {
        try await Self.renderedFrame(title: title, deadline: deadline, output: output, accepts: { $0.control(title: title) != nil }, readiness: readiness, capture: capture)
    }
    static func renderedFrame(title: String, deadline: TimeInterval, output: URL? = nil,
                              accepts: (RenderedFrame) -> Bool,
                              readiness: (() -> Bool)? = nil,
                              capture: () throws -> NSBitmapImageRep) async throws -> RenderedFrame {
        if let readiness {
            while true {
                guard ProcessInfo.processInfo.systemUptime < deadline else { throw RenderObservationError.projectionDeadline }
                if readiness() {
                    guard ProcessInfo.processInfo.systemUptime < deadline else { throw RenderObservationError.projectionDeadline }
                    break
                }
                try await Task.sleep(for: .milliseconds(10))
            }
        }
        var attempts = 0
        while true {
            attempts += 1
            let bitmap = try capture()
            let capturedAt = ProcessInfo.processInfo.systemUptime
            let image = try XCTUnwrap(bitmap.cgImage)
            // The immutable captured image crosses threads; AppKit and future
            // SwiftUI frames stay on MainActor while Vision recognizes it.
            let words = try await Task.detached { try Self.rowText(in: image, title: title) }.value
            let frame = RenderedFrame(bitmap: bitmap, words: words, capturedAt: capturedAt, attempts: attempts)
            if let output { try Self.save(bitmap, to: output.deletingPathExtension().appendingPathExtension("attempt-\(attempts).png")) }
            if accepts(frame) || attempts == 3 || ProcessInfo.processInfo.systemUptime >= deadline { return frame }
            try await Task.sleep(for: .milliseconds(100))
        }
    }
    static func save(_ bitmap: NSBitmapImageRep, to url: URL) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: url)
    }
    nonisolated static func rowText(in image: CGImage, title: String) throws -> [(String, CGRect)] {
        // These fixed-window fixtures sort the fresh target first. The top
        // title ROI preserves exact recognition on retained Intel CI frames
        // while avoiding recognition of all 64 older backfill rows.
        let titles = try text(in: image, region: CGRect(x: 0, y: 0.75, width: 0.55, height: 0.20))
        guard let label = titles.first(where: { $0.0.trimmingCharacters(in: .whitespacesAndNewlines) == title })?.1 else { return titles }
        // Keep padding wider than the same-row tolerance so clipping an
        // adjacent glyph cannot move its recognized center into that row.
        let lower = max(0, label.midY - 0.06), upper = min(1, label.midY + 0.06)
        return titles + (try text(in: image, region: CGRect(x: 0.75, y: lower, width: 0.15, height: upper - lower)))
    }
    private nonisolated static func text(in image: CGImage, region: CGRect) throws -> [(String, CGRect)] {
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        request.regionOfInterest = region
        try VNImageRequestHandler(cgImage: image, options: [:]).perform([request])
        return (request.results ?? []).compactMap { result in
            result.topCandidates(1).first.map { candidate in
                let box = result.boundingBox
                return (candidate.string, CGRect(x: region.minX + box.minX * region.width,
                    y: region.minY + box.minY * region.height, width: box.width * region.width, height: box.height * region.height))
            }
        }
    }
    private func rowBitmap(title: String, titleY: CGFloat = 600, controlY: CGFloat? = 600) throws -> NSBitmapImageRep {
        let bitmap = try XCTUnwrap(NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 1280, pixelsHigh: 800,
            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
            colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0))
        NSGraphicsContext.saveGraphicsState()
        defer { NSGraphicsContext.restoreGraphicsState() }
        NSGraphicsContext.current = try XCTUnwrap(NSGraphicsContext(bitmapImageRep: bitmap))
        NSColor.white.setFill(); NSRect(x: 0, y: 0, width: 1280, height: 800).fill()
        let attributes: [NSAttributedString.Key: Any] = [.font: NSFont.systemFont(ofSize: 28), .foregroundColor: NSColor.black]
        (title as NSString).draw(at: NSPoint(x: 40, y: titleY), withAttributes: attributes)
        if let controlY { ("Off" as NSString).draw(at: NSPoint(x: 1050, y: controlY), withAttributes: attributes) }
        return bitmap
    }
    func testRenderedRowRejectsAbsentWrongTitleAndOtherRowControl() async throws {
        let title = "Fresh Claude Orchard"
        for (visible, controlY) in [("", CGFloat(600)), ("Different Session", 600), (title, 300), (title, 572), (title, 628), (title, nil)] as [(String, CGFloat?)] {
            let bitmap = try rowBitmap(title: visible, controlY: controlY)
            let frame = try await renderedRowFrame(title: title, deadline: ProcessInfo.processInfo.systemUptime + 0.01) { bitmap }
            XCTAssertNil(frame.control(title: title), "Wrong/absent title or another row's Off must never satisfy pixel proof (controlY \(String(describing: controlY))): \(frame.words)")
        }
        let outside = try rowBitmap(title: title, titleY: 300, controlY: 300)
        let outsideWords = try Self.rowText(in: XCTUnwrap(outside.cgImage), title: title)
        XCTAssertNil(RenderedFrame(bitmap: outside, words: outsideWords, capturedAt: 0, attempts: 1).control(title: title),
                     "A title outside the fixture's first-row region must not satisfy pixel proof")
        let valid = try rowBitmap(title: title)
        let words = try Self.rowText(in: XCTUnwrap(valid.cgImage), title: title)
        let frame = RenderedFrame(bitmap: valid, words: words, capturedAt: 0, attempts: 1)
        let label = try XCTUnwrap(frame.title(title))
        XCTAssertEqual(label.minX, 40.0 / 1280, accuracy: 0.02)
        XCTAssertEqual(label.midY, 616.0 / 800, accuracy: 0.02, "Title ROI y bounds must return to the global frame")
        let control = try XCTUnwrap(frame.control(title: title))
        XCTAssertEqual(control.midX, 1071.0 / 1280, accuracy: 0.02, "ROI bounds must return to the global frame before clicking")
        XCTAssertEqual(control.midY, 616.0 / 800, accuracy: 0.02)
    }
    func testRenderedRowWaitsForExactFreshFrameWithinOriginalDeadline() async throws {
        let title = "Fresh Claude Orchard"
        let unreadable = try rowBitmap(title: "Different Session")
        let valid = try rowBitmap(title: title)
        var captures = 0
        let frame = try await renderedRowFrame(title: title, deadline: ProcessInfo.processInfo.systemUptime + 5) {
            captures += 1
            return captures == 1 ? unreadable : valid
        }
        XCTAssertEqual(frame.attempts, 2)
        XCTAssertTrue(frame.bitmap === valid, "Retain the exact accepted frame rather than recapturing later")
        XCTAssertNotNil(frame.control(title: title))
    }

    func testRenderedRowWaitsForPublishedProjectionBeforeFirstBitmap() async throws {
        let title = "Fresh Claude Orchard"
        let bitmap = try rowBitmap(title: title)
        // Replay each published value only after the predicate observes it;
        // a loaded MainActor must not be able to skip the old 64-row state.
        var count = 0
        var observed: [Int] = []
        var captures = 0
        let deadline = ProcessInfo.processInfo.systemUptime + 2
        let frame = try await renderedRowFrame(title: title, deadline: deadline, readiness: {
            observed.append(count)
            let ready = count == 65
            if count == 0 { count = 64 }
            else if count == 64 { count = 65 }
            return ready
        }) {
            captures += 1
            XCTAssertEqual(count, 65, "Neither initial 0 nor the old 64-row publication permits a bitmap")
            return bitmap
        }
        XCTAssertEqual(captures, 1)
        XCTAssertEqual(observed, [0, 64, 65])
        XCTAssertLessThanOrEqual(frame.capturedAt, deadline)
        XCTAssertNotNil(frame.control(title: title))
    }

    func testRenderedRowPublicationHintCannotReplacePixelsOrExtendDeadline() async throws {
        let title = "Fresh Claude Orchard"
        let wrong = try rowBitmap(title: "Different Session")
        let wrongFrame = try await renderedRowFrame(title: title, deadline: ProcessInfo.processInfo.systemUptime + 0.01,
                                                   readiness: { true }) { wrong }
        XCTAssertNil(wrongFrame.control(title: title), "Published count alone cannot replace the exact title and same-row Off pixels")
        let valid = try rowBitmap(title: title)
        for late in [false, true] {
            let count = Locked(64)
            let publication = Task { @MainActor in
                if late {
                    try await Task.sleep(for: .milliseconds(60))
                    count.value = 65
                }
            }
            defer { publication.cancel() }
            var captures = 0
            do {
                _ = try await renderedRowFrame(title: title, deadline: ProcessInfo.processInfo.systemUptime + 0.03,
                                               readiness: { count.value == 65 }) {
                    captures += 1
                    return valid
                }
                XCTFail("Absent or late projection publication must fail at the original deadline")
            } catch RenderObservationError.projectionDeadline { }
            XCTAssertEqual(captures, 0, "Do not start raster/OCR after readiness missed its deadline")
            _ = try await publication.value
        }
    }

}

private final class NativeVisibilityHost {
    let directory: URL
    let socket: String
    private let process = Process()
    private let input = Pipe()
    private let output = Pipe()
    private var hasLaunched = false
    private var processGroup: Int32?
    init(executable: String, probe: String? = nil) throws {
        directory = URL(fileURLWithPath: "/tmp/nv-\(UUID().uuidString.prefix(8))")
        socket = directory.appendingPathComponent("core.sock").path
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = [directory.path] + (probe.map { [$0] } ?? [])
        process.environment = ["HOME": directory.path, "PATH": "/usr/bin:/bin"]
        guard fcntl(input.fileHandleForWriting.fileDescriptor, F_SETNOSIGPIPE, 1) == 0 else {
            throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
        }
        process.standardInput = input
        process.standardOutput = output
        process.standardError = FileHandle.standardError
        do {
            try process.run()
            hasLaunched = true
            let ready = try read(timeout: 10)
            guard ready["ready"] as? Bool == true else { throw NSError(domain: "NativeVisibilityHost", code: 1) }
            if let group = ready["process_group"] as? Int, group == Int(process.processIdentifier), getpgid(process.processIdentifier) == Int32(group) { processGroup = Int32(group) }
        } catch {
            stop()
            throw error
        }
    }
    func command(_ value: [String: Any]) throws -> [String: Any] {
        try input.fileHandleForWriting.write(contentsOf: JSONSerialization.data(withJSONObject: value) + Data([10]))
        let timeout: TimeInterval = ["history_capture", "history_quiet", "backfill"].contains(value["op"] as? String ?? "") ? 245 : 15
        return try read(timeout: timeout)
    }
    private func read(timeout: TimeInterval) throws -> [String: Any] {
        let result = Locked<Result<[String: Any], Error>?>(nil)
        let done = DispatchSemaphore(value: 0)
        let reader = output.fileHandleForReading
        DispatchQueue.global().async {
            result.value = Result {
                var data = Data()
                while true {
                    let byte = reader.readData(ofLength: 1)
                    guard !byte.isEmpty else { throw NSError(domain: "NativeVisibilityHostEOF", code: 1) }
                    if byte == Data([10]) { break }
                    data.append(byte)
                }
                guard let value = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
                    throw NSError(domain: "NativeVisibilityHostResponse", code: 1)
                }
                return value
            }
            done.signal()
        }
        guard done.wait(timeout: .now() + timeout) == .success else {
            throw NSError(domain: "NativeVisibilityHostTimeout", code: 1)
        }
        return try result.value!.get()
    }
    func record(_ stage: String, _ value: [String: Any]) {
        let path = directory.appendingPathComponent("stages.jsonl")
        let descriptor = open(path.path, O_WRONLY | O_APPEND)
        guard descriptor >= 0 else { return }
        let file = FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
        defer { try? file.close() }
        guard flock(descriptor, LOCK_EX) == 0 else { return }
        if let data = try? JSONSerialization.data(withJSONObject: ["stage": stage, "value": value]) {
            try? file.write(contentsOf: data + Data([10]))
        }
    }
    func stop() {
        if process.isRunning { try? input.fileHandleForWriting.write(contentsOf: Data("{\"op\":\"stop\"}\n".utf8)) }
        try? input.fileHandleForWriting.close()
        if hasLaunched {
            let deadline = Date().addingTimeInterval(12)
            while process.isRunning && Date() < deadline { Thread.sleep(forTimeInterval: 0.01) }
            if process.isRunning {
                if let processGroup { kill(-processGroup, SIGTERM) } else { process.terminate() }
            }
            let stopped = Date().addingTimeInterval(2)
            while process.isRunning && Date() < stopped { Thread.sleep(forTimeInterval: 0.01) }
            if let processGroup { kill(-processGroup, SIGKILL) }
            else if process.isRunning { kill(process.processIdentifier, SIGKILL) }
            process.waitUntilExit()
        }
        if let output = ProcessInfo.processInfo.environment["RELAY_NATIVE_VISIBILITY_ARTIFACTS"] {
            let destination = URL(fileURLWithPath: output).appendingPathComponent(directory.lastPathComponent)
            try? FileManager.default.createDirectory(at: destination, withIntermediateDirectories: true)
            if let files = FileManager.default.enumerator(at: directory, includingPropertiesForKeys: nil) {
                for case let file as URL in files where file.pathExtension == "log" || file.lastPathComponent == "stages.jsonl" || file.pathComponents.contains("native-db-diagnostic") {
                    guard !file.hasDirectoryPath else { continue }
                    try? FileManager.default.copyItem(at: file, to: destination.appendingPathComponent(file.lastPathComponent))
                }
            }
        }
        try? FileManager.default.removeItem(at: directory)
    }
}
