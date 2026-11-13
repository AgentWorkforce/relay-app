import AppKit
import SwiftUI
import XCTest
import Vision
@testable import RelayDesktop
@testable import RelayDesktopCore

@MainActor
final class StartupViewTests: XCTestCase {
    func testStartupCopyFitsAtMinimumWindowSize() async throws {
        let suite = "startup-preview-\(UUID())"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let model = AppModel(manageSystemServices: false, defaults: defaults)
        let host = NSHostingView(rootView: SignInView().environment(model))
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 980, height: 650),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        defer { window.close() }
        try await Task.sleep(for: .milliseconds(100))
        host.layoutSubtreeIfNeeded()
        XCTAssertEqual(host.bounds.width, 980, accuracy: 1)
        let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: bitmap)
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        try VNImageRequestHandler(cgImage: XCTUnwrap(bitmap.cgImage), options: [:]).perform([request])
        let text = (request.results ?? []).compactMap { $0.topCandidates(1).first?.string }.joined(separator: " ")
        XCTAssertTrue(text.contains(StartupPresentation.title), "Startup heading is clipped: \(text)")
        XCTAssertTrue(StartupPresentation.messages.contains(where: { text.replacingOccurrences(of: "’", with: "'").contains($0.replacingOccurrences(of: "’", with: "'")) }), "Startup detail is clipped: \(text)")
        if let path = ProcessInfo.processInfo.environment["STARTUP_RENDER_PATH"] {
            try bitmap.representation(using: .png, properties: [:])?.write(to: URL(fileURLWithPath: path))
        }
    }

    /// The same cases as the core's `amp_connections_url_appends_the_cloud_app_once`.
    func testAmpOpensTheCloudConnectionsTabOnTheSignedInSite() {
        for site in ["https://agentrelay.com", "https://agentrelay.com/", "https://agentrelay.com/cloud", " https://agentrelay.com/cloud/ "] {
            XCTAssertEqual(AmpConnections.url(siteURL: site)?.absoluteString,
                           "https://agentrelay.com/cloud/dashboard/settings#connections", site)
        }
        XCTAssertEqual(AmpConnections.url(siteURL: "http://localhost:8787")?.absoluteString,
                       "http://localhost:8787/cloud/dashboard/settings#connections")
        for invalid in ["", "file:///tmp", "http://example.com", "https://agentrelay.com/other"] {
            XCTAssertNil(AmpConnections.url(siteURL: invalid), invalid)
        }
    }

    func testWorkspaceSetupOpensTheWorkspaceTabOnTheSignedInSite() {
        XCTAssertEqual(WorkspaceSetup.dashboardURL(siteURL: "https://agentrelay.com")?.absoluteString,
                       "https://agentrelay.com/cloud/dashboard/settings#workspace")
        XCTAssertEqual(WorkspaceSetup.dashboardURL(siteURL: "http://user:pw@local.test:3000/x?y=1")?.absoluteString,
                       "http://local.test:3000/cloud/dashboard/settings#workspace")
        XCTAssertNil(WorkspaceSetup.dashboardURL(siteURL: "file:///tmp"))
        XCTAssertEqual(AccountAction(rawValue: "create_workspace"), .createWorkspace)
    }
}
