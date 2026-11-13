import AppKit
import SwiftUI

private struct WindowVisibilityKey: EnvironmentKey {
    static let defaultValue = true
}

extension EnvironmentValues {
    /// Whether the window this view is in is on screen: ordered in, not
    /// minimized and not covered by other windows. Clocks, repeating
    /// animations and refreshes run only while it is.
    var windowIsVisible: Bool {
        get { self[WindowVisibilityKey.self] }
        set { self[WindowVisibilityKey.self] = newValue }
    }
}

extension View {
    /// Follows whether this view's window is on screen, hands it to
    /// `changed`, and tells every view inside through `windowIsVisible`.
    /// Each window's root view applies it once.
    func tracksWindowVisibility(_ changed: @escaping (Bool) -> Void = { _ in }) -> some View {
        modifier(WindowVisibilityModifier(changed: changed))
    }
}

private struct WindowVisibilityModifier: ViewModifier {
    let changed: (Bool) -> Void
    @State private var visible = true

    func body(content: Content) -> some View {
        content
            .environment(\.windowIsVisible, visible)
            .background(WindowReader(visible: { shown in
                if visible != shown { visible = shown }
                changed(shown)
            }))
    }
}

/// Hands back the window a view is in, and whether that window is on screen.
///
/// On screen means ordered in (`isVisible`, observed directly: a closed
/// `MenuBarExtra` panel is ordered out without an occlusion change), not
/// minimized, and not fully covered. Occlusion is believed only once the
/// window has reported it: macOS does not post it for every window, nor in
/// test processes.
struct WindowReader: NSViewRepresentable {
    var found: (NSWindow?) -> Void = { _ in }
    var visible: (Bool) -> Void = { _ in }

    func makeNSView(context: Context) -> NSView { Reader(found: found, visible: visible) }
    func updateNSView(_ nsView: NSView, context: Context) {}

    private final class Reader: NSView {
        let found: (NSWindow?) -> Void
        let visible: (Bool) -> Void
        private var orderedIn: NSKeyValueObservation?
        private var observers: [NSObjectProtocol] = []
        private var occluded = false
        private var reported: Bool?

        init(found: @escaping (NSWindow?) -> Void, visible: @escaping (Bool) -> Void) {
            self.found = found
            self.visible = visible
            super.init(frame: .zero)
        }
        required init?(coder: NSCoder) { nil }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            let window = window
            orderedIn = window?.observe(\.isVisible, options: [.new]) { [weak self] _, _ in
                MainActor.assumeIsolated { self?.report() }
            }
            observers.forEach(NotificationCenter.default.removeObserver)
            observers = []
            occluded = false
            if let window {
                let center = NotificationCenter.default
                observers.append(center.addObserver(forName: NSWindow.didChangeOcclusionStateNotification, object: window, queue: .main) { [weak self] _ in
                    MainActor.assumeIsolated {
                        guard let self, let window = self.window else { return }
                        self.occluded = !window.occlusionState.contains(.visible)
                        self.report()
                    }
                })
                for name in [NSWindow.didMiniaturizeNotification, NSWindow.didDeminiaturizeNotification] {
                    observers.append(center.addObserver(forName: name, object: window, queue: .main) { [weak self] _ in
                        MainActor.assumeIsolated { self?.report() }
                    })
                }
            }
            DispatchQueue.main.async {
                self.found(window)
                self.report()
            }
        }

        private func report() {
            let shown = window.map { $0.isVisible && !$0.isMiniaturized && !occluded } ?? false
            guard shown != reported else { return }
            reported = shown
            visible(shown)
        }

        deinit { observers.forEach(NotificationCenter.default.removeObserver) }
    }
}
