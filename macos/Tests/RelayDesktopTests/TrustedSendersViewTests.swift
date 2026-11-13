import AppKit
import SwiftUI
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

@MainActor
final class TrustedSendersViewTests: XCTestCase {
    nonisolated static func settings(trustsMembers: Bool) -> [String: Any] {
        var settings = AppModelTests.settings(analytics: true)
        settings["openclaw_gateway"] = true
        settings["trusted_relay_senders"] = ["ci-bot@node-9"]
        settings["trust_workspace_members"] = trustsMembers
        settings["workspace_members"] = trustsMembers
            ? [["name": "grace-hopper", "display_name": "Grace Hopper"], ["name": "ada-lovelace"]]
            : []
        return settings
    }

    /// The people in the workspace are listed read-only beside the field, and
    /// the switch sets `trust_workspace_members`. Opt-in renders for review.
    func testTrustedSendersListWorkspaceMembers() async throws {
        let core = FakeCore()
        core.setStatus(FakeCore.snapshot(revision: 2, account: FakeCore.signedIn))
        let suite = "trusted-senders-\(UUID())"
        let defaults = UserDefaults(suiteName: suite)!
        let model = AppModel(client: CoreClient(socketPath: core.path), service: FakeService(status: .enabled),
                             bundledCoreVersion: "1.0.0", manageSystemServices: false, defaults: defaults)
        core.on("settings.get") { _ in TrustedSendersViewTests.settings(trustsMembers: true) }
        core.on("settings.set") { params in
            TrustedSendersViewTests.settings(trustsMembers: params["trust_workspace_members"] as? Bool ?? true)
        }
        defer { model.shutdown(); core.stop(); defaults.removePersistentDomain(forName: suite) }
        await model.bootstrapIfNeeded()
        let subscribed = await eventually { model.isCoreConnected && model.phase == .attached }
        XCTAssertTrue(subscribed, "subscribe before publishing Settings invalidations")
        await model.loadSettings()
        XCTAssertEqual(model.settings?.trustWorkspaceMembers, true)
        XCTAssertEqual(model.settings?.workspaceMembers?.map { TrustedSendersCopy.memberTitle(name: $0.name, displayName: $0.displayName) },
                       ["Grace Hopper (grace-hopper)", "ada-lovelace"])

        if let directory = ProcessInfo.processInfo.environment["SETTINGS_RENDER_DIR"] {
            try FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
            for dark in [true, false] {
                let view = SettingsPaneView(pane: .relay)
                    .environment(model)
                    .background(CloudTheme.background)
                    .preferredColorScheme(dark ? .dark : .light)
                let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 900, height: 2400),
                                      styleMask: [.titled], backing: .buffered, defer: false)
                window.isReleasedWhenClosed = false
                window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
                let host = NSHostingView(rootView: view)
                window.contentView = host
                window.orderFront(nil)
                try await Task.sleep(for: .milliseconds(200))
                host.layoutSubtreeIfNeeded()
                let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
                host.cacheDisplay(in: host.bounds, to: bitmap)
                let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
                try data.write(to: URL(fileURLWithPath: directory)
                    .appendingPathComponent("trusted-senders-macos-\(dark ? "dark" : "light").png"))
                window.close()
            }
        }

        // Expired Cloud data invalidates Settings while the toggle stays on.
        core.on("settings.get") { _ in
            var settings = TrustedSendersViewTests.settings(trustsMembers: true)
            settings["workspace_members"] = []
            return settings
        }
        core.publish("invalidated", ["what": "settings"])
        let expired = await eventually { model.settings?.workspaceMembers == [] }
        XCTAssertTrue(expired)
        XCTAssertEqual(model.settings?.trustWorkspaceMembers, true)
        XCTAssertEqual(model.settings?.trustedRelaySenders, ["ci-bot@node-9"])

        let error = await model.setSetting(SettingsChange(trustWorkspaceMembers: false))
        XCTAssertNil(error)
        XCTAssertEqual(core.lastParams("settings.set")?["trust_workspace_members"] as? Bool, false)
        XCTAssertEqual(model.settings?.trustWorkspaceMembers, false)
        XCTAssertEqual(model.settings?.workspaceMembers, [])
        XCTAssertEqual(model.settings?.trustedRelaySenders, ["ci-bot@node-9"])
    }
}
