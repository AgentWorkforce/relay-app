import Foundation
import XCTest
@testable import RelayDesktopCore

final class CoreLineFramerTests: XCTestCase {
    func testFragmentedFramesExamineEveryByteOnce() {
        for size in [64 * 1024, 512 * 1024, 16 << 20] {
            for fragment in (size > 1 << 20 ? [4093, 64 * 1024] : [1, 4093, 64 * 1024]) {
                let input = [UInt8](repeating: 0x61, count: size) + [0x0A]
                var framer = CoreLineFramer(maximumLine: 16 << 20)
                var frames: [Data] = []
                for start in stride(from: 0, to: input.count, by: fragment) {
                    input.withUnsafeBufferPointer { bytes in
                        XCTAssertTrue(framer.append(UnsafeBufferPointer(rebasing: bytes[start..<min(start + fragment, input.count)])) { frames.append($0) })
                    }
                }
                XCTAssertEqual(frames, [Data(repeating: 0x61, count: size)])
                XCTAssertEqual(framer.bytesScanned, input.count,
                               "Delimiter work must not depend on fragmentation")
            }
        }
    }

    func testMultipleFramesAndTrailingPartialAcrossReads() {
        var framer = CoreLineFramer(maximumLine: 4)
        var frames: [String] = []
        for text in ["a\n\nab", "c\nx", "yz\n"] {
            Array(text.utf8).withUnsafeBufferPointer { bytes in
                XCTAssertTrue(framer.append(bytes) { frames.append(String(decoding: $0, as: UTF8.self)) })
            }
        }
        XCTAssertEqual(frames, ["a", "", "abc", "xyz"])
        XCTAssertEqual(framer.bytesScanned, 11)
    }

    func testMaximumAppliesEvenWhenOversizedFrameEndsInSameRead() {
        for parts in [["12345\n"], ["1234", "5\n"], ["ok\n12345\n"]] {
            var framer = CoreLineFramer(maximumLine: 4)
            var frames: [Data] = []
            var accepted = true
            for part in parts {
                accepted = Array(part.utf8).withUnsafeBufferPointer { framer.append($0) { frames.append($0) } }
                if !accepted { break }
            }
            XCTAssertFalse(accepted)
            XCTAssertTrue(frames.allSatisfy { $0.count <= 4 })
        }
        var framer = CoreLineFramer(maximumLine: 4)
        XCTAssertTrue(Array("1234\n".utf8).withUnsafeBufferPointer { framer.append($0) { XCTAssertEqual($0.count, 4) } })
    }
}
