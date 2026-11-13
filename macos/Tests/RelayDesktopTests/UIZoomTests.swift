import XCTest
import SwiftUI
@testable import RelayDesktop

final class UIZoomTests: XCTestCase {
    func testTheLadderStepsInAndOutAndStopsAtItsEnds() {
        XCTAssertEqual(UIZoom.allCases.map(\.percent), [100, 110, 125, 150, 175, 200])
        XCTAssertEqual(UIZoom.actualSize.zoomedIn, .percent110)
        XCTAssertEqual(UIZoom.percent125.zoomedIn, .percent150)
        XCTAssertEqual(UIZoom.percent125.zoomedOut, .percent110)
        XCTAssertEqual(UIZoom.percent200.zoomedIn, .percent200)
        XCTAssertEqual(UIZoom.actualSize.zoomedOut, .actualSize)
        XCTAssertFalse(UIZoom.percent200.canZoomIn)
        XCTAssertTrue(UIZoom.percent200.canZoomOut)
        XCTAssertFalse(UIZoom.actualSize.canZoomOut)
        XCTAssertTrue(UIZoom.actualSize.canZoomIn)
        XCTAssertEqual(UIZoom.percent150.label, "150%")
    }

    func testActualSizeIsTheFallbackForAnythingOffTheLadder() {
        XCTAssertEqual(UIZoom(percent: 175), .percent175)
        for stray in [0, -100, 105, 130, 250] {
            XCTAssertEqual(UIZoom(percent: stray), .actualSize, "\(stray)")
        }
    }

    func testTheZoomIsKeptUnderTheKeyTheGTKAppUses() {
        XCTAssertEqual(UIZoom.defaultsKey, "uiZoom")
        for zoom in UIZoom.allCases {
            XCTAssertEqual(UIZoom(percent: zoom.percent), zoom)
        }
    }

    func testSizesScaleWithTheZoomAndHairlinesStay() {
        XCTAssertEqual(UIZoom.actualSize.factor, 1)
        XCTAssertEqual(UIZoom.percent150.factor, 1.5)
        XCTAssertEqual(UIZoom.actualSize.scale(13), 13)
        XCTAssertEqual(UIZoom.percent150.scale(12), 18)
        XCTAssertEqual(UIZoom.percent200.scale(244), 488)
        XCTAssertEqual(UIZoom.percent125.scale(10), 12.5)
        XCTAssertEqual(UIZoom.percent200.scale(1), 1, "a hairline stays a hairline")
        XCTAssertEqual(UIZoom.percent200.scale(0), 0)
        XCTAssertEqual(UIZoom.percent200.scale(-3), -3)
        XCTAssertEqual(UIZoom.percent200.scale(.infinity), .infinity)
    }

    func testTheWindowMinimumFollowsTheZoomUpToTheScreen() {
        let minimum = CGSize(width: 980, height: 650)
        XCTAssertEqual(UIZoom.actualSize.fitted(minimum, in: nil), minimum)
        XCTAssertEqual(UIZoom.percent150.fitted(minimum, in: nil), CGSize(width: 1470, height: 975))
        let screen = UIZoom.usable(CGSize(width: 1920, height: 1080))
        XCTAssertEqual(screen, CGSize(width: 1632, height: 918))
        XCTAssertEqual(UIZoom.percent125.fitted(minimum, in: screen), CGSize(width: 1225, height: 812.5))
        XCTAssertEqual(UIZoom.percent200.fitted(minimum, in: screen), screen)
        // A screen smaller than the 100% minimum keeps the 100% minimum.
        let small = UIZoom.usable(CGSize(width: 1024, height: 700))
        XCTAssertEqual(UIZoom.percent150.fitted(minimum, in: small), minimum)
    }

    func testTextStylesScaleFromTheirMacPointSizes() {
        XCTAssertEqual(UIZoom.pointSize(of: .body), 13)
        XCTAssertEqual(UIZoom.pointSize(of: .headline), 13)
        XCTAssertEqual(UIZoom.pointSize(of: .callout), 12)
        XCTAssertEqual(UIZoom.pointSize(of: .footnote), 10)
        XCTAssertEqual(UIZoom.pointSize(of: .title3), 15)
        XCTAssertEqual(UIZoom.percent200.scale(UIZoom.pointSize(of: .caption)), 20)
    }

    func testTheZoomReachesViewsThroughTheEnvironment() {
        var environment = EnvironmentValues()
        XCTAssertEqual(environment.uiZoom, .actualSize)
        environment.uiZoom = .percent175
        XCTAssertEqual(environment.uiZoom, .percent175)
    }
}
