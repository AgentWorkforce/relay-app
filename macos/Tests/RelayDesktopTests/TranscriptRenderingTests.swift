import AppKit
import SwiftUI
import XCTest
import Vision
@testable import RelayDesktop
@testable import RelayDesktopCore

@MainActor private final class TranscriptRenderWindow: NSWindow {
    override var canBecomeKey: Bool { true }
}

@MainActor final class TranscriptRenderingTests: XCTestCase {
    private var captureIndex = 0

    func testReaderDisclosuresAndDetailsRenderAndRespondToClicks() async throws {
        let core = FakeCore()
        defer { core.stop() }
        let oversized = (["| A | Overflow |" + Array(repeating: " x |", count: 31).joined(),
                          "|" + Array(repeating: " --- |", count: 33).joined(),
                          "| Extraordinary | Boundary |" + Array(repeating: " x |", count: 31).joined()]).joined(separator: "\n")
        core.on("sessions.transcript") { _ in ["entries": [
            ["id":"context", "role":"user", "kind":"context", "text":"PRIVATE SETUP BODY", "timestamp_ms":1, "truncated":false],
            ["id":"tool", "role":"assistant", "kind":"tool_use", "text":"PRIVATE TOOL BODY", "timestamp_ms":2, "truncated":false],
            ["id":"image", "role":"user", "kind":"text", "text":"", "image_path":"/missing-reader-test-image.png", "timestamp_ms":3, "truncated":false],
            ["id":"answer", "role":"assistant", "kind":"text", "text":"Visible answer\n\n| Files | Messages |\n| --- | --- |\n| Shared files | Interruptions |" + "\n\n" + oversized, "timestamp_ms":4, "truncated":false]
        ], "next_cursor":NSNull()] as [String:Any] }
        let client = CoreClient(socketPath: core.path)
        let suite = "reader-render-\(UUID())"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let app = AppModel(client: client, manageSystemServices: false, defaults: defaults)
        let updater = AppUpdater(defaults: defaults, currentVersion: "1.0.0", terminate: {})
        let row = LocalSessionRow.merge(uploads: [Fixture.uploadSession(sessionId: "reader", title: "Read a session")], sessions: [])[0]
        app.page = .sessions
        app.selectedLocalSession = row
        let host = NSHostingView(rootView: WorkspaceShell(relay: app.relay, updater: updater) {
            LocalTranscriptView(row: row, client: client, relay: app.relay)
        }.environment(app))
        let window = TranscriptRenderWindow(contentRect: NSRect(x:0, y:0, width:1100, height:760), styleMask:[.borderless], backing:.buffered, defer:false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        host.frame = NSRect(x: 0, y: 0, width: 1100, height: 760)
        window.makeKeyAndOrderFront(nil)
        defer { window.close() }
        for _ in 0..<100 where core.callCount("sessions.transcript") == 0 {
            try await Task.sleep(for: .milliseconds(20))
        }
        try await Task.sleep(for: .milliseconds(250))
        host.layoutSubtreeIfNeeded()
        // Pipe-heavy rows confuse Vision's full-image text direction detector.
        // Read their leading columns separately, retaining every assertion.
        let leadingFraction = min(1, 420 / host.bounds.width)
        let initial = try recognized(in: host).map(\.text).joined(separator:"\n")
        for label in ["Activity", "System prompt", "Attached image", "Visible answer", "Shared files", "Interruptions", "Overflow", "Boundary"] {
            XCTAssertTrue(initial.contains(label), "Missing rendered label: \(label)\n\(initial)")
        }
        for hidden in ["PRIVATE SETUP BODY", "PRIVATE TOOL BODY", "Image unavailable", "tool calls", "Refresh transcript"] {
            XCTAssertFalse(initial.contains(hidden), "Closed reader content appeared: \(hidden)")
        }
        let header = try wordBounds("Messages", in: host)
        let value = try wordBounds("Interruptions", in: host)
        XCTAssertEqual(header.minX * host.bounds.width, value.minX * host.bounds.width, accuracy: 8,
                       "The rendered Markdown must align table columns; a zero budget would render raw pipes with different offsets")
        let overflowHeader = try wordBounds("Overflow", in: host, leadingFraction: leadingFraction)
        let overflowValue = try wordBounds("Boundary", in: host, leadingFraction: leadingFraction)
        XCTAssertGreaterThan(abs(overflowHeader.minX - overflowValue.minX) * host.bounds.width, 20,
                             "An oversized table stays as pipe text rather than allocating a grid")
        XCTAssertFalse(app.showsSessionDetails)
        try await click("System prompt", in: host, window: window)
        XCTAssertTrue(try recognized(in: host).contains { $0.text.contains("PRIVATE SETUP BODY") })
        try await click("Attached image", in: host, window: window)
        XCTAssertTrue(try recognized(in: host).contains { $0.text.contains("Image unavailable") })
        try await click("Details", in: host, window: window)
        XCTAssertTrue(app.showsSessionDetails)
        XCTAssertTrue(try recognized(in: host).contains { $0.text.contains("Refresh transcript") })
    }

    private func wordBounds(_ word: String, in host: NSView, leadingFraction: CGFloat = 1) throws -> CGRect {
        for observation in try observations(in: host, leadingFraction: leadingFraction) {
            guard let candidate = observation.topCandidates(1).first,
                  let range = candidate.string.range(of: word),
                  let rectangle = try candidate.boundingBox(for: range) else { continue }
            let box = rectangle.boundingBox
            return CGRect(x: box.minX * leadingFraction, y: box.minY,
                          width: box.width * leadingFraction, height: box.height)
        }
        XCTFail("Missing rendered word: \(word)")
        throw NSError(domain: "TranscriptRenderingTests", code: 1)
    }

    private func captureImage(in host: NSView) throws -> CGImage {
        let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: bitmap)
        if let directory = ProcessInfo.processInfo.environment["SESSION_COLUMNS_PREVIEW"] {
            try FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
            let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
            try data.write(to: URL(fileURLWithPath: directory).appendingPathComponent("transcript-reader-\(captureIndex).png"))
            captureIndex += 1
        }
        return try XCTUnwrap(bitmap.cgImage)
    }

    private func observations(in host: NSView, leadingFraction: CGFloat = 1) throws -> [VNRecognizedTextObservation] {
        try observations(in: captureImage(in: host), region: CGRect(x: 0, y: 0, width: leadingFraction, height: 1))
    }

    private func observations(in image: CGImage, region: CGRect) throws -> [VNRecognizedTextObservation] {
        let pixels = CGRect(x: region.minX * CGFloat(image.width), y: (1 - region.maxY) * CGFloat(image.height),
                            width: region.width * CGFloat(image.width), height: region.height * CGFloat(image.height))
        let crop = try XCTUnwrap(image.cropping(to: pixels))
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        request.recognitionLanguages = ["en-US"]
        try VNImageRequestHandler(cgImage: crop, options: [:]).perform([request])
        return request.results ?? []
    }

    private func recognized(in host: NSView) throws -> [(text:String, bounds:CGRect)] {
        let image = try captureImage(in: host)
        let header = min(1, 90 / host.bounds.height)
        // Dense pipe rows can confuse full-image direction detection. Keep
        // controls independently readable and map every crop back to the window.
        let regions = [CGRect(x: 0, y: 0, width: 1, height: 1),
                       CGRect(x: 0, y: 0, width: min(1, 420 / host.bounds.width), height: 1),
                       CGRect(x: 0, y: 1 - header, width: 1, height: header),
                       CGRect(x: 0.7, y: 0, width: 0.3, height: 1)]
        return try regions.flatMap { region in
            try observations(in: image, region: region).compactMap { observation in
                guard let candidate = observation.topCandidates(1).first else { return nil }
                let box = observation.boundingBox
                return (candidate.string, CGRect(x: region.minX + box.minX * region.width,
                    y: region.minY + box.minY * region.height, width: box.width * region.width, height: box.height * region.height))
            }
        }
    }

    private func click(_ label:String, in host:NSView, window:NSWindow) async throws {
        let observations = try recognized(in:host)
        let control = try XCTUnwrap(observations.first { $0.text.contains(label) }, "Missing control: \(label)")
        let point = CGPoint(x:control.bounds.midX * host.bounds.width, y:control.bounds.midY * host.bounds.height)
        for type in [NSEvent.EventType.leftMouseDown, .leftMouseUp] {
            let event = try XCTUnwrap(NSEvent.mouseEvent(with:type, location:point, modifierFlags:[], timestamp:ProcessInfo.processInfo.systemUptime, windowNumber:window.windowNumber, context:nil, eventNumber:0, clickCount:1, pressure:type == .leftMouseDown ? 1 : 0))
            window.sendEvent(event)
        }
        try await Task.sleep(for:.milliseconds(200))
        host.layoutSubtreeIfNeeded()
    }
}
