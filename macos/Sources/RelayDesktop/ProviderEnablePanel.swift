import SwiftUI

/// Providers the sidebar does not show on its own (not installed here, or
/// added explicitly, see `ToolsModel.addedExplicitly`), added by hand.
struct ProviderEnablePanel: View {
    static let title = "Add provider"
    static let detail = "Installed providers appear in the sidebar automatically. Add others here, including GitHub Copilot and Amp."

    var tools: ToolsModel

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            VStack(alignment: .leading, spacing: 4) {
                Text(Self.title).zoomFont(size: 20, weight: .semibold).accessibilityAddTraits(.isHeader)
                Text(Self.detail).zoomFont(size: 13).foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if tools.report == nil {
                Text("Loading providers…").zoomFont(size: 12).foregroundStyle(CloudTheme.secondary)
            } else if tools.availableProviders.isEmpty {
                Text("All providers are already in the sidebar.")
                    .zoomFont(size: 12).foregroundStyle(CloudTheme.secondary)
            } else {
                SettingsSection(title: "Available providers") {
                    VStack(spacing: 0) {
                        ForEach(Array(tools.availableProviders.enumerated()), id: \.element.id) { index, provider in
                            if index > 0 { SettingsDivider(leadingInset: 50) }
                            HStack(spacing: 12) {
                                AgentIconView(source: provider.id, size: 24).accessibilityHidden(true)
                                Text(provider.name).zoomFont(size: 13, weight: .medium)
                                Spacer(minLength: 12)
                                Button("Add") { Task { await tools.manage(provider.id, action: "enable") } }
                                    .disabled(tools.isWorking)
                                    .accessibilityLabel("Add \(provider.name)")
                            }
                            .zoomPadding(.horizontal, 14).zoomPadding(.vertical, 10)
                        }
                    }
                }
            }
        }
    }
}
