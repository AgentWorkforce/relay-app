import RelayDesktopCore
import SwiftUI

/// The provider's identity and status, with its one next step beside it.
struct ProviderConnectionView: View {
    let provider: ToolIntegration
    var tools: ToolsModel
    @Environment(AppModel.self) private var model

    var body: some View {
        let presentation = ProviderConnectionPresentation(provider, health: tools.report?.accountHealth)
        let state = ProviderConnectionState(provider, accountHealth: tools.report?.accountHealth)
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .center, spacing: 12) {
                AgentIconView(source: provider.id, size: 32).accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 3) {
                    Text(provider.name).zoomFont(size: 20, weight: .semibold).accessibilityAddTraits(.isHeader)
                    HStack(spacing: 6) {
                        Circle().fill(state.color).zoomFrame(width: 6, height: 6).accessibilityHidden(true)
                        Text(presentation.status).zoomFont(size: 12, weight: .medium)
                            .foregroundStyle(state == .attention ? CloudTheme.warning : CloudTheme.secondary)
                    }
                }
                Spacer(minLength: 16)
                if let action = presentation.action, let title = presentation.actionTitle {
                    Button(tools.isWorking ? "Working…" : title) { perform(action) }
                        .buttonStyle(AgentRelayPrimaryButtonStyle()).disabled(tools.isWorking)
                } else if let setup = CloudProviderSetup(rawValue: provider.id) {
                    Button(setup.action) { model.setUpCloudProvider(setup) }
                        .buttonStyle(AgentRelayPrimaryButtonStyle())
                } else if provider.id == "amp" {
                    Button(AmpConnections.action) { model.openAmpConnections() }
                        .buttonStyle(AgentRelayPrimaryButtonStyle())
                }
                if provider.canRepair || provider.canUninstall || tools.actions(for: provider).contains("disable") { maintenance }
            }
            if let summary = presentation.summary {
                Text(summary).zoomFont(size: 13).foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if CloudProviderSetup(rawValue: provider.id)?.copyOnly == true {
                MuseSetupSnippet()
            } else if let skillURL = CloudProviderSetup(rawValue: provider.id)?.skillURL {
                Link("Read setup skill", destination: skillURL).zoomFont(.footnote)
            }
            if provider.id == "capy" && provider.state != "configured" && model.capyPromptCopied {
                Text(CapySetup.copied).zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
            }
            if provider.id != "capy", model.cloudSetupStarted.contains(provider.id),
               let copied = CloudProviderSetup(rawValue: provider.id)?.copied {
                Text(copied).zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
            }
        }.id(provider.id)
    }

    private var maintenance: some View {
        Menu {
            if provider.canRepair {
                Button("Reinstall") { Task { await tools.manage(provider.id, action: "reinstall") } }
            }
            if provider.canUninstall {
                Button("Disconnect") { Task { await tools.manage(provider.id, action: "uninstall") } }
            }
            if tools.actions(for: provider).contains("disable") {
                Button(provider.detected ? "Deselect provider" : "Remove from sidebar") { Task { await tools.manage(provider.id, action: "disable") } }
            }
        } label: { Image(systemName: "ellipsis").zoomFrame(width: 28, height: 28).contentShape(Rectangle()) }
        .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
        .actionCursor()
        .accessibilityLabel("More options").help("More options").disabled(tools.isWorking)
    }

    private func perform(_ action: ProviderConnectionPresentation.Action) {
        switch action {
        case .install: Task { await tools.manage(provider.id, action: "install") }
        case .enable: Task { await tools.manage(provider.id, action: "enable") }
        case .repair: Task { await tools.manage(provider.id, action: "repair") }
        case .account: model.recoverProviderAccount(state: provider.state)
        case .check: Task { await tools.checkHealth() }
        }
    }
}

extension AppModel {
    /// A provider can need fresh desktop consent while the account itself is
    /// healthy. Start the existing browser renewal directly from its action.
    func recoverProviderAccount(state: String) {
        showSettingsWindow(.account)
        if state == "signed_out" { renewSignIn() }
    }
}

/// Technical evidence and the manual health check, collapsed by default.
/// Its state lives above the polled report, so background refreshes keep it.
struct ProviderConnectionDetails: View {
    let provider: ToolIntegration
    var tools: ToolsModel
    @Binding var isExpanded: Bool

    var body: some View {
        DisclosureGroup("Connection details", isExpanded: $isExpanded) {
            VStack(alignment: .leading, spacing: 10) {
                // Amp's on state needs a ready account, so it shows the account line;
                // it has no Check again, its one action is Open Cloud connections.
                if provider.supported || ["capy", "grok-bot", "dot", "amp"].contains(provider.id) {
                    Text("Agent Relay account: \(ToolsCopy.accountHealth(tools.report?.accountHealth))")
                }
                Text(provider.statusLabel).fontWeight(.medium)
                Text(provider.detail).fixedSize(horizontal: false, vertical: true)
                if provider.supported || ["capy", "grok-bot", "dot"].contains(provider.id) {
                    HStack(spacing: 12) {
                        Button(tools.isCheckingHealth ? "Checking…" : "Check again") {
                            Task { await tools.checkHealth() }
                        }.disabled(tools.isWorking)
                        if tools.isCheckingHealth { ProgressView().controlSize(.small) }
                        else if let checked = tools.lastCheckedAt {
                            Text("Checked at \(checked.formatted(date: .omitted, time: .shortened))")
                                .foregroundStyle(CloudTheme.secondary)
                        }
                    }
                }
            }
            .zoomFont(size: 12).foregroundStyle(CloudTheme.secondary)
            .frame(maxWidth: .infinity, alignment: .leading).zoomPadding(.top, 10)
        }
        .zoomFont(size: 13).foregroundStyle(CloudTheme.secondary)
    }
}
