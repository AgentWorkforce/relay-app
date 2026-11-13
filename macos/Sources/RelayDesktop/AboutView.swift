import AppKit
import RelayDesktopCore
import SwiftUI

/// An allowlisted diagnostic report: never serialize the account or settings
/// wholesale, since future fields may contain credentials or private content.
struct DesktopDiagnostics {
    struct Section {
        let title: String
        let rows: [Row]
    }
    struct Row {
        let label: String
        let value: String
        init(_ label: String, _ value: String?) {
            self.label = label
            self.value = value.flatMap { $0.isEmpty ? nil : $0 } ?? "Unavailable"
        }
    }

    static func endpoint(_ value: String?) -> String? {
        guard let value, var url = URLComponents(string: value),
              ["http", "https"].contains(url.scheme ?? ""), url.host != nil else { return nil }
        url.user = nil
        url.password = nil
        url.query = nil
        url.fragment = nil
        return url.string
    }

    static func relaycast(_ status: Status?, connected: Bool) -> String {
        guard connected, let status else { return "Unavailable · service disconnected" }
        guard status.workspace.state == .found else { return "Not configured" }
        switch status.workspace.verification {
        case .rejected: return "Credentials rejected"
        case .unreachable: return "Unreachable"
        case .pending: return "Checking connection"
        case .verified:
            let live = status.push.values.filter { $0 == .live }.count
            if status.push.isEmpty { return "Ready · no active channels" }
            if live == status.push.count { return "Connected · \(live) live channels" }
            if live > 0 { return "Partially connected · \(live)/\(status.push.count) live channels" }
            return "Reconnecting · no live channels"
        }
    }

    /// The core crates the app was built with, in the same words as GTK's
    /// About (`gtk/src/models/settings.rs`).
    static func crateRows(_ crates: AppInfo.CoreCrates?) -> [Row] {
        [
            Row("agent-relay-node crate", crates.map { String($0.agentRelayNode.prefix(12)) }),
            Row("relaycast crate", crates?.relaycast),
        ]
    }

    /// The Build section's rows; the crate rows follow the bundled service.
    static func buildRows(_ snapshot: Status?, crates: AppInfo.CoreCrates?) -> [Row] {
        [
            Row("Desktop", AppInfo.displayVersion),
            Row("Desktop Git revision", AppInfo.buildRevision),
            Row("Running service", snapshot?.daemon.version),
            Row("Service Git revision", snapshot?.daemon.buildRevision),
            Row("Bundled service", AppInfo.bundledCoreVersion),
        ] + crateRows(crates) + [
            Row("RelayHistory (ai-hist)", snapshot?.daemon.historyVersion),
            Row("Bridge protocol", snapshot.map { String($0.daemon.bridgeVersion) }),
            Row("macOS", ProcessInfo.processInfo.operatingSystemVersionString),
            Row("Architecture", architecture),
        ]
    }

    @MainActor
    static func sections(for model: AppModel) -> [Section] {
        let snapshot = model.core
        let service: String
        switch model.backgroundServiceStatus {
        case .enabled: service = "Enabled"
        case .requiresApproval: service = "Needs approval in Login Items"
        case .notRegistered: service = "Not registered"
        case .notFound: service = "Not installed"
        }
        return [
            Section(title: "Build", rows: buildRows(snapshot, crates: AppInfo.coreCrates)),
            Section(title: "Connections", rows: [
                Row("Background service", model.isCoreConnected ? "Connected" : "Disconnected"),
                Row("Login Items registration", service),
                Row("Service process ID", snapshot.map { String($0.daemon.pid) }),
                Row("Service started", snapshot.map { Date(timeIntervalSince1970: Double($0.daemon.startedAtMs) / 1000).formatted() }),
                Row("Agent Relay", relaycast(snapshot, connected: model.isCoreConnected)),
                Row("Agent Relay endpoint", endpoint(snapshot?.workspace.baseUrl)),
                Row("Agent Relay last checked", snapshot?.workspace.checkedAtMs.map { Date(timeIntervalSince1970: Double($0) / 1000).formatted() }),
                Row("Cloud site", endpoint(model.siteURL)),
                Row("Sign-in", model.isCoreConnected ? model.account.state.rawValue : "Unavailable · service disconnected"),
                Row("Last link failure", model.linkError ?? "None"),
            ]),
            Section(title: "Workspace", rows: [
                Row("Account ID", model.accountID),
                Row("Cloud workspace ID", model.workspaceID),
                Row("Agent Relay workspace ID", snapshot?.workspace.workspaceId),
                Row("Agent Relay workspace name", snapshot?.workspace.workspaceName),
                Row("Upload state", model.statusViewModel?.headline),
            ]),
        ]
    }

    static func report(_ sections: [Section]) -> String {
        sections.map { section in
            section.title + "\n" + section.rows.map { "\($0.label): \($0.value)" }.joined(separator: "\n")
        }.joined(separator: "\n\n")
    }

    private static var architecture: String {
        #if arch(arm64)
        return "Apple silicon (arm64)"
        #else
        return "Intel (x86_64)"
        #endif
    }
}

/// About's identity of a build other than production's: its profile, app
/// id, site host and core socket, so it is told apart from Agent Relay and
/// any other build on this Mac. Production's About shows nothing here.
/// Wording matches Linux and Windows.
struct BuildIdentitySection: View {
    var rows: [DesktopDiagnostics.Row] = AppInfo.identityRows

    var body: some View {
        if !rows.isEmpty {
            SettingsSection(
                title: "Build identity",
                description: "How this build is told apart from Agent Relay and other builds on this computer."
            ) {
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(rows, id: \.label) { row in
                        HStack(alignment: .firstTextBaseline, spacing: 24) {
                            Text(row.label).foregroundStyle(CloudTheme.secondary)
                            Spacer(minLength: 12)
                            Text(row.value)
                                .multilineTextAlignment(.trailing)
                                .textSelection(.enabled)
                        }
                        .zoomFont(size: 12)
                    }
                }
                .zoomPadding(16)
            }
        }
    }
}

struct AboutDiagnosticsView: View {
    @Environment(AppModel.self) private var model
    @State private var copied = false

    var body: some View {
        let sections = DesktopDiagnostics.sections(for: model)
        VStack(alignment: .leading, spacing: 20) {
            HStack {
                VStack(alignment: .leading, spacing: 5) {
                    Text("Diagnostic report").zoomFont(.title3, weight: .semibold)
                    Text("Build, connection and workspace details.")
                        .zoomFont(.callout).foregroundStyle(CloudTheme.secondary)
                }
                Spacer()
                Button(copied ? "Copied" : "Copy diagnostics") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(DesktopDiagnostics.report(sections), forType: .string)
                    copied = true
                }
                .task(id: copied) {
                    guard copied else { return }
                    try? await Task.sleep(for: .seconds(2))
                    copied = false
                }
            }
            if !model.isCoreConnected {
                Label("Service offline. Build and workspace details below are from the last received snapshot.", systemImage: "exclamationmark.circle")
                    .zoomFont(.callout).foregroundStyle(CloudTheme.secondary)
            }
            ForEach(sections, id: \.title) { section in
                DisclosureGroup(section.title) {
                    VStack(alignment: .leading, spacing: 14) {
                    ForEach(section.rows, id: \.label) { row in
                        HStack(alignment: .firstTextBaseline, spacing: 24) {
                            Text(row.label).foregroundStyle(CloudTheme.secondary)
                            Spacer(minLength: 12)
                            Text(row.value)
                                .multilineTextAlignment(.trailing)
                                .textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        .zoomFont(size: 12)
                    }
                }
                    .zoomPadding(.top, 16)
                }
                .zoomFont(size: 13, weight: .medium)
                .zoomPadding(16)
                .background(CloudTheme.card, in: RoundedRectangle(cornerRadius: 8))
                .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(CloudTheme.border))
            }
            Text("Includes workspace and account IDs. No credentials, session content, or logs are copied.")
                .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
        }
    }
}
