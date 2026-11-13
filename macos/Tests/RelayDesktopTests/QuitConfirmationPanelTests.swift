import AppKit
import XCTest
@testable import RelayDesktop

@MainActor
final class QuitConfirmationPanelTests: XCTestCase {
    /// Drive the real modal loop, with a deadline so a broken action fails
    /// rather than leaving the test runner in a quit dialog.
    private func response(to action: @escaping (QuitConfirmationPanel) -> Void) -> NSApplication.ModalResponse {
        _ = NSApplication.shared
        let panel = QuitConfirmationPanel()
        let deadline = DispatchWorkItem { NSApp.stopModal(withCode: .abort) }
        DispatchQueue.main.asyncAfter(deadline: .now() + 3, execute: deadline)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { action(panel) }
        defer { deadline.cancel() }
        return panel.runConfirmation()
    }

    private func press(_ panel: NSPanel, key: String, code: UInt16) {
        panel.sendEvent(NSEvent.keyEvent(with: .keyDown, location: .zero,
            modifierFlags: [], timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: panel.windowNumber, context: nil, characters: key,
            charactersIgnoringModifiers: key, isARepeat: false, keyCode: code)!)
    }

    func testReturnKeepsAgentsConnected() {
        XCTAssertEqual(response { self.press($0, key: "\r", code: 36) }, .alertFirstButtonReturn)
    }

    func testEscapeAndWindowCloseCancelQuitting() {
        XCTAssertEqual(response { self.press($0, key: "\u{1b}", code: 53) }, .cancel)
        XCTAssertEqual(response { $0.performClose(nil) }, .cancel)
    }

    func testPrimaryActionUsesBrandBlueInBothAppearances() throws {
        _ = NSApplication.shared
        for appearance in [NSAppearance.Name.aqua, .darkAqua] {
            let panel = QuitConfirmationPanel()
            panel.appearance = NSAppearance(named: appearance)
            let host = try XCTUnwrap(panel.contentView)
            host.layoutSubtreeIfNeeded()
            let bitmap = try SessionRenderFixture.bitmap(for: host)
            if let directory = ProcessInfo.processInfo.environment["QUIT_CONFIRMATION_RENDER_DIR"] {
                let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
                try data.write(to: URL(fileURLWithPath: directory).appendingPathComponent("quit-\(appearance.rawValue).png"))
            }
            var brandPixels = 0
            for y in 0..<bitmap.pixelsHigh {
                for x in 0..<bitmap.pixelsWide {
                    guard let color = bitmap.colorAt(x: x, y: y),
                          abs(color.redComponent - 45 / 255.0) < 0.01,
                          abs(color.greenComponent - 106 / 255.0) < 0.01,
                          abs(color.blueComponent - 156 / 255.0) < 0.01 else { continue }
                    brandPixels += 1
                }
            }
            XCTAssertGreaterThan(brandPixels, 5_000, "The primary action must use the shared brand fill in \(appearance)")
        }
    }
}
