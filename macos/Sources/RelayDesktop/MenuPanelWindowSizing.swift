import AppKit
import SwiftUI

/// A window-style MenuBarExtra can retain the larger signed-out size after
/// its content switches to the summary. Size the native panel as well as the
/// SwiftUI view, keeping the top-left position chosen by the system.
struct MenuPanelWindowSizing: NSViewRepresentable {
    let size: CGSize

    func makeNSView(context: Context) -> SizeView { SizeView(size: size) }

    func updateNSView(_ view: SizeView, context: Context) {
        view.size = size
        view.scheduleResize()
    }

    final class SizeView: NSView {
        var size: CGSize
        private var resizePending = false

        init(size: CGSize) {
            self.size = size
            super.init(frame: .zero)
        }

        required init?(coder: NSCoder) { nil }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            scheduleResize()
        }

        func scheduleResize() {
            guard !resizePending else { return }
            resizePending = true
            // Apply the latest measurement after SwiftUI's layout pass, not
            // during updateNSView, and never resize a detached/previous window.
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                self.resizePending = false
                guard let window = self.window,
                      self.size.width.isFinite, self.size.height.isFinite,
                      self.size.width > 0, self.size.height > 0 else { return }
                let size = NSSize(width: ceil(self.size.width), height: ceil(self.size.height))
                window.contentMinSize = size
                window.contentMaxSize = size
                if window.contentRect(forFrameRect: window.frame).size != size {
                    // setContentSize can keep the bottom edge on newer macOS,
                    // leaving a gap above the smaller menu. Preserve its top.
                    let previous = window.frame
                    var frame = window.frameRect(forContentRect: NSRect(origin: .zero, size: size))
                    frame.origin = NSPoint(x: previous.minX, y: previous.maxY - frame.height)
                    window.setFrame(frame, display: window.isVisible)
                }
            }
        }
    }
}
