import AppKit
import SwiftUI

/// Animated Quartz ribbons work without a Metal device, including remote Macs.
/// The window visibility clock and Reduce Motion preference stop animation.
struct LoginRibbonsView: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.windowIsVisible) private var windowIsVisible
    @State private var started = Date()

    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 30, paused: reduceMotion || !windowIsVisible)) { clock in
            LoginRibbonsSurface(time: reduceMotion ? 14 : 14 + clock.date.timeIntervalSince(started))
        }
        .accessibilityHidden(true)
    }
}

private struct LoginRibbonsSurface: NSViewRepresentable {
    let time: Double
    func makeNSView(context: Context) -> LoginRibbonsCanvas { LoginRibbonsCanvas() }
    func updateNSView(_ view: LoginRibbonsCanvas, context: Context) {
        guard view.time != time else { return }
        view.time = time
        view.needsDisplay = true
    }
}

private final class LoginRibbonsCanvas: NSView {
    var time = 14.0
    override var isFlipped: Bool { true }
    override var isOpaque: Bool { true }
    override func draw(_ dirtyRect: NSRect) {
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        LoginRibbonDrawing.draw(context, size: bounds.size, time: time)
    }
}
