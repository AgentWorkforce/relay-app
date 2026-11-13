import AppKit
import XCTest
@testable import RelayDesktop

final class LoginRibbonDrawingTests: XCTestCase {
    func testRibbonsDrawAndAnimateWithoutAGPU() throws {
        let first = try frame(time: 14)
        XCTAssertEqual(first, try frame(time: 14), "A paused frame must remain still")
        XCTAssertNotEqual(first, try frame(time: 15), "The animation must keep moving")
        XCTAssertGreaterThan(Set(stride(from: 0, to: first.count, by: 4).map { first[$0] }).count, 8,
                             "The CPU image must include the gradient and ribbons")
        XCTAssertTrue(stride(from: 3, to: first.count, by: 4).allSatisfy { first[$0] == 255 })
    }

    private func frame(time: Double) throws -> Data {
        let width = 320, height = 240
        let context = try XCTUnwrap(CGContext(data: nil, width: width, height: height,
            bitsPerComponent: 8, bytesPerRow: width * 4, space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue))
        LoginRibbonDrawing.draw(context, size: CGSize(width: width, height: height), time: time)
        return Data(bytes: try XCTUnwrap(context.data), count: width * height * 4)
    }
}
