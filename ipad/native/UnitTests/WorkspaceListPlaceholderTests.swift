import XCTest
@testable import AgentRelayiPad

/// An empty list during a slow first load says it is loading; once the roster
/// has answered, an empty list really is empty.
final class WorkspaceListPlaceholderTests: XCTestCase {
    func testEmptyListBeforeTheFirstRosterSaysLoading() {
        XCTAssertEqual(WorkspaceListPlaceholder.text(rosterLoaded: false), "Loading…")
    }

    func testEmptyListAfterTheRosterAnsweredSaysEmpty() {
        XCTAssertEqual(WorkspaceListPlaceholder.text(rosterLoaded: true), "No agents in this view")
    }
}
