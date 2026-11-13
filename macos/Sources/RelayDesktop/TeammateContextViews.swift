import RelayDesktopCore
import SwiftUI

/// The first-run chooser's teammate-context line (unticked until the person ticks it).
struct TeammateContextChoice: View {
    var tools: ToolsModel

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Toggle(TeammateContextCopy.title, isOn: Binding(
                get: { tools.teammateContextChoice },
                set: { tools.setTeammateContextChoice($0) }
            ))
            .labelsHidden().toggleStyle(.checkbox)
            .disabled(tools.isWorking)
            .accessibilityLabel(TeammateContextCopy.title)
            VStack(alignment: .leading, spacing: 4) {
                Text(TeammateContextCopy.title).zoomFont(size: 13, weight: .semibold)
                Text(TeammateContextCopy.detail).zoomFont(size: 12).foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}

/// Settings › Providers › Claude Code, Codex, Copilot or Devin: teammate context.
struct TeammateContextSettings: View {
    var tools: ToolsModel

    var body: some View {
        if let enabled = tools.teammateContextEnabled {
            SettingsSection(title: "Teammates") {
                HStack(alignment: .top, spacing: 14) {
                    SettingsIcon(systemImage: enabled ? "checkmark.seal" : "person.2")
                    VStack(alignment: .leading, spacing: 4) {
                        Text(TeammateContextCopy.title).zoomFont(.body, weight: .medium)
                        Text(TeammateContextCopy.detail)
                            .zoomFont(.footnote).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                        if let error = tools.teammateContextError {
                            Label(error, systemImage: "exclamationmark.circle.fill")
                                .zoomFont(.footnote).foregroundStyle(.red)
                                .fixedSize(horizontal: false, vertical: true).zoomPadding(.top, 2)
                        }
                    }
                    Spacer(minLength: 12)
                    if enabled {
                        Button("Turn Off") { Task { await tools.setTeammateContext(false) } }
                            .help(TeammateContextCopy.turnOffHelp)
                            .disabled(tools.isWorking)
                    } else {
                        Button("Turn On") { Task { await tools.setTeammateContext(true) } }
                            .help(TeammateContextCopy.turnOnHelp)
                            .disabled(tools.isWorking)
                    }
                }
                .zoomPadding(16)
            }
        }
    }
}
