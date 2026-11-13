import AppKit
import SwiftUI
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

@MainActor
final class ToolsModelTests: XCTestCase {
    func testReopeningChooserPreservesChoicesAndNotNowDoesNotSave() async {
        let core = FakeCore()
        defer { core.stop() }
        core.on("tools.status") { _ in FakeCore.toolsReport(tools: [self.tool(selected: true), self.tool("cursor")]) }
        let tools = model(core)
        await tools.refresh()
        XCTAssertFalse(tools.shouldChooseTools)
        tools.requestOnboarding()
        XCTAssertTrue(tools.shouldChooseTools)
        XCTAssertEqual(tools.selectedIDs, ["codex"])
        tools.setSelected("cursor", true)
        await tools.refresh()
        XCTAssertEqual(tools.selectedIDs, ["codex", "cursor"], "Passive status keeps the reopened draft")
        await tools.completeOnboarding(skip: true)
        XCTAssertFalse(tools.shouldChooseTools)
        XCTAssertEqual(core.callCount("tools.select"), 0)
        tools.requestOnboarding()
        XCTAssertEqual(tools.selectedIDs, ["codex"], "Cancel leaves the saved choices unchanged")
    }
    func testReopenedContinuePreservesHiddenChoicesButRemovesDependentHiddenHosts() async {
        let core = FakeCore()
        defer { core.stop() }
        var hidden = tool("opencode", selected: true); hidden["detected"] = false
        var cloud = tool("replicas", supported: false, selected: true); cloud["detected"] = false
        var host = tool("paseo", supported: false, selected: true); host["detected"] = false
        let rows = [tool(selected: true), tool("cursor"), hidden, cloud, host]
        core.on("tools.status") { _ in FakeCore.toolsReport(tools: rows) }
        core.on("tools.select") { _ in FakeCore.toolsReport() }
        let tools = model(core)
        await tools.refresh()
        tools.requestOnboarding()
        tools.setSelected("codex", false)
        tools.setSelected("cursor", true)
        await tools.refresh()
        await tools.completeOnboarding(skip: false)
        XCTAssertEqual(core.lastParams("tools.select")?["tool_ids"] as? [String], ["cursor", "opencode", "replicas"])
    }

    func testReopenedChooserMovesSavedChoicesWhenDetectionChanges() async {
        let core = FakeCore()
        defer { core.stop() }
        var hidden = tool("opencode", selected: true); hidden["detected"] = false
        core.on("tools.status") { _ in FakeCore.toolsReport(tools: [hidden]) }
        core.on("tools.select") { _ in FakeCore.toolsReport() }
        let tools = model(core)
        await tools.refresh()
        tools.requestOnboarding()
        core.on("tools.status") { _ in FakeCore.toolsReport(tools: [self.tool("opencode", selected: true)]) }
        await tools.refresh()
        XCTAssertEqual(tools.selectedIDs, ["opencode"], "A newly visible saved choice is checked")
        tools.setSelected("opencode", false)
        await tools.completeOnboarding(skip: false)
        XCTAssertEqual(core.lastParams("tools.select")?["tool_ids"] as? [String], [])
    }

    func testProviderOnboardingDoesNotWaitForInitialHistoryScan() {
        XCTAssertTrue(AccountStatus(state: .signedIn).canChooseProviders)
        XCTAssertTrue(AccountStatus(phase: "preparing_local_sessions", state: .preparing).canChooseProviders)
        XCTAssertFalse(AccountStatus(phase: "connecting_workspace", state: .preparing).canChooseProviders)
        XCTAssertFalse(AccountStatus(state: .pendingApproval).canChooseProviders)
        XCTAssertFalse(AccountStatus(phase: "preparing_local_sessions", state: .signedOut).canChooseProviders)
    }

    func testCursorIsSelectableAndSubmittedWithTheProviderChoices() async {
        let core = FakeCore()
        defer { core.stop() }
        core.on("tools.status") { _ in FakeCore.toolsReport(onboardingComplete: false, tools: [self.tool("cursor")]) }
        core.on("tools.select") { _ in FakeCore.toolsReport(tools: [self.tool("cursor", selected: true)]) }
        let tools = model(core)
        await tools.refresh()
        XCTAssertTrue(tools.canChoose(tools.report!.tools[0]))
        tools.setSelected("cursor", true)
        await tools.completeOnboarding(skip: false)
        XCTAssertEqual(core.lastParams("tools.select")?["tool_ids"] as? [String], ["cursor"])
    }

    func testCloudSetupActionsDoNotImplyPresenceOrLocalMCP() async {
        let core = FakeCore()
        defer { core.stop() }
        let external = ["capy", "grok-bot", "dot"].map { id -> [String: Any] in
            var row = tool(id, supported: false)
            row["detected"] = false
            return row
        }
        core.on("tools.status") { _ in FakeCore.toolsReport(onboardingComplete: false, tools: external) }
        core.on("tools.select") { _ in FakeCore.toolsReport() }
        let tools = model(core)
        await tools.refresh()
        XCTAssertEqual(Set(tools.onboardingTools.map(\.id)), ["grok-bot", "dot"])
        XCTAssertTrue(tools.detectedTools.isEmpty)
        for id in ["capy", "grok-bot", "dot"] { tools.setSelected(id, true) }
        await tools.completeOnboarding(skip: false)
        XCTAssertEqual(core.lastParams("tools.select")?["tool_ids"] as? [String], [])
    }

    func testAbsentAmpAndCapyAreNotOfferedByTheirCatalogActions() async {
        let core = FakeCore()
        defer { core.stop() }
        var ampRow = tool("amp", supported: false)
        ampRow["detected"] = false
        ampRow["state"] = "external_setup"
        var capyRow = tool("capy", supported: false)
        capyRow["detected"] = false
        capyRow["state"] = "external_setup"
        let rows = [ampRow, capyRow, tool("copilot"), tool("claude", supported: false)]
        core.on("tools.status") { _ in FakeCore.toolsReport(onboardingComplete: false, tools: rows) }
        let tools = model(core)
        await tools.refresh()
        func row(_ id: String) -> ToolIntegration { tools.report!.tools.first { $0.id == id }! }
        XCTAssertEqual(Set(tools.onboardingTools.map(\.id)), ["copilot", "claude"])
        XCTAssertFalse(ToolsModel.isChooserEligible(row("amp")))
        XCTAssertTrue(ToolsModel.isChooserEligible(row("copilot")))
        XCTAssertEqual(tools.eligibleIDs, ["copilot"])
        // Amp's row opens Cloud connections instead of offering a checkbox;
        // an eligible local provider's row is its checkbox.
        XCTAssertEqual(ToolsModel.onboardingAction(for: row("amp")), .openCloudConnections)
        XCTAssertEqual(ToolsModel.onboardingAction(for: row("copilot")), .checkbox)
        XCTAssertEqual(ToolsModel.onboardingAction(for: row("capy")), .cloudSetup(.capy))
        XCTAssertEqual(ToolsModel.onboardingAction(for: row("claude")), .noChoice)
    }

    func testSetupCapabilitiesAloneDoNotOfferAbsentAmpOrCapy() async {
        let core = FakeCore()
        defer { core.stop() }
        let rows = ["amp", "capy"].map { id -> [String: Any] in
            var candidate = tool(id, supported: true)
            candidate["detected"] = false
            candidate["state"] = "external_setup"
            candidate["can_repair"] = true
            candidate["can_uninstall"] = true
            return candidate
        }
        core.on("tools.status") { _ in FakeCore.toolsReport(onboardingComplete: false, tools: rows) }
        let tools = model(core)
        await tools.refresh()
        XCTAssertTrue(tools.onboardingTools.isEmpty)
        XCTAssertTrue(tools.eligibleIDs.isEmpty)
        XCTAssertEqual(core.callCount("tools.select"), 0)
        XCTAssertEqual(core.callCount("tools.manage"), 0)
    }

    func testOnboardingRequiresAnInstalledWitnessDespiteOldDetectionAndChoices() async {
        let core = FakeCore()
        defer { core.stop() }
        let tools = model(core)
        for id in ["amp", "capy"] {
            for evidence in ["detected", "selected", "configured"] {
                var candidate = tool(id, supported: false, selected: evidence == "selected")
                candidate["detected"] = evidence == "detected"
                candidate["state"] = evidence == "configured" ? "configured" : "external_setup"
                core.on("tools.status") { _ in FakeCore.toolsReport(onboardingComplete: false, tools: [candidate]) }
                await tools.refresh()
                XCTAssertTrue(tools.onboardingTools.isEmpty, "\(id): \(evidence) is not installation")
                candidate["onboarding_available"] = false
                core.on("tools.status") { _ in FakeCore.toolsReport(onboardingComplete: false, tools: [candidate]) }
                await tools.refresh()
                XCTAssertTrue(tools.onboardingTools.isEmpty)
                candidate["onboarding_available"] = true
                core.on("tools.status") { _ in FakeCore.toolsReport(onboardingComplete: false, tools: [candidate]) }
                await tools.refresh()
                XCTAssertEqual(tools.onboardingTools.map(\.id), [id], "Core installation witness admits the row")
                XCTAssertTrue(tools.eligibleIDs.isEmpty, "Cloud setup never becomes a local MCP choice")
            }
        }
        XCTAssertEqual(core.callCount("tools.select"), 0)
        XCTAssertEqual(core.callCount("tools.manage"), 0)
    }

    func testContinueKeepsOnlyExistingHiddenCloudChoicesAndNeverAddsAbsentDefaults() async {
        let core = FakeCore()
        defer { core.stop() }
        var selected = tool("capy", supported: false, selected: true)
        selected["onboarding_available"] = false
        var absent = tool("amp", supported: true)
        absent["onboarding_available"] = false
        absent["state"] = "configured"
        core.on("tools.status") { _ in FakeCore.toolsReport(onboardingComplete: false,
            tools: [selected, absent, self.tool("codex", selected: true)]) }
        core.on("tools.select") { _ in FakeCore.toolsReport() }
        let tools = model(core)
        await tools.refresh()
        XCTAssertEqual(tools.onboardingTools.map(\.id), ["codex"])
        XCTAssertEqual(tools.eligibleIDs, ["codex"])
        tools.setSelected("amp", true)
        XCTAssertEqual(tools.selectedIDs, ["codex"])
        await tools.completeOnboarding(skip: false)
        XCTAssertEqual(core.lastParams("tools.select")?["tool_ids"] as? [String], ["capy", "codex"],
                       "Retain only the core's old choice without silently adding the absent default")
        selected["onboarding_available"] = true
        core.on("tools.status") { _ in FakeCore.toolsReport(onboardingComplete: false,
            tools: [selected, absent, self.tool("codex", selected: true)]) }
        await tools.refresh()
        await tools.completeOnboarding(skip: false)
        XCTAssertEqual(core.lastParams("tools.select")?["tool_ids"] as? [String], ["capy", "codex"],
                       "An installed Cloud setup row is not a deselection checkbox either")
    }

    nonisolated private func tool(_ id: String = "codex", supported: Bool = true, selected: Bool = false) -> [String: Any] {
        ["id": id, "name": id.capitalized, "detected": true, "supported": supported,
         "selected": selected, "state": "not_installed", "status_label": "Not installed",
         "detail": "Configuration does not verify that a running chat has loaded tools.",
         "can_install": supported, "can_repair": supported, "can_uninstall": supported]
    }

    private func model(_ core: FakeCore) -> ToolsModel {
        ToolsModel(socketPath: core.path, requestTimeout: 0.5)
    }

    func testChoosingToolsRequiresExplicitContinueAndExcludesUnsupportedTools() async {
        let core = FakeCore()
        defer { core.stop() }
        core.on("tools.status") { _ in FakeCore.toolsReport(onboardingComplete: false, tools: [self.tool(), self.tool("opencode", supported: false)]) }
        core.on("tools.select") { _ in FakeCore.toolsReport(tools: [self.tool(selected: true)]) }
        let tools = model(core)
        await tools.load(identity: "account/workspace")
        XCTAssertTrue(tools.shouldChooseTools)
        XCTAssertTrue(core.allParams("tools.select").isEmpty)
        XCTAssertTrue(core.allParams("tools.manage").isEmpty)
        tools.setSelected("codex", true)
        tools.setSelected("opencode", true)
        await tools.completeOnboarding(skip: false)
        XCTAssertEqual(core.lastParams("tools.select")?["tool_ids"] as? [String], ["codex"])
        XCTAssertFalse(tools.shouldChooseTools)
    }

    func testNotNowPersistsAnExplicitEmptySelection() async {
        let core = FakeCore()
        defer { core.stop() }
        core.on("tools.status") { _ in FakeCore.toolsReport(onboardingComplete: false, tools: [self.tool(selected: true)]) }
        core.on("tools.select") { _ in FakeCore.toolsReport() }
        let tools = model(core)
        await tools.refresh()
        await tools.completeOnboarding(skip: true)
        XCTAssertEqual(core.lastParams("tools.select")?["tool_ids"] as? [String], [])
        XCTAssertFalse(tools.shouldChooseTools)
    }

    func testHostChoicesStayInDraftAndAreSentOnContinue() async {
        let core = FakeCore()
        defer { core.stop() }
        let hosts = [tool(), tool("conductor", supported: false), tool("paseo", supported: false), tool("capy", supported: false)]
        core.on("tools.status") { _ in FakeCore.toolsReport(onboardingComplete: false, tools: hosts) }
        core.on("tools.select") { _ in FakeCore.toolsReport() }
        let tools = model(core)
        await tools.refresh()
        XCTAssertEqual(tools.eligibleIDs, ["codex", "conductor", "paseo"])
        XCTAssertTrue(tools.canChoose(tools.report!.tools.first { $0.id == "conductor" }!))
        XCTAssertFalse(tools.canChoose(tools.report!.tools.first { $0.id == "capy" }!))
        tools.setSelected("conductor", true)
        tools.setSelected("paseo", true)
        tools.setSelected("capy", true)
        await tools.refresh()
        XCTAssertEqual(tools.selectedIDs, ["codex", "conductor", "paseo"])
        XCTAssertEqual(core.callCount("tools.select"), 0)
        tools.setSelected("conductor", false)
        tools.setSelected("codex", false)
        XCTAssertTrue(tools.selectedIDs.isEmpty, "Deselecting a provider clears the hosts that require it")
        tools.setSelected("paseo", true)
        await tools.completeOnboarding(skip: false)
        XCTAssertEqual(core.lastParams("tools.select")?["tool_ids"] as? [String], ["codex", "paseo"])
    }

    func testDetectedSelectedHostCanBeDeselectedWithoutRemovingItsProviders() async {
        let core = FakeCore()
        defer { core.stop() }
        core.on("tools.status") { _ in FakeCore.toolsReport(tools: [self.tool("paseo", supported: false, selected: true), self.tool(selected: true)]) }
        core.on("tools.manage") { _ in FakeCore.toolsReport(tools: [self.tool("paseo", supported: false), self.tool(selected: true)]) }
        let tools = model(core)
        await tools.refresh()
        XCTAssertTrue(tools.actions(for: tools.report!.tools[0]).contains("disable"))
        await tools.manage("paseo", action: "disable")
        XCTAssertEqual(core.lastParams("tools.manage")?["action"] as? String, "disable")
        XCTAssertFalse(tools.report!.tools[0].selected)
        XCTAssertTrue(tools.report!.tools[1].selected)
        XCTAssertTrue(tools.actions(for: tools.report!.tools[0]).contains("enable"))
    }

    func testUnavailableStatusCanRetryWithoutBlockingTheNormalApp() async {
        let core = FakeCore()
        defer { core.stop() }
        core.on("tools.status") { _ in throw FakeCore.Failure(kind: "method_not_found", message: "Older core") }
        let tools = model(core)
        await tools.refresh()
        XCTAssertFalse(tools.shouldChooseTools)
        XCTAssertNotNil(tools.error)
        core.on("tools.status") { _ in FakeCore.toolsReport(onboardingComplete: false, tools: [self.tool()]) }
        await tools.retry()
        XCTAssertTrue(tools.shouldChooseTools)
        XCTAssertNil(tools.error)
    }

    func testFailedChoiceStaysVisibleUntilExplicitEscapeAndRetainsDraftForRetry() async {
        let core = FakeCore()
        defer { core.stop() }
        core.on("tools.status") { _ in FakeCore.toolsReport(onboardingComplete: false, tools: [self.tool()]) }
        core.on("tools.select") { _ in throw FakeCore.Failure(kind: "store_failed", message: "Could not save") }
        let tools = model(core)
        await tools.refresh()
        tools.setSelected("codex", true)
        await tools.completeOnboarding(skip: false)
        XCTAssertTrue(tools.shouldChooseTools)
        XCTAssertNotNil(tools.error)
        tools.continueToApp()
        XCTAssertFalse(tools.shouldChooseTools)
        XCTAssertNotNil(tools.error)
        await tools.retry()
        XCTAssertTrue(tools.shouldChooseTools)
        XCTAssertEqual(tools.selectedIDs, ["codex"])
    }

    func testFailedUninstallKeepsConfirmedStateAndCheckHealthUsesTheCore() async {
        let core = FakeCore()
        defer { core.stop() }
        core.on("tools.status") { _ in FakeCore.toolsReport(tools: [self.tool(selected: true)]) }
        core.on("tools.manage") { _ in throw FakeCore.Failure(kind: "refused", message: "Changed configuration") }
        core.on("tools.check") { _ in FakeCore.toolsReport(tools: [self.tool(selected: true)]) }
        let tools = model(core)
        await tools.refresh()
        await tools.manage("codex", action: "uninstall")
        XCTAssertTrue(tools.report?.tools.first?.selected == true)
        XCTAssertNotNil(tools.error)
        XCTAssertEqual(core.lastParams("tools.manage")?["action"] as? String, "uninstall")
        await tools.refresh()
        XCTAssertNotNil(tools.error, "Automatic status refresh must not hide an action failure")
        await tools.checkHealth()
        XCTAssertEqual(core.callCount("tools.check"), 1)
        XCTAssertNil(tools.error)
    }

    func testUnsupportedLegacyToolCanStillBeUninstalled() async {
        let core = FakeCore()
        defer { core.stop() }
        var legacy = tool("opencode", supported: false, selected: true)
        legacy["can_uninstall"] = true
        core.on("tools.status") { _ in FakeCore.toolsReport(tools: [legacy]) }
        let tools = model(core)
        await tools.refresh()
        XCTAssertEqual(tools.actions(for: tools.report!.tools[0]), ["uninstall", "disable"])
    }

    func testResponseForAnOldAccountCannotOpenOnboardingForTheNewAccount() async {
        let core = FakeCore()
        defer { core.stop() }
        let entered = expectation(description: "old request entered")
        core.on("tools.status") { _ in
            entered.fulfill()
            Thread.sleep(forTimeInterval: 0.1)
            return FakeCore.toolsReport(onboardingComplete: false, tools: [self.tool()])
        }
        let tools = model(core)
        let pending = Task { await tools.load(identity: "old") }
        await fulfillment(of: [entered], timeout: 1)
        tools.reset()
        await pending.value
        XCTAssertNil(tools.report)
        XCTAssertFalse(tools.shouldChooseTools)
        XCTAssertFalse(tools.isWorking)
    }

    func testLongCatalogScrollsWhileTheChooserFooterFitsTheMinimumWindow() async {
        let core = FakeCore()
        defer { core.stop() }
        core.on("tools.status") { _ in FakeCore.toolsReport(onboardingComplete: false, tools: (0..<40).map { self.tool("tool-\($0)") }) }
        let tools = model(core)
        await tools.refresh()
        let app = AppModel(client: CoreClient(socketPath: core.path), manageSystemServices: false)
        defer { app.shutdown() }
        let host = NSHostingView(rootView: ChooseToolsView(tools: tools).environment(app))
        host.frame = NSRect(x: 0, y: 0, width: 620, height: 540)
        host.layoutSubtreeIfNeeded()
        XCTAssertLessThanOrEqual(host.fittingSize.width, 980)
        XCTAssertLessThanOrEqual(host.fittingSize.height, 650)
        XCTAssertTrue(containsScrollView(host), "The catalog scrolls independently of the footer")
    }

    private func containsScrollView(_ view: NSView) -> Bool {
        view is NSScrollView || view.subviews.contains(where: containsScrollView)
    }
}
