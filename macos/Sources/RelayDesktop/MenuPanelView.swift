import AppKit
import RelayDesktopCore
import SwiftUI

/// An at-a-glance summary: this Mac's agents on the relay and how many are
/// active, sessions uploaded and not, and a way into the app.
struct MenuPanelView: View {
    @Environment(AppModel.self) private var model
    var relay: RelayModel
    @Environment(\.openWindow) private var openWindow
    @State private var panelWindow: NSWindow?
    /// Whether the panel is on screen. `MenuBarExtra` keeps this view while
    /// the panel is closed; a closed panel draws a placeholder, so it reads,
    /// and re-renders for, nothing the summary shows.
    @State private var panelVisible = true
    @State private var summaryHeight: CGFloat = 0

    private var showsSummary: Bool {
        model.phase == .attached || model.phase == .preparingSessions
    }

    var body: some View {
        let _ = BodyEvaluations.record("MenuPanelView")
        VStack(alignment: .leading, spacing: 0) {
            if showsSummary {
                // Sign-in can finish while the panel is closed. Measure the
                // summary once before using its cached-height placeholder.
                if panelVisible || summaryHeight == 0 {
                    VStack(alignment: .leading, spacing: 0) { summary }
                        .background(GeometryReader { geometry in
                            Color.clear
                                .onAppear { summaryHeight = geometry.size.height }
                                .onChange(of: geometry.size.height) { summaryHeight = $1 }
                        })
                } else {
                    // The closed panel keeps its size, so it opens at the
                    // right height before the summary is counted again.
                    Color.clear.zoomFrame(height: summaryHeight)
                }
            } else {
                LoggedOutMenuView(
                    signingIn: model.phase == .signingIn || model.phase == .waitingForBrowser,
                    signIn: { leavePanel(model.showMainWindow) },
                    quit: model.quit
                )
            }
        }
        .zoomPadding(showsSummary ? 6 : 0)
        .zoomFrame(width: showsSummary ? 260 : 350)
        .fixedSize()
        .background(GeometryReader { geometry in
            MenuPanelWindowSizing(size: geometry.size)
        })
        .foregroundStyle(CloudTheme.foreground)
        .tint(CloudTheme.primary)
        .background(CloudTheme.background)
        .background(WindowReader(found: { panelWindow = $0 }))
        .tracksWindowVisibility { visible in
            panelVisible = visible
            if visible { model.panelDidAppear() } else { model.panelDidDisappear() }
        }
        .onAppear { model.registerOpenWindow { openWindow(id: $0) } }
        .onDisappear { model.panelDidDisappear() }
    }

    // MARK: Summary

    @ViewBuilder private var summary: some View {
        let counts = model.menuSummary()
        // Counts read as live only while the relay and uploads are reachable.
        let online = model.phase == .attached && WorkspaceConnection(model: model, relay: relay).isHealthy

        MenuSectionTitle("Messages") {
            let status = messagesStatus
            StatusDot(color: status.color, explanation: status.text) {
                leavePanel {
                    if model.account.authHealth != "ready" || model.accountHealthError != nil { model.showSettingsWindow(.account) }
                    else if status.problem { model.showDiagnosticsFromMenu() } else { model.showSessionsFromMenu(search: MenuSummaryFilter.receiving.search) }
                }
            }
        }
        countRow(.receiving, count: counts[.receiving], online: online)
        countRow(.localOnly, count: counts[.localOnly], online: online)
        MenuDivider()
        MenuSectionTitle("Uploads") {
            let problem = model.uploadStatusColor == CloudTheme.warning
            StatusDot(color: model.uploadStatusColor, explanation: model.uploadStatusText) {
                leavePanel {
                    if problem { model.showDiagnosticsFromMenu() } else { model.showSessionsFromMenu(search: "") }
                }
            }
        }
        countRow(.uploaded, count: counts[.uploaded], online: online)
        countRow(.notUploaded, count: counts[.notUploaded], online: online)
        MenuDivider()
        MenuRow("All sessions", shortcut: "⌘O") { leavePanel { model.showSessionsFromMenu(search: "") } }
            .keyboardShortcut("o", modifiers: .command)
        MenuRow("Settings…", shortcut: "⌘,") { leavePanel { model.showSettingsWindow() } }
            .keyboardShortcut(",", modifiers: .command)
        MenuRow("Quit Agent Relay", shortcut: "⌘Q", action: model.quit)
            .keyboardShortcut("q", modifiers: .command)
            .help(model.keepsAgentsConnectedAfterQuit ? "Quit Agent Relay (agents stay connected)" : "Quit Agent Relay")
    }

    /// Whether messages flow, in the relay banners' words. A problem is
    /// orange and opens Diagnostics.
    private var messagesStatus: (color: Color, text: String, problem: Bool) {
        if let problem = model.relayProblem { return (CloudTheme.warning, problem, true) }
        if model.isPaused { return (CloudTheme.secondary, "Paused. Agents don't receive messages and nothing uploads.", false) }
        if relay.workspace == nil {
            return relay.isConnectingWorkspace
                ? (CloudTheme.secondary, "Finish connecting the workspace in your browser.", false)
                : (CloudTheme.warning, relay.workspaceConnectError ?? "Connect a relay workspace to put agents on the relay.", true)
        }
        let connection = WorkspaceConnection(model: model, relay: relay)
        if !connection.isHealthy { return (connection.color, connection.help, true) }
        return (CloudTheme.success, "Connected to the relay", false)
    }

    private func countRow(_ filter: MenuSummaryFilter, count: Int, online: Bool) -> some View {
        MenuRow(filter.title, count: count, live: online && filter.isLive) {
            leavePanel { model.showSessionsFromMenu(search: filter.search) }
        }
        .help("Show sessions matching \(filter.search)")
    }

    /// A menu closes once something in it is chosen, so the window it opens
    /// isn't left under the panel.
    ///
    /// `MenuBarExtra` tracks whether its panel is shown only through its own
    /// status item click and its click-outside monitor. Closing the window
    /// any other way (even Escape) leaves it thinking the panel is open, and
    /// the next click on the icon is spent closing nothing. Pressing the
    /// status item's own button closes the panel through that same path.
    private func leavePanel(_ action: () -> Void) {
        if let button = Self.statusItemButton() {
            button.performClick(nil)
        } else {
            panelWindow?.close()
        }
        action()
    }

    private static func statusItemButton() -> NSStatusBarButton? {
        for window in NSApp.windows where window.className.contains("StatusBarWindow") {
            if let button = window.contentView?.firstSubview(of: NSStatusBarButton.self) { return button }
        }
        return nil
    }

}

/// A menu section's label, with optional trailing detail.
private struct MenuSectionTitle<Detail: View>: View {
    let title: String
    @ViewBuilder var detail: () -> Detail

    init(_ title: String, @ViewBuilder detail: @escaping () -> Detail = { EmptyView() }) {
        self.title = title
        self.detail = detail
    }

    var body: some View {
        HStack(spacing: 8) {
            Text(title).zoomFont(size: 11, weight: .semibold).accessibilityAddTraits(.isHeader)
            Spacer(minLength: 8)
            detail().zoomFont(size: 11)
        }
        .foregroundStyle(CloudTheme.secondary)
        .zoomPadding(.horizontal, 10)
        .zoomPadding(.top, 4)
        .zoomFrame(height: 24)
    }
}

/// A menu item: a label with a count badge or a shortcut on the right.
private struct MenuRow: View {
    private enum Trailing { case count(Int, live: Bool), shortcut(String) }

    let title: String
    private let trailing: Trailing
    let action: () -> Void

    /// `live` counts take the brand blue while nonzero; the rest stay muted.
    init(_ title: String, count: Int, live: Bool = false, action: @escaping () -> Void) {
        self.title = title
        trailing = .count(count, live: live)
        self.action = action
    }

    init(_ title: String, shortcut: String, action: @escaping () -> Void) {
        self.title = title
        trailing = .shortcut(shortcut)
        self.action = action
    }

    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Text(title).zoomFont(size: 13)
                Spacer(minLength: 8)
                switch trailing {
                case let .count(count, live):
                    CountBadge(count: count, live: live && count > 0)
                case let .shortcut(keys):
                    Text(keys).zoomFont(size: 13).foregroundStyle(CloudTheme.secondary)
                }
            }
        }
        .buttonStyle(MenuPanelButtonStyle(kind: .menuItem, animatesInteraction: false))
        .accessibilityLabel(accessibilityLabel)
    }

    private var accessibilityLabel: String {
        switch trailing {
        case let .count(count, _): return "\(title), \(count)"
        case let .shortcut(keys): return "\(title), \(keys)"
        }
    }
}

private struct CountBadge: View {
    let count: Int
    let live: Bool

    var body: some View {
        Text(count.formatted())
            .zoomFont(size: 11, weight: .semibold).monospacedDigit()
            .foregroundStyle(live ? AgentRelayButtonTheme.foreground : CloudTheme.secondary)
            .zoomPadding(.horizontal, 7)
            .zoomFrame(minWidth: 24, minHeight: 18)
            .background(live ? AgentRelayButtonTheme.background : CloudTheme.secondary.opacity(0.16), in: Capsule())
    }
}

/// A section's status: its colour at a glance, what it means on hover, and
/// a click to where the app explains it (Diagnostics for a problem).
private struct StatusDot: View {
    let color: Color
    let explanation: String
    let open: () -> Void

    var body: some View {
        Button(action: open) {
            Circle().fill(color).zoomFrame(width: 7, height: 7)
                .zoomFrame(width: 16, height: 16)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(explanation)
        .accessibilityLabel(explanation)
    }
}

private struct MenuDivider: View {
    var body: some View {
        Divider().overlay(CloudTheme.border).zoomPadding(.horizontal, 10).zoomPadding(.vertical, 5)
    }
}

private extension NSView {
    func firstSubview<T: NSView>(of type: T.Type) -> T? {
        if let match = self as? T { return match }
        for subview in subviews {
            if let match = subview.firstSubview(of: type) { return match }
        }
        return nil
    }
}

/// Real SwiftUI buttons retain keyboard activation and accessibility semantics.
/// Only user interaction animates; upload monitoring remains still.
struct MenuPanelButtonStyle: ButtonStyle {
    enum Kind { case primary, secondary, quiet, session, navigation, menuItem }
    var kind: Kind
    var controlHeight: CGFloat? = nil
    var isSelected = false
    // Menus track the pointer immediately; other app surfaces keep their transitions.
    var animatesInteraction = true

    @ViewBuilder func makeBody(configuration: Configuration) -> some View {
        if kind == .primary {
            AgentRelayPrimaryButtonStyle(height: controlHeight ?? 38, expands: true).makeBody(configuration: configuration)
        } else {
            PanelButtonBody(configuration: configuration, kind: kind, controlHeight: controlHeight, isSelected: isSelected, animatesInteraction: animatesInteraction)
        }
    }

    private struct PanelButtonBody: View {
        let configuration: Configuration
        let kind: Kind
        let controlHeight: CGFloat?
        let isSelected: Bool
        let animatesInteraction: Bool
        @Environment(\.isEnabled) private var isEnabled
        @Environment(\.isFocused) private var isFocused
        @Environment(\.accessibilityReduceMotion) private var reduceMotion
        @State private var hovered = false

        private var shouldAnimate: Bool { animatesInteraction && !reduceMotion }

        private var active: Bool { isEnabled && (hovered || configuration.isPressed) }
        private var isBorderless: Bool { kind == .quiet || kind == .session || kind == .navigation || kind == .menuItem }
        private var background: Color {
            if kind == .navigation && isSelected { return CloudTheme.hover }
            if active { return CloudTheme.hover }
            return isBorderless ? .clear : CloudTheme.muted
        }

        var body: some View {
            configuration.label
                .zoomFont(size: 12, weight: kind == .quiet ? .regular : .medium)
                .foregroundStyle(CloudTheme.foreground)
                .zoomPadding(.horizontal, kind == .session ? 8 : (kind == .menuItem ? 10 : 12))
                .zoomFrame(maxWidth: .infinity, minHeight: controlHeight ?? (kind == .menuItem ? 24 : (kind == .session ? 48 : (kind == .quiet ? 32 : 38))))
                .background(background, in: RoundedRectangle(cornerRadius: kind == .menuItem ? 5 : 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(
                    isFocused ? CloudTheme.ring : CloudTheme.border.opacity(isBorderless ? 0 : 1),
                    lineWidth: isFocused ? 2 : 1))
                .opacity(isEnabled ? (configuration.isPressed ? 0.85 : 1) : 0.45)
                .scaleEffect(kind == .navigation && isEnabled && configuration.isPressed && !reduceMotion ? 0.98 : 1)
                .contentShape(RoundedRectangle(cornerRadius: 8))
                // SwiftUI's hosting view can replace AppKit cursor rectangles.
                // Reassert it on every move, but only invalidate the view on entry/exit.
                .onContinuousHover { phase in
                    switch phase {
                    case .active:
                        if !hovered { hovered = true }
                        (isEnabled ? NSCursor.pointingHand : NSCursor.arrow).set()
                    case .ended:
                        if hovered { hovered = false }
                        NSCursor.arrow.set()
                    }
                }
                .onChange(of: isEnabled) { _, enabled in
                    if hovered { (enabled ? NSCursor.pointingHand : NSCursor.arrow).set() }
                }
                .onDisappear {
                    if hovered {
                        hovered = false
                        NSCursor.arrow.set()
                    }
                }
                .animation(shouldAnimate ? .easeOut(duration: 0.12) : nil, value: active)
                .animation(shouldAnimate ? .easeOut(duration: 0.12) : nil, value: configuration.isPressed)
                .animation(shouldAnimate ? .easeOut(duration: 0.16) : nil, value: isSelected)
        }
    }
}
