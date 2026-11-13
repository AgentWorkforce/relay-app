import AppKit
import SwiftUI

/// Launch Services is authoritative for installed apps, including apps outside /Applications.
@MainActor @Observable
final class SessionApplications {
    static let shared = SessionApplications()
    private(set) var claudeDesktop = false
    private(set) var codexDesktop = false
    @ObservationIgnored private var activation: NSObjectProtocol?

    /// Looks again whenever Agent Relay comes to the front: an app installed
    /// meanwhile shows without a relaunch.
    init() {
        refresh()
        activation = NotificationCenter.default.addObserver(forName: NSApplication.didBecomeActiveNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.refresh() }
        }
    }

    func refresh() {
        let claude = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.anthropic.claudefordesktop") != nil
        let codex = URL(string: "codex://threads/").flatMap { NSWorkspace.shared.urlForApplication(toOpen: $0) } != nil
        if claudeDesktop != claude { claudeDesktop = claude }
        if codexDesktop != codex { codexDesktop = codex }
    }

    func desktopAvailable(for source: String) -> Bool {
        switch source {
        case "claude": return claudeDesktop
        case "codex": return codexDesktop
        default: return false
        }
    }
}
