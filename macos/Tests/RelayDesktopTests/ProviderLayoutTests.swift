import AppKit
import SwiftUI
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

@MainActor
final class ProviderLayoutTests: XCTestCase {
    func testProviderSettingsFitNarrowAndWideWindowsWithoutChangingSettings() async throws {
        let core = FakeCore()
        defer { core.stop() }
        core.on("tools.status") { _ in
            ["account_health": "ready", "onboarding_complete": true, "tools": [
                ["id": "claude", "name": "Claude Code", "detected": true, "supported": true,
                 "selected": false, "state": "not_installed", "status_label": "Not installed",
                 "detail": "Agent Relay MCP is not selected for this provider.",
                 "can_install": true, "can_repair": false, "can_uninstall": false],
                ["id": "codex", "name": "Codex", "detected": true, "supported": true,
                 "selected": true, "state": "action_required", "status_label": "MCP not ready",
                 "detail": "Automatic reload could not reach or verify an existing Codex daemon.",
                 "can_install": false, "can_repair": true, "can_uninstall": true],
            ]]
        }
        core.on("settings.get") { _ in
            var settings = AppModelTests.settings(analytics: false)
            settings["claude_config_dirs"] = [["path": "/Users/example/Library/Application Support/Agent Relay/a-long-folder/.claude", "origin": "default", "shared": true]]
            settings["claude_cloud_sessions"] = false
            return settings
        }
        let app = AppModel(client: CoreClient(socketPath: core.path), manageSystemServices: false)
        defer { app.shutdown() }
        await app.loadSettings()
        await app.tools.refresh()
        XCTAssertNotNil(app.settings?.claudeConfigDirs)
        for provider in ["claude", "codex"] {
            app.tools.showProvider(provider)
            for dark in [false, true] {
                for width in [900.0, 1200.0] {
                    let view = HStack(spacing: 0) {
                        VStack {
                            ProvidersSidebar(tools: app.tools, isSelected: true, openProviders: {})
                            Spacer()
                        }.padding(10).frame(width: 244).background(CloudTheme.sidebar)
                        Divider().overlay(CloudTheme.border)
                        SettingsPaneView(pane: .tools)
                    }
                    .environment(app)
                    .background(CloudTheme.background)
                    .preferredColorScheme(dark ? .dark : .light)
                    let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: width, height: 760),
                                          styleMask: [.titled], backing: .buffered, defer: false)
                    window.isReleasedWhenClosed = false
                    window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
                    let host = NSHostingView(rootView: view)
                    window.contentView = host
                    window.orderFront(nil)
                    try await Task.sleep(for: .milliseconds(80))
                    host.layoutSubtreeIfNeeded()
                    XCTAssertEqual(host.bounds.width, width, accuracy: 1)
                    func checkScroll(_ view: NSView) {
                        if let scroll = view as? NSScrollView, let document = scroll.documentView {
                            XCTAssertLessThanOrEqual(document.frame.width, scroll.contentSize.width + 1,
                                                     "\(provider) settings overflow at \(width)")
                        }
                        view.subviews.forEach(checkScroll)
                    }
                    checkScroll(host)
                    if let directory = ProcessInfo.processInfo.environment["PROVIDERS_RENDER_DIR"] {
                        try FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
                        let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
                        host.cacheDisplay(in: host.bounds, to: bitmap)
                        let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
                        try data.write(to: URL(fileURLWithPath: directory)
                            .appendingPathComponent("\(provider)-\(dark ? "dark" : "light")-\(Int(width)).png"))
                    }
                    window.close()
                }
            }
        }
        XCTAssertEqual(core.callCount("tools.manage"), 0)
        XCTAssertEqual(core.callCount("settings.set"), 0)
    }
}
