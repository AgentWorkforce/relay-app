import AppKit
import RelayDesktopCore
import SwiftUI

/// The window's content: the sessions on this machine, or a Settings pane.
enum DesktopPage: Hashable {
    var telemetrySurface: TelemetrySurface {
        switch self {
        case .sessions: return .sessions
        case .settings(let pane):
            switch pane {
            case .general: return .settingsGeneral
            case .tools: return .settingsTools
            case .opening: return .settingsOpening
            case .relay: return .settingsRelay
            case .sharing: return .settingsSharing
            case .account: return .settingsAccount
            case .about: return .settingsAbout
            case .diagnostics: return .settingsDiagnostics
            }
        }
    }

    case sessions
    case settings(SettingsPane)

    var isSettings: Bool {
        if case .settings = self { return true }
        return false
    }

    var title: String {
        switch self {
        case .sessions: return "Sessions"
        case .settings(let pane): return pane.title
        }
    }

    var subtitle: String {
        switch self {
        case .sessions: return "This machine"
        case .settings(let pane): return pane.subtitle
        }
    }
}

enum SettingsPane: String, CaseIterable, Identifiable {
    case general, tools, opening, relay, sharing, account, about, diagnostics

    var id: String { rawValue }

    var destination: SettingsPane {
        switch self {
        case .opening: return .tools
        case .sharing: return .general
        default: return self
        }
    }

    var title: String {
        switch self {
        case .general: return "General"
        case .tools: return "Providers"
        case .opening: return "Opening sessions"
        case .relay: return "Messages"
        case .sharing: return "Session sharing"
        case .account: return "Account"
        case .about: return "Updates"
        case .diagnostics: return "Diagnostics"
        }
    }

    var subtitle: String {
        switch self {
        case .general: return "Appearance, startup, sharing, privacy and session history."
        // Each provider page names itself; a pane subtitle would repeat it.
        case .tools: return ""
        case .opening: return "Choose where local sessions open."
        case .relay: return "The workspace your agents join and how messages reach them."
        case .sharing: return "Which coding sessions Agent Relay shares with your workspace."
        case .account: return "Your profile and connected workspace."
        case .about: return "Keep Agent Relay up to date."
        case .diagnostics: return "Troubleshoot connections and collect a diagnostic report."
        }
    }

    var symbol: String {
        switch self {
        case .general: return "gearshape"
        case .tools: return "wrench.and.screwdriver"
        case .opening: return "arrow.up.forward.app"
        case .relay: return "dot.radiowaves.left.and.right"
        case .sharing: return "arrow.up.circle"
        case .account: return "person.crop.circle"
        case .about: return "arrow.down.circle"
        case .diagnostics: return "questionmark.circle"
        }
    }

    /// Sidebar groups, in order.
    static let groups: [(title: String, panes: [SettingsPane])] = [
        ("Preferences", [.general]),
        ("Sessions", [.relay]),
        ("Account & app", [.account, .about, .diagnostics]),
    ]
}

// MARK: - Appearance

/// Light, dark, or whatever macOS uses. Applies to every window and the
/// menu-bar panel through the app's appearance.
enum AppAppearance: String, CaseIterable, Identifiable {
    case system, light, dark

    static let storageKey = "AppAppearance"

    var id: String { rawValue }

    var title: String {
        switch self {
        case .system: return "Match system"
        case .light: return "Light"
        case .dark: return "Dark"
        }
    }

    var symbol: String {
        switch self {
        case .system: return "circle.lefthalf.filled"
        case .light: return "sun.max"
        case .dark: return "moon"
        }
    }

    static var stored: AppAppearance {
        UserDefaults.standard.string(forKey: storageKey).flatMap(AppAppearance.init(rawValue:)) ?? .system
    }

    @MainActor
    func apply() {
        switch self {
        case .system: NSApp.appearance = nil
        case .light: NSApp.appearance = NSAppearance(named: .aqua)
        case .dark: NSApp.appearance = NSAppearance(named: .darkAqua)
        }
    }
}

// MARK: - Hover styles

/// Navigation controls in the window: sidebar rows, top-bar tabs
/// and icon buttons. Each shows hover, press, selection and focus.
struct NavButtonStyle: ButtonStyle {
    enum Kind { case sidebar, tab, icon }
    var kind: Kind
    var isSelected = false

    func makeBody(configuration: Configuration) -> some View {
        StyleBody(configuration: configuration, kind: kind, isSelected: isSelected)
    }

    private struct StyleBody: View {
        let configuration: Configuration
        let kind: Kind
        let isSelected: Bool
        @Environment(\.isEnabled) private var isEnabled
        @Environment(\.isFocused) private var isFocused
        @Environment(\.accessibilityReduceMotion) private var reduceMotion
        @State private var hovered = false

        private var active: Bool { isEnabled && hovered }

        private var fill: Color {
            if kind == .tab { return active ? CloudTheme.muted : .clear }
            if isSelected || (configuration.isPressed && isEnabled) { return CloudTheme.hover }
            return active ? CloudTheme.muted : .clear
        }

        private var foreground: Color {
            if isSelected { return kind == .tab ? CloudTheme.foreground : CloudTheme.selectedText }
            return active ? CloudTheme.foreground : CloudTheme.secondary
        }

        private var radius: CGFloat { kind == .tab ? 0 : 7 }

        var body: some View {
            configuration.label
                .foregroundStyle(foreground)
                .background(fill, in: RoundedRectangle(cornerRadius: radius, style: .continuous))
                .overlay(alignment: .bottom) {
                    if kind == .tab {
                        Rectangle().fill(isSelected ? Color.accentColor : Color.primary.opacity(active ? 0.15 : 0))
                            .zoomFrame(height: 2)
                    }
                }
                .overlay(RoundedRectangle(cornerRadius: radius, style: .continuous)
                    .strokeBorder(CloudTheme.ring.opacity(isFocused ? 1 : 0), lineWidth: 2))
                .contentShape(RoundedRectangle(cornerRadius: radius, style: .continuous))
                .opacity(isEnabled ? 1 : 0.4)
                .scaleEffect(configuration.isPressed && isEnabled && !reduceMotion && kind != .tab ? 0.97 : 1)
                .onHover { hovered = $0 }
                .actionCursor()
                .animation(reduceMotion ? nil : .easeOut(duration: 0.12), value: hovered)
                .animation(reduceMotion ? nil : .easeOut(duration: 0.08), value: configuration.isPressed)
                .animation(reduceMotion ? nil : .easeOut(duration: 0.16), value: isSelected)
        }
    }
}

/// An avatar for a person: their picture, else initials.
@MainActor
struct PersonAvatar: View {
    private let avatars = PersonAvatarStore.shared
    let name: String
    let url: URL?
    var size: CGFloat = 26
    var cornerRadius: CGFloat? = nil

    private var initials: String {
        let letters = name.split(whereSeparator: \.isWhitespace).prefix(2).compactMap(\.first)
        return letters.isEmpty ? "?" : String(letters).uppercased()
    }

    static func inlineImage(_ url: URL?) -> NSImage? {
        guard let url, url.scheme == "data" else { return nil }
        let parts = url.absoluteString.split(separator: ",", maxSplits: 1).map(String.init)
        guard parts.count == 2,
              ["data:image/png;base64", "data:image/jpeg;base64", "data:image/webp;base64", "data:image/gif;base64"].contains(parts[0]),
              parts[1].count <= 2_796_208,
              let data = Data(base64Encoded: parts[1]), data.count <= 2_097_152 else { return nil }
        return NSImage(data: data)
    }

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: cornerRadius ?? size / 2).fill(Color.accentColor.opacity(0.14))
            if let uploaded = Self.inlineImage(url) {
                Image(nsImage: uploaded).resizable().scaledToFill()
            } else if let image = avatars.image(for: url) {
                Image(nsImage: image).resizable().scaledToFill()
            } else {
                fallback
            }
        }
        .zoomFrame(width: size, height: size)
        .clipShape(RoundedRectangle(cornerRadius: cornerRadius ?? size / 2))
        .accessibilityHidden(true)
        .task(id: url) { if let url { await avatars.load(url) } }
    }

    private var fallback: some View {
        Text(initials)
            .zoomFont(size: size * 0.38, weight: .semibold)
            .foregroundStyle(Color.accentColor)
    }
}
