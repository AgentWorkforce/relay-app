import RelayDesktopCore
import XCTest

final class TrustedSendersCopyTests: XCTestCase {
    func testWordingMatchesLinux() {
        XCTAssertEqual(TrustedSendersCopy.title, "Trusted senders")
        XCTAssertEqual(TrustedSendersCopy.detail, "Relay names or addresses trusted like you: their messages reach your own OpenClaw and Hermes, with their memory and tools. Everyone else gets an answer-only chat of their own. Separate them with commas.")
        XCTAssertEqual(TrustedSendersCopy.placeholder, "teammate, name@machine")
        XCTAssertEqual(TrustedSendersCopy.membersToggle, "Trust people in your workspace")
        XCTAssertEqual(TrustedSendersCopy.membersDetail, "Messages they send as themselves reach your own OpenClaw and Hermes too. Their agents are not included: add the ones you trust below.")
        XCTAssertEqual(TrustedSendersCopy.memberLabel, "Workspace member")
        XCTAssertEqual(TrustedSendersCopy.membersEmpty, "No people from your workspace to show yet.")
        XCTAssertEqual(TrustedSendersCopy.othersLabel, "Other names and agents")
        XCTAssertEqual(TrustedSendersCopy.memberTitle(name: "grace-hopper", displayName: "Grace Hopper"), "Grace Hopper (grace-hopper)")
        XCTAssertEqual(TrustedSendersCopy.memberTitle(name: "ada-lovelace", displayName: nil), "ada-lovelace")
    }

    func testSettingsDecodeTheWorkspaceMembers() throws {
        let json = #"{"allow_self_registration": true, "auto_activate": false, "trusted_relay_senders": [], "trust_workspace_members": true, "workspace_members": [{"name": "grace-hopper", "display_name": "Grace Hopper"}, {"name": "ada-lovelace"}]}"#
        let settings = try JSONDecoder().decode(SettingsValues.self, from: Data(json.utf8))
        XCTAssertEqual(settings.trustWorkspaceMembers, true)
        XCTAssertEqual(settings.workspaceMembers?.map(\.name), ["grace-hopper", "ada-lovelace"])
        XCTAssertEqual(settings.workspaceMembers?.first?.displayName, "Grace Hopper")
        let change = try JSONEncoder().encode(SettingsChange(trustWorkspaceMembers: false))
        XCTAssertEqual(String(data: change, encoding: .utf8), #"{"trust_workspace_members":false}"#)
    }

    func testFieldParsesToTheList() {
        XCTAssertEqual(TrustedSendersCopy.senders(from: " ada-lovelace, ,bot@node-9 "), ["ada-lovelace", "bot@node-9"])
        XCTAssertEqual(TrustedSendersCopy.senders(from: ""), [])
    }
}
