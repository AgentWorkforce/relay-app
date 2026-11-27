import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

@MainActor
final class ProviderPreparationTests: XCTestCase {
    private func preparing() -> ToolIntegration {
        ToolIntegration(canInstall: false, canRepair: false, canUninstall: true,
                        detail: "You’re signed in. Agent Relay is preparing your local sessions.",
                        detected: true, id: "codex", name: "Codex", selected: true,
                        state: "preparing", statusLabel: "Preparing sessions", supported: true)
    }

    func testPreparationDoesNotAskForSignInOrSetup() {
        let tool = preparing()
        let healthStates: [String?] = [nil, "ready"]
        for health in healthStates {
            let shown = ProviderConnectionPresentation(tool, health: health)
            XCTAssertEqual(shown.status, "Preparing sessions")
            XCTAssertEqual(shown.summary, tool.detail)
            XCTAssertNil(shown.action)
            XCTAssertNil(shown.actionTitle)
        }
    }

    func testPreparationDoesNotHideActualAccountFailures() {
        let tool = preparing()
        let shown = ProviderConnectionPresentation(tool, health: "sign_in_required")
        XCTAssertEqual(shown.status, "Sign in required")
        XCTAssertEqual(shown.action, .account)
        XCTAssertEqual(ProviderConnectionPresentation(tool, health: "offline").action, .check)
    }
}