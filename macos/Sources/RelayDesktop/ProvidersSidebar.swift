import RelayDesktopCore
import SwiftUI

/// Configuration and account health only; this never infers a running chat's tools.
enum ProviderConnectionState: Equatable {
    case on, attention, unverified, off

    init(_ provider: ToolIntegration, accountHealth: String?) {
        // Cloud-connected providers, Amp included, are on once Agent Relay Cloud
        // reports them configured and the account is ready, selected or not.
        if ["capy", "grok-bot", "dot", "amp"].contains(provider.id), provider.state == "configured", accountHealth == "ready" { self = .on }
        else if provider.state == "external_setup" || provider.state == "checking" || (provider.selected && provider.state == "unavailable") { self = .unverified }
        else if !provider.selected { self = .off }
        else if provider.state == "installed" { self = .unverified }
        else if provider.state == "configured" && accountHealth == "ready" { self = .on }
        else { self = .attention }
    }

    var label: String {
        switch self {
        case .on: return "On"
        case .attention: return "Needs attention"
        case .unverified: return "Not yet verified"
        case .off: return "Off"
        }
    }

    var color: Color {
        switch self {
        case .on: return CloudTheme.success
        case .attention: return CloudTheme.warning
        case .off, .unverified: return CloudTheme.secondary
        }
    }

    var explanation: String {
        switch self {
        case .on: return "On. Configuration and account health are verified. Running chats are not verified."
        case .attention: return "Needs attention. Running chats are not verified."
        case .unverified: return "Not yet verified. Running chats are not verified."
        case .off: return "Off. Running chats are not verified."
        }
    }
}

/// Detected and explicitly enabled providers, in alphabetical order.
struct ProvidersSidebar: View {
    var tools: ToolsModel
    let isSelected: Bool
    let openProviders: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack {
                Button("Providers") { tools.showProvidersOverview(); openProviders() }
                    .buttonStyle(NativeActionButtonStyle(base: PlainButtonStyle()))
                    .accessibilityAddTraits(.isHeader)
                Spacer()
                Text("\(tools.providersOnCount) on").monospacedDigit()
            }
            .zoomFont(size: 11, weight: .medium)
            .foregroundStyle(CloudTheme.secondary)
            .zoomPadding(.horizontal, 10).zoomPadding(.top, 6).zoomPadding(.bottom, 4)
            ForEach(tools.sidebarProviders, id: \.id) { provider in
                providerRow(provider)
            }
            Button {
                tools.showProvidersOverview()
                openProviders()
            } label: {
                Label("Add provider", systemImage: "plus")
                    .zoomFont(size: 13)
                    .zoomPadding(.horizontal, 10).zoomFrame(height: 34)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
            }
            .buttonStyle(NavButtonStyle(kind: .sidebar, isSelected: isSelected && tools.displayedProviderID == nil))
            .accessibilityAddTraits(isSelected && tools.displayedProviderID == nil ? .isSelected : [])
            if tools.report == nil {
                Button(tools.isWorking ? "Checking providers…" : "Check provider setup") { openProviders() }
                    .zoomFont(size: 12)
                    .buttonStyle(NativeActionButtonStyle(base: PlainButtonStyle()))
                    .zoomPadding(.horizontal, 10).zoomPadding(.vertical, 8)
            }
        }
        .task {
            while !Task.isCancelled {
                await tools.refresh()
                do { try await Task.sleep(nanoseconds: 2_000_000_000) }
                catch { break }
            }
        }
    }

    private func providerRow(_ provider: ToolIntegration) -> some View {
        let state = ProviderConnectionState(provider, accountHealth: tools.report?.accountHealth)
        let selected = isSelected && tools.displayedProviderID == provider.id
        return Button {
            tools.showProvider(provider.id)
            openProviders()
        } label: {
            HStack(spacing: 10) {
                AgentIconView(source: provider.id, size: 19)
                    .saturation(state == .off ? 0 : 1).opacity(state == .off ? 0.45 : 1)
                    .accessibilityHidden(true)
                Text(provider.name).lineLimit(1)
                    .foregroundStyle(selected ? CloudTheme.selectedText : state == .off ? CloudTheme.secondary : CloudTheme.foreground)
                Spacer(minLength: 4)
                Circle().fill(state.color).zoomFrame(width: 6, height: 6).opacity(state == .off ? 0 : 1)
            }
            .zoomFont(size: 13, weight: selected ? .medium : .regular)
            .zoomPadding(.horizontal, 10).zoomFrame(height: 34)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(NavButtonStyle(kind: .sidebar, isSelected: selected))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(provider.name). \(state.explanation)")
        .accessibilityAddTraits(.isButton)
        .accessibilityAddTraits(selected ? .isSelected : [])
        .help("\(provider.statusLabel). \(provider.detail) \(state.explanation)")
    }
}
