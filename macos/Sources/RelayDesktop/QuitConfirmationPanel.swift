import AppKit
import RelayDesktopCore
import SwiftUI

/// A modal quit decision with the same branded primary action as the app.
@MainActor
final class QuitConfirmationPanel: NSPanel {
    init() {
        super.init(contentRect: NSRect(x: 0, y: 0, width: 480, height: 250),
                   styleMask: [.titled, .closable], backing: .buffered, defer: false)
        title = "Quit Agent Relay"
        isReleasedWhenClosed = false
        hidesOnDeactivate = false
        contentView = NSHostingView(rootView: QuitConfirmationView { response in
            NSApp.stopModal(withCode: response)
        })
    }

    override var canBecomeKey: Bool { true }

    func runConfirmation() -> NSApplication.ModalResponse {
        center()
        makeKeyAndOrderFront(nil)
        defer { orderOut(nil) }
        return NSApp.runModal(for: self)
    }

    override func cancelOperation(_ sender: Any?) {
        NSApp.stopModal(withCode: .cancel)
    }

    override func performClose(_ sender: Any?) {
        cancelOperation(sender)
    }
}

private struct QuitConfirmationView: View {
    let answer: (NSApplication.ModalResponse) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Image(systemName: "exclamationmark.triangle.fill")
                .zoomFont(size: 32)
                .foregroundStyle(.yellow)
            Text(QuitConfirmation.title)
                .zoomFont(size: 20, weight: .semibold)
            Text(QuitConfirmation.detail)
                .zoomFont(size: 13)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            HStack(spacing: 12) {
                Button("Cancel") { answer(.cancel) }
                    .keyboardShortcut(.cancelAction)
                    .buttonStyle(NativeActionButtonStyle(base: BorderedButtonStyle()))
                    .zoomFrame(width: 80)
                Button(QuitConfirmation.quitAnyway, role: .destructive) { answer(.alertSecondButtonReturn) }
                    .buttonStyle(NativeActionButtonStyle(base: BorderedButtonStyle()))
                    .zoomFrame(width: 100)
                Spacer(minLength: 0)
                Button(QuitConfirmation.keepAgentsConnected) { answer(.alertFirstButtonReturn) }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(AgentRelayPrimaryButtonStyle(height: 32))
            }
        }
        .zoomPadding(24)
        .zoomFrame(width: 480, height: 250)
        .foregroundStyle(CloudTheme.foreground)
        .background(CloudTheme.background)
    }
}
