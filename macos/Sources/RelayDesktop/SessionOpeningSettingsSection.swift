import AppKit
import SwiftUI
import RelayDesktopCore

/// Where this provider's local sessions open, on this computer only.
struct SessionOpeningSettingsSection: View {
    static let title = "Local sessions"
    static let rowTitle = "Open with"
    static let help = "Automatic opens the desktop app when installed, otherwise Terminal. Applies only to this computer."

    let source: String
    private let applications = SessionApplications.shared
    @AppStorage("sessionOpening.claude") private var claude: SessionOpenPreference = .automatic
    @AppStorage("sessionOpening.codex") private var codex: SessionOpenPreference = .automatic

    var body: some View {
        SettingsSection(title: Self.title) {
            switch source {
            case "claude":
                row(name: "Claude Code", detail: applications.claudeDesktop ? "Claude Desktop is installed" : "Claude Desktop isn’t installed. Sessions open in Terminal.", preference: $claude)
            case "codex":
                row(name: "Codex", detail: applications.codexDesktop ? "Codex is installed" : "Codex isn’t installed. Sessions open in Terminal.", preference: $codex)
            case "grok":
                HStack(spacing: 14) {
                    SettingsIcon(systemImage: "terminal")
                    Text(Self.rowTitle).zoomFont(.body, weight: .medium)
                    Spacer(minLength: 12)
                    Text("Terminal").foregroundStyle(CloudTheme.secondary)
                }
                .zoomPadding(.horizontal, 16).zoomPadding(.vertical, 12)
                .accessibilityElement(children: .combine)
            default: EmptyView()
            }
        }
        .onAppear { applications.refresh() }
    }

    private func row(name: String, detail: String, preference: Binding<SessionOpenPreference>) -> some View {
        HStack(alignment: .center, spacing: 14) {
            SettingsIcon(systemImage: "arrow.up.forward.app")
            VStack(alignment: .leading, spacing: 2) {
                Text(Self.rowTitle).zoomFont(.body, weight: .medium)
                Text(detail).zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 12)
            Picker(Self.rowTitle, selection: preference) {
                ForEach(SessionOpenPreference.allCases) { Text($0.title).tag($0) }
            }
            .labelsHidden().fixedSize().actionCursor()
            .accessibilityLabel("Open \(name) sessions")
            .help(Self.help)
        }
        .zoomPadding(.horizontal, 16).zoomPadding(.vertical, 12)
    }
}

/// What opening a local session needs, collapsed beside the other diagnostics.
struct SessionOpeningRequirements: View {
    static let title = "Opening requirements"

    let source: String
    @Binding var isExpanded: Bool
    private let applications = SessionApplications.shared

    var body: some View {
        DisclosureGroup(Self.title, isExpanded: $isExpanded) {
            VStack(alignment: .leading, spacing: 10) {
                if source == "claude" {
                    Text("Opening in Claude requires Claude Code 2.1.285 or later and a Claude subscription. If a session is still open elsewhere, close it there first or choose Terminal.")
                }
                Text("Open returns to a verified running app or terminal pane first. Your preference applies when resuming a closed session. Terminal resume requires the matching CLI and the session’s original folder. Older sessions may have no recorded location.")
                HStack(spacing: 12) {
                    if source != "grok" {
                        Button("Check installed apps") { applications.refresh() }
                    }
                    if source == "claude" {
                        if !applications.claudeDesktop {
                            Link("Download Claude Desktop", destination: URL(string: "https://claude.com/download")!)
                                .actionCursor()
                        }
                        Link("Set up Claude Code", destination: URL(string: "https://code.claude.com/docs/en/quickstart")!)
                            .actionCursor()
                    }
                }
            }
            .zoomFont(size: 12).foregroundStyle(CloudTheme.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading).zoomPadding(.top, 10)
        }
        .zoomFont(size: 13).foregroundStyle(CloudTheme.secondary)
    }
}
