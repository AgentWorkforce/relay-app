import AppKit
import CoreGraphics
import RelayDesktopCore
import SwiftUI

/// The Agent Relay mark, drawn from the path data in `RelayDesktopCore`.
enum BrandMark {
    static let menuBarLength: CGFloat = 18

    /// What the menu bar icon says without opening the panel.
    enum MenuBarState: Hashable {
        case normal
        /// Messages and uploads are paused: the mark is dimmed.
        case paused
        /// Something stops messages or uploads: the mark is drawn in the
        /// warning orange.
        case attention
    }

    /// The menu bar image. With no `tint` it is a template image: black, so
    /// macOS tints it for light and dark menu bars. A `tint` draws it in that
    /// colour instead, which sets Agent Relay Dev apart from the release app.
    /// Needing attention draws it in the warning orange, over any tint.
    /// The chevron behind is drawn at half strength. A `badge` (a build
    /// other than production's, `AppInfo.profileBadge`) is written after
    /// the mark in the same colour, so the build is told apart at a glance.
    static func menuBarImage(
        length: CGFloat = BrandMark.menuBarLength,
        state: MenuBarState = .normal,
        tint: NSColor? = nil,
        badge: String? = nil
    ) -> NSImage {
        let key = MenuBarImageKey(length: length, state: state, tint: tint, badge: badge)
        menuBarImagesLock.lock()
        defer { menuBarImagesLock.unlock() }
        if let image = menuBarImages[key] { return image }
        let image = drawMenuBarImage(length: length, state: state, tint: tint, badge: badge)
        menuBarImages[key] = image
        return image
    }

    private struct MenuBarImageKey: Hashable {
        var length: CGFloat
        var state: MenuBarState
        var tint: NSColor?
        var badge: String?
    }

    /// One image per state. The label's body hands the status item the same
    /// instance each time it is evaluated, so the item redraws only when the
    /// state changes.
    private static var menuBarImages: [MenuBarImageKey: NSImage] = [:]
    private static let menuBarImagesLock = NSLock()

    private static func drawMenuBarImage(length: CGFloat, state: MenuBarState, tint: NSColor?, badge: String?) -> NSImage {
        let tint = state == .attention ? CloudTheme.warningColor : tint
        let color = tint ?? .black
        let strength: CGFloat = state == .paused ? 0.35 : 1
        let badgeText = badge.map {
            NSAttributedString(string: $0, attributes: [
                .font: NSFont.systemFont(ofSize: length * 0.6, weight: .semibold),
                .foregroundColor: color.withAlphaComponent(strength),
            ])
        }
        let badgeGap = length * 0.2
        let badgeWidth = badgeText.map { ceil($0.size().width) + badgeGap } ?? 0
        let size = NSSize(width: length + badgeWidth, height: length)
        let image = NSImage(size: size, flipped: false) { bounds in
            guard let context = NSGraphicsContext.current?.cgContext else { return true }
            let rect = NSRect(x: 0, y: 0, width: length, height: length)
            if let badgeText {
                let height = badgeText.size().height
                badgeText.draw(at: NSPoint(x: length + badgeGap, y: (bounds.height - height) / 2))
            }
            let transform = SVGPath.fittingTransform(
                viewBox: BrandMarkPaths.viewBox,
                into: rect,
                flipVertically: true
            )
            context.saveGState()
            context.setFillColor(color.withAlphaComponent(0.5 * strength).cgColor)
            context.addPath(SVGPath.cgPath(BrandMarkPaths.back, transform: transform))
            context.fillPath()
            context.setFillColor(color.withAlphaComponent(strength).cgColor)
            context.addPath(SVGPath.cgPath(BrandMarkPaths.front, transform: transform))
            context.fillPath(using: .evenOdd)
            context.restoreGState()
            return true
        }
        image.isTemplate = tint == nil
        return image
    }
}

/// One of the two chevrons, scaled to fit whatever space it is given.
struct BrandMarkShape: Shape {
    enum Layer {
        case front
        case back
    }

    var layer: Layer

    /// Each chevron is built once, in its view box; a layout only scales it.
    private static let front = Path(SVGPath.cgPath(BrandMarkPaths.front))
    private static let back = Path(SVGPath.cgPath(BrandMarkPaths.back))

    func path(in rect: CGRect) -> Path {
        let transform = SVGPath.fittingTransform(
            viewBox: BrandMarkPaths.viewBox,
            into: rect,
            flipVertically: false
        )
        return (layer == .front ? Self.front : Self.back).applying(transform)
    }
}

/// The full-colour mark, for the sign-in window.
struct BrandMarkView: View {
    var width: CGFloat = 96

    private var height: CGFloat {
        width * (BrandMarkPaths.viewBox.height / BrandMarkPaths.viewBox.width)
    }

    private var frontColor: Color {
        Color(
            red: BrandMarkPaths.frontColor.red,
            green: BrandMarkPaths.frontColor.green,
            blue: BrandMarkPaths.frontColor.blue
        )
    }

    private var backColor: Color {
        Color(
            red: BrandMarkPaths.backColor.red,
            green: BrandMarkPaths.backColor.green,
            blue: BrandMarkPaths.backColor.blue
        )
    }

    var body: some View {
        if AppInfo.marksWithAppIcon {
            // A profile's mark is its app icon (build-app.sh --icon), square.
            Image(nsImage: NSApplication.shared.applicationIconImage)
                .resizable()
                .zoomFrame(width: width, height: width)
                .accessibilityHidden(true)
        } else {
            ZStack {
                BrandMarkShape(layer: .front).fill(frontColor)
                BrandMarkShape(layer: .back).fill(backColor)
            }
            .zoomFrame(width: width, height: height)
            .accessibilityHidden(true)
        }
    }
}

/// The menu bar item's label. It is also the app's earliest live view, so it
/// is where SwiftUI's window-opening action is handed to the model.
struct MenuBarLabel: View {
    @Environment(AppModel.self) private var model
    var relay: RelayModel
    @Environment(\.openWindow) private var openWindow

    private var state: BrandMark.MenuBarState {
        if model.relayProblem != nil { return .attention }
        if model.phase == .attached && (model.account.authHealth != "ready" || model.accountHealthError != nil || model.isCheckingAccountHealth) { return .attention }
        return model.isPaused ? .paused : .normal
    }

    var body: some View {
        let _ = BodyEvaluations.record("MenuBarLabel")
        Image(nsImage: BrandMark.menuBarImage(state: state, tint: AppInfo.isDevelopment ? .systemGreen : nil,
                                              badge: AppInfo.profileBadge))
            .accessibilityLabel(AppInfo.windowTitle)
            .accessibilityValue(state == .paused ? "Paused" : state == .attention ? "Needs attention" : "")
            // This handler stays mounted when the main window is closed.
            .onOpenURL { model.openConnection($0) }
            .onAppear {
                model.registerOpenWindow { identifier in
                    openWindow(id: identifier)
                }
            }
    }
}

/// Canonical Agent Relay wordmark from the website brand kit, kept as vector
/// outlines so its lettering stays exact at every display scale.
private struct BrandWordmarkShape: Shape {
    /// Built once, in its view box; a layout only scales it.
    private static let outline = Path(SVGPath.cgPath(BrandWordmarkPaths.commands))

    func path(in rect: CGRect) -> Path {
        Self.outline.applying(SVGPath.fittingTransform(viewBox: BrandWordmarkPaths.viewBox, into: rect, flipVertically: false))
    }
}

/// The agentrelay wordmark, `height` points tall; for any build but
/// production's, the product name in text followed by its profile badge
/// (the wordmark is Agent Relay's), as on Linux and Windows.
struct BrandWordmarkView: View {
    var height: CGFloat = 24
    var productName: String = AppInfo.displayName
    var bundleIdentifier: String = AppInfo.markedBundleIdentifier
    var badge: String? = AppInfo.profileBadge

    var drawsArtwork: Bool { AppInfo.drawsWordmark(bundleIdentifier: bundleIdentifier, productName: productName) }

    var body: some View {
        if drawsArtwork {
            BrandWordmarkShape()
                .fill(.foreground)
                .zoomFrame(width: height * BrandWordmarkPaths.viewBox.width / BrandWordmarkPaths.viewBox.height, height: height)
                .accessibilityLabel(productName)
        } else {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(productName)
                    .zoomFont(.headline)
                if let badge {
                    Text(badge)
                        .zoomFont(.caption, weight: .semibold)
                        .foregroundStyle(CloudTheme.secondary)
                }
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(AppInfo.windowTitle(productName: productName, badge: badge))
        }
    }
}
