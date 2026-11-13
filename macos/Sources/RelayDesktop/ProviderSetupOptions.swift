import SwiftUI

/// Hosts reuse their agents' configuration; cloud providers keep their existing setup.
struct ProviderSetupOptions: View {
    let providerID: String
    var tools: ToolsModel

    static func agentProviders(for id: String) -> [(id: String, title: String)] {
        switch id {
        case "conductor": return [("codex", "Codex settings"), ("claude", "Claude Code settings")]
        case "claude-cloud": return [("claude", "Claude Code settings")]
        case "paseo": return [("codex", "Codex settings"), ("claude", "Claude Code settings"),
                               ("grok", "Grok settings"), ("muse", "Muse Code settings")]
        default: return []
        }
    }

    var body: some View {
        let agents = Self.agentProviders(for: providerID)
        if !agents.isEmpty {
            SettingsSection(title: "Agent settings") {
                VStack(spacing: 0) {
                    ForEach(Array(agents.enumerated()), id: \.element.id) { index, agent in
                        if index > 0 { SettingsDivider(leadingInset: 50) }
                        Button { tools.showProvider(agent.id) } label: {
                            HStack(spacing: 12) {
                                AgentIconView(source: agent.id, size: 22).accessibilityHidden(true)
                                Text(agent.title).zoomFont(.body, weight: .medium).foregroundStyle(CloudTheme.foreground)
                                Spacer(minLength: 12)
                                Image(systemName: "chevron.right").zoomFont(size: 11, weight: .semibold)
                                    .foregroundStyle(CloudTheme.secondary).accessibilityHidden(true)
                            }
                            .zoomPadding(.horizontal, 14).zoomPadding(.vertical, 11)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(HoverPlainButtonStyle(shape: Rectangle()))
                        .disabled(tools.report?.tools.contains { $0.id == agent.id } != true)
                    }
                }
            }
        }
    }
}
