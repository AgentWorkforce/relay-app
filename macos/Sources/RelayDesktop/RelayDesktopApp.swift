import AppKit
import RelayDesktopCore
import OSLog
import SwiftUI
import UserNotifications

/// Closing the window keeps the core and the menu-bar controls running.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, UNUserNotificationCenterDelegate {
    let model: AppModel
    let updater: AppUpdater
    private let confirmQuit: @MainActor () -> NSApplication.ModalResponse
    private var automaticUpdateTask: Task<Void, Never>?

    override init() {
        // One Agent Relay app per Mac: before the model reads its settings
        // or registers the core, replace any other Agent Relay product.
        ProductTakeover.runAtLaunch()
        model = AppModel(detectCapy: { CapySetup.isInstalled(home: FileManager.default.homeDirectoryForCurrentUser) })
        updater = .shared
        confirmQuit = { QuitConfirmationPanel().runConfirmation() }
        super.init()
    }

    init(model: AppModel, confirmQuit: @escaping @MainActor () -> NSApplication.ModalResponse = { QuitConfirmationPanel().runConfirmation() }) {
        self.model = model
        updater = .shared
        self.confirmQuit = confirmQuit
        super.init()
    }

    // Fallback for URLs delivered through AppKit rather than a SwiftUI scene.
    func application(_ application: NSApplication, open urls: [URL]) {
        for url in urls { model.openConnection(url) }
    }

    func applicationDidBecomeActive(_ notification: Notification) {
        model.refreshCapySetup()
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        // A visible menu-bar panel is not a substitute for the main window.
        model.showMainWindow()
        return false
    }

    private let logger = Logger(subsystem: AppInfo.subsystem, category: "app")

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        AppAppearance.stored.apply()
        if Bundle.main.bundleIdentifier != nil { UNUserNotificationCenter.current().delegate = self }
        logger.info("Agent Relay started")
        Task { await model.bootstrapIfNeeded() }
        automaticUpdateTask = Task { [updater] in await updater.runAutomaticChecks() }
    }

    /// A quit the person asked for, while it would take their agents offline,
    /// is confirmed first. An update's restart and a logout, restart or
    /// shutdown quit without asking.
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        if case .installing = updater.state { return .terminateNow }
        guard model.asksBeforeQuitting, !Self.systemIsEnding else { return .terminateNow }
        NSApp.activate(ignoringOtherApps: true)
        switch confirmQuit() {
        case .alertFirstButtonReturn:
            model.setKeepsAgentsConnectedAfterQuit(true)
            return .terminateNow
        case .alertSecondButtonReturn:
            return .terminateNow
        default:
            return .terminateCancel
        }
    }

    /// The quit Apple event carries a reason only when macOS logs out,
    /// restarts or shuts down.
    private static var systemIsEnding: Bool {
        guard let event = NSAppleEventManager.shared().currentAppleEvent,
              event.eventID == kAEQuitApplication else { return false }
        return event.attributeDescriptor(forKeyword: kAEQuitReason) != nil
    }

    /// The core is launchd's LaunchAgent and keeps running after the app
    /// quits; this only closes the app's connections to it.
    func applicationWillTerminate(_ notification: Notification) {
        automaticUpdateTask?.cancel()
        model.shutdown()
    }

    /// A `notify` event's link (a provider's connect page, say) opens when
    /// its notification is clicked.
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
                                            withCompletionHandler completionHandler: @escaping () -> Void) {
        let link = response.notification.request.content.userInfo["link"] as? String
        Task { @MainActor in
            if let link, let url = URL(string: link), ["https", "http"].contains(url.scheme ?? "") {
                NSWorkspace.shared.open(url)
            }
            completionHandler()
        }
    }

    /// Shown while Agent Relay is in front, too.
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
                                            withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .sound])
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }
}

@main
struct RelayDesktopApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    /// The window's zoom percent (`UIZoom`), changed from the View menu and
    /// Settings. The main window reads it as `\.uiZoom`.
    @AppStorage(UIZoom.defaultsKey) private var uiZoomPercent = UIZoom.actualSize.percent

    private var uiZoom: UIZoom { UIZoom(percent: uiZoomPercent) }

    var body: some Scene {
        Window(AppInfo.windowTitle, id: AppInfo.mainWindowID) {
            MainWindowView()
                .environment(appDelegate.model)
                .environment(\.uiZoom, uiZoom)
        }
        .defaultSize(uiZoom.fitted(SessionColumnLayout.defaultWindowSize, in: UIZoom.usableMainScreen))
        .windowStyle(.hiddenTitleBar)
        .windowResizability(.contentMinSize)
        .commands {
            CommandGroup(replacing: .appSettings) {
                Button("Agent Relay Settings…") {
                    appDelegate.model.showSettingsWindow()
                }
                .keyboardShortcut(",")
            }
            CommandGroup(after: .appInfo) {
                Button("Check for Updates…") {
                    appDelegate.model.showSettingsWindow(.about)
                    Task { await appDelegate.updater.checkForUpdates() }
                }
            }
            CommandGroup(after: .toolbar) {
                Button("Zoom In") { uiZoomPercent = uiZoom.zoomedIn.percent }
                    .keyboardShortcut("=", modifiers: .command)
                    .disabled(!uiZoom.canZoomIn)
                Button("Zoom Out") { uiZoomPercent = uiZoom.zoomedOut.percent }
                    .keyboardShortcut("-", modifiers: .command)
                    .disabled(!uiZoom.canZoomOut)
                Button("Actual Size") { uiZoomPercent = UIZoom.actualSize.percent }
                    .keyboardShortcut("0", modifiers: .command)
                    .disabled(uiZoom == .actualSize)
                Divider()
            }
        }

        MenuBarExtra {
            MenuPanelView(relay: appDelegate.model.relay).environment(appDelegate.model)
        } label: {
            MenuBarLabel(relay: appDelegate.model.relay).environment(appDelegate.model)
        }
        .menuBarExtraStyle(.window)
    }
}
