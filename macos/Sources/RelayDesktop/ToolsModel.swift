import Foundation
import Observation
import RelayDesktopCore

/// What one provider's row in the onboarding chooser offers.
enum OnboardingRowAction: Equatable {
    /// A checkbox: the provider is set up on this computer when chosen.
    case checkbox
    /// The Cloud provider's own setup action (Capy, Grok Bot, Dot, Muse).
    case cloudSetup(CloudProviderSetup)
    /// Open Cloud connections on the signed-in site (Amp).
    case openCloudConnections
    /// Nothing to choose: listed only because it was detected here.
    case noChoice
}

/// One source of tool choices and repair state for Settings and onboarding.
@MainActor
@Observable
final class ToolsModel {
    private(set) var report: ToolsReport?
    private(set) var error: String?
    private(set) var isWorking = false
    private(set) var isCheckingHealth = false
    private(set) var lastCheckedAt: Date?
    private(set) var selectedIDs: Set<String> = []
    private(set) var displayedProviderID: String?
    private(set) var onboardingDeferred = false
    private(set) var onboardingRequested = false
    /// The person's tick or untick of the chooser's teammate-context line
    /// since it opened; nil follows the saved state the report gives.
    private var teammateContextDraft: Bool?
    private(set) var teammateContextError: String?

    private let client: CoreClient
    private let operations: CoreClient
    private var identity: String?
    private var generation = UUID()
    private var operationRevision = 0
    private var isReadingStatus = false
    private var preservesActionError = false
    private var providerOverviewChosen = false
    private var retainedOnboardingIDs: Set<String> = []

    init(socketPath: String, requestTimeout: TimeInterval = 30) {
        client = CoreClient(socketPath: socketPath, requestTimeout: requestTimeout)
        operations = CoreClient(socketPath: socketPath)
    }

    /// The teammate-context hook's state, or nil where Claude Code is neither
    /// on this computer nor chosen.
    var teammateContextEnabled: Bool? { report?.teammateContext }
    /// The chooser offers the teammate-context line once Claude Code is here
    /// or chosen.
    /// The providers whose sessions Agent Relay's core gives teammate context
    /// (desktop-core's `teammate_context::PROVIDERS`).
    static let teammateContextProviders = ["claude", "codex", "copilot", "devin"]

    var offersTeammateContext: Bool {
        report?.teammateContext != nil
            || Self.teammateContextProviders.contains(where: selectedIDs.contains)
    }

    /// What the chooser's line shows: the person's edit, else the saved
    /// state, else unchecked where nothing is saved yet: the hook is off by
    /// default.
    var teammateContextChoice: Bool { teammateContextDraft ?? report?.teammateContext ?? false }

    func setTeammateContextChoice(_ on: Bool) { teammateContextDraft = on }

    /// What Continue saves: only the person's edit. An untouched line saves
    /// nothing, so the hook stays off by default and a reopened chooser never
    /// undoes Settings.
    private var teammateContextToSave: Bool? {
        guard offersTeammateContext else { return nil }
        return teammateContextDraft
    }

    /// Settings' Turn On and Turn Off for the teammate-context hook.
    func setTeammateContext(_ on: Bool) async {
        guard !isWorking else { return }
        operationRevision += 1
        let request = generation
        teammateContextError = nil
        isWorking = true
        defer { if generation == request { isWorking = false } }
        do {
            let result = try await operations.toolsTeammateContext(ToolsTeammateContextParams(enabled: on))
            guard generation == request else { return }
            apply(result)
        } catch {
            guard generation == request else { return }
            teammateContextError = TeammateContextCopy.failed
        }
    }

    var shouldChooseTools: Bool { (report?.onboardingComplete == false || onboardingRequested) && !onboardingDeferred }
    var displayedProvider: ToolIntegration? { report?.tools.first { $0.id == displayedProviderID } }
    var providersOnCount: Int { report?.tools.filter { ProviderConnectionState($0, accountHealth: report?.accountHealth) == .on }.count ?? 0 }
    var sidebarProviders: [ToolIntegration] { orderedProviders.filter { Self.isVisible($0) } }
    var availableProviders: [ToolIntegration] { orderedProviders.filter { !Self.isVisible($0) } }
    /// Providers that are added explicitly and never auto-shown. Detection on
    /// this computer (VS Code for GitHub Copilot) does not put them in the
    /// sidebar: they stay under Available providers until the person adds
    /// them there or chooses them during onboarding.
    /// GTK applies the same list in `gtk/src/ui/tools.rs`.
    static let addedExplicitly: Set<String> = ["copilot"]

    /// Cloud providers the sidebar shows once Agent Relay Cloud reports their
    /// connection configured. Amp is one of them: the core never reports Amp
    /// as detected, so no local Amp install or folder can show it; it appears
    /// once it is added or once Cloud reports its connection.
    static let cloudConfigured: Set<String> = ["capy", "grok-bot", "dot", "amp"]

    /// The sidebar shows a provider when it is selected, when Agent Relay
    /// Cloud reports a Cloud provider's connection configured, or when it is
    /// detected here and is not one of the providers that are added explicitly.
    private static func isVisible(_ tool: ToolIntegration) -> Bool {
        (tool.detected && !addedExplicitly.contains(tool.id)) || tool.selected
            || (cloudConfigured.contains(tool.id) && tool.state == "configured")
    }
    private var orderedProviders: [ToolIntegration] {
        (report?.tools ?? []).sorted {
            let left = $0.name.lowercased(), right = $1.name.lowercased()
            return left == right ? $0.id < $1.id : left < right
        }
    }

    func showProvider(_ id: String) {
        if report?.tools.contains(where: { $0.id == id }) == true {
            guard sidebarProviders.contains(where: { $0.id == id }) else {
                showProvidersOverview()
                return
            }
            providerOverviewChosen = false
            displayedProviderID = id
        }
    }

    func showProvidersOverview() {
        providerOverviewChosen = true
        displayedProviderID = nil
    }

    var detectedTools: [ToolIntegration] { orderedProviders.filter(\.detected) }
    var onboardingTools: [ToolIntegration] {
        orderedProviders.filter {
            // Initial onboarding needs the core's local-install witness, not
            // a saved choice, Cloud connection or retained provider data.
            if ["amp", "capy"].contains($0.id) { return $0.onboardingAvailable == true }
            return $0.detected || Self.onboardingAction(for: $0) != .noChoice
        }
    }
    /// What a provider's onboarding chooser row offers: a checkbox for a local
    /// choice, the Cloud provider's setup, or, for Amp, Open Cloud connections.
    /// Amp has no checkbox: its token lives on the Cloud workspace, never here.
    /// GTK decides the same in `onboarding_action` in `gtk/src/ui/tools.rs`.
    static func onboardingAction(for tool: ToolIntegration) -> OnboardingRowAction {
        if let setup = CloudProviderSetup(rawValue: tool.id) { return .cloudSetup(setup) }
        if tool.id == "amp" { return .openCloudConnections }
        return isChooserEligible(tool) ? .checkbox : .noChoice
    }
    static func isChooserEligible(_ tool: ToolIntegration) -> Bool {
        // These are Cloud handoffs, never local MCP checkbox defaults.
        !["amp", "capy"].contains(tool.id)
            && tool.detected && (tool.supported || ["conductor", "paseo"].contains(tool.id))
    }
    var eligibleIDs: Set<String> { Set(report?.tools.filter(Self.isChooserEligible).map(\.id) ?? []) }
    func canChoose(_ tool: ToolIntegration) -> Bool { Self.isChooserEligible(tool) && !isWorking }
    var problems: [String] {
        ([error] + (report?.tools.filter { $0.selected && $0.state == "action_required" }.map { Optional("\($0.name): \($0.detail)") } ?? [])).compactMap { $0 }
    }

    func reset() {
        generation = UUID()
        operationRevision += 1
        isReadingStatus = false
        client.reset()
        operations.reset()
        identity = nil
        preservesActionError = false
        providerOverviewChosen = false
        report = nil
        error = nil
        selectedIDs = []
        displayedProviderID = nil
        onboardingDeferred = false
        onboardingRequested = false
        retainedOnboardingIDs = []
        isWorking = false
        isCheckingHealth = false
        lastCheckedAt = nil
    }

    func load(identity next: String) async {
        if identity != next { reset(); identity = next }
        await refresh()
    }

    func refresh() async { await read(check: false) }
    func checkHealth() async { await read(check: true) }

    private func read(check: Bool) async {
        guard !isWorking, check || !isReadingStatus else { return }
        let request = generation
        if check {
            operationRevision += 1
            preservesActionError = false
            error = nil
            isCheckingHealth = true
            isWorking = true
        } else { isReadingStatus = true }
        let revision = operationRevision
        defer {
            if generation == request {
                if check { isWorking = false; isCheckingHealth = false }
                else { isReadingStatus = false }
            }
        }
        do {
            let result: ToolsReport
            if check { result = try await operations.toolsCheck() }
            else { result = try await client.toolsStatus() }
            guard generation == request, operationRevision == revision else { return }
            apply(result)
            if check { lastCheckedAt = Date() }
            if !preservesActionError && error != nil { error = nil }
        } catch {
            guard generation == request, operationRevision == revision, !Task.isCancelled else { return }
            if !preservesActionError { self.error = "Couldn’t load provider status. Try again." }
        }
    }

    func setSelected(_ id: String, _ selected: Bool) {
        guard eligibleIDs.contains(id), !isWorking else { return }
        if selected {
            selectedIDs.insert(id)
            selectedIDs.formUnion(ProviderSetupOptions.agentProviders(for: id).map(\.id).filter { eligibleIDs.contains($0) })
        } else {
            selectedIDs.remove(id)
            retainedOnboardingIDs.remove(id)
            let hosts = selectedIDs.union(retainedOnboardingIDs).filter { host in
                ProviderSetupOptions.agentProviders(for: host).contains { $0.id == id }
            }
            selectedIDs.subtract(hosts)
            retainedOnboardingIDs.subtract(hosts)
        }
    }

    /// Only an explicit Continue or Not now writes the onboarding choice.
    func completeOnboarding(skip: Bool) async {
        guard !isWorking else { return }
        if skip && onboardingRequested { continueToApp(); return }
        operationRevision += 1
        let request = generation
        let ids = skip ? [] : selectedIDs.intersection(eligibleIDs).union(retainedOnboardingIDs).union(preservedOnboardingChoices).sorted()
        preservesActionError = false
        error = nil
        isWorking = true
        defer { if generation == request { isWorking = false } }
        do {
            // Not now leaves the teammate-context default as it is.
            let teammate = skip ? nil : teammateContextToSave
            let result = try await operations.toolsSelect(ToolsSelectParams(teammateContext: teammate, toolIds: ids))
            guard generation == request else { return }
            apply(result)
            guard result.onboardingComplete else {
                preservesActionError = true
                error = "Your provider choices could not be saved. Try again in Settings › Providers."
                return
            }
            error = nil
            teammateContextDraft = nil
            onboardingRequested = false
            onboardingDeferred = true
        } catch {
            guard generation == request else { return }
            preservesActionError = true
            self.error = "Your provider choices could not be saved. Try again in Settings › Providers."
        }
    }

    /// Hiding an uninstalled Cloud provider is presentation, not deselection.
    /// Only the core's already-saved choices are retained; never add a default.
    private var preservedOnboardingChoices: Set<String> {
        Set(report?.tools.filter {
            ["amp", "capy"].contains($0.id) && $0.selected
        }.map(\.id) ?? [])
    }

    /// An explicit escape after a failed save keeps the error and unsaved choice.
    func continueToApp() { onboardingDeferred = true; onboardingRequested = false; retainedOnboardingIDs = [] }

    func requestOnboarding() {
        guard !isWorking, let report else { return }
        selectedIDs = Set(report.tools.filter { $0.selected && Self.isChooserEligible($0) }.map(\.id))
        retainedOnboardingIDs = Set(report.tools.filter {
            $0.selected && !Self.isChooserEligible($0)
        }.map(\.id))
        onboardingDeferred = false
        onboardingRequested = true
        teammateContextDraft = nil
        preservesActionError = false
        error = nil
    }

    func retry() async {
        preservesActionError = false
        error = nil
        await refresh()
        if report?.onboardingComplete == false { onboardingDeferred = false }
    }

    func actions(for tool: ToolIntegration) -> [String] {
        var actions: [String] = []
        if tool.canInstall { actions.append("install") }
        if tool.canRepair { actions += ["repair", "reinstall"] }
        if tool.canUninstall { actions.append("uninstall") }
        if !tool.selected && (!tool.detected || ["conductor", "paseo"].contains(tool.id) || Self.addedExplicitly.contains(tool.id)) {
            actions.append("enable")
        }
        if tool.selected && !tool.supported { actions.append("disable") }
        return actions
    }

    func manage(_ id: String, action: String) async {
        guard !isWorking, let tool = report?.tools.first(where: { $0.id == id }), actions(for: tool).contains(action) else { return }
        operationRevision += 1
        let request = generation
        preservesActionError = false
        error = nil
        isWorking = true
        defer { if generation == request { isWorking = false } }
        do {
            let result = try await operations.toolsManage(ToolsManageParams(action: action, toolId: id))
            guard generation == request else { return }
            apply(result)
            if action == "enable", result.tools.contains(where: { $0.id == id && $0.selected }) {
                showProvider(id)
            }
            error = nil
        } catch {
            guard generation == request else { return }
            preservesActionError = true
            self.error = "Couldn’t update this connection. Try again or open Connection details."
        }
    }

    private func apply(_ result: ToolsReport) {
        let keepDraft = onboardingRequested || (report?.onboardingComplete == false && !result.onboardingComplete)
        if report != result { report = result }
        if !providerOverviewChosen && !sidebarProviders.contains(where: { $0.id == displayedProviderID }) { displayedProviderID = sidebarProviders.first?.id }
        if keepDraft {
            let eligible = eligibleIDs
            if onboardingRequested {
                let newlyHidden = result.tools.filter { $0.selected && selectedIDs.contains($0.id) && !eligible.contains($0.id) }.map(\.id)
                retainedOnboardingIDs.formUnion(newlyHidden)
                let newlyVisible = retainedOnboardingIDs.intersection(eligible)
                selectedIDs.formUnion(newlyVisible)
                retainedOnboardingIDs.subtract(newlyVisible)
            }
            selectedIDs.formIntersection(eligible)
        }
        else { selectedIDs = Set(result.tools.filter { $0.selected && Self.isChooserEligible($0) }.map(\.id)) }
    }
}
