import SwiftUI

/// One provider: identity and next step, its settings, then diagnostics.
/// Disclosure state lives here, above the polled report, so it survives refreshes.
struct ProviderSettingsPage: View {
    var tools: ToolsModel
    @Environment(AppModel.self) private var model
    /// Open disclosures, as "<provider>.<disclosure>".
    @State private var expanded: Set<String> = []

    static func opensLocalSessions(_ id: String) -> Bool { ["claude", "codex", "grok"].contains(id) }

    private func disclosure(_ provider: String, _ name: String) -> Binding<Bool> {
        let key = "\(provider).\(name)"
        return Binding(
            get: { expanded.contains(key) },
            set: { if $0 { expanded.insert(key) } else { expanded.remove(key) } }
        )
    }

    var body: some View {
        ToolsSettingsView(tools: tools)
        if let provider = tools.displayedProvider {
            VStack(alignment: .leading, spacing: 24) {
                ProviderSetupOptions(providerID: provider.id, tools: tools)
                ProviderSpawnSettings(providerID: provider.id)
                if Self.opensLocalSessions(provider.id) {
                    SessionOpeningSettingsSection(source: provider.id)
                }
                if ["grok", "opencode", "devin", "cursor", "codex", "openclaw", "hermes"].contains(provider.id) {
                    RelaySettingsSection(providerID: provider.id, relay: model.relay)
                }
                if provider.id == "claude" {
                    if let folders = model.settings?.claudeConfigDirs {
                        ClaudeFoldersSection(folders: folders, isExpanded: disclosure(provider.id, "folders"))
                    }
                    if model.phase == .attached || model.phase == .preparingSessions {
                        ClaudeProviderMessages(relay: model.relay)
                        ClaudeDeliverySettings(delivery: model.claudeDelivery)
                    }
                }
                VStack(alignment: .leading, spacing: 10) {
                    ProviderConnectionDetails(provider: provider, tools: tools, isExpanded: disclosure(provider.id, "details"))
                    if Self.opensLocalSessions(provider.id) {
                        SessionOpeningRequirements(source: provider.id, isExpanded: disclosure(provider.id, "requirements"))
                    }
                }
            }.id(provider.id)
        }
    }
}
