// Compose the runner artwork, source brand vectors and installation cue at 1x/2x.
// Usage: swift scripts/render-dmg-background.swift OUTPUT_DIR [PRODUCT_NAME [MARK_SVG]]
//            [--cobrand-mark FILE --cobrand-name NAME]
// A site profile's image names its product and draws its mark (docs/
// workspace-layout.md, "Site profiles"): the wordmark is Agent Relay's, so
// another product's name is set in text beside the mark. A co-branded
// image adds a "for" line under the lockup: the customer's mark (an SVG or
// a PNG) and name, which is never printed.
// Coordinates are in points, from the top left; keep in sync with dmg-settings.py.
import AppKit

let usage = """
Usage: swift scripts/render-dmg-background.swift OUTPUT_DIR [PRODUCT_NAME [MARK_SVG]]
           [--cobrand-mark FILE --cobrand-name NAME]
"""
let arguments = CommandLine.arguments
var positional: [String] = []
var coBrandMarkPath: String?
var coBrandName: String?
var argumentsUsable = true
var argumentIndex = 1
while argumentIndex < arguments.count {
    let argument = arguments[argumentIndex]
    if argument == "--cobrand-mark" || argument == "--cobrand-name" {
        guard argumentIndex + 1 < arguments.count else {
            argumentsUsable = false
            break
        }
        let value = arguments[argumentIndex + 1]
        if argument == "--cobrand-mark" { coBrandMarkPath = value } else { coBrandName = value }
        argumentIndex += 2
    } else {
        positional.append(argument)
        argumentIndex += 1
    }
}
guard argumentsUsable, (1...3).contains(positional.count), positional[0] != "--help",
      (coBrandMarkPath == nil) == (coBrandName == nil) else {
    print(usage)
    exit(positional.first == "--help" ? 0 : 1)
}
let output = URL(fileURLWithPath: positional[0], isDirectory: true)
let defaultProduct = "Agent Relay"
let product = positional.count >= 2 && !positional[1].isEmpty ? positional[1] : defaultProduct
let markPath = positional.count >= 3 && !positional[2].isEmpty ? positional[2] : nil
try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
guard let artwork = NSImage(contentsOf: root.appendingPathComponent("assets/dmg-background.png")) else {
    fputs("render-dmg-background: could not load the installer artwork\n", stderr)
    exit(1)
}
// Use the real wordmark geometry, recolored for the pale installer background.
let wordmarkSVG = try String(contentsOf: root.appendingPathComponent("assets/agent-relay-wordmark.svg"), encoding: .utf8)
    .replacingOccurrences(of: "#FFFFFF", with: "#202B35")
let markURL = markPath.map { URL(fileURLWithPath: $0) } ?? root.appendingPathComponent("assets/agent-relay-mark.svg")
guard let wordmark = NSImage(data: Data(wordmarkSVG.utf8)),
      let mark = NSImage(contentsOf: markURL) else {
    fputs("render-dmg-background: could not load the brand vectors\n", stderr)
    exit(1)
}
let brandColor = NSColor(srgbRed: 0.125, green: 0.169, blue: 0.208, alpha: 1)  // #202B35, the wordmark's
// The co-brand: the customer's mark and name, or neither.
var coBrand: (mark: NSImage, name: String)?
if let path = coBrandMarkPath, let name = coBrandName, !name.isEmpty {
    guard let image = NSImage(contentsOf: URL(fileURLWithPath: path)) else {
        fputs("render-dmg-background: could not load the co-brand mark\n", stderr)
        exit(1)
    }
    coBrand = (image, name)
}

let width = 768
let height = 576
for scale in [1, 2] {
    let bitmap = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: width * scale,
        pixelsHigh: height * scale, bitsPerSample: 8, samplesPerPixel: 4,
        hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
        bytesPerRow: 0, bitsPerPixel: 0)!
    bitmap.size = NSSize(width: width, height: height)
    NSGraphicsContext.saveGraphicsState()
    let graphics = NSGraphicsContext(bitmapImageRep: bitmap)!
    NSGraphicsContext.current = graphics
    // NSBitmapImageRep.size already gives this context its Retina scale.
    graphics.cgContext.translateBy(x: 0, y: CGFloat(height))
    graphics.cgContext.scaleBy(x: 1, y: -1)
    NSGraphicsContext.current = NSGraphicsContext(cgContext: graphics.cgContext, flipped: true)
    NSGraphicsContext.current?.imageInterpolation = .high

    artwork.draw(in: NSRect(x: 0, y: 0, width: width, height: height),
                 from: .zero, operation: .copy, fraction: 1,
                 respectFlipped: true, hints: nil)

    // A profile's mark is square (docs/workspace-layout.md); Agent Relay's is wider than tall.
    let markHeight = 34 * 91 / 112
    let markRect = markPath == nil
        ? NSRect(x: 42, y: 42, width: 34, height: markHeight)
        : NSRect(x: 42, y: 42 - (34 - markHeight) / 2, width: 34, height: 34)
    mark.draw(in: markRect, from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
    if product == defaultProduct {
        wordmark.draw(in: NSRect(x: 82, y: 39, width: 144, height: 144 * 54 / 264),
                      from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
    } else {
        (product as NSString).draw(
            in: NSRect(x: 82, y: 40, width: 400, height: 30),
            withAttributes: [.font: NSFont.systemFont(ofSize: 20, weight: .semibold),
                             .foregroundColor: brandColor])
    }

    // The co-brand's "for" line under the lockup, on one baseline: "for",
    // the customer's mark fitted into 16x16 and the name, 6 points apart.
    if let coBrand {
        let baseline: CGFloat = 100
        let gap: CGFloat = 6
        let forFont = NSFont.systemFont(ofSize: 13)
        let nameFont = NSFont.systemFont(ofSize: 13, weight: .medium)
        let forText = "for" as NSString
        let forAttributes: [NSAttributedString.Key: Any] = [
            .font: forFont,
            .foregroundColor: NSColor(srgbRed: 0.357, green: 0.4, blue: 0.447, alpha: 1),  // #5b6672
        ]
        forText.draw(at: NSPoint(x: 42, y: baseline - forFont.ascender), withAttributes: forAttributes)
        var x = 42 + forText.size(withAttributes: forAttributes).width + gap
        let markSize = coBrand.mark.size
        let fit = min(16 / max(markSize.width, 1), 16 / max(markSize.height, 1))
        let fitted = NSSize(width: markSize.width * fit, height: markSize.height * fit)
        // Centred on the text's cap height.
        let middle = baseline - forFont.capHeight / 2
        coBrand.mark.draw(in: NSRect(x: x + (16 - fitted.width) / 2, y: middle - fitted.height / 2,
                                     width: fitted.width, height: fitted.height),
                          from: .zero, operation: .sourceOver, fraction: 1, respectFlipped: true, hints: nil)
        x += 16 + gap
        let nameAttributes: [NSAttributedString.Key: Any] = [.font: nameFont, .foregroundColor: brandColor]
        (coBrand.name as NSString).draw(
            with: NSRect(x: x, y: baseline - nameFont.ascender, width: CGFloat(width) - 42 - x,
                         height: nameFont.ascender - nameFont.descender),
            options: [.usesLineFragmentOrigin, .truncatesLastVisibleLine],
            attributes: nameAttributes)
    }

    // Finder supplies the draggable app and Applications icons at (220, 252)
    // and (548, 252). Do not bake duplicate icons or filename labels into the art.
    let cueColor = NSColor(srgbRed: 0.57, green: 0.59, blue: 0.63, alpha: 1)
    let arrow = NSBezierPath()
    arrow.move(to: NSPoint(x: 314, y: 252))
    arrow.line(to: NSPoint(x: 434, y: 252))
    arrow.lineWidth = 1
    arrow.setLineDash([4, 6], count: 2, phase: 0)
    cueColor.setStroke()
    arrow.stroke()
    let arrowhead = NSBezierPath()
    arrowhead.move(to: NSPoint(x: 440, y: 252))
    arrowhead.line(to: NSPoint(x: 432, y: 247))
    arrowhead.line(to: NSPoint(x: 432, y: 257))
    arrowhead.close()
    cueColor.setFill()
    arrowhead.fill()

    let paragraph = NSMutableParagraphStyle()
    paragraph.alignment = .center
    ("Drag \(product) into your Applications folder" as NSString).draw(
        in: NSRect(x: 40, y: 522, width: 688, height: 26),
        withAttributes: [.font: NSFont.systemFont(ofSize: 15),
                         .foregroundColor: NSColor(srgbRed: 0.47, green: 0.49, blue: 0.53, alpha: 1),
                         .paragraphStyle: paragraph])

    NSGraphicsContext.restoreGraphicsState()
    let name = scale == 1 ? "background.png" : "background@2x.png"
    try bitmap.representation(using: .png, properties: [:])!.write(to: output.appendingPathComponent(name))
}
