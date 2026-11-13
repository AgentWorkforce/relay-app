import AppKit
import SwiftUI
import Vision
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

@MainActor
final class SessionOpenActionRenderingTests: XCTestCase {
    func testSharedClaudePlansRenderProviderArtworkAndFillTheActionRow() async throws {
        let location = SessionLocation(application: "Terminal", bundleId: "com.apple.Terminal",
            observedAtMs: 1000, pid: 31, processStart: 32, tty: "/dev/ttys001")
        let otherLocation = SessionLocation(application: "Terminal", bundleId: "com.apple.Terminal",
            observedAtMs: 1000, pid: 41, processStart: 42, tty: "/dev/ttys002")
        for (plan, title) in [
            (SessionOpenPlan(action: .openLink, choices: [], url: "claude://code/session/render-plan"), "Open in Claude"),
            (SessionOpenPlan(action: .choose, choices: [location, otherLocation]), "Open running session")
        ] {
            let action = plan.action.rawValue
            let answer = try XCTUnwrap(try JSONSerialization.jsonObject(with: JSONEncoder().encode(plan)) as? [String: Any])
            let core = FakeCore()
            defer { core.stop() }
            core.on("sessions.list") { _ in ["sessions": []] }
            core.on("sessions.location") { _ in ["current": [], "previous": [], "running": false, "supported": true] }
            core.on("sessions.open_plan") { _ in answer }
            let suite = "session-open-plan-render-\(UUID())"
            let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
            defer { defaults.removePersistentDomain(forName: suite) }
            let app = AppModel(client: CoreClient(socketPath: core.path), service: FakeService(status: .enabled),
                               manageSystemServices: false, defaults: defaults)
            await app.relay.refresh(.sessions)
            let row = LocalSessionRow.merge(uploads: [Fixture.uploadSession(source: "claude", sessionId: "render-plan",
                title: "Synthetic rendering session", cwd: "/work/synthetic")], sessions: [])[0]
            let view = VStack(alignment: .leading, spacing: 12) {
                SessionOpenButton(row: row)
                Button {} label: {
                    Label("Refresh transcript", systemImage: "arrow.clockwise").frame(maxWidth: .infinity)
                }.buttonStyle(CloudControlStyle(height: 38, horizontalPadding: 12))
            }.padding(8).frame(width: 280).background(CloudTheme.background)
                .environment(app).environment(\.colorScheme, .light).defaultAppStorage(defaults)
            let host = NSHostingView(rootView: view)
            let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 280, height: 150),
                                  styleMask: [.borderless], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            window.contentView = host
            host.frame = NSRect(x: 0, y: 0, width: 280, height: 150)
            defer { window.close() }
            let clock = ContinuousClock()
            let deadline = clock.now.advanced(by: .seconds(2))
            var renderedImage: NSBitmapImageRep?
            while clock.now < deadline {
                host.layoutSubtreeIfNeeded()
                if core.callCount("sessions.open_plan") > 0 {
                    let candidate = try bitmap(host)
                    // This isolated view has provider artwork only after the
                    // negotiated plan renders. Keep OCR out of the main-actor
                    // wait so SwiftUI can process the response between polls.
                    if orangePixels(candidate) > 12 && clock.now < deadline {
                        renderedImage = candidate
                        break
                    }
                }
                try await Task.sleep(for: .milliseconds(20))
            }
            XCTAssertGreaterThan(core.callCount("sessions.open_plan"), 0, "Exercise the actual negotiated core-plan branch")
            XCTAssertEqual(core.lastParams("sessions.open_plan")?["source"] as? String, "claude")
            XCTAssertEqual(core.lastParams("sessions.open_plan")?["session_id"] as? String, "render-plan")
            let image = try XCTUnwrap(renderedImage, "Wait for the negotiated plan's provider artwork to render")
            if let directory = ProcessInfo.processInfo.environment["SESSION_OPEN_RENDER_DIR"] {
                let path = URL(fileURLWithPath: directory, isDirectory: true)
                try FileManager.default.createDirectory(at: path, withIntermediateDirectories: true)
                try XCTUnwrap(image.representation(using: .png, properties: [:]))
                    .write(to: path.appendingPathComponent("\(action).png"))
            }
            let open = try textBounds(title, image: image)
            let refresh = try textBounds("Refresh transcript", image: image)
            XCTAssertEqual(open.midX, refresh.midX, accuracy: 0.08,
                           "The core-plan action is centered across the same full row as Refresh")
            XCTAssertGreaterThan(borderWidth(image, around: open), 220,
                                 "The actual planned button border fills the sidebar, not just its text")
            XCTAssertEqual(borderWidth(image, around: open), borderWidth(image, around: refresh), accuracy: 8)
            XCTAssertGreaterThan(orangePixels(image), 12,
                                 "The actual core-plan button must render Claude's bundled provider artwork")

            // Regression control: the former location-plan branch was a plain
            // text button even though the separate handover label had artwork.
            let old = NSHostingView(rootView: VStack(alignment: .leading) {
                Button(title) {}.buttonStyle(CloudControlStyle(height: 38, horizontalPadding: 12))
            }.padding(8).frame(width: 280).background(CloudTheme.background).environment(\.colorScheme, .light))
            old.frame = NSRect(x: 0, y: 0, width: 280, height: 80)
            old.layoutSubtreeIfNeeded()
            XCTAssertEqual(orangePixels(try bitmap(old)), 0, "The negative control has no provider artwork")
        }
    }

    private func bitmap(_ view: NSView) throws -> NSBitmapImageRep {
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        return bitmap
    }

    private func textBounds(_ text: String, image: NSBitmapImageRep) throws -> CGRect {
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        request.recognitionLanguages = ["en-US"]
        try VNImageRequestHandler(cgImage: XCTUnwrap(image.cgImage), options: [:]).perform([request])
        for result in request.results ?? [] {
            guard let candidate = result.topCandidates(1).first,
                  let range = candidate.string.range(of: text),
                  let bounds = try candidate.boundingBox(for: range) else { continue }
            return bounds.boundingBox
        }
        let recognized = (request.results ?? []).compactMap { $0.topCandidates(1).first?.string }
        XCTFail("The actual planned action was not rendered: \(text); recognized: \(recognized)")
        throw NSError(domain: "SessionOpenActionRenderingTests", code: 1)
    }

    private func orangePixels(_ image: NSBitmapImageRep) -> Int {
        var count = 0
        for y in 0..<image.pixelsHigh {
            for x in 0..<image.pixelsWide {
                guard let c = image.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB) else { continue }
                if c.alphaComponent > 0.8 && c.redComponent > 0.65 && c.redComponent - c.greenComponent > 0.15
                    && c.greenComponent - c.blueComponent > 0.04 { count += 1 }
            }
        }
        return count
    }

    private func borderWidth(_ image: NSBitmapImageRep, around text: CGRect) -> CGFloat {
        let center = Int((1 - text.midY) * CGFloat(image.pixelsHigh))
        let scale = CGFloat(image.pixelsWide) / 280
        var best = 0
        for y in max(0, center - Int(24 * scale))..<min(image.pixelsHigh, center + Int(24 * scale)) {
            guard let background = image.colorAt(x: 0, y: y)?.usingColorSpace(.deviceRGB) else { continue }
            var run = 0
            for x in 0..<image.pixelsWide {
                guard let c = image.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB) else { continue }
                let difference = max(abs(c.redComponent - background.redComponent),
                                     abs(c.greenComponent - background.greenComponent), abs(c.blueComponent - background.blueComponent))
                run = difference > 0.035 ? run + 1 : 0
                best = max(best, run)
            }
        }
        return CGFloat(best) / scale
    }
}
