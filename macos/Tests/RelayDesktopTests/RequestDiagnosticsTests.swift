import AppKit
import SwiftUI
import Vision
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

@MainActor
final class RequestDiagnosticsTests: XCTestCase {
    func testDesktopIdentityRefusalHasSpecificStatusSummary() {
        let entry = RequestEntry(
            endpoint: "https://agentrelay.com/cloud/api/v1/workspaces/ws/messaging/desktop-identities",
            failed: true, id: "identity-403", method: "PUT", request: "{}", response: "{}",
            service: "Cloud", startedAtMs: 0, status: 403
        )
        XCTAssertEqual(entry.failureSummary, "Agents couldn't link to Cloud")
    }

    func testConnectRequestsAreNamedForTheRoom() {
        let connect = RequestEntry(
            connect: true, endpoint: "https://cast.agentrelay.com/", failed: true, id: "connect-429",
            method: "SDK: heartbeat", request: "{}", response: "{}", service: "Relaycast", startedAtMs: 0, status: 429
        )
        XCTAssertEqual(connect.serviceName, "Relay Connect")
        XCTAssertEqual(connect.failureSummary, "Relay Connect request failed")
        let team = RequestEntry(
            endpoint: "https://cast.agentrelay.com/", failed: true, id: "team-429",
            method: "SDK: heartbeat", request: "{}", response: "{}", service: "Relaycast", startedAtMs: 0, status: 429
        )
        XCTAssertEqual(team.serviceName, "Agent Relay")
    }

    func testCopyProblemsWritesToPasteboard() {
        let pasteboard = NSPasteboard.withUniqueName()
        defer { pasteboard.releaseGlobally() }
        let details = "Agent Relay's background service stopped."
        XCTAssertTrue(RelayClipboard.copy(details, to: pasteboard))
        XCTAssertEqual(pasteboard.string(forType: .string), details)
    }

    func testRequestLogEntriesPublishOnlyWhileDiagnosticsShows() async throws {
        let core = FakeCore()
        core.on("diagnostics.requests") { _ in ["entries": [[
            "id": "failure-7", "started_at_ms": 123, "service": "Relaycast", "method": "GET",
            "endpoint": "https://cast.agentrelay.com/v1/workspace", "request": "null",
            "response": "Workspace temporarily unavailable", "status": 503, "duration_ms": 650, "failed": true
        ]]] }
        let model = AppModel(client: CoreClient(socketPath: core.path), manageSystemServices: false)
        defer { model.shutdown(); core.stop() }
        model.page = .sessions
        await model.refreshRequestLog()
        XCTAssertEqual(model.requestFailure?.id, "failure-7", "A new failure is noticed on any page")
        XCTAssertTrue(model.requestEntries.isEmpty, "Entries nobody can see are not published")
        model.page = .settings(.diagnostics)
        await model.refreshRequestLog()
        XCTAssertEqual(model.requestEntries.map(\.id), ["failure-7"])
    }

    func testFailureStaysInDiagnosticsAndOpensResponse() async throws {
        let core = FakeCore()
        core.on("diagnostics.requests") { _ in ["entries": [[
            "id": "failure-42", "started_at_ms": 123, "service": "Relaycast", "method": "GET",
            "endpoint": "https://cast.agentrelay.com/v1/workspace", "request": "null",
            "response": "Workspace temporarily unavailable", "status": 503, "duration_ms": 650, "failed": true
        ]]] }
        let model = AppModel(client: CoreClient(socketPath: core.path), manageSystemServices: false)
        defer { model.shutdown(); core.stop() }
        model.page = .settings(.diagnostics)
        await model.refreshRequestLog()
        let view = WorkspaceShell(relay: model.relay, updater: AppUpdater.shared) {
            SettingsPaneView(pane: .diagnostics)
        }.environment(model).background(CloudTheme.background)
        let host = NSHostingView(rootView: view)
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1200, height: 780), styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.orderFront(nil)
        defer { window.close() }
        try await Task.sleep(for: .milliseconds(300))
        func render(_ name: String) throws -> [VNRecognizedTextObservation] {
            host.layoutSubtreeIfNeeded()
            // Keep OCR input resolution independent of the runner's display backing scale.
            let scale = 2.0
            let bitmap = try XCTUnwrap(NSBitmapImageRep(
                bitmapDataPlanes: nil, pixelsWide: Int(host.bounds.width * scale),
                pixelsHigh: Int(host.bounds.height * scale), bitsPerSample: 8,
                samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0
            ))
            bitmap.size = host.bounds.size
            host.cacheDisplay(in: host.bounds, to: bitmap)
            if let directory = ProcessInfo.processInfo.environment["REQUEST_RENDER_DIR"] {
                try FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
                try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: URL(fileURLWithPath: directory).appendingPathComponent(name + ".png"))
            }
            let request = VNRecognizeTextRequest()
            request.recognitionLevel = .accurate
            try VNImageRequestHandler(cgImage: XCTUnwrap(bitmap.cgImage), options: [:]).perform([request])
            return request.results ?? []
        }
        let initial = try render("request-failure-link")
        let recognized = initial.map {
            "\($0.topCandidates(1).first?.string ?? "") at \($0.boundingBox)"
        }.joined(separator: " | ")
        XCTAssertFalse(initial.contains { observation in
            observation.boundingBox.minY < 0.1 && observation.boundingBox.midX < 0.35 &&
                observation.topCandidates(1).first?.string.contains("Agent Relay request failed") == true
        }, "Request failures must stay out of the footer: \(recognized)")
        XCTAssertFalse(initial.contains { $0.topCandidates(1).first?.string.contains("Recent Agent Relay") == true })
        let header = try XCTUnwrap(initial.first { $0.topCandidates(1).first?.string.contains("GET https://cast.agentrelay.com") == true })
        func clickHeader() {
            // Stay inside the row when macOS reserves width for its scroll bar.
            let point = NSPoint(x: host.bounds.width - 65, y: header.boundingBox.midY * host.bounds.height)
            for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
                window.sendEvent(NSEvent.mouseEvent(with: type, location: point, modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber, context: nil, eventNumber: 0, clickCount: 1, pressure: type == .leftMouseUp ? 0 : 1)!)
            }
        }
        clickHeader()
        try await Task.sleep(for: .milliseconds(150))
        let clicked = try render("request-header-expanded")
        XCTAssertTrue(clicked.contains { $0.topCandidates(1).first?.string.contains("Workspace temporarily unavailable") == true }, "The far-right header area must expand the full-width request")
        clickHeader()
        try await Task.sleep(for: .milliseconds(150))
        let collapsed = try render("request-header-collapsed")
        XCTAssertFalse(collapsed.contains { $0.topCandidates(1).first?.string.contains("Workspace temporarily unavailable") == true })
        model.openAppStatus()
        try await Task.sleep(for: .milliseconds(300))
        let expanded = try render("request-expanded")
        // Vision can split the selected, highlighted response into word groups.
        // Keep checking the exact response across those adjacent observations.
        let expandedText = expanded.compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        XCTAssertTrue(expandedText.contains("Workspace temporarily unavailable"), "Selected response OCR: \(expandedText)")
        XCTAssertEqual(model.selectedRequest?.id, "failure-42")
    }
}
