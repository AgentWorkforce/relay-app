import AppKit
import QuartzCore
import RelayDesktopCore
import SwiftUI

/// Provider-native artwork from RelayHistory, rendered without provider-file reads.
struct SessionAvatarView: View {
    let avatar: SessionAvatar
    let image: NSImage?
    let animationFrames: [NSImage]?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    init?(avatar: SessionAvatar) {
        let image = Self.inlineImage(avatar.imageDataUrl)
        let frames = image == nil ? Self.frames(shape: avatar.shape ?? "") : nil
        guard image != nil || frames != nil else { return nil }
        self.avatar = avatar
        self.image = image
        self.animationFrames = frames
    }

    static func frames(shape: String) -> [NSImage]? { atlases[shape] }

    private static let atlases: [String: [NSImage]] = {
        let resources = Bundle.main.url(forResource: "RelayDesktop_RelayDesktop", withExtension: "bundle")
            .flatMap { Bundle(url: $0) } ?? .module
        var result: [String: [NSImage]] = [:]
        for shape in ["blob", "pebble", "bean", "egg", "squircle", "tablet", "capsule", "cylinder", "hex", "gem", "crystal", "wedge", "shield", "dome", "arch", "cloud", "teardrop", "leaf", "square", "sparkle", "clover", "flower", "heart", "house", "star"] {
            guard let url = resources.url(forResource: "session-avatar-\(shape)", withExtension: "png"),
                  let data = try? Data(contentsOf: url), let bitmap = NSBitmapImageRep(data: data),
                  let atlas = bitmap.cgImage else { continue }
            let frames = (0..<32).compactMap { index -> NSImage? in
                guard let image = atlas.cropping(to: CGRect(x: index % 8 * 64 + 8, y: index / 8 * 64 + 8, width: 48, height: 48)) else { return nil }
                let frame = NSImage(cgImage: image, size: NSSize(width: 48, height: 48))
                frame.isTemplate = true
                return frame
            }
            if frames.count == 32 { result[shape] = frames }
        }
        return result
    }()

    static func inlineImage(_ value: String?) -> NSImage? {
        guard let value, value.count <= 512 * 1024,
              ["png", "jpeg", "gif", "webp"].contains(where: { value.hasPrefix("data:image/\($0);base64,") }),
              let encoded = value.split(separator: ",", maxSplits: 1).last,
              let data = Data(base64Encoded: String(encoded)) else { return nil }
        return NSImage(data: data)
    }

    /// The avatar's observed color, or the theme's dynamic foreground.
    var tint: NSColor {
        let values: [String: UInt32] = ["brown": 0x936439, "red": 0xFF263C, "orange": 0xFF6700,
            "yellow": 0xFF9800, "green": 0x00C972, "cyan": 0x00BCA6, "blue": 0x1084FE,
            "violet": 0x9159FE, "magenta": 0xFF309B, "gray": 0x777777]
        guard let hex = values[avatar.color ?? ""] else { return NSColor(CloudTheme.foreground) }
        return NSColor(srgbRed: CGFloat((hex >> 16) & 255) / 255, green: CGFloat((hex >> 8) & 255) / 255,
                       blue: CGFloat(hex & 255) / 255, alpha: 1)
    }

    var body: some View {
        if let image {
            RasterAvatar(image: image, animates: !reduceMotion)
        } else if let frames = animationFrames {
            FrameAvatar(frames: frames, color: tint, animates: !reduceMotion)
        }
    }
}

/// Core Animation plays the atlas frames in the render server, so an animated
/// avatar costs no main-thread updates and never invalidates the SwiftUI rows
/// around it.
private struct FrameAvatar: NSViewRepresentable {
    let frames: [NSImage]
    let color: NSColor
    let animates: Bool
    func makeNSView(context: Context) -> AvatarFramesView { AvatarFramesView() }
    func updateNSView(_ view: AvatarFramesView, context: Context) {
        view.configure(frames: frames, color: color, animates: animates)
    }
}

/// A tint-filled layer masked by the current atlas frame.
@MainActor
final class AvatarFramesView: NSView {
    static let framesPerSecond: Double = 8
    static let animationKey = "avatar-frames"

    let frameMask = CALayer()
    private var frames: [CGImage] = []
    private var color: NSColor?
    private var animates = false

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        frameMask.contentsGravity = .resizeAspect
        layer?.mask = frameMask
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    func configure(frames images: [NSImage], color: NSColor, animates: Bool) {
        let frames = images.compactMap { $0.cgImage(forProposedRect: nil, context: nil, hints: nil) }
        if color != self.color {
            self.color = color
            applyColor()
        }
        guard frames.count != self.frames.count || animates != self.animates
            || zip(frames, self.frames).contains(where: { $0 !== $1 }) else { return }
        self.frames = frames
        self.animates = animates
        applyFrames()
    }

    override func layout() {
        super.layout()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        frameMask.frame = bounds
        CATransaction.commit()
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        applyColor()
    }

    private func applyColor() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        effectiveAppearance.performAsCurrentDrawingAppearance {
            layer?.backgroundColor = color?.cgColor
        }
        CATransaction.commit()
    }

    private func applyFrames() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        frameMask.removeAnimation(forKey: Self.animationKey)
        frameMask.contents = frames.first
        CATransaction.commit()
        guard animates, frames.count > 1 else { return }
        let animation = CAKeyframeAnimation(keyPath: "contents")
        animation.values = frames
        animation.calculationMode = .discrete
        animation.duration = Double(frames.count) / Self.framesPerSecond
        animation.repeatCount = .infinity
        animation.isRemovedOnCompletion = false
        frameMask.add(animation, forKey: Self.animationKey)
    }
}

private struct RasterAvatar: NSViewRepresentable {
    let image: NSImage
    let animates: Bool
    func makeNSView(context: Context) -> NSImageView {
        let view = NSImageView()
        view.imageScaling = .scaleProportionallyUpOrDown
        return view
    }
    func updateNSView(_ view: NSImageView, context: Context) {
        view.image = image
        view.animates = animates
    }
}
