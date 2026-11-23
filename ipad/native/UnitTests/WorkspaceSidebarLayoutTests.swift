import XCTest
@testable import AgentRelayiPad

final class WorkspaceSidebarLayoutTests: XCTestCase {
    private func defaults() -> UserDefaults {
        let defaults = UserDefaults(suiteName: "WorkspaceSidebarLayoutTests")!
        defaults.removePersistentDomain(forName: "WorkspaceSidebarLayoutTests")
        return defaults
    }

    func testExpandedShowsTheLeftSideAndTheToggle() {
        let layout = WorkspaceSidebarLayout(width: 1024, collapsed: false)
        XCTAssertFalse(layout.isCompact)
        XCTAssertTrue(layout.showsLeftSide)
        XCTAssertTrue(layout.showsToggle)
        XCTAssertEqual(layout.sidebarWidth, 285)
    }

    func testCollapsedHidesTheLeftSideButKeepsTheToggle() {
        let layout = WorkspaceSidebarLayout(width: 1024, collapsed: true)
        XCTAssertFalse(layout.isCompact)
        XCTAssertFalse(layout.showsLeftSide)
        XCTAssertTrue(layout.showsToggle)
    }

    func testCompactWidthIgnoresTheCollapsedChoice() {
        for collapsed in [false, true] {
            let layout = WorkspaceSidebarLayout(width: 390, collapsed: collapsed)
            XCTAssertTrue(layout.isCompact)
            XCTAssertFalse(layout.showsLeftSide)
            XCTAssertFalse(layout.showsToggle)
        }
        XCTAssertEqual(WorkspaceSidebarLayout(width: 390, collapsed: false),
                       WorkspaceSidebarLayout(width: 390, collapsed: true))
    }

    func testPhoneLayoutStopsAt700Points() {
        XCTAssertTrue(WorkspaceSidebarLayout(width: 699.5, collapsed: false).isCompact)
        let regular = WorkspaceSidebarLayout(width: 700, collapsed: false)
        XCTAssertFalse(regular.isCompact)
        XCTAssertTrue(regular.showsLeftSide)
        XCTAssertTrue(regular.showsToggle)
    }

    func testSidebarWidensAt850Points() {
        XCTAssertEqual(WorkspaceSidebarLayout(width: 700, collapsed: false).sidebarWidth, 245)
        XCTAssertEqual(WorkspaceSidebarLayout(width: 849.5, collapsed: false).sidebarWidth, 245)
        XCTAssertEqual(WorkspaceSidebarLayout(width: 850, collapsed: false).sidebarWidth, 285)
    }

    func testTogglingPersistsAndFlipsBack() {
        let defaults = defaults()
        XCTAssertFalse(WorkspaceSidebarLayout.storedCollapsed(in: defaults))
        XCTAssertTrue(WorkspaceSidebarLayout.toggleStored(in: defaults))
        XCTAssertTrue(WorkspaceSidebarLayout.storedCollapsed(in: defaults))
        XCTAssertFalse(WorkspaceSidebarLayout(width: 1024, collapsed: WorkspaceSidebarLayout.storedCollapsed(in: defaults)).showsLeftSide)
        XCTAssertFalse(WorkspaceSidebarLayout.toggleStored(in: defaults))
        XCTAssertFalse(WorkspaceSidebarLayout.storedCollapsed(in: defaults))
        XCTAssertTrue(WorkspaceSidebarLayout(width: 1024, collapsed: WorkspaceSidebarLayout.storedCollapsed(in: defaults)).showsLeftSide)
    }
}
