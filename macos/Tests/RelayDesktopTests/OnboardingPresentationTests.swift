import AppKit
import SwiftUI
import Vision
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

@MainActor
final class OnboardingPresentationTests: XCTestCase {
    func testInitialChooserRendersOnlyTheInstalledCloudProviderWitness() async throws {
        let core = FakeCore()
        defer { core.stop() }
        let tools = ToolsModel(socketPath: core.path, requestTimeout: 0.5)
        for installed in [false, true] {
            let rows: [[String: Any]] = [
                row("codex", name: "Codex", detected: true, supported: true),
                row("amp", name: "Amp", detected: false, supported: false,
                    selected: true, state: "configured", available: false),
                // Data-only detection and saved connection do not admit Capy.
                row("capy", name: "Capy", detected: true, supported: false,
                    selected: true, state: "configured", available: installed),
            ]
            core.on("tools.status") { _ in FakeCore.toolsReport(onboardingComplete: false, tools: rows) }
            await tools.refresh()
            let text = try renderedText(tools: tools, core: core)
            XCTAssertTrue(text.contains("Codex"), "The actual chooser renders its ordinary installed provider")
            XCTAssertFalse(text.contains("Amp"), "A saved Cloud choice is not a local install")
            XCTAssertEqual(text.contains("Capy"), installed,
                           "Use the actual row-rendering branch, including a visible positive control")
            XCTAssertFalse(text.contains("Available providers"), "Initial onboarding is not the Settings catalog")
            XCTAssertEqual(core.callCount("tools.select"), 0)
            XCTAssertEqual(core.callCount("tools.manage"), 0)
        }
    }

    private func row(_ id: String, name: String, detected: Bool, supported: Bool,
                     selected: Bool = false, state: String = "not_installed", available: Bool? = nil) -> [String: Any] {
        var result: [String: Any] = ["id": id, "name": name, "detected": detected,
            "supported": supported, "selected": selected, "state": state,
            "status_label": "Setup", "detail": "Synthetic chooser presentation.",
            "can_install": false, "can_repair": false, "can_uninstall": false]
        if let available { result["onboarding_available"] = available }
        return result
    }

    private func renderedText(tools: ToolsModel, core: FakeCore) throws -> String {
        let suite = "installed-provider-render-\(UUID())"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let app = AppModel(client: CoreClient(socketPath: core.path), manageSystemServices: false, defaults: defaults)
        defer { app.shutdown() }
        let host = NSHostingView(rootView: ChooseToolsView(tools: tools).environment(app))
        let bounds = NSRect(x: 0, y: 0, width: 620, height: 540)
        let window = NSWindow(contentRect: bounds, styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        host.frame = bounds
        defer { window.close() }
        host.layoutSubtreeIfNeeded()
        let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: bounds))
        host.cacheDisplay(in: bounds, to: bitmap)
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        request.recognitionLanguages = ["en-US"]
        try VNImageRequestHandler(cgImage: XCTUnwrap(bitmap.cgImage), options: [:]).perform([request])
        return (request.results ?? []).compactMap { $0.topCandidates(1).first?.string }.joined(separator: "\n")
    }
}
