import RelayDesktopCore
import SwiftUI

struct ToolsSettingsView: View {
    var tools: ToolsModel

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            if let provider = tools.displayedProvider {
                ProviderConnectionView(provider: provider, tools: tools).id(provider.id)
            } else {
                ProviderEnablePanel(tools: tools)
                Button("Choose providers…") { tools.requestOnboarding() }
                    .disabled(tools.isWorking || tools.report == nil)
            }
            if let error = tools.error {
                Label(error, systemImage: "exclamationmark.circle")
                    .zoomFont(.footnote).foregroundStyle(CloudTheme.warning)
                    .fixedSize(horizontal: false, vertical: true)
                Button("Try again") { Task { await tools.retry() } }.disabled(tools.isWorking)
            }
        }
        .task { await tools.refresh() }
    }
}

/// Onboarding also lets hosts select their underlying provider integrations.
struct ToolsList: View {
    @Environment(AppModel.self) private var model
    var tools: ToolsModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(tools.onboardingTools, id: \.id) { tool in
                HStack(alignment: .top, spacing: 12) {
                    let action = ToolsModel.onboardingAction(for: tool)
                    if action != .checkbox {
                        Color.clear.zoomFrame(width: 14, height: 28).accessibilityHidden(true)
                    } else {
                    Toggle(tool.name, isOn: Binding(
                        get: { tools.selectedIDs.contains(tool.id) },
                        set: { tools.setSelected(tool.id, $0) }
                    ))
                    .labelsHidden().toggleStyle(.checkbox)
                    .zoomFrame(height: 28)
                    .disabled(!tools.canChoose(tool))
                    .accessibilityLabel(tool.name)
                    }
                    AgentIconView(source: tool.id, size: 28).accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 5) {
                        Text(tool.name).zoomFont(size: 13, weight: .semibold)
                            .zoomFrame(height: 28, alignment: .leading)
                        Text(tool.onboardingDetail(account: model.account)).zoomFont(size: 12).foregroundStyle(CloudTheme.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                        if case .cloudSetup(let setup) = action {
                            Button(setup.action) { model.setUpCloudProvider(setup) }
                                .buttonStyle(AgentRelayPrimaryButtonStyle())
                                .disabled(tools.isWorking)
                            if setup.copyOnly {
                                MuseSetupSnippet()
                            } else if let skillURL = setup.skillURL {
                                Link("Read setup skill", destination: skillURL).zoomFont(.caption)
                            }
                            if model.cloudSetupStarted.contains(tool.id), let copied = setup.copied {
                                Text(copied).zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
                            }
                        } else if action == .openCloudConnections {
                            Button(AmpConnections.action) { model.openAmpConnections() }
                                .buttonStyle(AgentRelayPrimaryButtonStyle())
                                .disabled(tools.isWorking)
                        }
                    }.frame(maxWidth: .infinity, alignment: .leading)
                }.zoomPadding(.vertical, 14)
            }
        }
    }
}

struct ChooseToolsView: View {
    var tools: ToolsModel

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text("Choose your providers").zoomFont(size: 25, weight: .bold)
            Text("Choose where to enable Agent Relay. Running chats may need a reload or a new session.")
                .zoomFont(size: 13).foregroundStyle(CloudTheme.secondary)
                .fixedSize(horizontal: false, vertical: true)
            ScrollView { ToolsList(tools: tools).frame(maxWidth: .infinity, alignment: .leading) }
                .frame(maxHeight: .infinity)
            if tools.offersTeammateContext { TeammateContextChoice(tools: tools) }
            if tools.eligibleIDs.isEmpty {
                Text("No supported providers were detected. You can continue and add providers later in Settings › Providers.")
                    .zoomFont(size: 12).foregroundStyle(CloudTheme.secondary)
            }
            if let error = tools.error {
                Text(error).zoomFont(size: 12).foregroundStyle(CloudTheme.warning)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Text("You can change this later in Settings › Providers.")
                .zoomFont(size: 12).foregroundStyle(CloudTheme.secondary)
            HStack(spacing: 12) {
                if tools.isWorking { ProgressView().controlSize(.small) }
                if tools.error != nil {
                    Button("Continue to app") { tools.continueToApp() }.disabled(tools.isWorking)
                }
                Spacer()
                Button("Not now") { Task { await tools.completeOnboarding(skip: true) } }
                    .disabled(tools.isWorking)
                Button("Continue") { Task { await tools.completeOnboarding(skip: false) } }
                    .buttonStyle(AgentRelayPrimaryButtonStyle())
                    .keyboardShortcut(.defaultAction)
                    .disabled(tools.isWorking)
            }
        }
        .zoomPadding(28)
        .zoomFrame(width: 620, height: 540)
        .foregroundStyle(CloudTheme.foreground)
        .background(CloudTheme.background)
        .buttonStyle(NativeActionButtonStyle(base: BorderedButtonStyle()))
        .interactiveDismissDisabled(true)
    }
}

enum ToolsCopy {
    static func accountHealth(_ health: String?) -> String {
        switch health {
        case "ready": return "Verified"
        case "sign_in_required": return "Sign in required"
        case "offline": return "Cannot verify sign-in"
        case "checking": return "Checking sign-in"
        default: return "Not checked"
        }
    }
}


/// The same visible, selectable copy/paste handoff in onboarding and settings.
struct MuseSetupSnippet: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let setup = CloudProviderSetup.museAssistant
        VStack(alignment: .leading, spacing: 10) {
            if let prompt = setup.prompt {
                Text(prompt)
                    .zoomFont(size: 12, design: .monospaced)
                    .foregroundStyle(CloudTheme.codeForeground)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .zoomPadding(12)
                    .background(CloudTheme.codeBackground, in: RoundedRectangle(cornerRadius: 8))
            }
            HStack(spacing: 12) {
                Button("Open Muse") { model.openCloudProviderApp(setup) }
                if let skillURL = setup.skillURL {
                    Link("Read setup skill", destination: skillURL)
                }
            }.zoomFont(.footnote)
        }
    }
}
