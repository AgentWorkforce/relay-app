import AppKit
import RelayDesktopCore
import SwiftUI

// MARK: - Claude Code folders

/// Every Claude Code config folder whose sessions this computer finds and
/// uploads, and why; folders added here can be removed here. The folders sit
/// behind one summary row; the upload explanation opens with the switches.
struct ClaudeFoldersSection: View {
    static let title = "Sharing"

    let folders: [ClaudeConfigDir]
    @Binding var isExpanded: Bool
    @Environment(AppModel.self) private var model
    @State private var error: String?

    /// How many watched folders are shared, in a few words.
    static func summary(_ folders: [ClaudeConfigDir]) -> String {
        let shared = folders.filter(\.shared).count
        if folders.isEmpty { return "No folders found" }
        if shared == 0 { return ClaudeFolders.notShared }
        if shared == folders.count { return folders.count == 1 ? "Shared" : "All \(folders.count) shared" }
        return "\(shared) of \(folders.count) shared"
    }

    var body: some View {
        SettingsSection(title: Self.title) {
            VStack(spacing: 0) {
                disclosureRow
                if isExpanded { folderRows }
                cloudRow
                if let error {
                    SettingsDivider()
                    Label(error, systemImage: "exclamationmark.circle.fill")
                        .zoomFont(.footnote).foregroundStyle(.red)
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .zoomPadding(16)
                }
            }
        }
    }

    private var disclosureRow: some View {
        Button { isExpanded.toggle() } label: {
            HStack(alignment: .center, spacing: 14) {
                SettingsIcon(systemImage: "folder")
                Text(ClaudeFolders.title).zoomFont(.body, weight: .medium).foregroundStyle(CloudTheme.foreground)
                Spacer(minLength: 12)
                Text(Self.summary(folders)).zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                Image(systemName: "chevron.right").zoomFont(size: 11, weight: .semibold)
                    .foregroundStyle(CloudTheme.secondary)
                    .rotationEffect(.degrees(isExpanded ? 90 : 0))
                    .accessibilityHidden(true)
            }
            .zoomPadding(.horizontal, 16).zoomPadding(.vertical, 12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(HoverPlainButtonStyle(shape: Rectangle()))
        .accessibilityLabel(ClaudeFolders.title)
        .accessibilityValue("\(Self.summary(folders)), \(isExpanded ? "expanded" : "collapsed")")
        .accessibilityHint(isExpanded ? "Hides the folders" : "Shows the folders")
    }

    @ViewBuilder private var folderRows: some View {
        Text(ClaudeFolders.detail)
            .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .zoomPadding(.leading, 54).zoomPadding(.trailing, 16).zoomPadding(.bottom, 8)
        ForEach(folders, id: \.path) { folder in
            SettingsDivider()
            HStack(alignment: .center, spacing: 14) {
                SettingsIcon(systemImage: "folder")
                VStack(alignment: .leading, spacing: 2) {
                    Text(ClaudeFolders.displayPath(folder.path, home: NSHomeDirectory()))
                        .zoomFont(.body, weight: .medium)
                        .lineLimit(1).truncationMode(.middle)
                        .textSelection(.enabled)
                        .help(folder.path)
                    Text(ClaudeFolders.rowDetail(folder))
                        .zoomFont(.footnote).foregroundStyle(.secondary)
                }
                Spacer(minLength: 12)
                if ClaudeFolders.isRemovable(folder) {
                    Button(ClaudeFolders.removeLabel) {
                        set(ClaudeFolders.removing(folder.path, from: folders))
                    }
                }
                Toggle(ClaudeFolders.shareHelp, isOn: Binding(
                    get: { folder.shared },
                    set: { share(folder.path, $0) }
                ))
                .toggleStyle(.switch)
                .labelsHidden()
                .actionCursor()
                .help(ClaudeFolders.shareHelp)
            }
            .zoomPadding(.horizontal, 16).zoomPadding(.vertical, 12)
        }
        SettingsDivider()
        HStack {
            Spacer(minLength: 0)
            Button(ClaudeFolders.addLabel) { chooseFolder() }
                .help(ClaudeFolders.addHelp)
        }
        .zoomPadding(.horizontal, 16).zoomPadding(.vertical, 10)
    }

    @ViewBuilder private var cloudRow: some View {
        if let cloud = model.settings?.claudeCloudSessions {
            SettingsDivider()
            HStack(alignment: .top, spacing: 14) {
                SettingsIcon(systemImage: "cloud")
                VStack(alignment: .leading, spacing: 2) {
                    Text(ClaudeFolders.cloudTitle).zoomFont(.body, weight: .medium)
                    Text(ClaudeFolders.cloudDetail)
                        .zoomFont(.footnote).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    if cloud {
                        let health = model.status?.claudeCloud
                        Label(ClaudeCloudHealthCopy.label(health), systemImage: healthIcon(health))
                            .zoomFont(.footnote, weight: .semibold)
                            .foregroundStyle(healthColor(health))
                            .zoomPadding(.top, 4)
                        Text(ClaudeCloudHealthCopy.detail(health))
                            .zoomFont(.footnote).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                Spacer(minLength: 12)
                Toggle(ClaudeFolders.cloudTitle, isOn: Binding(
                    get: { cloud },
                    set: { on in
                        Task { error = await model.setSetting(SettingsChange(claudeCloudSessions: on)) }
                    }
                ))
                .toggleStyle(.switch)
                .labelsHidden()
                .actionCursor()
            }
            .zoomPadding(.horizontal, 16).zoomPadding(.vertical, 12)
        }
    }

    private func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.showsHiddenFiles = true
        panel.directoryURL = URL(fileURLWithPath: NSHomeDirectory())
        guard panel.runModal() == .OK, let url = panel.url else { return }
        set(ClaudeFolders.adding(url.path, to: folders))
    }

    private func set(_ added: [String]) {
        Task { error = await model.setSetting(SettingsChange(claudeConfigDirs: added)) }
    }

    private func share(_ path: String, _ shared: Bool) {
        Task {
            error = await model.setSetting(SettingsChange(claudeConfigDir: ClaudeConfigDirChange(path: path, shared: shared)))
        }
    }

    private func healthIcon(_ health: ClaudeCloudHealth?) -> String {
        switch health?.state {
        case .working?: return "checkmark.circle.fill"
        case .checking?, nil: return "clock.fill"
        case .blocked?, .needsAttention?: return "exclamationmark.triangle.fill"
        }
    }

    private func healthColor(_ health: ClaudeCloudHealth?) -> Color {
        switch health?.state {
        case .working?: return .green
        case .checking?, nil: return .secondary
        case .blocked?, .needsAttention?: return .orange
        }
    }
}

