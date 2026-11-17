@testable import RelayDesktopCore
import XCTest

final class OrganizationPolicyCopyTests: XCTestCase {
    private let policy = OrganizationPolicyState(
        defaults: ["auto_join_general"],
        locked: ["spawn_max_agents", "analytics_enabled"],
        managed: false,
        mismatched: ["accept_spawns"]
    )

    func testWordingMatchesLinux() {
        XCTAssertEqual(OrganizationPolicyCopy.locked, "This setting is set by your organization.")
        XCTAssertEqual(OrganizationPolicyCopy.managed, "Managed by your organization.")
        XCTAssertEqual(OrganizationPolicyCopy.mismatched, "Your organization turns this on only on computers it enrolls. This computer keeps your own choice.")
        XCTAssertEqual(OrganizationPolicyCopy.held, "Your organization holds messages from senders who are not its admins. Approve each message as it arrives.")
        XCTAssertEqual(OrganizationPolicyCopy.approvalsTitle, "Message approvals")
        XCTAssertEqual(OrganizationPolicyCopy.repositoriesTitle, "Repositories that join the relay")
        XCTAssertEqual(OrganizationPolicyCopy.repositoriesDetail, "Sessions working in these repositories join the relay by themselves.")
    }

    func testEachSettingGetsItsOwnNote() {
        XCTAssertEqual(OrganizationPolicyCopy.note(policy, "spawn_max_agents"), OrganizationPolicyCopy.locked)
        XCTAssertEqual(OrganizationPolicyCopy.note(policy, "auto_join_general"), OrganizationPolicyCopy.managed)
        XCTAssertEqual(OrganizationPolicyCopy.note(policy, "accept_spawns"), OrganizationPolicyCopy.mismatched)
        XCTAssertNil(OrganizationPolicyCopy.note(policy, "spawn_native_input"))
        XCTAssertNil(OrganizationPolicyCopy.note(nil, "spawn_max_agents"))
        XCTAssertEqual(
            OrganizationPolicyCopy.note(policy, any: ["spawn_full_access_codex", "spawn_max_agents"]),
            OrganizationPolicyCopy.locked
        )
    }

    func testOnlyALockKeepsThePersonFromChangingIt() {
        XCTAssertTrue(OrganizationPolicyCopy.isLocked(policy, "analytics_enabled"))
        XCTAssertFalse(OrganizationPolicyCopy.isLocked(policy, "auto_join_general"))
        XCTAssertFalse(OrganizationPolicyCopy.isLocked(policy, "accept_spawns"))
        XCTAssertFalse(OrganizationPolicyCopy.isLocked(nil, "analytics_enabled"))
        XCTAssertTrue(OrganizationPolicyCopy.isLocked(policy, any: ["x", "spawn_max_agents"]))
    }

    func testTheRepositoriesListReadsAsOneLine() {
        XCTAssertEqual(OrganizationPolicyCopy.repositories(["acme/web", "tools/*"]), "acme/web, tools/*")
        XCTAssertEqual(OrganizationPolicyCopy.repositories([]), "None")
        XCTAssertEqual(OrganizationPolicyCopy.repository("acme/web"), "Repository: acme/web")
    }
}
