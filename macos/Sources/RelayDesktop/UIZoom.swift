import AppKit
import Foundation
import SwiftUI

/// The main window's zoom, as a browser's: View > Zoom In, Zoom Out and
/// Actual Size step along a fixed ladder of percents, remembered in
/// `UserDefaults` under `uiZoom`. The GTK app (`gtk/src/prefs.rs`,
/// `UiZoom`) keeps the same ladder under the same key.
///
/// The zoom reaches the views as `\.uiZoom` in the environment, and the
/// window scales its own type, padding and frames with the `zoomFont`,
/// `zoomPadding` and `zoomFrame` modifiers below: the layout grows with
/// the text instead of the text alone growing, as the GTK app scales every
/// size it sets.
enum UIZoom: Int, CaseIterable, Equatable, Sendable {
    case actualSize = 100
    case percent110 = 110
    case percent125 = 125
    case percent150 = 150
    case percent175 = 175
    case percent200 = 200

    /// The `UserDefaults` key the percent is stored under.
    static let defaultsKey = "uiZoom"

    /// The share of the screen's visible frame a zoomed window may claim as
    /// its smallest size, as `USABLE_SHARE` in `gtk/src/ui/zoom.rs`.
    static let usableShare: CGFloat = 0.85

    /// The ladder's step at `percent`; anything off the ladder is 100%.
    init(percent: Int) {
        self = UIZoom(rawValue: percent) ?? .actualSize
    }

    var percent: Int { rawValue }

    /// The percent as the menus and Settings show it: "150%".
    var label: String { "\(rawValue)%" }

    /// One step larger, stopping at 200%.
    var zoomedIn: UIZoom { step(by: 1) }

    /// One step smaller, stopping at 100%.
    var zoomedOut: UIZoom { step(by: -1) }

    var canZoomIn: Bool { zoomedIn != self }

    var canZoomOut: Bool { zoomedOut != self }

    /// 1 at 100%, 2 at 200%.
    var factor: CGFloat { CGFloat(rawValue) / 100 }

    /// A size of `value` points at 100%, at this zoom. Hairlines (a point or
    /// less), unbounded and unset sizes are left alone.
    func scale(_ value: CGFloat) -> CGFloat {
        guard self != .actualSize, value.isFinite, value > 1 else { return value }
        return value * factor
    }

    /// `size` at this zoom, but never larger than `screen` unless it
    /// already was at 100%: a window zoomed to 200% still fits the screen.
    /// `screen` is the space windows may use, as `usable(_:)` gives it.
    func fitted(_ size: CGSize, in screen: CGSize?) -> CGSize {
        let scaled = CGSize(width: scale(size.width), height: scale(size.height))
        guard let screen else { return scaled }
        return CGSize(width: min(scaled.width, max(screen.width, size.width)),
                      height: min(scaled.height, max(screen.height, size.height)))
    }

    /// What a window may use of a screen whose visible frame is `visible`:
    /// `usableShare` of each side, leaving room for the title bar.
    static func usable(_ visible: CGSize) -> CGSize {
        CGSize(width: (visible.width * usableShare).rounded(.down),
               height: (visible.height * usableShare).rounded(.down))
    }

    /// What a window may use of the main screen; nil with no screen.
    @MainActor static var usableMainScreen: CGSize? {
        NSScreen.main.map { usable($0.visibleFrame.size) }
    }

    /// The point size macOS draws `style` at by default, for scaling a
    /// text style by the zoom.
    static func pointSize(of style: Font.TextStyle) -> CGFloat {
        switch style {
        case .largeTitle: return 26
        case .title: return 22
        case .title2: return 17
        case .title3: return 15
        case .headline, .body: return 13
        case .callout: return 12
        case .subheadline: return 11
        case .footnote, .caption, .caption2: return 10
        @unknown default: return 13
        }
    }

    private func step(by offset: Int) -> UIZoom {
        let ladder = UIZoom.allCases
        let index = ladder.firstIndex(of: self) ?? 0
        return ladder[min(max(index + offset, 0), ladder.count - 1)]
    }
}

private struct UIZoomKey: EnvironmentKey {
    static let defaultValue = UIZoom.actualSize
}

extension EnvironmentValues {
    /// The zoom the main window is drawn at; 100% everywhere else.
    var uiZoom: UIZoom {
        get { self[UIZoomKey.self] }
        set { self[UIZoomKey.self] = newValue }
    }
}

/// `.font(.system(size:weight:design:))` at the zoom.
private struct ZoomSystemFont: ViewModifier {
    @Environment(\.uiZoom) private var zoom
    let size: CGFloat
    let weight: Font.Weight?
    let design: Font.Design?

    func body(content: Content) -> some View {
        content.font(.system(size: zoom.scale(size), weight: weight, design: design))
    }
}

/// `.font(.system(style, design:weight:))`: the system's own text style at
/// 100%, its default point size scaled by the zoom above it.
private struct ZoomStyleFont: ViewModifier {
    @Environment(\.uiZoom) private var zoom
    let style: Font.TextStyle
    let weight: Font.Weight?
    let design: Font.Design?

    func body(content: Content) -> some View {
        content.font(font)
    }

    private var font: Font {
        guard zoom != .actualSize else {
            return .system(style, design: design, weight: weight)
        }
        let headline = style == .headline && weight == nil
        return .system(size: zoom.scale(UIZoom.pointSize(of: style)),
                       weight: headline ? .bold : weight,
                       design: design)
    }
}

private struct ZoomPadding: ViewModifier {
    @Environment(\.uiZoom) private var zoom
    let insets: (UIZoom) -> EdgeInsets?
    let edges: Edge.Set

    @ViewBuilder
    func body(content: Content) -> some View {
        if let insets = insets(zoom) {
            content.padding(insets)
        } else {
            content.padding(edges, nil)
        }
    }
}

private struct ZoomFixedFrame: ViewModifier {
    @Environment(\.uiZoom) private var zoom
    let width: CGFloat?
    let height: CGFloat?
    let alignment: Alignment

    func body(content: Content) -> some View {
        content.frame(width: width.map(zoom.scale), height: height.map(zoom.scale), alignment: alignment)
    }
}

private struct ZoomFlexibleFrame: ViewModifier {
    @Environment(\.uiZoom) private var zoom
    let minWidth: CGFloat?
    let idealWidth: CGFloat?
    let maxWidth: CGFloat?
    let minHeight: CGFloat?
    let idealHeight: CGFloat?
    let maxHeight: CGFloat?
    let alignment: Alignment

    func body(content: Content) -> some View {
        content.frame(minWidth: minWidth.map(zoom.scale),
                      idealWidth: idealWidth.map(zoom.scale),
                      maxWidth: maxWidth.map(zoom.scale),
                      minHeight: minHeight.map(zoom.scale),
                      idealHeight: idealHeight.map(zoom.scale),
                      maxHeight: maxHeight.map(zoom.scale),
                      alignment: alignment)
    }
}

extension View {
    /// `.font(.system(size:weight:design:))`, scaled by `\.uiZoom`.
    func zoomFont(size: CGFloat, weight: Font.Weight? = nil, design: Font.Design? = nil) -> some View {
        modifier(ZoomSystemFont(size: size, weight: weight, design: design))
    }

    /// `.font(.<style>)`, scaled by `\.uiZoom`.
    func zoomFont(_ style: Font.TextStyle, weight: Font.Weight? = nil, design: Font.Design? = nil) -> some View {
        modifier(ZoomStyleFont(style: style, weight: weight, design: design))
    }

    /// `.padding(edges, length)`, scaled by `\.uiZoom`; `nil` is the
    /// system's default padding, left alone.
    func zoomPadding(_ edges: Edge.Set = .all, _ length: CGFloat? = nil) -> some View {
        modifier(ZoomPadding(insets: { zoom in
            length.map { length in
                let value = zoom.scale(length)
                return EdgeInsets(top: edges.contains(.top) ? value : 0,
                                  leading: edges.contains(.leading) ? value : 0,
                                  bottom: edges.contains(.bottom) ? value : 0,
                                  trailing: edges.contains(.trailing) ? value : 0)
            }
        }, edges: edges))
    }

    /// `.padding(length)`, scaled by `\.uiZoom`.
    func zoomPadding(_ length: CGFloat) -> some View {
        zoomPadding(.all, length)
    }

    /// `.padding(insets)`, scaled by `\.uiZoom`.
    func zoomPadding(_ insets: EdgeInsets) -> some View {
        modifier(ZoomPadding(insets: { zoom in
            EdgeInsets(top: zoom.scale(insets.top), leading: zoom.scale(insets.leading),
                       bottom: zoom.scale(insets.bottom), trailing: zoom.scale(insets.trailing))
        }, edges: .all))
    }

    /// `.frame(width:height:alignment:)`, scaled by `\.uiZoom`.
    func zoomFrame(width: CGFloat? = nil, height: CGFloat? = nil, alignment: Alignment = .center) -> some View {
        modifier(ZoomFixedFrame(width: width, height: height, alignment: alignment))
    }

    /// `.frame(minWidth:…maxHeight:alignment:)`, scaled by `\.uiZoom`;
    /// `.infinity` stays unbounded.
    func zoomFrame(minWidth: CGFloat? = nil, idealWidth: CGFloat? = nil, maxWidth: CGFloat? = nil,
                   minHeight: CGFloat? = nil, idealHeight: CGFloat? = nil, maxHeight: CGFloat? = nil,
                   alignment: Alignment = .center) -> some View {
        modifier(ZoomFlexibleFrame(minWidth: minWidth, idealWidth: idealWidth, maxWidth: maxWidth,
                                   minHeight: minHeight, idealHeight: idealHeight, maxHeight: maxHeight,
                                   alignment: alignment))
    }
}
