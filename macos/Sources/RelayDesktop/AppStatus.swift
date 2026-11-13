import AppKit
import RelayDesktopCore
import SwiftUI

extension AppModel {
    var appStatusProblems: [String] {
        var seen = Set<String>()
        let uploadProblem: String?
        if let status = statusViewModel, case .needsAttention(let message) = status.state {
            uploadProblem = message
        } else { uploadProblem = nil }
        return ([uploadProblem, sessionActionError, sessionsError, previewError,
                coreProblem, lastError, relayProblem, accountHealthError, account.authError,
                relay.storeError, relay.integrationError, relay.deliveryError, uploadActionError] + tools.problems.map { Optional($0) })
            .compactMap { $0 }.filter { !$0.isEmpty && seen.insert($0).inserted }
    }

    func openAppStatus() {
        if requestFailure != nil { openFailedRequest() }
        else if !appStatusProblems.isEmpty {
            selectedRequest = nil
            page = .settings(.diagnostics)
        } else { showLocalSessions() }
    }

    /// The uploader's row for a relay session, once the uploader lists it.
    func uploadEntry(for session: LocalSession) -> UploadSession? {
        guard let key = session.uploadKey else { return nil }
        return sessions.first { $0.id == key }
    }

    var uploadStatusText: String {
        if phase == .preparingSessions { return "Preparing uploads" }
        guard phase == .attached else { return phase == .launching ? "Connecting…" : "Not connected" }
        guard let status = statusViewModel else { return "Checking uploads" }
        if isPaused { return "Uploads paused" }
        return status.state == .upToDate ? "Uploads up to date" : status.headline
    }

    var uploadStatusColor: Color {
        guard phase == .attached, let status = statusViewModel else { return CloudTheme.secondary }
        switch status.state {
        case .upToDate: return CloudTheme.success
        case .uploading, .preparing: return CloudTheme.primary
        case .needsAttention, .conflict, .offline: return CloudTheme.warning
        default: return CloudTheme.secondary
        }
    }
}

enum RelayClipboard {
    @discardableResult
    static func copy(_ text: String, to pasteboard: NSPasteboard = .general) -> Bool {
        pasteboard.clearContents()
        return pasteboard.setString(text, forType: .string)
    }

    /// Copies `@name` and tells VoiceOver, since the row's only visible
    /// confirmation is brief.
    static func copyName(_ name: String) {
        copy("@\(name)")
        NSAccessibility.post(element: NSApp as Any, notification: .announcementRequested, userInfo: [
            .announcement: "Copied @\(name)",
            .priority: NSAccessibilityPriorityLevel.high.rawValue
        ])
    }
}

/// A switch that shows its state the same way whether or not its window is
/// key. AppKit's switch drops its accent fill in a non-key window, which a
/// menu-bar panel often is, leaving on and off both grey.
///
/// On is a filled accent track with the knob right; off is a neutral track
/// with the knob left, so position carries the state as well as color.
struct RelaySwitchStyle: ToggleStyle {
    func makeBody(configuration: Configuration) -> some View {
        SwitchBody(configuration: configuration)
    }

    private struct SwitchBody: View {
        let configuration: Configuration
        @Environment(\.isEnabled) private var isEnabled
        @Environment(\.isFocused) private var isFocused
        @Environment(\.colorScheme) private var colorScheme
        @Environment(\.accessibilityReduceMotion) private var reduceMotion
        @State private var hovered = false

        private static let size = CGSize(width: 30, height: 18)
        private var isOn: Bool { configuration.isOn }
        private var highlighted: Bool { hovered && isEnabled }
        private var track: Color {
            if isOn { return CloudTheme.settingsAction.opacity(highlighted ? 0.85 : 1) }
            return CloudTheme.input
        }

        var body: some View {
            Button {
                configuration.isOn.toggle()
            } label: {
                ZStack(alignment: isOn ? .trailing : .leading) {
                    Capsule()
                        .fill(track)
                        .overlay(Capsule().strokeBorder(CloudTheme.border, lineWidth: 0.5))
                    Circle()
                        .fill(colorScheme == .dark ? CloudTheme.foreground : CloudTheme.background)
                        .shadow(color: .black.opacity(0.25), radius: 1, y: 0.5)
                        .zoomPadding(2)
                }
                .zoomFrame(width: Self.size.width, height: Self.size.height)
                .overlay(Capsule().strokeBorder(CloudTheme.ring.opacity(isFocused || highlighted ? 0.8 : 0), lineWidth: 2).zoomPadding(-3))
                .opacity(isEnabled ? 1 : 0.4)
                .contentShape(Capsule())
                .animation(reduceMotion ? nil : .spring(response: 0.25, dampingFraction: 0.8), value: isOn)
                .animation(reduceMotion ? nil : .easeOut(duration: 0.12), value: highlighted)
            }
            .buttonStyle(.plain)
            // The pointing hand says it can be pressed; SwiftUI's hosting view
            // can replace cursor rectangles, so it is reasserted on movement.
            .onContinuousHover { phase in
                switch phase {
                case .active:
                    hovered = true
                    (isEnabled ? NSCursor.pointingHand : NSCursor.arrow).set()
                case .ended:
                    hovered = false
                    NSCursor.arrow.set()
                }
            }
            .onDisappear {
                if hovered {
                    hovered = false
                    NSCursor.arrow.set()
                }
            }
            .accessibilityRepresentation {
                Toggle(isOn: configuration.$isOn) { configuration.label }
            }
        }
    }
}

/// Plain content with a soft fill under the pointer and when pressed.
struct HoverPlainButtonStyle<S: Shape>: ButtonStyle {
    let shape: S

    func makeBody(configuration: Configuration) -> some View {
        HoverBody(configuration: configuration, shape: shape)
    }

    private struct HoverBody: View {
        let configuration: Configuration
        let shape: S
        @Environment(\.isEnabled) private var isEnabled
        @Environment(\.accessibilityReduceMotion) private var reduceMotion
        @Environment(\.isFocused) private var isFocused
        @State private var hovering = false

        var body: some View {
            configuration.label
                .background(Color.primary.opacity(configuration.isPressed && isEnabled ? 0.12 : (hovering && isEnabled ? 0.07 : 0)), in: shape)
                .overlay(shape.stroke(CloudTheme.ring.opacity(isFocused && isEnabled ? 1 : 0), lineWidth: 2))
                .contentShape(shape)
                .opacity(isEnabled ? 1 : 0.45)
                .onHover { hovering = $0 }
                .actionCursor()
                .animation(reduceMotion ? nil : .easeOut(duration: 0.1), value: hovering)
        }
    }
}
