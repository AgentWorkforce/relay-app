import AppKit
import RelayDesktopCore
import SwiftUI

/// One global header; Settings alone adds contextual navigation below it.
struct WorkspaceShell<Content: View>: View {
    @Environment(AppModel.self) private var model
    var relay: RelayModel
    var updater: AppUpdater
    @ViewBuilder let content: () -> Content

    var body: some View {
        let _ = BodyEvaluations.record("WorkspaceShell")
        VStack(spacing: 0) {
            globalHeader
            Divider().hidden().overlay(CloudTheme.border)
            if model.needsSignIn {
                signInAgainBanner
                Divider().hidden().overlay(CloudTheme.border)
            }
            HStack(spacing: 0) {
                if model.page.isSettings {
                    sidebar
                    Divider().hidden().overlay(CloudTheme.border)
                }
                VStack(spacing: 0) {
                    if model.page.isSettings || (model.page != .sessions && model.selectedLocalSession == nil) {
                        topBar
                        Divider().hidden().overlay(CloudTheme.border)
                    }
                    content()
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            Divider().hidden().overlay(CloudTheme.border)
            WorkspaceStatusBar(relay: relay)
        }
        .alert("Could not change Receive messages", isPresented: Binding(
            get: { relay.receiveMessagesError != nil },
            set: { if !$0 { relay.receiveMessagesError = nil } }
        )) {
            Button("OK") { relay.receiveMessagesError = nil }
        } message: {
            Text(relay.receiveMessagesError ?? "")
        }
        .overlay(alignment: .bottomTrailing) {
            if let upgrade = model.relaycastUpgrade {
                VStack(alignment: .leading, spacing: 10) {
                    HStack {
                        Label(upgrade.title, systemImage: "exclamationmark.triangle")
                            .zoomFont(.headline)
                        Spacer()
                        Button { model.dismissRelaycastUpgrade() } label: {
                            Image(systemName: "xmark")
                        }.buttonStyle(.plain).accessibilityLabel("Dismiss upgrade notice")
                    }
                    Text(upgrade.message).zoomFont(.subheadline)
                    Button(RelaycastUpgrade.action) { model.openRelaycastBilling() }
                        .buttonStyle(AgentRelayPrimaryButtonStyle())
                }
                .zoomPadding(16)
                .zoomFrame(width: 360)
                .background {
                    RoundedRectangle(cornerRadius: 12).fill(.regularMaterial).shadow(radius: 8)
                }
                .zoomPadding(.trailing, 20)
                .zoomPadding(.bottom, 44)
            }
        }
    }

    // MARK: Sign in again

    /// Shown while the core has no Cloud session to act as the person.
    private var signInAgainBanner: some View {
        HStack(spacing: 12) {
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
            VStack(alignment: .leading, spacing: 2) {
                Text("Agent Relay needs you to sign in again.")
                    .zoomFont(size: 12, weight: .semibold)
                Text("Messages, session summaries and subscriptions wait until you do.")
                    .zoomFont(size: 11).foregroundStyle(CloudTheme.secondary)
            }
            Spacer(minLength: 0)
            Button(model.renewingSignIn ? "Waiting for the browser…" : "Sign in again") { model.renewSignIn() }
                .buttonStyle(AgentRelayPrimaryButtonStyle())
                .disabled(model.renewingSignIn)
        }
        .zoomPadding(.horizontal, 24)
        .zoomPadding(.vertical, 10)
        .background(Color.orange.opacity(0.08))
    }

    // MARK: Global header

    private func currentTitleRow(_ row: LocalSessionRow) -> LocalSessionRow {
        model.sessionSearchSource.rows(model: model, relay: relay).values.first { $0.id == row.id } ?? row
    }

    private var globalHeader: some View {
        @Bindable var model = model
        return HStack(spacing: 20) {
            Button { model.showLocalSessions() } label: {
                HStack(spacing: 10) {
                    BrandMarkView(width: 24)
                    BrandWordmarkView(height: 19)
                        .foregroundStyle(CloudTheme.foreground)
                }.zoomFrame(height: 44)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Sessions")
            if !model.page.isSettings && model.selectedLocalSession == nil {
                SessionHeaderSearch(relay: relay)
            } else if !model.page.isSettings, let row = model.selectedLocalSession {
                HStack(spacing: 12) {
                    backToSessionsButton
                    Divider().zoomFrame(height: 28).overlay(CloudTheme.border)
                    AgentIconView(source: row.iconSource, size: 26)
                    VStack(alignment: .leading, spacing: 3) {
                        SessionTitleEditor(state: relay.titleState(for: currentTitleRow(row)),
                            preload: { relay.preloadTitle(row) },
                            save: { title in Task { await relay.setSessionTitle(title, for: currentTitleRow(row)) } })
                            .id(row.id)
                        SessionHeaderIdentity(row: row, relay: relay)
                    }
                    Spacer(minLength: 0)
                    Button { model.showsSessionDetails.toggle() } label: {
                        Label("Details", systemImage: "sidebar.right")
                    }
                    .buttonStyle(CloudControlStyle(flat: true, height: 32))
                    .accessibilityValue(model.showsSessionDetails ? "Expanded" : "Collapsed")
                }.frame(maxWidth: .infinity, alignment: .leading)
            } else {
                backToSessionsButton
                Spacer(minLength: 0)
            }
            HStack(spacing: 6) {
                if !model.page.isSettings && model.selectedLocalSession == nil {
                    SavedSessionViewControls()
                    SessionColumnSettings(layout: $model.sessionColumns, grouping: $model.sessionGrouping, showHidden: $model.showHiddenSessions)
                }
                HeaderAccountButton(relay: relay, updater: updater)
            }
        }
        .zoomPadding(.horizontal, 24)
        .zoomFrame(height: 64)
        .background(CloudTheme.sidebar)
    }

    private var backToSessionsButton: some View {
        Button { model.showLocalSessions() } label: {
            Label("Back to sessions", systemImage: "chevron.left")
        }
        .buttonStyle(CloudControlStyle(flat: true, height: 32))
        .fixedSize().help("Back to sessions")
    }

    // MARK: Sidebar

    private var sidebar: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 2) {
                    settingsSidebar
                }
                .zoomPadding(10)
            }
        }
        .zoomFrame(width: 244)
        .background(CloudTheme.sidebar)
    }

    @ViewBuilder private var settingsSidebar: some View {
        ForEach(Array(SettingsPane.groups.enumerated()), id: \.offset) { index, group in
            sectionTitle(group.title).zoomPadding(.top, index == 0 ? 0 : 14)
            ForEach(group.panes) { pane in
                sidebarRow(pane.title, symbol: pane.symbol, selected: model.page == .settings(pane),
                           badge: pane == .about && updater.state.isAvailable ? "Update" : nil) {
                    model.page = .settings(pane)
                }
            }
            if index == 0 {
                ProvidersSidebar(tools: model.tools, isSelected: model.page == .settings(.tools)) {
                    model.page = .settings(.tools)
                }.zoomPadding(.top, 14)
            }
        }
    }

    private func sectionTitle(_ title: String) -> some View {
        Text(title).zoomFont(size: 11, weight: .medium)
            .foregroundStyle(CloudTheme.secondary).zoomPadding(.horizontal, 10).zoomPadding(.top, 6).zoomPadding(.bottom, 4)
            .accessibilityAddTraits(.isHeader)
    }

    private func sidebarRow(_ title: String, symbol: String? = nil, icon: AnyView? = nil, selected: Bool = false,
                            count: Int? = nil, live: Bool = false, badge: String? = nil,
                            action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 9) {
                Group {
                    if let icon { icon } else if let symbol { Image(systemName: symbol) }
                }
                .zoomFrame(width: 18)
                Text(title).lineLimit(1).foregroundStyle(selected ? CloudTheme.selectedText : CloudTheme.foreground)
                Spacer(minLength: 4)
                if let badge {
                    Circle().fill(CloudTheme.primary).zoomFrame(width: 6, height: 6)
                        .accessibilityLabel(badge + " available")
                        .help(badge + " available")
                }
                if let count {
                    HStack(spacing: 4) {
                        if live { Circle().fill(CloudTheme.success).zoomFrame(width: 5, height: 5) }
                        Text(count.formatted()).zoomFont(.caption).monospacedDigit()
                    }
                    .foregroundStyle(CloudTheme.secondary)
                }
            }
            .zoomFont(size: 13, weight: selected ? .medium : .regular)
            .zoomPadding(.horizontal, 10).zoomFrame(height: 34)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .buttonStyle(NavButtonStyle(kind: .sidebar, isSelected: selected))
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    // MARK: Top bar

    private var topBar: some View {
        HStack(spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(model.page.title).zoomFont(size: 15, weight: .semibold).lineLimit(1)
                if !model.page.subtitle.isEmpty {
                    Text(model.page.subtitle).zoomFont(size: 12).foregroundStyle(CloudTheme.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                        .help(model.page.subtitle)
                }
            }
            Spacer(minLength: 12)
        }
        .zoomPadding(.horizontal, 28).zoomFrame(height: 64)
    }

}

/// The window's footer: the machine's session count and live agents. Its
/// own view, so a count changing re-renders only it.
struct WorkspaceStatusBar: View {
    @Environment(AppModel.self) private var model
    var relay: RelayModel

    var body: some View {
        let _ = BodyEvaluations.record("WorkspaceStatusBar")
        HStack(spacing: 4) {
            Text(model.localSessionCountText)
                .foregroundStyle(CloudTheme.secondary)
                .zoomPadding(.horizontal, 8)
            Spacer(minLength: 8)
            Button { model.showLocalSessions(search: "is:active is:on-relay") } label: {
                HStack(spacing: 6) {
                    Circle().fill(liveRelayCount > 0 ? CloudTheme.success : CloudTheme.secondary)
                        .zoomFrame(width: 6, height: 6).accessibilityHidden(true)
                    Text("Live Agents on Relay: \(liveRelayCount)")
                }
                .zoomPadding(.horizontal, 8).zoomFrame(height: 22)
            }
            .buttonStyle(NavButtonStyle(kind: .icon))
            .help("Show live sessions on the relay")

        }
        .zoomFont(size: 11)
        .zoomPadding(.horizontal, 6).zoomFrame(height: 28)
        .background(CloudTheme.card)
    }

    private var liveRelayCount: Int { relay.liveOnRelayCount }
}

/// Whether messages and uploads can flow, shown by the header's account dot.
struct WorkspaceConnection {
    let label: String
    let color: Color
    let help: String
    let isHealthy: Bool

    @MainActor
    init(model: AppModel, relay: RelayModel) {
        if let problem = model.coreProblem {
            self.init("Needs attention", CloudTheme.warning, problem)
        } else if model.phase != .launching && !model.isCoreConnected {
            self.init("Reconnecting", CloudTheme.warning, "Agent Relay is reconnecting to its background service.")
        } else if model.isPaused {
            self.init("Paused", CloudTheme.warning, "Agents don't receive messages and nothing uploads.")
        } else if let problem = model.relayProblem {
            self.init("Needs attention", CloudTheme.warning, problem)
        } else if relay.workspace == nil {
            self.init("No relay workspace", CloudTheme.warning, "Connect a workspace in Settings › Messages.")
        } else if model.phase == .preparingSessions {
            self.init("Preparing sessions", .accentColor, "Reading your local history.")
        } else if model.phase != .attached {
            self.init("Not connected", CloudTheme.warning, "Agent Relay is not connected to its background service.")
        } else if model.isCheckingAccountHealth {
            self.init("Checking sign-in", CloudTheme.warning, "Checking your account and workspace access.")
        } else if let error = model.accountHealthError {
            self.init("Health check failed", CloudTheme.warning, error)
        } else {
            let health = AccountHealthPresentation(model.account)
            self.init(health.label, health.isHealthy ? CloudTheme.success : CloudTheme.warning, health.detail, healthy: health.isHealthy)
        }
    }

    private init(_ label: String, _ color: Color, _ help: String, healthy: Bool = false) {
        self.label = label
        self.color = color
        self.help = help
        isHealthy = healthy
    }
}

extension AppUpdateState {
    var isAvailable: Bool { if case .available = self { return true }; return false }
}

struct HeaderAccountButton: View {
    var relay: RelayModel
    var updater: AppUpdater
    @Environment(AppModel.self) private var model
    @State private var isShowingMenu = false
    @State private var isConfirmingLogOut = false
    @State private var isConfirmingWorkspaceSignOut = false

    private var name: String { model.accountName?.trimmingCharacters(in: .whitespaces).nilIfBlank ?? email }
    private var email: String { model.accountEmail?.trimmingCharacters(in: .whitespaces).nilIfBlank ?? "Signed in" }
    private var connection: WorkspaceConnection { WorkspaceConnection(model: model, relay: relay) }

    var body: some View {
        let _ = BodyEvaluations.record("HeaderAccountButton")
        Button { isShowingMenu.toggle() } label: {
            PersonAvatar(name: name, url: model.accountAvatarURL.flatMap(URL.init(string:)), size: 30)
                .overlay(alignment: .bottomTrailing) {
                    Circle().fill(connection.color).zoomFrame(width: 7, height: 7)
                        .background(Circle().fill(CloudTheme.sidebar).zoomPadding(-2))
                }
                .zoomFrame(width: 40, height: 40)
        }
        .buttonStyle(NavButtonStyle(kind: .icon, isSelected: isShowingMenu))
        .help("Signed in as \(email)")
        .accessibilityLabel("Account: signed in as \(email)")
        .popover(isPresented: $isShowingMenu, arrowEdge: .bottom) {
            HeaderAccountMenu(model: model, updater: updater, connection: connection,
                              name: name, email: email, isShowingMenu: $isShowingMenu,
                              isConfirmingLogOut: $isConfirmingLogOut,
                              isConfirmingWorkspaceSignOut: $isConfirmingWorkspaceSignOut)
        }
        .confirmationDialog("Sign out to switch workspace?", isPresented: $isConfirmingWorkspaceSignOut) {
            Button("Sign out", role: .destructive) { Task { await model.disconnect() } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Uploads stop and stay tied to \(model.workspaceName). Sign back in to that workspace to resume without uploading existing history again.")
        }
        .confirmationDialog("Log out of Agent Relay?", isPresented: $isConfirmingLogOut) {
            Button("Log out", role: .destructive) { Task { await model.disconnect() } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Uploads stop and you will need to sign in again to reconnect.")
        }
    }
}

struct HeaderAccountMenu: View {
    var model: AppModel
    var updater: AppUpdater
    let connection: WorkspaceConnection
    let name: String
    let email: String
    @Binding var isShowingMenu: Bool
    @Binding var isConfirmingLogOut: Bool
    @Binding var isConfirmingWorkspaceSignOut: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 10) {
                PersonAvatar(name: name, url: model.accountAvatarURL.flatMap(URL.init(string:)), size: 34)
                VStack(alignment: .leading, spacing: 2) {
                    if model.accountName != nil { Text(name).zoomFont(.callout, weight: .semibold) }
                    Text(email).zoomFont(.caption).foregroundStyle(CloudTheme.secondary).textSelection(.enabled)
                }
            }
            .zoomPadding(.bottom, 4)
            HStack(spacing: 6) {
                Circle().fill(connection.color).zoomFrame(width: 6, height: 6)
                Text(connection.isHealthy ? "Signed in · \(model.workspaceName)" : connection.help)
                    .lineLimit(2)
            }
            .zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
            Divider().hidden().overlay(CloudTheme.border).zoomPadding(.vertical, 4)
            if model.hasMultipleWorkspaces {
                Button {
                    isShowingMenu = false
                    isConfirmingWorkspaceSignOut = true
                } label: {
                    Label("Switch workspace…", systemImage: "square.stack.3d.up")
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .buttonStyle(MenuPanelButtonStyle(kind: .quiet))
                .disabled(!model.canLogOut)
                Divider().hidden().overlay(CloudTheme.border).zoomPadding(.vertical, 4)
            }
            Button {
                isShowingMenu = false
                model.page = .settings(.general)
            } label: {
                Label("Settings…", systemImage: "gearshape").frame(maxWidth: .infinity, alignment: .leading)
            }
            .buttonStyle(MenuPanelButtonStyle(kind: .quiet))
            if updater.state.isAvailable {
                Button {
                    isShowingMenu = false
                    model.page = .settings(.about)
                } label: {
                    Label("Update available", systemImage: "arrow.down.circle")
                        .frame(maxWidth: .infinity, alignment: .leading)
                }.buttonStyle(MenuPanelButtonStyle(kind: .quiet))
            }
            Button(role: .destructive) {
                isShowingMenu = false
                isConfirmingLogOut = true
            } label: {
                Label("Log out…", systemImage: "rectangle.portrait.and.arrow.right")
                    .foregroundStyle(CloudTheme.danger).frame(maxWidth: .infinity, alignment: .leading)
            }
            .buttonStyle(MenuPanelButtonStyle(kind: .quiet))
            .disabled(!model.canLogOut)
        }
        .zoomPadding(14)
        .zoomFrame(width: 270, alignment: .leading)
    }
}

private extension String {
    var nilIfBlank: String? { isEmpty ? nil : self }
}

/// Authentication is checked by the shared core, including while the app is closed.
struct AccountHealthPresentation {
    let label: String
    let detail: String
    let isHealthy: Bool

    init(_ account: AccountStatus) {
        isHealthy = account.authHealth == "ready"
        switch account.authHealth {
        case "ready":
            label = "Connected"
            detail = "Account and workspace access verified. Sign-in is checked automatically."
        case "sign_in_required":
            label = "Sign in required"
            detail = account.authError ?? "Sign in again to restore account and workspace access."
        case "offline":
            label = "Cannot verify sign-in"
            detail = account.authError ?? "Cannot reach Agent Relay. Sign-in will be checked again automatically."
        default:
            label = "Checking sign-in"
            detail = "Checking your account and workspace access."
        }
    }
}
