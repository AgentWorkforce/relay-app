import AppKit
import SwiftUI
import XCTest
@testable import RelayDesktop
@testable import RelayDesktopCore

@MainActor
final class ProvidersTests: XCTestCase {
    nonisolated private func provider(_ id: String, selected: Bool = true, state: String = "configured", supported: Bool = true, label: String? = nil, detected: Bool = true, name: String? = nil) -> ToolIntegration {
        ToolIntegration(canInstall: supported && !selected, canRepair: supported && selected,
                        canUninstall: selected, detail: "Configuration does not verify that a running chat has loaded its MCP tools.",
                        detected: detected, id: id, name: name ?? (id == "claude" ? "Claude Code" : AgentIconView.name(for: id)), selected: selected,
                        state: state, statusLabel: label ?? (state == "configured" ? "Configured" : "Unavailable"), supported: supported)
    }

    nonisolated private func fixture(_ providers: [ToolIntegration], health: String? = "ready") throws -> Any {
        let report = ToolsReport(accountHealth: health, onboardingComplete: true, tools: providers)
        return try JSONSerialization.jsonObject(with: JSONEncoder().encode(report))
    }

    func testAuthenticatedPreparationDoesNotAskForSignInAgain() {
        let tool = provider("codex", state: "signed_out")
        let preparing = AccountStatus(phase: "preparing_local_sessions", state: .preparing)
        XCTAssertEqual(tool.onboardingDetail(account: preparing), "Choose this provider now. Setup will finish after your local sessions are prepared.")
        XCTAssertEqual(tool.onboardingDetail(account: AccountStatus(state: .signedOut)), tool.detail)
    }

    func testCursorPreservesCoreAvailability() {
        let cursor = provider("cursor", selected: false, state: "external_setup", supported: false, label: "Unavailable on Windows")
        let shown = ProviderConnectionPresentation(cursor, health: "ready")
        XCTAssertEqual(shown.status, cursor.statusLabel)
        XCTAssertEqual(shown.summary, cursor.detail)
        XCTAssertNil(shown.action)
    }

    func testCopilotReadySetupOffersConnectionAndRoutesItsAction() async throws {
        let copilot = provider("copilot", selected: false, state: "onboarding_needed", supported: true, label: "Choose providers", name: "GitHub Copilot (VS Code)")
        let shown = ProviderConnectionPresentation(copilot, health: "ready")
        XCTAssertEqual(shown.action, .install)
        XCTAssertEqual(shown.actionTitle, "Connect GitHub Copilot (VS Code)")
        let core = FakeCore()
        defer { core.stop() }
        core.on("tools.status") { _ in try self.fixture([copilot]) }
        core.on("tools.manage") { _ in try self.fixture([self.provider("copilot", state: "installed", label: "Setup saved")]) }
        let model = ToolsModel(socketPath: core.path, requestTimeout: 0.5)
        await model.refresh()
        await model.manage("copilot", action: "install")
        XCTAssertEqual(core.lastParams("tools.manage")?["tool_id"] as? String, "copilot")
        XCTAssertEqual(core.lastParams("tools.manage")?["action"] as? String, "install")
    }

    func testCopilotPreservesInstalledAndMessagingUnavailableStatus() {
        let installed = ProviderConnectionPresentation(provider("copilot", state: "installed", label: "Setup saved"), health: "ready")
        XCTAssertEqual(installed.status, "Setup saved")
        XCTAssertNil(installed.action)
        let unavailable = ProviderConnectionPresentation(provider("copilot", state: "action_required", label: "Messaging unavailable"), health: "ready")
        XCTAssertEqual(unavailable.status, "Messaging unavailable")
        XCTAssertNil(unavailable.action)
    }

    func testAmpShowsItsCloudConnectionWithoutLocalActions() {
        let amp = provider("amp", state: "external_setup", supported: false, label: "Connects through Agent Relay Cloud", detected: false)
        let shown = ProviderConnectionPresentation(amp, health: "ready")
        XCTAssertEqual(shown.status, "Connects through Agent Relay Cloud")
        XCTAssertEqual(shown.summary, amp.detail)
        XCTAssertNil(shown.action)
        XCTAssertEqual(AmpConnections.action, "Open Cloud connections")
        let connected = provider("amp", state: "configured", supported: false, detected: false)
        XCTAssertEqual(ProviderConnectionPresentation(connected, health: "ready").status, "Connected in Agent Relay Cloud")
    }

    func testConfiguredAmpIsOnWithoutACheckAction() {
        let amp = provider("amp", selected: false, state: "configured", supported: false, detected: false)
        XCTAssertEqual(ProviderConnectionState(amp, accountHealth: "ready"), .on)
        XCTAssertEqual(ProviderConnectionState(amp, accountHealth: "offline"), .off)
        let shown = ProviderConnectionPresentation(amp, health: "ready")
        XCTAssertEqual(shown.status, "Connected in Agent Relay Cloud")
        XCTAssertNil(shown.action, "Amp's one action is Open Cloud connections, not Check connection")
        let pending = provider("amp", selected: false, state: "external_setup", supported: false, detected: false)
        XCTAssertNotEqual(ProviderConnectionState(pending, accountHealth: "ready"), .on)
    }

    func testAmpOpensTheAppSitesCloudConnections() {
        let suite = "amp-connections-\(UUID())"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let model = AppModel(siteURL: "http://localhost:3100", manageSystemServices: false, defaults: defaults)
        XCTAssertEqual(model.ampConnectionsURL?.absoluteString,
                       "http://localhost:3100/cloud/dashboard/settings#connections")
    }

    func testCursorSupportsManagedInstallAndRepair() {
        XCTAssertEqual(ProviderConnectionPresentation(provider("cursor", selected: false, state: "not_installed"), health: "ready").action, .install)
        XCTAssertEqual(ProviderConnectionPresentation(provider("cursor", state: "action_required"), health: "ready").action, .repair)
    }

    func testProviderOnRequiresSelectedConfiguredAndVerifiedAccount() {
        let configured = provider("codex")
        XCTAssertEqual(ProviderConnectionState(configured, accountHealth: "ready"), .on)
        XCTAssertEqual(ProviderConnectionState(configured, accountHealth: nil), .attention)
        XCTAssertEqual(ProviderConnectionState(configured, accountHealth: "offline"), .attention)
        XCTAssertEqual(ProviderConnectionState(provider("codex", state: "action_required"), accountHealth: "ready"), .attention)
        XCTAssertEqual(ProviderConnectionState(provider("codex", selected: false), accountHealth: "ready"), .off)
        XCTAssertEqual(ProviderConnectionState(provider("codex", state: "installed"), accountHealth: "ready"), .unverified)
        XCTAssertEqual(ProviderConnectionState.unverified.label, "Not yet verified")
        XCTAssertTrue(ProviderConnectionState.on.explanation.contains("Running chats are not verified"))
    }

    func testVerifiedExternalConnectionsKeepLocalSelectionSeparate() {
        for id in ["capy", "grok-bot", "dot"] {
            let cloud = provider(id, selected: false, state: "configured", supported: false, label: "Connected")
            XCTAssertEqual(ProviderConnectionState(cloud, accountHealth: "ready"), .on)
            XCTAssertEqual(ProviderConnectionPresentation(cloud, health: "ready").action, .check)
            XCTAssertEqual(ProviderConnectionPresentation(cloud, health: "offline").status, "Connection not verified")
            XCTAssertNotEqual(ProviderConnectionState(cloud, accountHealth: "offline"), .on)
            XCTAssertFalse(cloud.selected, "Cloud proof must not add an unsupported installer selection")
            let inactive = provider(id, selected: false, state: "external_setup", supported: false, label: "No active bot")
            XCTAssertEqual(ProviderConnectionPresentation(inactive, health: "ready").status, "No active bot")
            XCTAssertNotEqual(ProviderConnectionState(inactive, accountHealth: "ready"), .on)
        }
        for id in ["conductor", "paseo"] {
            let host = provider(id, state: "configured", supported: false, label: "Connected")
            XCTAssertEqual(ProviderConnectionState(host, accountHealth: "ready"), .on)
            XCTAssertEqual(ProviderConnectionPresentation(host, health: "ready").status, "Connected")
        }
    }

    func testNextActionAddressesTheProblemWithoutOfferingFalseRepairs() {
        XCTAssertEqual(ProviderConnectionPresentation(provider("codex", selected: false), health: "ready").action, .install)
        XCTAssertNil(ProviderConnectionPresentation(provider("codex"), health: "ready").action)
        XCTAssertEqual(ProviderConnectionPresentation(provider("codex"), health: "offline").action, .check)
        XCTAssertEqual(ProviderConnectionPresentation(provider("codex"), health: "sign_in_required").action, .account)
        XCTAssertEqual(ProviderConnectionPresentation(provider("codex", state: "action_required"), health: "ready").action, .repair)
        let unsupportedMessaging = ProviderConnectionPresentation(provider("claude", state: "action_required", label: "Messaging unavailable"), health: "ready")
        XCTAssertNil(unsupportedMessaging.action, "Reinstalling cannot fix missing hosted identity support")
        XCTAssertEqual(unsupportedMessaging.status, "Messaging unavailable")
        for id in ["capy", "conductor", "paseo", "replicas", "claude-cloud"] {
            let external = ProviderConnectionPresentation(provider(id, selected: false, state: "external_setup", supported: false), health: "ready")
            XCTAssertEqual(external.action, ["conductor", "paseo"].contains(id) ? .enable : nil)
            XCTAssertNotEqual(external.status, "Connected", "App sign-in does not verify an external provider")
        }
    }

    func testDisabledHostCanBeEnabledDespiteStaleConfiguredState() {
        for id in ["conductor", "paseo"] {
            let tool = provider(id, selected: false, state: "configured", supported: false)
            let shown = ProviderConnectionPresentation(tool, health: "ready")
            XCTAssertEqual(shown.action, .enable)
            XCTAssertNotEqual(shown.status, "Connected")
            XCTAssertEqual(ProviderConnectionState(tool, accountHealth: "ready"), .off)
        }
    }

    func testCodexRuntimeFailureAsksForReloadAndChecksInsteadOfReinstalling() {
        let tool = provider("codex", state: "action_required", label: "MCP not ready")
        let shown = ProviderConnectionPresentation(tool, health: "ready")
        XCTAssertEqual(shown.status, "Reload Codex chats")
        XCTAssertEqual(shown.action, .check)
        XCTAssertEqual(shown.actionTitle, "Check connection")
        XCTAssertTrue(shown.summary?.contains("Reload or start a new Codex chat") == true)
        XCTAssertEqual(ProviderConnectionPresentation(tool, health: "sign_in_required").action, .account)
    }

    func testPrimaryActionNamesWhatItDoesAndSummariesOnlyAddInformation() {
        let off = ProviderConnectionPresentation(provider("claude", selected: false), health: "ready")
        XCTAssertEqual(off.status, "Not connected")
        XCTAssertEqual(off.actionTitle, "Connect Claude Code")
        XCTAssertNil(off.summary, "The Connect action already says what happens")
        let missing = ProviderConnectionPresentation(provider("claude", selected: false, detected: false), health: "ready")
        XCTAssertEqual(missing.summary, "Install Claude Code on this computer to connect it.")
        XCTAssertEqual(ProviderConnectionPresentation(provider("codex", state: "action_required"), health: "ready").actionTitle,
                       "Fix Codex connection")
        XCTAssertEqual(ProviderConnectionPresentation(provider("codex"), health: "sign_in_required").actionTitle,
                       "Sign in to Agent Relay")
        let offline = ProviderConnectionPresentation(provider("codex"), health: "offline")
        XCTAssertEqual(offline.actionTitle, "Check connection")
        XCTAssertEqual(offline.summary, "Check your internet connection, then try again.")
        let connected = ProviderConnectionPresentation(provider("codex"), health: "ready")
        XCTAssertEqual(connected.status, "Connected")
        XCTAssertNil(connected.summary)
        let saved = ProviderConnectionPresentation(provider("codex", state: "installed"), health: "ready")
        XCTAssertEqual(saved.status, "Setup saved", "Saved setup is not verified access")
        XCTAssertEqual(saved.summary, "Access from running chats hasn’t been verified yet.")
        let unsupported = ProviderConnectionPresentation(provider("opencode", selected: false, state: "unavailable", supported: false), health: "ready")
        XCTAssertEqual(unsupported.summary, "Agent Relay hasn’t added automatic setup for this provider yet.")
        XCTAssertNil(unsupported.action)
        XCTAssertEqual(SettingsPane.tools.subtitle, "", "Provider pages name themselves")
    }

    func testFolderDisclosureSummarizesSharing() throws {
        func folders(_ shared: [Bool]) throws -> [ClaudeConfigDir] {
            let items = shared.enumerated().map { #"{"path":"/f\#($0.offset)","origin":"default","shared":\#($0.element)}"# }
            return try JSONDecoder().decode([ClaudeConfigDir].self, from: Data("[\(items.joined(separator: ","))]".utf8))
        }
        XCTAssertEqual(ClaudeFoldersSection.summary(try folders([])), "No folders found")
        XCTAssertEqual(ClaudeFoldersSection.summary(try folders([true])), "Shared")
        XCTAssertEqual(ClaudeFoldersSection.summary(try folders([true, true])), "All 2 shared")
        XCTAssertEqual(ClaudeFoldersSection.summary(try folders([true, false, true])), "2 of 3 shared")
        XCTAssertEqual(ClaudeFoldersSection.summary(try folders([false])), "Not shared")
    }

    func testMissingProviderSetupIsNotReportedAsAnAccountOrMCPRestriction() {
        for id in ["grok", "opencode", "devin", "muse", "pi", "openclaw", "hermes", "droid"] {
            for health in ["ready", "sign_in_required"] {
                let shown = ProviderConnectionPresentation(
                    provider(id, selected: false, state: "unavailable", supported: false), health: health)
                XCTAssertEqual(shown.status, "Setup not available yet")
                XCTAssertEqual(shown.summary, "Agent Relay hasn’t added automatic setup for this provider yet.")
                XCTAssertNil(shown.action)
            }
        }
    }

    func testPendingHelperVerificationIsNeutralWithoutRepairAction() {
        let checking = provider("codex", state: "checking", label: "Checking")
        for health: String? in [nil, "ready", "offline"] {
            XCTAssertEqual(ProviderConnectionState(checking, accountHealth: health), .unverified)
            let shown = ProviderConnectionPresentation(checking, health: health)
            XCTAssertEqual(shown.status, "Checking")
            XCTAssertEqual(shown.summary, "Agent Relay is checking setup. This will finish automatically.")
            XCTAssertNil(shown.action)
            XCTAssertNil(shown.actionTitle)
        }
    }

    func testFailedDisconnectRemainsVisibleAfterDeselection() {
        let failed = ProviderConnectionPresentation(provider("codex", selected: false, state: "action_required"), health: "ready")
        XCTAssertEqual(failed.status, "Needs attention")
        XCTAssertEqual(failed.summary, "Agent Relay couldn’t finish changing this connection. Check Connection details and try again.")
        XCTAssertNil(failed.action, "Reconnecting must not replace unfinished cleanup")
        XCTAssertNil(failed.actionTitle)
    }

    func testSessionOpeningSettingsSelectsTheFailedProvider() async throws {
        let core = FakeCore()
        defer { core.stop() }
        core.on("tools.status") { _ in try self.fixture([self.provider("codex"), self.provider("claude")]) }
        let tools = ToolsModel(socketPath: core.path, requestTimeout: 0.5)
        await tools.refresh()
        for source in ["claude", "codex"] {
            XCTAssertEqual(SessionOpenButton.settingsDestination(source: source, tools: tools), .settings(.tools))
            XCTAssertEqual(tools.displayedProviderID, source)
        }
    }

    func testOnlyAnExplicitHealthCheckRecordsACheckTime() async throws {
        let core = FakeCore()
        defer { core.stop() }
        core.on("tools.status") { _ in try self.fixture([self.provider("codex")]) }
        core.on("tools.check") { _ in try self.fixture([self.provider("codex")]) }
        let model = ToolsModel(socketPath: core.path, requestTimeout: 0.5)
        await model.refresh()
        XCTAssertNil(model.lastCheckedAt)
        XCTAssertFalse(model.isCheckingHealth)
        await model.checkHealth()
        let checked = try XCTUnwrap(model.lastCheckedAt)
        await model.refresh()
        XCTAssertEqual(model.lastCheckedAt, checked)
        model.reset()
        XCTAssertNil(model.lastCheckedAt)
    }

    func testPassiveStatusCannotBlockActionsOrOverwriteTheirResults() async throws {
        for check in [false, true] {
            let core = FakeCore()
            defer { core.stop() }
            let held = DispatchSemaphore(value: 0)
            defer { held.signal() }
            core.on("tools.status") { _ in try self.fixture([self.provider("codex", selected: false)]) }
            let model = ToolsModel(socketPath: core.path, requestTimeout: 5)
            await model.refresh()
            core.on("tools.status") { _ in
                held.wait()
                return try self.fixture([self.provider("codex", selected: false)], health: "offline")
            }
            core.on(check ? "tools.check" : "tools.manage") { _ in
                try self.fixture([self.provider("codex")])
            }
            let poll = Task { await model.refresh() }
            let started = await eventually { core.callCount("tools.status") == 2 }
            XCTAssertTrue(started)
            XCTAssertFalse(model.isWorking, "Background reads must leave actions available")
            if check { await model.checkHealth() }
            else { await model.manage("codex", action: "install") }
            XCTAssertEqual(core.callCount(check ? "tools.check" : "tools.manage"), 1)
            held.signal()
            await poll.value
            XCTAssertEqual(model.report?.accountHealth, "ready")
            XCTAssertEqual(model.report?.tools.first?.selected, true, "A stale poll must not replace the action result")
            XCTAssertFalse(model.isWorking)
        }
    }

    func testProviderNavigationUsesCatalogAndPreservesUnavailableDetailsAcrossRefresh() async throws {
        let core = FakeCore()
        defer { core.stop() }
        let providers = [provider("codex"), provider("claude"), provider("opencode", state: "unavailable", supported: false)]
        core.on("tools.status") { _ in try self.fixture(providers) }
        let model = ToolsModel(socketPath: core.path, requestTimeout: 0.5)
        await model.refresh()
        XCTAssertEqual(model.providersOnCount, 2)
        XCTAssertEqual(model.displayedProviderID, "claude")
        model.showProvider("opencode")
        XCTAssertEqual(model.displayedProvider?.name, "OpenCode")
        XCTAssertEqual(model.actions(for: try XCTUnwrap(model.displayedProvider)), ["uninstall", "disable"])
        model.showProvider("azure-openai")
        XCTAssertEqual(model.displayedProviderID, "opencode", "Navigation cannot invent a provider absent from the core catalog")
        core.on("tools.status") { _ in try self.fixture(providers, health: "offline") }
        await model.refresh()
        XCTAssertEqual(model.providersOnCount, 0)
        XCTAssertEqual(model.displayedProviderID, "opencode")
        model.showProvidersOverview()
        await model.refresh()
        XCTAssertNil(model.displayedProvider, "Opening the Providers heading keeps the overview across background refresh")
        XCTAssertEqual(SettingsPane.tools.title, "Providers")
        XCTAssertFalse(SettingsPane.groups.flatMap(\.panes).contains(.tools))
    }

    func testHostAndCloudProvidersRemainNavigableWithoutClaimingMCPInstallation() async throws {
        let core = FakeCore()
        defer { core.stop() }
        let ids = ["conductor", "paseo", "capy", "replicas", "claude-cloud"]
        let providers = ids.map { provider($0, selected: false, state: "external_setup", supported: false) }
        core.on("tools.status") { _ in try self.fixture(providers) }
        let model = ToolsModel(socketPath: core.path, requestTimeout: 0.5)
        await model.refresh()
        for id in ids {
            model.showProvider(id)
            let shown = try XCTUnwrap(model.displayedProvider)
            XCTAssertEqual(shown.id, id)
            XCTAssertEqual(ProviderConnectionState(shown, accountHealth: "ready"), .unverified)
            XCTAssertEqual(model.actions(for: shown), ["conductor", "paseo"].contains(id) ? ["enable"] : [])
            await model.refresh()
            XCTAssertEqual(model.displayedProviderID, id)
        }
        XCTAssertEqual(model.providersOnCount, 0)
        XCTAssertEqual(model.eligibleIDs, ["conductor", "paseo"])
        XCTAssertEqual(ProviderSetupOptions.agentProviders(for: "conductor").map(\.id), ["codex", "claude"])
        XCTAssertEqual(ProviderSetupOptions.agentProviders(for: "paseo").map(\.id), ["codex", "claude", "grok", "muse"])
        XCTAssertTrue(ProviderSetupOptions.agentProviders(for: "capy").isEmpty)
        XCTAssertEqual(ProviderSetupOptions.agentProviders(for: "claude-cloud").map(\.id), ["claude"])
        XCTAssertEqual(core.callCount("tools.manage"), 0)
    }

    func testActiveCloudProviderAppearsWithoutLocalAppOrInstallerSelection() async throws {
        let core = FakeCore()
        defer { core.stop() }
        let cloud = provider("grok-bot", selected: false, state: "configured", supported: false, label: "Connected", detected: false)
        core.on("tools.status") { _ in try self.fixture([cloud]) }
        let model = ToolsModel(socketPath: core.path, requestTimeout: 0.5)
        await model.refresh()
        XCTAssertEqual(model.sidebarProviders.map(\.id), ["grok-bot"])
        XCTAssertTrue(model.availableProviders.isEmpty)
        XCTAssertEqual(model.providersOnCount, 1)
        model.showProvider("grok-bot")
        XCTAssertEqual(model.displayedProviderID, "grok-bot")
        XCTAssertTrue(model.eligibleIDs.isEmpty)
    }

    func testSidebarShowsOnlyDetectedOrEnabledProvidersAlphabetically() async throws {
        let core = FakeCore()
        defer { core.stop() }
        let providers = [provider("opencode", state: "unavailable", supported: false, detected: false),
                         provider("codex", selected: false),
                         provider("capy", selected: false, state: "external_setup", supported: false, detected: false),
                         provider("claude", selected: false)]
        core.on("tools.status") { _ in try self.fixture(providers) }
        let model = ToolsModel(socketPath: core.path, requestTimeout: 0.5)
        await model.refresh()
        XCTAssertEqual(model.sidebarProviders.map(\.id), ["claude", "codex", "opencode"])
        XCTAssertEqual(model.availableProviders.map(\.id), ["capy"])
        XCTAssertEqual(model.detectedTools.map(\.id), ["claude", "codex"])
        model.showProvider("capy")
        XCTAssertNil(model.displayedProvider, "Hidden providers must be explicitly enabled before their page appears")
        await model.refresh()
        XCTAssertNil(model.displayedProvider)
        XCTAssertEqual(ProviderConnectionState(providers[0], accountHealth: "ready"), .unverified)
        XCTAssertEqual(ProviderConnectionState(provider("opencode", state: "action_required", supported: false), accountHealth: "ready"), .attention)
    }

    func testCopilotIsAddedExplicitlyAndAmpFollowsItsCloudConnection() async throws {
        let core = FakeCore()
        defer { core.stop() }
        let amp = provider("amp", selected: false, state: "external_setup", supported: false, label: "Connects through Agent Relay Cloud", detected: false)
        let copilot = provider("copilot", selected: false, state: "not_installed")
        let codex = provider("codex", selected: false)
        let added = provider("copilot")
        core.on("tools.status") { _ in try self.fixture([amp, codex, copilot]) }
        core.on("tools.manage") { _ in try self.fixture([amp, codex, added]) }
        let model = ToolsModel(socketPath: core.path, requestTimeout: 0.5)
        await model.refresh()
        XCTAssertEqual(model.sidebarProviders.map(\.id), ["codex"])
        XCTAssertEqual(Set(model.availableProviders.map(\.id)), ["amp", "copilot"])
        XCTAssertTrue(model.actions(for: copilot).contains("enable"))
        XCTAssertTrue(model.actions(for: amp).contains("enable"))
        XCTAssertEqual(ToolsModel.addedExplicitly, ["copilot"])
        XCTAssertEqual(ToolsModel.cloudConfigured, ["capy", "grok-bot", "dot", "amp"])
        XCTAssertEqual(ProviderEnablePanel.detail,
                       "Installed providers appear in the sidebar automatically. Add others here, including GitHub Copilot and Amp.")
        await model.manage("copilot", action: "enable")
        XCTAssertEqual(core.lastParams("tools.manage")?["tool_id"] as? String, "copilot")
        XCTAssertEqual(model.sidebarProviders.map(\.id), ["codex", "copilot"])
        XCTAssertEqual(model.availableProviders.map(\.id), ["amp"])
        let reported = provider("amp", selected: false, state: "configured", supported: false, detected: false)
        core.on("tools.status") { _ in try self.fixture([reported, codex, added]) }
        await model.refresh()
        XCTAssertEqual(model.sidebarProviders.map(\.id), ["amp", "codex", "copilot"])
        XCTAssertTrue(model.availableProviders.isEmpty)
    }

    func testManualEnablePersistsNavigationAndRemovalReturnsToOverview() async throws {
        let core = FakeCore()
        defer { core.stop() }
        let off = provider("capy", selected: false, state: "external_setup", supported: false, detected: false)
        let enabled = provider("capy", state: "external_setup", supported: false, detected: false)
        core.on("tools.status") { _ in try self.fixture([off]) }
        core.on("tools.manage") { _ in try self.fixture([enabled]) }
        let model = ToolsModel(socketPath: core.path, requestTimeout: 0.5)
        await model.refresh()
        XCTAssertTrue(model.sidebarProviders.isEmpty)
        XCTAssertNil(model.displayedProvider)
        await model.manage("capy", action: "enable")
        XCTAssertEqual(core.lastParams("tools.manage")?["action"] as? String, "enable")
        XCTAssertEqual(core.lastParams("tools.manage")?["tool_id"] as? String, "capy")
        XCTAssertEqual(model.sidebarProviders.map(\.id), ["capy"])
        XCTAssertEqual(model.displayedProviderID, "capy")
        XCTAssertTrue(model.availableProviders.isEmpty)
        XCTAssertEqual(model.providersOnCount, 0)
        core.on("tools.status") { _ in try self.fixture([enabled]) }
        let reopened = ToolsModel(socketPath: core.path, requestTimeout: 0.5)
        await reopened.refresh()
        XCTAssertEqual(reopened.sidebarProviders.map(\.id), ["capy"])
        core.on("tools.manage") { _ in try self.fixture([off]) }
        await model.manage("capy", action: "disable")
        XCTAssertTrue(model.sidebarProviders.isEmpty)
        XCTAssertEqual(model.availableProviders.map(\.id), ["capy"])
        XCTAssertNil(model.displayedProvider)
    }

    /// Opt-in synthetic capture; this calls only fake tools.status, never installation.
    func testProvidersSidebarAndSelectedDetailsRenderFixture() async throws {
        guard let directory = ProcessInfo.processInfo.environment["PROVIDERS_RENDER_DIR"] else {
            throw XCTSkip("Set PROVIDERS_RENDER_DIR to capture the provider sidebar and details")
        }
        let core = FakeCore()
        defer { core.stop() }
        let providers = [provider("codex"), provider("claude"), provider("opencode", selected: false, state: "unavailable", supported: false), provider("grok", selected: false, state: "not_installed")]
            + ["conductor", "paseo", "capy", "replicas", "claude-cloud"].map {
                provider($0, selected: false, state: "external_setup", supported: false)
            }
        core.on("tools.status") { _ in try self.fixture(providers) }
        let model = ToolsModel(socketPath: core.path, requestTimeout: 0.5)
        await model.refresh()
        model.showProvider("capy")
        let app = AppModel(client: CoreClient(socketPath: core.path), manageSystemServices: false)
        try FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)
        for dark in [false, true] {
            let view = HStack(spacing: 0) {
                VStack {
                    ProvidersSidebar(tools: model, isSelected: true, openProviders: {})
                    Spacer()
                }.padding(10).frame(width: 244).background(CloudTheme.sidebar)
                Divider().overlay(CloudTheme.border)
                ScrollView {
                    VStack(alignment: .leading, spacing: 28) { ProviderSettingsPage(tools: model) }
                        .padding(28)
                }.environment(app)
            }
            .frame(width: 980, height: 650)
            .background(CloudTheme.background)
            .preferredColorScheme(dark ? .dark : .light)
            let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 980, height: 650), styleMask: [.titled], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            window.appearance = NSAppearance(named: dark ? .darkAqua : .aqua)
            let host = NSHostingView(rootView: view)
            window.contentView = host
            window.orderFront(nil)
            try await Task.sleep(for: .milliseconds(100))
            host.layoutSubtreeIfNeeded()
            XCTAssertEqual(host.bounds.width, 980, accuracy: 1)
            let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
            host.cacheDisplay(in: host.bounds, to: bitmap)
            let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
            try data.write(to: URL(fileURLWithPath: directory).appendingPathComponent("providers-\(dark ? "dark" : "light").png"))
            window.close()
        }
        XCTAssertEqual(core.callCount("tools.manage"), 0)
        XCTAssertEqual(core.callCount("tools.select"), 0)
    }
}
