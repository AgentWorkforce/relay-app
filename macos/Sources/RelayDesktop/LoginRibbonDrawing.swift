import AppKit

/// Core Graphics draws into the view's ordinary backing store; no GPU is needed.
enum LoginRibbonDrawing {
    private static func color(_ red: CGFloat, _ green: CGFloat, _ blue: CGFloat, _ alpha: CGFloat = 1) -> CGColor {
        CGColor(red: red, green: green, blue: blue, alpha: alpha)
    }

    static func draw(_ context: CGContext, size: CGSize, time: Double) {
        guard size.width > 0, size.height > 0 else { return }
        context.saveGState()
        defer { context.restoreGState() }
        context.setFillColor(color(0.012, 0.027, 0.05))
        context.fill(CGRect(origin: .zero, size: size))
        radial(context, colors: [color(0.08, 0.19, 0.29), color(0.08, 0.19, 0.29, 0)],
               center: CGPoint(x: size.width, y: size.height / 2), radius: max(size.width, size.height))
        let aspect = size.width / size.height
        for index in 0..<9 {
            let mix = (Double(index) * 0.37).truncatingRemainder(dividingBy: 1)
            let rgb: (CGFloat, CGFloat, CGFloat) = [2, 6].contains(index) ? (0.86, 0.55, 0.42) :
                (0.20 - 0.18 * mix, 0.50 + 0.30 * mix, 0.78 + 0.18 * mix)
            func point(_ x: Double) -> CGPoint {
                CGPoint(x: x * size.width, y: (1 - ribbon(x, index: index, aspect: aspect, time: time)) * size.height)
            }
            let path = CGMutablePath()
            path.move(to: point(0))
            for step in 1...128 { path.addLine(to: point(Double(step) / 128)) }
            let halo = color(rgb.0, rgb.1, rgb.2, 0.17)
            context.saveGState()
            context.setShadow(offset: .zero, blur: 4, color: halo)
            context.setStrokeColor(halo)
            context.setLineWidth(5)
            context.addPath(path)
            context.strokePath()
            context.restoreGState()
            context.setStrokeColor(color(rgb.0, rgb.1, rgb.2, 0.36))
            context.setLineWidth(1)
            context.addPath(path)
            context.strokePath()
            let speed = 0.07 + 0.025 * (Double(index) * 0.73).truncatingRemainder(dividingBy: 1)
            let phase = 0.5 + time * speed - Double(index) * 0.41
            let x = (phase - floor(phase)) / 0.9
            if x < 1 {
                let center = point(x)
                context.saveGState()
                context.addEllipse(in: CGRect(x: center.x - 12, y: center.y - 5, width: 24, height: 10))
                context.clip()
                radial(context, colors: [color(rgb.0, rgb.1, rgb.2, 0.85), color(rgb.0, rgb.1, rgb.2, 0)],
                       center: center, radius: 12)
                context.restoreGState()
            }
        }
    }

    private static func radial(_ context: CGContext, colors: [CGColor], center: CGPoint, radius: CGFloat) {
        guard let gradient = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(), colors: colors as CFArray,
                                        locations: [0, 1]) else { return }
        context.drawRadialGradient(gradient, startCenter: center, startRadius: 0,
                                   endCenter: center, endRadius: radius, options: [])
    }

    private static func ribbon(_ x: Double, index: Int, aspect: Double, time: Double) -> Double {
        let fi = Double(index)
        let converge = x * x * (3 - 2 * x)
        let wave = 0.07 * sin(x * aspect * 1.7 + time * 0.22 + fi * 1.3)
            + 0.04 * sin(x * aspect * 3.3 - time * 0.17 + fi * 2.1)
            + 0.05 * (noise(x * aspect * 1.2 + fi * 7, time * 0.08) - 0.5)
        return (0.12 + fi * 0.095) * (1 - converge * 0.94) + 0.5 * converge * 0.94
            + wave * (1 - 0.9 * converge)
    }

    private static func noise(_ x: Double, _ y: Double) -> Double {
        func hash(_ x: Double, _ y: Double) -> Double {
            let value = sin(x * 127.1 + y * 311.7) * 43758.5453
            return value - floor(value)
        }
        let i = floor(x), j = floor(y), fx = x - i, fy = y - j
        let u = fx * fx * (3 - 2 * fx), v = fy * fy * (3 - 2 * fy)
        return (hash(i, j) * (1 - u) + hash(i + 1, j) * u) * (1 - v)
            + (hash(i, j + 1) * (1 - u) + hash(i + 1, j + 1) * u) * v
    }
}
