import AppKit
import RelayDesktopCore
import SwiftUI

// MARK: - Panes

/// One Settings pane, chosen in the sidebar.
struct SettingsPaneView: View {
    let pane: SettingsPane
    @Environment(AppModel.self) private var model
    var updater = AppUpdater.shared

    private var isConnected: Bool { model.phase == .attached || model.phase == .preparingSessions }

    var body: some View {
        ScrollViewReader { scroll in
            ScrollView {
                VStack(alignment: .leading, spacing: 28) {
                    switch pane {
                    case .general: general
                    case .tools, .opening: ProviderSettingsPage(tools: model.tools)
                    case .relay: relay
                    case .sharing:
                        general
                    case .account:
                        AccountSettingsSection()
                    case .about:
                        SettingsSection(
                            title: "Software updates",
                            description: "Check for new signed and notarized Agent Relay releases."
                        ) {
                            SoftwareUpdateSettingsRow(updater: updater)
                        }
                        BuildIdentitySection()
                    case .diagnostics:
                        RequestDiagnosticsView()
                        TroubleshootingSettingsSection()
                        AboutDiagnosticsView()
                    }
                }
                .zoomFrame(maxWidth: pane == .diagnostics ? .infinity : 720, alignment: .leading)
                .zoomPadding(28)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .onAppear {
                if let id = model.selectedRequest?.id { scroll.scrollTo(id, anchor: .top) }
            }
            .onChange(of: model.selectedRequest?.id) { _, id in
                if let id { withAnimation { scroll.scrollTo(id, anchor: .top) } }
                else if !model.appStatusProblems.isEmpty { scroll.scrollTo("app-problems", anchor: .top) }
            }
        }
        .id(pane)
        .buttonStyle(SettingsActionStyle())
        .foregroundStyle(CloudTheme.foreground)

    }

    @ViewBuilder private var general: some View {
        AppearanceSettingsSection()
        SettingsSection(title: "Startup") {
            VStack(spacing: 0) {
                if LoginItem.isAvailable {
                    SettingsToggleRow(
                        title: "Open at login",
                        detail: "Start Agent Relay in the menu bar and its background service when you log in. When off, neither starts at login.",
                        systemImage: "power",
                        notice: model.loginItemNeedsApproval
                            ? "Allow Agent Relay in System Settings › General › Login Items."
                            : nil,
                        isOn: Binding(
                            get: { model.launchAtLogin },
                            set: { model.setLaunchAtLogin($0) }
                        )
                    )
                    SettingsDivider()
                }

                SettingsToggleRow(
                    title: "Keep agents connected after quitting",
                    detail: "When off, quitting Agent Relay disconnects your agents and stops uploads until you open it again.",
                    systemImage: "bolt.horizontal.circle",
                    isOn: Binding(
                        get: { model.keepsAgentsConnectedAfterQuit },
                        set: { model.setKeepsAgentsConnectedAfterQuit($0) }
                    )
                )

                if model.coreNeedsApproval {
                    SettingsDivider()
                    BackgroundServiceRow()
                }
            }
        }
        GlobalSessionSharingSettings()
        SettingsSection(title: "Privacy") {
            SettingsToggleRow(
                    title: "Share diagnostics",
                    detail: "Send feature usage and error categories, linked to your Agent Relay account after sign-in. They never contain message or session text, searches, paths, URLs, credentials or raw errors. Turning this off stops all of it.",
                    systemImage: "waveform.path.ecg",
                    notice: model.settingErrors["analytics_enabled"],
                    isPending: model.isSettingPending("analytics_enabled"),
                    isOn: Binding(
                        get: { model.analyticsEnabled },
                        set: { model.setAnalyticsEnabled($0) }
                    )
                )
        }
        if let days = model.settings?.historyRetentionDays {
            SettingsSection(title: HistoryRetentionCopy.section) {
                HistoryRetentionRow(
                    days: days,
                    uploadsHeld: !isConnected || model.isPaused,
                    notice: model.settingErrors["history_retention_days"],
                    isPending: model.isSettingPending("history_retention_days"),
                    set: { value in Task { await model.setSetting(SettingsChange(historyRetentionDays: value)) } }
                )
            }
        }
    }

    @ViewBuilder private var relay: some View {
        if isConnected {
            RelaySettingsSection(relay: model.relay)
        }
        SettingsSection(title: "Joining the relay") {
            SettingsToggleRow(
                title: "Let sessions put themselves on the relay",
                detail: "Allow a supported local agent session to register only itself through the private desktop socket.",
                systemImage: "person.badge.plus",
                notice: model.settingErrors["allow_self_registration"],
                isPending: model.isSettingPending("allow_self_registration"),
                isOn: Binding(
                    get: { model.settings?.allowSelfRegistration ?? true },
                    set: { value in Task { await model.setSetting(SettingsChange(allowSelfRegistration: value)) } }
                )
            )
        }
        if model.settings?.acceptSpawns != nil {
            SpawnSettingsSection()
        }
    }


}

/// Agent Relay starting agents here, headless. Whether teammates may spawn
/// here and the maximum show only while spawns are accepted. Provider-specific
/// permissions live on each provider’s settings page.
private struct SpawnSettingsSection: View {
    @Environment(AppModel.self) private var model

    private var accepts: Bool { model.settings?.acceptSpawns ?? false }

    var body: some View {
        SettingsSection(title: SpawnCopy.title, description: SpawnCopy.description) {
            VStack(spacing: 0) {
                SettingsToggleRow(
                    title: SpawnCopy.acceptTitle,
                    detail: SpawnCopy.acceptDetail,
                    systemImage: "sparkles.rectangle.stack",
                    notice: model.settingErrors["accept_spawns"] ?? (accepts ? SpawnCopy.statusLine(model.spawn) : nil),
                    noticeIsStatus: model.settingErrors["accept_spawns"] == nil && SpawnCopy.statusIsHealthy(model.spawn),
                    isPending: model.isSettingPending("accept_spawns"),
                    isOn: Binding(
                        get: { accepts },
                        set: { value in Task { await model.setSetting(SettingsChange(acceptSpawns: value)) } }
                    )
                )
                if accepts {
                    SettingsDivider()
                    SettingsToggleRow(
                        title: SpawnCopy.teammatesTitle,
                        detail: SpawnCopy.teammatesDetail,
                        systemImage: "person.2",
                        notice: SpawnCopy.claimLine(model.spawn?.claim),
                        noticeIsStatus: model.spawn?.claim?.state == .claimed,
                        noticeAction: SpawnCopy.claimCanRetry(model.spawn?.claim)
                            ? (SpawnCopy.claimRetry, { Task { await model.retrySpawnClaim() } })
                            : nil,
                        isPending: model.isSettingPending("spawn_allow_teammates"),
                        isOn: Binding(
                            get: { model.settings?.spawnAllowTeammates ?? false },
                            set: { value in Task { await model.setSetting(SettingsChange(spawnAllowTeammates: value)) } }
                        )
                    )
                    SettingsDivider()
                    SettingsToggleRow(
                        title: SpawnCopy.nativeInputTitle,
                        detail: SpawnCopy.nativeInputDetail,
                        systemImage: "text.bubble",
                        notice: model.settingErrors["spawn_native_input"],
                        isPending: model.isSettingPending("spawn_native_input"),
                        isOn: Binding(
                            get: { model.settings?.spawnNativeInput ?? true },
                            set: { value in Task { await model.setSetting(SettingsChange(spawnNativeInput: value)) } }
                        )
                    )
                    if model.settings?.spawnNativeInput ?? true {
                        SettingsDivider()
                        SettingsToggleRow(
                            title: SpawnCopy.requesterApprovalsTitle,
                            detail: SpawnCopy.requesterApprovalsDetail,
                            systemImage: "checkmark.shield",
                            notice: model.settingErrors["spawn_requester_approvals"],
                            isPending: model.isSettingPending("spawn_requester_approvals"),
                            isOn: Binding(
                                get: { model.settings?.spawnRequesterApprovals ?? false },
                                set: { value in Task { await model.setSetting(SettingsChange(spawnRequesterApprovals: value)) } }
                            )
                        )
                    }
                    SettingsDivider()
                    SpawnMaxAgentsRow(
                        value: model.settings?.spawnMaxAgents ?? SpawnCopy.maxAgentsDefault,
                        set: { value in await model.setSetting(SettingsChange(spawnMaxAgents: value)) }
                    )
                }
            }
        }
    }
}

/// The spawn permission belonging to the provider shown in Settings.
struct ProviderSpawnSettings: View {
    let providerID: String
    @Environment(AppModel.self) private var model

    var body: some View {
        if model.settings?.acceptSpawns == true,
           let harness = SpawnCopy.fullAccessHarnesses(model.spawn).first(where: { $0.id == providerID }) {
            SettingsSection(title: "Spawned agents") {
                SettingsToggleRow(
                    title: SpawnCopy.fullAccessTitle(harness),
                    detail: SpawnCopy.fullAccessDetail(harness),
                    systemImage: "lock.open",
                    notice: SpawnCopy.fullAccessChange(harness.id, true).fields.compactMap { model.settingErrors[$0] }.first,
                    isPending: SpawnCopy.fullAccessChange(harness.id, true).fields.contains(where: model.isSettingPending),
                    isOn: Binding(
                        get: { SpawnCopy.fullAccess(model.settings, harness) },
                        set: { value in Task { await model.setSetting(SpawnCopy.fullAccessChange(harness.id, value)) } }
                    )
                )
            }
        }
    }
}

/// The field always returns to the last confirmed cap after a failed write.
@MainActor
@Observable
final class SpawnCapEditor {
    var draft = ""
    private(set) var error: String?
    private(set) var saving = false
    private var confirmed = 0

    func confirm(_ value: Int) {
        confirmed = value
        draft = value == 0 ? "" : String(value)
    }

    func commit(set: (Int) async -> String?) async {
        guard !saving else { return }
        guard let next = SpawnCopy.maxAgentsInput(draft) else {
            confirm(confirmed)
            return
        }
        guard next != confirmed else { return }
        saving = true
        error = await set(next)
        if error != nil { confirm(confirmed) }
        saving = false
    }
}

/// How many spawned agents may run here at once.
private struct SpawnMaxAgentsRow: View {
    let value: Int
    let set: (Int) async -> String?
    @State private var editor = SpawnCapEditor()

    private func commit() {
        Task { await editor.commit(set: set) }
    }

    private var capField: some View {
        TextField(SpawnCopy.noLimit, text: $editor.draft)
        .textFieldStyle(.roundedBorder)
        .zoomFrame(width: 100)
        .monospacedDigit()
        .onSubmit { commit() }
        .onAppear { editor.confirm(value) }
        .onChange(of: value) { _, next in editor.confirm(next) }
        .disabled(editor.saving)
        .accessibilityLabel(SpawnCopy.maxAgentsTitle)
    }

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: "square.stack.3d.up")

            VStack(alignment: .leading, spacing: 4) {
                Text(SpawnCopy.maxAgentsTitle)
                    .zoomFont(.body, weight: .medium)
                Text(SpawnCopy.maxAgentsDetail)
                    .zoomFont(.footnote)
                    .foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if let error = editor.error {
                    Text(error).zoomFont(.footnote).foregroundStyle(.red)
                }
            }

            Spacer(minLength: 20)

            capField
            Button("Save", action: commit)
                .buttonStyle(AgentRelayPrimaryButtonStyle())
                .controlSize(.small)
                .disabled(editor.saving)
        }
        .zoomPadding(16)
        .accessibilityElement(children: .contain)
    }
}

/// How long this computer keeps local session history, with the note that
/// history not yet uploaded is kept longer (Linux's `history_retention_row`).
struct HistoryRetentionRow: View {
    let days: Int
    /// Signed out or uploads paused: any limit can remove what has not uploaded.
    var uploadsHeld = false
    var notice: String? = nil
    var isPending = false
    let set: (Int) -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: "clock.arrow.circlepath")

            VStack(alignment: .leading, spacing: 4) {
                Text(HistoryRetentionCopy.title)
                    .zoomFont(.body, weight: .medium)
                Text(HistoryRetentionCopy.detail)
                    .zoomFont(.footnote)
                    .foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if let warning = HistoryRetentionCopy.warning(days, uploadsHeld: uploadsHeld) {
                    Label(warning, systemImage: "info.circle.fill")
                        .zoomFont(.footnote)
                        .foregroundStyle(CloudTheme.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                        .zoomPadding(.top, 2)
                }
                if let notice {
                    Label(notice, systemImage: "exclamationmark.circle.fill")
                        .zoomFont(.footnote)
                        .foregroundStyle(CloudTheme.warning)
                        .fixedSize(horizontal: false, vertical: true)
                        .zoomPadding(.top, 2)
                }
            }

            Spacer(minLength: 20)

            if isPending {
                ProgressView().controlSize(.small)
                    .accessibilityLabel("Saving")
            }
            Picker(HistoryRetentionCopy.title, selection: Binding(
                get: { days },
                set: { next in if next != days { set(next) } }
            )) {
                ForEach(HistoryRetentionCopy.choices, id: \.self) { choice in
                    Text(HistoryRetentionCopy.label(choice)).tag(choice)
                }
            }
            .labelsHidden()
            .pickerStyle(.menu)
            .zoomFrame(width: 170)
            .accessibilityLabel(HistoryRetentionCopy.title)
        }
        .zoomPadding(16)
        .accessibilityElement(children: .contain)
    }
}

/// Light, dark, or match macOS.
private struct AppearanceSettingsSection: View {
    @AppStorage(AppAppearance.storageKey) private var appearance: AppAppearance = .system

    var body: some View {
        SettingsSection(title: "Appearance", description: "Match system follows macOS, including its automatic light and dark schedule.") {
            VStack(spacing: 0) {
                HStack(spacing: 12) {
                    ForEach(AppAppearance.allCases) { option in
                        AppearanceOption(option: option, isSelected: appearance == option) {
                            appearance = option
                        }
                    }
                }
                .zoomPadding(16)

                SettingsDivider()

                ZoomSettingsRow()
            }
        }
        .onChange(of: appearance) { $1.apply() }
    }
}

/// The window's zoom: minus, the percent, plus, and Reset back to 100%.
/// The View menu's Zoom In, Zoom Out and Actual Size change the same
/// `uiZoom` default. The GTK counterpart is `append_zoom_row` in
/// `gtk/src/ui/zoom.rs`.
private struct ZoomSettingsRow: View {
    @AppStorage(UIZoom.defaultsKey) private var uiZoomPercent = UIZoom.actualSize.percent

    private var zoom: UIZoom { UIZoom(percent: uiZoomPercent) }

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: "plus.magnifyingglass")

            VStack(alignment: .leading, spacing: 4) {
                Text("Zoom")
                    .zoomFont(.body, weight: .medium)
                Text("Make text and controls larger or smaller. Command-plus and Command-minus zoom too.")
                    .zoomFont(.footnote)
                    .foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            Spacer(minLength: 20)

            HStack(spacing: 6) {
                zoomButton("Zoom Out", systemImage: "minus.magnifyingglass", enabled: zoom.canZoomOut) {
                    uiZoomPercent = zoom.zoomedOut.percent
                }
                Text(zoom.label)
                    .zoomFont(.body, weight: .medium)
                    .monospacedDigit()
                    .zoomFrame(minWidth: 44)
                    .accessibilityLabel("Zoom level \(zoom.label)")
                zoomButton("Zoom In", systemImage: "plus.magnifyingglass", enabled: zoom.canZoomIn) {
                    uiZoomPercent = zoom.zoomedIn.percent
                }
                Button("Reset") { uiZoomPercent = UIZoom.actualSize.percent }
                    .disabled(zoom == .actualSize)
                    .buttonStyle(SettingsActionStyle())
                    .controlSize(.regular)
                    .fixedSize()
                    .accessibilityLabel("Reset zoom")
            }
        }
        .zoomPadding(16)
    }

    private func zoomButton(_ title: String, systemImage: String, enabled: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .zoomFrame(width: 28, height: 28)
                .contentShape(Rectangle())
        }
        .buttonStyle(HoverPlainButtonStyle(shape: RoundedRectangle(cornerRadius: 6)))
        .disabled(!enabled)
        .help(title)
        .accessibilityLabel(title)
    }
}

private struct AppearanceOption: View {
    let option: AppAppearance
    let isSelected: Bool
    let action: () -> Void
    @State private var hovered = false

    var body: some View {
        Button(action: action) {
            VStack(spacing: 10) {
                preview
                    .zoomFrame(height: 74)
                    .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .strokeBorder(isSelected ? CloudTheme.primary : Color.primary.opacity(hovered ? 0.3 : 0.12),
                                      lineWidth: isSelected ? 2.5 : 1))
                    .scaleEffect(hovered && !isSelected ? 1.02 : 1)
                HStack(spacing: 6) {
                    Image(systemName: isSelected ? "checkmark.circle.fill" : option.symbol)
                        .foregroundStyle(isSelected ? CloudTheme.primary : .secondary)
                    Text(option.title).zoomFont(size: 12, weight: isSelected ? .semibold : .regular)
                }
            }
            .frame(maxWidth: .infinity)
            .contentShape(Rectangle())
        }
        .buttonStyle(HoverPlainButtonStyle(shape: RoundedRectangle(cornerRadius: 6)))
        .onHover { hovered = $0 }
        .animation(.easeOut(duration: 0.12), value: hovered)
        .accessibilityLabel(option.title)
        .accessibilityAddTraits(isSelected ? .isSelected : [])
    }

    @ViewBuilder private var preview: some View {
        switch option {
        case .light: miniWindow(dark: false)
        case .dark: miniWindow(dark: true)
        case .system:
            HStack(spacing: 0) {
                miniWindow(dark: false)
                miniWindow(dark: true)
            }
        }
    }

    /// A tiny drawing of the window: rail, sidebar and content.
    private func miniWindow(dark: Bool) -> some View {
        let base = CloudTheme.previewBackground(dark: dark)
        let panel = CloudTheme.previewPanel(dark: dark)
        let line = CloudTheme.previewBorder(dark: dark)
        return HStack(spacing: 0) {
            panel.zoomFrame(width: 12)
            VStack(alignment: .leading, spacing: 5) {
                Capsule().fill(CloudTheme.primary.opacity(0.7)).zoomFrame(width: 22, height: 5)
                Capsule().fill(line).zoomFrame(width: 18, height: 4)
                Capsule().fill(line).zoomFrame(width: 20, height: 4)
                Spacer(minLength: 0)
            }
            .zoomPadding(6)
            .zoomFrame(width: 34, alignment: .leading)
            .background(panel.opacity(0.6))
            VStack(alignment: .leading, spacing: 5) {
                Capsule().fill(line).zoomFrame(height: 5)
                RoundedRectangle(cornerRadius: 3).fill(panel).zoomFrame(height: 14)
                RoundedRectangle(cornerRadius: 3).fill(panel).zoomFrame(height: 14)
                Spacer(minLength: 0)
            }
            .zoomPadding(6)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(base)
    }
}

private struct SoftwareUpdateSettingsRow: View {
    @Bindable var updater: AppUpdater
    @State private var isConfirmingUpdate = false

    var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .top, spacing: 14) {
                SettingsIcon(systemImage: stateIcon)

                VStack(alignment: .leading, spacing: 4) {
                    Text(headline)
                        .zoomFont(.body, weight: .medium)
                    Text(detail)
                        .zoomFont(.footnote)
                        .foregroundStyle(detailColor)
                        .fixedSize(horizontal: false, vertical: true)
                }

                Spacer(minLength: 20)

                if updater.isBusy {
                    ProgressView()
                        .controlSize(.small)
                        .zoomPadding(.top, 7)
                }

                Button(actionTitle) { performAction() }
                    .disabled(updater.isBusy)
                    .buttonStyle(SettingsActionStyle(prominent: updater.state.isAvailable))
                    .controlSize(.regular)
                    .fixedSize()
            }
            .zoomPadding(16)

            SettingsDivider()

            SettingsToggleRow(
                title: "Automatically check for updates",
                detail: "Check once a day while Agent Relay is running. You choose when to install an available update.",
                systemImage: "clock.arrow.circlepath",
                isOn: $updater.automaticallyChecksForUpdates
            )

            SettingsDivider()

            SettingsToggleRow(
                title: "Include beta releases",
                detail: "Try upcoming features before they reach the stable channel. Beta releases may be less reliable. Turning this off keeps your current version until a newer stable release is available.",
                systemImage: "flask",
                isOn: $updater.includeBetaReleases
            )
            .disabled(updater.isBusy)
            .onChange(of: updater.includeBetaReleases) {
                Task { await updater.checkForUpdates() }
            }
        }
        .confirmationDialog("Update Agent Relay now?", isPresented: $isConfirmingUpdate) {
            if case let .available(release) = updater.state {
                Button("Update") { Task { await updater.downloadAndInstall(release) } }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Agent Relay will quit to install the update and reopen. Your agents reconnect and uploads continue as soon as the new version starts.")
        }
    }

    private var headline: String {
        switch updater.state {
        case .idle: return "Agent Relay"
        case .checking: return "Checking for updates…"
        case .upToDate: return "Agent Relay is up to date"
        case let .available(release), let .downloading(release), let .installing(release):
            return "Agent Relay \(release.version) is available"
        case .unavailable: return "Updates unavailable"
        case .failed: return "Update failed"
        }
    }

    private var detail: String {
        switch updater.state {
        case .idle:
            return "You’re running version \(AppInfo.version)."
        case .checking:
            return updater.includeBetaReleases
                ? "Looking for the latest beta or stable release."
                : "Looking for the latest stable release."
        case let .upToDate(date):
            return "You’re running version \(AppInfo.version). No newer release is available. Checked \(date.formatted(date: .omitted, time: .shortened))."
        case .available:
            return "Install this update and reopen Agent Relay."
        case .downloading:
            return "Downloading and verifying the update…"
        case .installing:
            return "Preparing the update. Agent Relay will reopen when installation finishes."
        case let .unavailable(message), let .failed(message):
            return message
        }
    }

    private var detailColor: Color {
        if case .failed = updater.state { return .red }
        return CloudTheme.secondary
    }

    private var stateIcon: String {
        switch updater.state {
        case .upToDate: return "checkmark.circle"
        case .available, .downloading, .installing: return "arrow.down.circle"
        case .failed: return "exclamationmark.triangle"
        default: return "arrow.triangle.2.circlepath"
        }
    }

    private var actionTitle: String {
        switch updater.state {
        case .available: return "Update"
        case .downloading: return "Downloading…"
        case .installing: return "Installing…"
        case .checking: return "Checking…"
        default: return "Check now"
        }
    }

    private func performAction() {
        switch updater.state {
        case .available:
            isConfirmingUpdate = true
        default:
            Task { await updater.checkForUpdates() }
        }
    }
}

struct SettingsSection<Content: View>: View {
    let title: String
    let description: String?
    let content: Content

    init(
        title: String,
        description: String? = nil,
        @ViewBuilder content: () -> Content
    ) {
        self.title = title
        self.description = description
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .zoomFont(size: 13, weight: .semibold)
                    .accessibilityAddTraits(.isHeader)
                if let description {
                    Text(description)
                        .zoomFont(.footnote)
                        .foregroundStyle(CloudTheme.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            content
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(CloudTheme.card)
                .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                .overlay {
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .strokeBorder(CloudTheme.border)
                }
        }
    }
}

private struct SettingsToggleRow: View {
    let title: String
    let detail: String
    let systemImage: String
    var notice: String? = nil
    /// A notice that reports how things stand rather than a problem.
    var noticeIsStatus = false
    /// A button under the notice that acts on it, such as Retry.
    var noticeAction: (title: String, run: () -> Void)? = nil
    /// The switch shows the state asked for while the core saves it.
    var isPending = false
    @Binding var isOn: Bool

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: systemImage)

            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .zoomFont(.body, weight: .medium)
                Text(detail)
                    .zoomFont(.footnote)
                    .foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if let notice {
                    Label(notice, systemImage: noticeIsStatus ? "checkmark.circle.fill" : "exclamationmark.circle.fill")
                        .zoomFont(.footnote)
                        .foregroundStyle(noticeIsStatus ? CloudTheme.secondary : CloudTheme.warning)
                        .fixedSize(horizontal: false, vertical: true)
                        .zoomPadding(.top, 2)
                }
                if let noticeAction {
                    Button(noticeAction.title, action: noticeAction.run)
                        .buttonStyle(AgentRelayPrimaryButtonStyle())
                        .zoomPadding(.top, 4)
                }
            }

            Spacer(minLength: 20)

            if isPending {
                ProgressView().controlSize(.small)
                    .accessibilityLabel("Saving")
            }
            Toggle(title, isOn: $isOn)
                .labelsHidden()
                .toggleStyle(SwitchToggleStyle(tint: CloudTheme.settingsAction))
                .actionCursor()
                .controlSize(.regular)
                .zoomPadding(.top, 2)
        }
        .zoomPadding(16)
        .accessibilityElement(children: .contain)
    }
}

struct UploadPolicyRow: View {
    let mode: SharingMode
    let isSelected: Bool
    /// Chosen, and the uploader has not confirmed it yet.
    var isPending = false
    let action: () -> Void
    @State private var hovered = false

    var body: some View {
        Button(action: action) {
            HStack(alignment: .top, spacing: 14) {
                Group {
                    if isPending {
                        ProgressView().controlSize(.small)
                            .accessibilityLabel("Saving")
                    } else {
                        Image(systemName: isSelected ? "checkmark.circle.fill" : (hovered ? "circle.inset.filled" : "circle"))
                            .zoomFont(size: 18, weight: .medium)
                            .foregroundStyle(isSelected ? CloudTheme.primary : Color.secondary.opacity(0.7))
                    }
                }
                .zoomFrame(width: 24, height: 24)

                VStack(alignment: .leading, spacing: 4) {
                    Text(mode.title)
                        .zoomFont(.body, weight: .medium)
                        .foregroundStyle(CloudTheme.foreground)
                    Text(mode.explanation)
                        .zoomFont(.footnote)
                        .foregroundStyle(CloudTheme.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }

                Spacer(minLength: 12)
            }
            .zoomPadding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(isSelected ? CloudTheme.primary.opacity(0.08) : Color.primary.opacity(hovered ? 0.04 : 0))
            .contentShape(Rectangle())
        }
        .buttonStyle(HoverPlainButtonStyle(shape: RoundedRectangle(cornerRadius: 6)))
        .onHover { hovered = $0 }
        .animation(.easeOut(duration: 0.12), value: hovered)
        .accessibilityAddTraits(isSelected ? .isSelected : [])
        .accessibilityValue(isSelected ? "Selected" : "Not selected")
    }
}

struct SettingsUnavailableRow: View {
    let title: String
    let detail: String

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: "arrow.up.circle")
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .zoomFont(.body, weight: .medium)
                Text(detail)
                    .zoomFont(.footnote)
                    .foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
        }
        .zoomPadding(16)
    }
}

struct SettingsIcon: View {
    let systemImage: String

    var body: some View {
        Image(systemName: systemImage)
            .zoomFont(size: 16, weight: .regular)
            .foregroundStyle(CloudTheme.secondary)
            .zoomFrame(width: 24, height: 28)
            .accessibilityHidden(true)
    }
}

struct SettingsDivider: View {
    var leadingInset: CGFloat = 54

    var body: some View {
        Divider().hidden().overlay(CloudTheme.border)
            .zoomPadding(.leading, leadingInset)
    }
}

struct SharingConfirmationSheet: View {
    var mode: SharingMode
    var onCancel: () -> Void
    var onConfirm: (SharingMode) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(mode.confirmationTitle)
                .zoomFont(.headline)
            Text(mode.explanation)
                .fixedSize(horizontal: false, vertical: true)
            Text("Changing this rebuilds the upload queue and can take a few minutes. Previously uploaded data is not deleted.")
                .zoomFont(.footnote)
                .foregroundColor(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack {
                Spacer()
                Button("Cancel", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                Button(mode.confirmationAction) {
                    onConfirm(mode)
                }
                .keyboardShortcut(.defaultAction)
                .buttonStyle(AgentRelayPrimaryButtonStyle())
            }
        }
        .zoomPadding(20)
        .zoomFrame(width: 420)
    }
}

// MARK: - Relay

/// The relay workspace sessions join, and how Claude Code takes relayed
/// messages.
struct RelaySettingsSection: View {
    var providerID: String? = nil
    var relay: RelayModel
    @Environment(AppModel.self) private var model
    @State private var isConfirmingGrokLeader = false
    @State private var grokLeaderError: String?
    @State private var openCodePluginError: String?
    @State private var openClawError: String?
    @State private var trustedSendersText = ""
    @State private var trustedSendersError: String?
    @State private var hermesError: String?
    @State private var devinDeliveryError: String?
    @State private var cursorDeliveryError: String?
    @State private var codexStopHookError: String?
    @State private var showRelayMessagesError: String?
    @State private var autoJoinGeneralError: String?

    private var hasMessages: Bool {
        switch providerID {
        case nil, "cursor": return true // Cursor has an explicit readiness explanation.
        case "grok": return model.settings?.grokLeader != nil
        case "opencode": return model.settings?.opencodePlugin != nil
        case "openclaw": return model.settings?.openclawGateway != nil
        case "hermes": return model.settings?.hermesAgent != nil
        case "devin": return model.settings?.devinDirectDelivery != nil
        case "codex": return model.settings?.codexStopHook != nil
        default: return false
        }
    }

    var body: some View {
        if hasMessages {
        SettingsSection(
            title: providerID == nil ? "Relay" : "Messages",
            description: providerID == nil ? "Sessions you put on the relay get a name anyone in this workspace can message." : nil
        ) {
            VStack(spacing: 0) {
                if providerID == nil { workspaceRow }
                if providerID == "grok", model.settings?.grokLeader != nil {
                    grokLeaderRow
                }
                if providerID == "opencode", model.settings?.opencodePlugin != nil {
                    openCodePluginRow
                }
                if providerID == "openclaw", model.settings?.openclawGateway != nil {
                    SettingsDivider()
                    openClawRow
                }
                if providerID == "openclaw", model.settings?.openclawGateway == true {
                    SettingsDivider()
                    OpenClawToolsRowView()
                }

                if providerID == "hermes", model.settings?.hermesAgent != nil {
                    SettingsDivider()
                    hermesRow
                }
                if (providerID == "openclaw" && model.settings?.openclawGateway == true) || (providerID == "hermes" && model.settings?.hermesAgent == true),
                   model.settings?.trustedRelaySenders != nil {
                    SettingsDivider()
                    trustedSendersRow
                }
                if providerID == "devin", model.settings?.devinDirectDelivery != nil {
                    devinDeliveryRow
                }
                if providerID == "cursor" {
                    if model.settings?.cursorDirectDelivery != nil {
                        cursorDeliveryRow
                    } else {
                        Text("Start a Cursor Agent chat once, then return here to connect its messages.")
                            .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                            .fixedSize(horizontal: false, vertical: true).zoomPadding(16)
                    }
                }
                if providerID == "codex", model.settings?.codexStopHook != nil {
                    codexStopHookRow
                }
                if providerID == nil {
                if model.settings?.mobileClaudeHandoff != nil || model.settings?.mobileNativeContinuation != nil {
                    SettingsDivider()
                    mobileContinuationRow
                }
                if model.settings?.teammateNativeInput != nil {
                    SettingsDivider()
                    SettingsToggleRow(
                        title: TeammateNativeInputCopy.title,
                        detail: TeammateNativeInputCopy.detail,
                        systemImage: "text.bubble",
                        notice: model.settingErrors["teammate_native_input"],
                        isPending: model.isSettingPending("teammate_native_input"),
                        isOn: Binding(
                            get: { model.settings?.teammateNativeInput ?? true },
                            set: { value in Task { await model.setSetting(SettingsChange(teammateNativeInput: value)) } }
                        )
                    )
                }
                if model.coordinationAdvisories != nil {
                    SettingsDivider()
                    SettingsToggleRow(
                        title: CoordinationAdvisoriesCopy.title,
                        detail: CoordinationAdvisoriesCopy.detail,
                        systemImage: "person.2",
                        notice: model.settingErrors[CoordinationAdvisoriesCopy.settingKey],
                        isPending: model.isCoordinationAdvisoriesPending,
                        isOn: Binding(
                            get: { model.coordinationAdvisories ?? false },
                            set: { value in Task { await model.setCoordinationAdvisories(value) } }
                        )
                    )
                }
                if model.settings?.showRelayMessages != nil {
                    SettingsDivider()
                    showRelayMessagesRow
                }
                if model.settings?.autoJoinGeneral != nil {
                    SettingsDivider()
                    autoJoinGeneralRow
                }
                if let changes = model.settings?.agentChanges, !changes.isEmpty {
                    SettingsDivider()
                    agentChangesRow(changes)
                }
                }
            }
        }
        }
    }

    private var workspaceRow: some View {
        HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: relay.workspace == nil ? "antenna.radiowaves.left.and.right.slash" : "dot.radiowaves.left.and.right")
            VStack(alignment: .leading, spacing: 4) {
                Text(relay.workspace == nil ? "No relay workspace" : "Workspace from your Agent Relay account")
                    .zoomFont(.body, weight: .medium)
                Text(relay.workspace == nil
                     ? "Connect uses your Agent Relay sign-in."
                     : "Uploads and messages use this workspace. Switch workspaces from the account menu.")
                    .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if let error = relay.workspaceConnectError { notice(error, color: .red) }
                if let error = relay.storeError { notice(error, color: .red) }
                if let error = relay.integrationError { notice(error, color: .orange) }
                if let error = relay.deliveryError { notice(error, color: .orange) }
                if relay.isConnectingWorkspace || relay.workspace == nil {
                    HStack(spacing: 8) {
                        if relay.isConnectingWorkspace {
                            ProgressView().controlSize(.small)
                            Button("Cancel") { relay.cancelConnectWorkspace() }
                        } else {
                            Button("Connect Workspace") {
                                Task { await relay.connectWorkspace() }
                            }
                            .disabled(model.workspaceID == nil)
                            .help("Fetch this workspace's relay key with your Agent Relay sign-in")
                        }
                    }
                    .zoomPadding(.top, 4)
                }
            }
            Spacer(minLength: 0)
        }
        .zoomPadding(16)
    }

    /// Grok's `[cli] use_leader`. A plain Grok session opens no route in;
    /// one on the shared leader can be listed and messaged like a Claude
    /// Code or Codex session.
    private var grokLeaderRow: some View {
        let enabled = model.settings?.grokLeader == true
        return HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: enabled ? "checkmark.seal" : "bolt.horizontal.circle")
            VStack(alignment: .leading, spacing: 4) {
                Text("Grok sessions").zoomFont(.body, weight: .medium)
                Group {
                    if enabled {
                        Text("Grok runs its sessions on a shared leader: `use_leader` is `true` in your Grok config. Sessions you open in Grok appear here and can be messaged.")
                    } else {
                        Text("A plain Grok session has no way in. Turn on leader mode so sessions you open in Grok appear here and can be messaged, the way Claude Code and Codex sessions are.")
                    }
                }
                .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                .fixedSize(horizontal: false, vertical: true)
                if let grokLeaderError { notice(grokLeaderError, color: .red) }
            }
            Spacer(minLength: 12)
            if enabled {
                Button("Turn Off") { setGrokLeader(false) }
                    .help("Set use_leader = false in your Grok config")
            } else {
                Button("Turn On Leader Mode…") { isConfirmingGrokLeader = true }
            }
        }
        .zoomPadding(16)
        .confirmationDialog("Run Grok sessions on a shared leader?", isPresented: $isConfirmingGrokLeader) {
            Button("Turn on leader mode") { setGrokLeader(true) }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This sets use_leader = true under [cli] in ~/.grok/config.toml. Grok sessions you open afterwards run on a shared `grok agent leader` process, which stays running after you close Grok and is what Agent Relay delivers through. Sessions already open are not affected until you restart them. Grok refuses leader mode when a sandbox profile is on. Your current file is kept as config.toml.relay-desktop-backup.")
        }
    }

    private func setGrokLeader(_ enabled: Bool) {
        Task { grokLeaderError = await model.setSetting(SettingsChange(grokLeader: enabled)) }
    }

    /// Agent Relay's OpenCode plugin. A plain OpenCode session opens no
    /// port; with the plugin, each OpenCode window can be listed and
    /// messaged like a Claude Code or Codex session.
    private var openCodePluginRow: some View {
        let enabled = model.settings?.opencodePlugin == true
        return HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: enabled ? "checkmark.seal" : "bolt.horizontal.circle")
            VStack(alignment: .leading, spacing: 4) {
                Text("OpenCode sessions").zoomFont(.body, weight: .medium)
                Group {
                    if enabled {
                        Text("Agent Relay's plugin is in ~/.config/opencode/plugins/agent-relay.js. OpenCode sessions you open appear here and can be messaged. Restart OpenCode windows that were open before it was on.")
                    } else {
                        Text("A plain OpenCode session has no way in. Turn this on to add Agent Relay's plugin, so sessions you open in OpenCode appear here and can be messaged, the way Claude Code and Codex sessions are.")
                    }
                }
                .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                .fixedSize(horizontal: false, vertical: true)
                if let openCodePluginError { notice(openCodePluginError, color: .red) }
            }
            Spacer(minLength: 12)
            if enabled {
                Button("Turn Off") { setOpenCodePlugin(false) }
                    .help("Remove Agent Relay's plugin from OpenCode")
            } else {
                Button("Turn On") { setOpenCodePlugin(true) }
                    .help("Add Agent Relay's plugin to OpenCode")
            }
        }
        .zoomPadding(16)
    }

    private func setOpenCodePlugin(_ enabled: Bool) {
        Task { openCodePluginError = await model.setSetting(SettingsChange(opencodePlugin: enabled)) }
    }

    /// The agents of the person's OpenClaw Gateway. Agent Relay connects to
    /// it on this computer; each relay sender gets a session of its own on
    /// the agent, and its answer goes back to the sender only.
    private var openClawRow: some View {
        let enabled = model.settings?.openclawGateway == true
        return HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: enabled ? "checkmark.seal" : "bolt.horizontal.circle")
            VStack(alignment: .leading, spacing: 4) {
                Text(OpenClawCopy.title).zoomFont(.body, weight: .medium)
                Group {
                    if enabled {
                        Text(OpenClawCopy.onDetail)
                    } else {
                        Text(OpenClawCopy.offDetail)
                    }
                }
                .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                .fixedSize(horizontal: false, vertical: true)
                // A Gateway older than 2026.8.1 cannot make relay sessions
                // answer-only: said while it is on.
                if enabled, let version = model.settings?.openclawUnlimitedVersion {
                    notice(OpenClawCopy.unlimitedNotice(version), color: .orange)
                }
                if let openClawError { notice(openClawError, color: .red) }
            }
            Spacer(minLength: 12)
            if enabled {
                Button("Turn Off") { setOpenClaw(false) }
                    .help(OpenClawCopy.turnOffHelp)
            } else {
                Button("Turn On") { setOpenClaw(true) }
                    .help(OpenClawCopy.turnOnHelp)
            }
        }
        .zoomPadding(16)
    }

    private func setOpenClaw(_ enabled: Bool) {
        Task { openClawError = await model.setSetting(SettingsChange(openclawGateway: enabled)) }
    }

    /// The relay senders trusted like the person: their messages reach the
    /// person's own OpenClaw and Hermes instead of an answer-only session.
    private var trustedSendersRow: some View {
        let saved = (model.settings?.trustedRelaySenders ?? []).joined(separator: ", ")
        return HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: "person.badge.shield.checkmark")
            VStack(alignment: .leading, spacing: 6) {
                Text(TrustedSendersCopy.title).zoomFont(.body, weight: .medium)
                Text(TrustedSendersCopy.detail)
                    .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if let trustsMembers = model.settings?.trustWorkspaceMembers {
                    workspaceMembersList(trustsMembers)
                    if trustsMembers {
                        Text(TrustedSendersCopy.othersLabel)
                            .zoomFont(.footnote, weight: .medium).foregroundStyle(CloudTheme.secondary)
                    }
                }
                TextField(TrustedSendersCopy.placeholder, text: $trustedSendersText)
                    .textFieldStyle(.roundedBorder)
                    .onSubmit { saveTrustedSenders() }
                if let trustedSendersError { notice(trustedSendersError, color: .red) }
            }
            Spacer(minLength: 12)
            Button("Save") { saveTrustedSenders() }
                .disabled(TrustedSendersCopy.senders(from: trustedSendersText) == TrustedSendersCopy.senders(from: saved))
        }
        .zoomPadding(16)
        .onAppear { trustedSendersText = saved }
        .onChange(of: saved) { _, value in trustedSendersText = value }
    }

    /// The people in the workspace, trusted while the toggle is on: listed
    /// read-only, since Agent Relay keeps them current.
    private func workspaceMembersList(_ trustsMembers: Bool) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Toggle(isOn: Binding(get: { trustsMembers }, set: { setTrustsMembers($0) })) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(TrustedSendersCopy.membersToggle)
                    Text(TrustedSendersCopy.membersDetail)
                        .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .toggleStyle(.switch)
            .accessibilityIdentifier("trusted-workspace-members-toggle")
            if trustsMembers {
                let members = model.settings?.workspaceMembers ?? []
                if members.isEmpty {
                    Text(TrustedSendersCopy.membersEmpty)
                        .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                } else {
                    ForEach(members, id: \.name) { member in
                        HStack(spacing: 8) {
                            Image(systemName: "person.crop.circle").foregroundStyle(CloudTheme.secondary)
                            Text(TrustedSendersCopy.memberTitle(name: member.name, displayName: member.displayName))
                            Spacer(minLength: 8)
                            Text(TrustedSendersCopy.memberLabel)
                                .zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
                                .zoomPadding(.horizontal, 6).zoomPadding(.vertical, 2)
                                .background(Capsule().fill(CloudTheme.secondary.opacity(0.12)))
                        }
                        .accessibilityElement(children: .combine)
                    }
                }
            }
        }
    }

    private func setTrustsMembers(_ on: Bool) {
        Task { trustedSendersError = await model.setSetting(SettingsChange(trustWorkspaceMembers: on)) }
    }

    private func saveTrustedSenders() {
        let senders = TrustedSendersCopy.senders(from: trustedSendersText)
        Task {
            trustedSendersError = await model.setSetting(SettingsChange(trustedRelaySenders: senders))
        }
    }

    /// The person's Hermes, through its loopback API server. Each relay
    /// sender gets a Hermes session of its own; only the final answer goes
    /// back to the sender.
    private var hermesRow: some View {
        let enabled = model.settings?.hermesAgent == true
        return HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: enabled ? "checkmark.seal" : "bolt.horizontal.circle")
            VStack(alignment: .leading, spacing: 4) {
                Text(HermesCopy.title).zoomFont(.body, weight: .medium)
                Group {
                    if enabled {
                        Text(HermesCopy.onDetail)
                    } else {
                        Text(HermesCopy.offDetail)
                    }
                }
                .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                .fixedSize(horizontal: false, vertical: true)
                if let hermesError { notice(hermesError, color: .red) }
            }
            Spacer(minLength: 12)
            if enabled {
                Button("Turn Off") { setHermes(false) }
                    .help(HermesCopy.turnOffHelp)
            } else {
                Button("Turn On") { setHermes(true) }
                    .help(HermesCopy.turnOnHelp)
            }
        }
        .zoomPadding(16)
    }

    private func setHermes(_ enabled: Bool) {
        Task { hermesError = await model.setSetting(SettingsChange(hermesAgent: enabled)) }
    }

    /// Agent Relay's Devin lifecycle hooks. A Devin session has no port to
    /// write into; with the hooks in its config.json, a staged message
    /// reaches the conversation at each hook boundary.
    private var devinDeliveryRow: some View {
        let enabled = model.settings?.devinDirectDelivery == true
        return HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: enabled ? "checkmark.seal" : "bolt.horizontal.circle")
            VStack(alignment: .leading, spacing: 4) {
                Text("Devin sessions").zoomFont(.body, weight: .medium)
                Group {
                    if enabled {
                        Text("Agent Relay's hooks are in ~/.config/devin/config.json. Messages reach a Devin session mid-turn and when its turn ends. Start Devin sessions that were open before it was on again.")
                    } else {
                        Text("A Devin session has no way in: messages wait in its inbox, unheard. Turn this on to add Agent Relay's hooks to Devin's config.json, so sessions you open in Devin appear here and can be messaged, the way Claude Code and Codex sessions are.")
                    }
                }
                .zoomFont(.footnote).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
                if let devinDeliveryError { notice(devinDeliveryError, color: .red) }
            }
            Spacer(minLength: 12)
            if enabled {
                Button("Turn Off") { setDevinDelivery(false) }
                    .help("Remove Agent Relay's hooks from Devin")
            } else {
                Button("Turn On") { setDevinDelivery(true) }
                    .help("Add Agent Relay's hooks to Devin")
            }
        }
        .zoomPadding(16)
    }

    private func setDevinDelivery(_ enabled: Bool) {
        Task { devinDeliveryError = await model.setSetting(SettingsChange(devinDirectDelivery: enabled)) }
    }

    /// Agent Relay's Cursor CLI hooks. A Cursor CLI session has no port to
    /// write into; with the hooks in ~/.cursor/hooks.json, a staged message
    /// reaches it at its next tool call or when its turn ends, and a Cursor
    /// editor chat that has finished its turn as soon as it arrives.
    private var cursorDeliveryRow: some View {
        let enabled = model.settings?.cursorDirectDelivery == true
        return HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: enabled ? "checkmark.seal" : "bolt.horizontal.circle")
            VStack(alignment: .leading, spacing: 4) {
                Text(CursorHooks.title).zoomFont(.body, weight: .medium)
                Text(enabled ? CursorHooks.onExplanation : CursorHooks.offExplanation)
                    .zoomFont(.footnote).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if let cursorDeliveryError { notice(cursorDeliveryError, color: .red) }
            }
            Spacer(minLength: 12)
            if enabled {
                Button("Turn Off") { setCursorDelivery(false) }
                    .help(CursorHooks.turnOffHelp)
            } else {
                Button("Turn On") { setCursorDelivery(true) }
                    .help(CursorHooks.turnOnHelp)
            }
        }
        .zoomPadding(16)
    }

    private func setCursorDelivery(_ enabled: Bool) {
        Task { cursorDeliveryError = await model.setSetting(SettingsChange(cursorDirectDelivery: enabled)) }
    }

    private var codexStopHookRow: some View {
        let enabled = model.settings?.codexStopHook == true
        return HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: enabled ? "checkmark.seal" : "bubble.left.and.text.bubble.right")
            VStack(alignment: .leading, spacing: 4) {
                Text(CodexHookCopy.title).zoomFont(.body, weight: .medium)
                Text(enabled ? CodexHookCopy.turnedOn : CodexHookCopy.turnedOff)
                    .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if let codexStopHookError { notice(codexStopHookError, color: .red) }
            }
            Spacer(minLength: 12)
            if enabled {
                Button("Turn Off") { setCodexStopHook(false) }
                    .help(CodexHookCopy.turnOffHelp)
            } else {
                Button("Turn On") { setCodexStopHook(true) }
                    .help(CodexHookCopy.turnOnHelp)
            }
        }
        .zoomPadding(16)
    }

    private func setCodexStopHook(_ enabled: Bool) {
        Task { codexStopHookError = await model.setSetting(SettingsChange(codexStopHook: enabled)) }
    }

    /// Relay messages in agent chats: hidden (off, the default; the agent
    /// still reads each one) or shown in full.
    /// One switch for every native route from the phone: Claude Code's
    /// plugin and the other adapters are turned on or off together, on
    /// unless turned off. Until this computer's ownership is verified the
    /// switch is off and a status line says what it waits for.
    private var mobileContinuationRow: some View {
        let supported = MobileNativeContinuationCopy.isSupported(model.settings)
        let claimed = model.spawn?.claim?.state == .claimed
        let error = model.settingErrors["mobile_native_continuation"] ?? model.settingErrors["mobile_claude_handoff"]
        return SettingsToggleRow(
            title: MobileNativeContinuationCopy.title,
            detail: supported ? MobileNativeContinuationCopy.detail : MobileNativeContinuationCopy.unavailable,
            systemImage: "iphone",
            notice: error ?? (supported && !claimed ? MobileNativeContinuationCopy.waiting : nil),
            noticeIsStatus: error == nil,
            isPending: model.isSettingPending("mobile_claude_handoff") || model.isSettingPending("mobile_native_continuation"),
            isOn: Binding(
                get: { MobileNativeContinuationCopy.isEnabled(model.settings) },
                set: { value in
                    Task {
                        await model.setSetting(
                            MobileNativeContinuationCopy.change(enabled: value, settings: model.settings)
                        )
                    }
                }
            )
        ).disabled(!supported || !claimed)
    }

    private var showRelayMessagesRow: some View {
        let enabled = model.settings?.showRelayMessages == true
        return HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: enabled ? "text.bubble" : "eye.slash")
            VStack(alignment: .leading, spacing: 4) {
                Text(ShowRelayMessages.title).zoomFont(.body, weight: .medium)
                Text(enabled ? ShowRelayMessages.shownExplanation : ShowRelayMessages.explanation)
                    .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if let showRelayMessagesError { notice(showRelayMessagesError, color: .red) }
            }
            Spacer(minLength: 12)
            if enabled {
                Button("Turn Off") { setShowRelayMessages(false) }
                    .help(ShowRelayMessages.turnOffHelp)
            } else {
                Button("Turn On") { setShowRelayMessages(true) }
                    .help(ShowRelayMessages.turnOnHelp)
            }
        }
        .zoomPadding(16)
    }

    private func setShowRelayMessages(_ enabled: Bool) {
        Task { showRelayMessagesError = await model.setSetting(SettingsChange(showRelayMessages: enabled)) }
    }

    /// Whether agents put on the relay from now on join #general. Off unless
    /// turned on; agents already on the relay keep their channels.
    private var autoJoinGeneralRow: some View {
        let enabled = model.settings?.autoJoinGeneral == true
        return HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: enabled ? "checkmark.circle" : "number")
            VStack(alignment: .leading, spacing: 4) {
                Text(AutoJoinGeneral.title).zoomFont(.body, weight: .medium)
                Text(enabled ? AutoJoinGeneral.joinedExplanation : AutoJoinGeneral.explanation)
                    .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if let autoJoinGeneralError { notice(autoJoinGeneralError, color: .red) }
            }
            Spacer(minLength: 12)
            if enabled {
                Button("Turn Off") { setAutoJoinGeneral(false) }
                    .help(AutoJoinGeneral.turnOffHelp)
            } else {
                Button("Turn On") { setAutoJoinGeneral(true) }
                    .help(AutoJoinGeneral.turnOnHelp)
            }
        }
        .zoomPadding(16)
    }

    private func setAutoJoinGeneral(_ enabled: Bool) {
        Task { autoJoinGeneralError = await model.setSetting(SettingsChange(autoJoinGeneral: enabled)) }
    }

    /// The latest settings agents changed on the session socket, newest
    /// first; shown once one has.
    private func agentChangesRow(_ changes: [AgentSettingsChange]) -> some View {
        HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: "square.and.pencil")
            VStack(alignment: .leading, spacing: 4) {
                Text(AgentSettingsChanges.title).zoomFont(.body, weight: .medium)
                Text(AgentSettingsChanges.explanation)
                    .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                ForEach(Array(changes.prefix(AgentSettingsChanges.shown).enumerated()), id: \.offset) { _, change in
                    Text(AgentSettingsChanges.row(change))
                        .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 12)
        }
        .zoomPadding(16)
    }

    private func notice(_ text: String, color: Color) -> some View {
        Label(text, systemImage: "exclamationmark.circle.fill")
            .zoomFont(.footnote).foregroundStyle(color)
            .fixedSize(horizontal: false, vertical: true)
            .zoomPadding(.top, 2)
    }
}

/// Agent Relay's own tools for the person's OpenClaw agents, under OpenClaw's
/// row while it is on. Connect adds Agent Relay's hosted MCP to
/// openclaw.json and opens its consent page in the browser; while that page
/// is open the row reads the status again every two seconds, until it is
/// connected or Settings goes away.
private struct OpenClawToolsRowView: View {
    @Environment(AppModel.self) private var model
    @State private var status: OpenClawToolsStatus?
    @State private var error: String?
    @State private var connecting = false
    /// Connects asked for, so a status reply asked before the latest one
    /// is dropped.
    @State private var connects = 0

    var body: some View {
        let row = status.map(OpenClawToolsCopy.row(for:))
        return HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: status?.state == .connected ? "checkmark.seal" : "wrench.and.screwdriver")
            VStack(alignment: .leading, spacing: 4) {
                Text(OpenClawToolsCopy.title).zoomFont(.body, weight: .medium)
                if let row, !row.detail.isEmpty {
                    Text(row.detail)
                        .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                ForEach(errors(row), id: \.self) { text in
                    Label(text, systemImage: "exclamationmark.circle.fill")
                        .zoomFont(.footnote).foregroundStyle(.red)
                        .fixedSize(horizontal: false, vertical: true)
                        .zoomPadding(.top, 2)
                }
            }
            Spacer(minLength: 12)
            if let action = row?.action {
                switch action {
                case .connect:
                    Button(action.label) { connect() }
                        .buttonStyle(AgentRelayPrimaryButtonStyle())
                        .disabled(connecting)
                case .openAgain:
                    Button(action.label) { connect() }
                        .disabled(connecting)
                }
            }
        }
        .zoomPadding(16)
        // Restarts when the state changes, so a waiting login is followed,
        // and on every Settings invalidation (a Gateway that came up, a
        // login that ended); ends when the row goes away.
        .task(id: "\(status?.state.rawValue ?? "")#\(model.settingsInvalidations)") { await follow() }
    }

    /// Connect's error and the last sign-in's failure, each once, never
    /// repeating the detail.
    private func errors(_ row: OpenClawToolsCopy.Row?) -> [String] {
        var shown: [String] = []
        for text in [error, row?.error].compactMap({ $0 }) where text != row?.detail && !shown.contains(text) {
            shown.append(text)
        }
        return shown
    }

    private func follow() async {
        await model.followOpenClawTools(read: read, waiting: { OpenClawToolsCopy.polls(status) })
    }

    /// Reads the status, unless a Connect was asked for meanwhile: its
    /// answer is newer.
    private func read() async {
        let asked = connects
        let answer = await model.openClawToolsStatus()
        if asked == connects { status = answer }
    }

    private func connect() {
        guard !connecting else { return }
        connecting = true
        connects += 1
        error = nil
        Task {
            switch await model.connectOpenClawTools() {
            case .success(let state):
                status = OpenClawToolsStatus(state: state)
            case .failure(let failure):
                error = failure.message
                // The core may now say why it is unavailable.
                await read()
            }
            connecting = false
        }
    }
}

/// The core runs as a LaunchAgent: at login, without a window, through app
/// updates. Only show this recovery row when macOS requires approval.
private struct BackgroundServiceRow: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            SettingsIcon(systemImage: "bolt.horizontal.circle")
            VStack(alignment: .leading, spacing: 4) {
                Text("Background service").zoomFont(.body, weight: .medium)
                Text("Starts with Open at login to keep your agents connected and uploads running.")
                    .zoomFont(.footnote).foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if model.coreNeedsApproval {
                    Label("Allow Agent Relay in System Settings › General › Login Items.", systemImage: "exclamationmark.circle.fill")
                        .zoomFont(.footnote).foregroundStyle(CloudTheme.warning)
                }
            }
            Spacer(minLength: 20)
            if model.coreNeedsApproval {
                Button("Open Login Items…") { model.openLoginItemsSettings() }
            }
        }
        .zoomPadding(16)
    }
}

// MARK: - Caller requests

/// The wording of the caller requests section; GTK's
/// `settings_page/caller_requests.rs` uses the same sentences.
enum CallerRequestsCopy {
    static let title = "Caller requests"
    static let description = "Apps that ask to start agents on this computer wait here until you approve or deny them."
    static let loading = "Checking for requests…"
    static let empty = "No caller is waiting for a decision."
    static let signedOut = "Sign in to see caller requests."
    static let unattested = "This build of Agent Relay can't confirm it is the signed app, so caller requests can't be listed or decided here."

    /// What a caller asks for, in the person's words.
    static func detail(_ caller: CallerSummary) -> String {
        let harnesses = caller.harnesses.isEmpty ? "no" : caller.harnesses.joined(separator: ", ")
        let roots = caller.directoryRoots.isEmpty ? "no folder" : caller.directoryRoots.joined(separator: ", ")
        if caller.state == .pendingRotation {
            return "Asks for a new secret to keep starting \(harnesses) agents in \(roots)."
        }
        return "Wants to start \(harnesses) agents in \(roots)."
    }

    /// A core error, in the person's words; the core names the class in
    /// the message.
    static func error(_ error: CoreError) -> String {
        switch error.message {
        case "app_attestation_unavailable": return unattested
        case "not_signed_in": return signedOut
        default: return "Agent Relay couldn't complete that (\(error.message))."
        }
    }

    /// Only the callers that wait for a decision are listed.
    static func waiting(_ callers: [CallerSummary]) -> [CallerSummary] {
        callers.filter { $0.state == .pendingProvision || $0.state == .pendingRotation }
    }
}

/// Caller requests waiting for a decision (`spawn_callers.list`), each
/// approved or denied through `spawn_callers.decide`.
private struct CallerRequestsSection: View {
    @Environment(AppModel.self) private var model
    @State private var pending: [CallerSummary]?
    @State private var loading = false
    @State private var deciding: String?
    @State private var error: String?

    var body: some View {
        SettingsSection(title: CallerRequestsCopy.title, description: CallerRequestsCopy.description) {
            VStack(alignment: .leading, spacing: 12) {
                if !model.canSeeCallerRequests {
                    Text(CallerRequestsCopy.signedOut)
                        .zoomFont(size: 12).foregroundStyle(CloudTheme.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                } else {
                    let waiting = CallerRequestsCopy.waiting(pending ?? [])
                    if loading && pending == nil {
                        Text(CallerRequestsCopy.loading)
                            .zoomFont(size: 12).foregroundStyle(CloudTheme.secondary)
                    } else if waiting.isEmpty && error == nil {
                        Text(CallerRequestsCopy.empty)
                            .zoomFont(size: 12).foregroundStyle(CloudTheme.secondary)
                    }
                    ForEach(waiting, id: \.callerId) { caller in
                        VStack(alignment: .leading, spacing: 6) {
                            Text(caller.name).zoomFont(size: 13, weight: .medium)
                            Text(CallerRequestsCopy.detail(caller))
                                .zoomFont(size: 12).foregroundStyle(CloudTheme.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                            HStack(spacing: 12) {
                                Button("Approve") { decide(caller.callerId, approved: true) }
                                    .buttonStyle(AgentRelayPrimaryButtonStyle())
                                Button("Deny") { decide(caller.callerId, approved: false) }
                            }
                            .disabled(deciding != nil || !model.isCoreConnected)
                        }
                    }
                    if let error {
                        Text(error).zoomFont(size: 12).foregroundStyle(CloudTheme.warning)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Button("Refresh") { Task { await load() } }
                        .disabled(!model.isCoreConnected || loading)
                }
            }.zoomPadding(20)
        }
        // Loads on first show and again after a sign-in, perhaps as
        // someone else; a sign-out forgets the list.
        .task(id: model.canSeeCallerRequests) {
            pending = nil
            error = nil
            if model.canSeeCallerRequests { await load() }
        }
    }

    private func load() async {
        guard model.isCoreConnected, !loading else { return }
        loading = true
        error = nil
        switch await model.callerRequests() {
        case .success(let callers): pending = callers
        case .failure(let failure):
            pending = []
            error = CallerRequestsCopy.error(failure)
        }
        loading = false
    }

    private func decide(_ callerId: String, approved: Bool) {
        deciding = callerId
        error = nil
        Task {
            let result = await model.decideCallerRequest(callerId, approved: approved)
            deciding = nil
            switch result {
            case .success: await load()
            case .failure(let failure): error = CallerRequestsCopy.error(failure)
            }
        }
    }
}

// MARK: - Account

/// Who is signed in, the workspace, and logging out.
private struct AccountSettingsSection: View {
    @Environment(AppModel.self) private var model
    @State private var isConfirmingLogOut = false

    var body: some View {
        SettingsSection(title: "Profile") {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .center, spacing: 14) {
                    PersonAvatar(name: model.accountName ?? model.accountEmail ?? "?",
                                 url: model.accountAvatarURL.flatMap(URL.init(string:)), size: 44)
                    VStack(alignment: .leading, spacing: 5) {
                        Text(model.accountName ?? model.accountEmail ?? "Not signed in")
                            .zoomFont(size: 15, weight: .semibold)
                            .fixedSize(horizontal: false, vertical: true)
                        if model.accountName != nil, let email = model.accountEmail {
                            Text(email).zoomFont(size: 13).foregroundStyle(CloudTheme.secondary)
                                .textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                        }
                    }.frame(maxWidth: .infinity, alignment: .leading)
                    Button("Log out…") { isConfirmingLogOut = true }
                        .disabled(!model.canLogOut)
                }
                Text("Logging out stops uploads and disconnects this app. Local sessions stay where they are.")
                    .zoomFont(size: 12).foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }.zoomPadding(20)
        }
        SettingsSection(title: "Account health") {
            let health = AccountHealthPresentation(model.account)
            VStack(alignment: .leading, spacing: 12) {
                Text(model.isCheckingAccountHealth ? "Checking sign-in" : model.accountHealthError != nil ? "Health check failed" : health.label)
                    .zoomFont(size: 13, weight: .medium)
                Text(model.isCheckingAccountHealth ? "Checking your account and workspace access." : model.accountHealthError ?? health.detail)
                    .zoomFont(size: 12).foregroundStyle(CloudTheme.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                HStack {
                    Button(model.isCheckingAccountHealth ? "Checking…" : "Check health") {
                        Task { await model.checkAccountHealth() }
                    }
                    .disabled(!model.isCoreConnected || model.isCheckingAccountHealth)
                    if model.account.authHealth == "sign_in_required" {
                        Button("Sign in again") { model.renewSignIn() }
                            .buttonStyle(AgentRelayPrimaryButtonStyle())
                            .disabled(model.renewingSignIn)
                    }
                }
                if let error = model.signInError {
                    Text(error).zoomFont(size: 12).foregroundStyle(CloudTheme.warning)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }.zoomPadding(20)
        }
        CallerRequestsSection()
        SettingsSection(title: "Workspace") {
            HStack(alignment: .top, spacing: 14) {
                SettingsIcon(systemImage: "square.stack.3d.up")
                VStack(alignment: .leading, spacing: 6) {
                    Text(model.workspaceName).zoomFont(size: 13, weight: .medium)
                        .fixedSize(horizontal: false, vertical: true)
                    Text("Shared sessions belong to this workspace. Log out and sign in again to switch workspaces.")
                        .zoomFont(size: 12).foregroundStyle(CloudTheme.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }.frame(maxWidth: .infinity, alignment: .leading)
            }.zoomPadding(20)
            if let plan = model.workspacePlan {
                SettingsDivider()
                HStack(alignment: .top, spacing: 14) {
                    SettingsIcon(systemImage: "creditcard")
                    VStack(alignment: .leading, spacing: 6) {
                        Text(WorkspacePlanPresentation.title(plan)).zoomFont(size: 13, weight: .medium)
                            .accessibilityIdentifier("workspace-plan-title")
                        Text(WorkspacePlanPresentation.summary(plan, usage: model.workspaceUsage))
                            .zoomFont(size: 12).foregroundStyle(CloudTheme.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                            .accessibilityIdentifier("workspace-plan-summary")
                    }.frame(maxWidth: .infinity, alignment: .leading)
                    Button(WorkspacePlanPresentation.manageAction) { model.openPlan() }
                        .buttonStyle(AgentRelayPrimaryButtonStyle())
                        .help("Open your plan in Agent Relay")
                }.zoomPadding(20)
            }
        }
        .confirmationDialog(
            "Log out of Agent Relay?",
            isPresented: $isConfirmingLogOut
        ) {
            Button("Log out", role: .destructive) {
                Task {
                    await model.disconnect()
                }
            }
            Button("Cancel", role: .cancel) {
                isConfirmingLogOut = false
            }
        } message: {
            Text("Uploads stop and you will need to sign in again to reconnect.")
        }
    }
}

struct ClaudeDeliverySettings: View {
    var delivery: ClaudeDeliveryModel
    @State private var isExpanded = false

    var body: some View {
        DisclosureGroup(isExpanded: $isExpanded) {
            VStack(alignment: .leading, spacing: 12) {
                Text("Choose which Claude Code sessions and workspace members may receive messages directly. Ask first holds each message in Agent Relay for your approval.")
                    .zoomFont(.footnote)
                Toggle("Only allowed members may deliver directly", isOn: Binding(
                    get: { delivery.status?.restricted ?? false },
                    set: { value in Task { await delivery.restrict(value) } }
                ))
                if delivery.status?.restricted == true && delivery.status?.allowedSenders.isEmpty != false {
                    Text("No members are allowed yet. Their messages will wait for approval.")
                        .zoomFont(.caption).foregroundStyle(.secondary)
                }
                if let error = delivery.error {
                    Label(error, systemImage: "exclamationmark.triangle").zoomFont(.footnote).foregroundStyle(.red)
                }
                ForEach(delivery.status?.sessions ?? [], id: \.id) { session in
                    Toggle(isOn: Binding(
                        get: { session.direct },
                        set: { value in Task { await delivery.setSession(session.id, direct: value) } }
                    )) {
                        VStack(alignment: .leading) {
                            Text(session.title.isEmpty ? session.agent : session.title).zoomFont(.headline)
                            Text(session.direct ? "Direct delivery" : "Ask first").zoomFont(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
                ForEach(delivery.status?.allowedSenders ?? [], id: \.address) { sender in
                    HStack {
                        VStack(alignment: .leading) {
                            Text(sender.name).zoomFont(.headline)
                            Text(sender.address).zoomFont(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button("Remove") { Task { await delivery.setSender(sender, allowed: false) } }
                    }
                }
                ForEach(delivery.status?.held ?? [], id: \.id) { message in
                    VStack(alignment: .leading, spacing: 6) {
                        Text("\(message.senderName) → @\(message.agent)").zoomFont(.headline)
                        Text(message.text).lineLimit(8).textSelection(.enabled)
                        HStack {
                            Button("Approve once") { Task { await delivery.approve(message.id) } }
                            if !message.sender.isEmpty {
                                Button("Always allow") { Task { await delivery.alwaysAllow(message) } }
                            }
                            Button("Dismiss") { Task { await delivery.dismiss(message.id) } }
                        }
                    }
                }
                Button("Refresh") { Task { await delivery.refresh() } }
            }
            .zoomPadding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(CloudTheme.card)
            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .strokeBorder(CloudTheme.border)
            }
        } label: {
            Text("Claude Code message approvals")
                .zoomFont(size: 13, weight: .semibold)
                .accessibilityAddTraits(.isHeader)
        }
        .task { await delivery.refresh() }
    }
}


private struct TroubleshootingSettingsSection: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        SettingsSection(title: "Support") {
            HStack(alignment: .top, spacing: 14) {
                SettingsIcon(systemImage: "doc.text.magnifyingglass")
                VStack(alignment: .leading, spacing: 5) {
                    Text("Log file").zoomFont(size: 13, weight: .medium)
                    Text("What the background service recorded, for a bug report.")
                        .zoomFont(size: 12).foregroundStyle(CloudTheme.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }.frame(maxWidth: .infinity, alignment: .leading)
                Button("Open log file") { model.openLogFile() }
                    .disabled(model.installDirectory == nil)
            }.zoomPadding(20)
        }
    }
}

/// Explicit foreground/background pairs avoid macOS tint washing out actions.
/// The label keeps its intrinsic width while descriptions wrap beside it.
struct SettingsActionStyle: ButtonStyle {
    var prominent = false
    @ViewBuilder func makeBody(configuration: Configuration) -> some View {
        if prominent {
            AgentRelayPrimaryButtonStyle(height: 32).makeBody(configuration: configuration)
                .fixedSize(horizontal: true, vertical: false)
        } else {
            Action(configuration: configuration)
        }
    }
    private struct Action: View {
        let configuration: Configuration
        @Environment(\.isEnabled) private var enabled
        @Environment(\.isFocused) private var focused
        @State private var hovered = false

        var body: some View {
            configuration.label
                .zoomFont(size: 12, weight: .medium)
                .fixedSize(horizontal: true, vertical: false)
                .foregroundStyle(configuration.role == .destructive ? CloudTheme.settingsDanger : CloudTheme.foreground)
                .zoomPadding(.horizontal, 12).zoomFrame(minHeight: 32)
                .background(hovered && enabled ? CloudTheme.muted : CloudTheme.background,
                            in: RoundedRectangle(cornerRadius: 6))
                .overlay(RoundedRectangle(cornerRadius: 6)
                    .strokeBorder(focused ? CloudTheme.ring : CloudTheme.input,
                                  lineWidth: focused ? 2 : 1))
                .brightness(configuration.isPressed && enabled ? -0.06 : 0)
                .opacity(enabled ? 1 : 0.45)
                .contentShape(RoundedRectangle(cornerRadius: 6))
                .onHover { hovered = $0 }.actionCursor()
        }
    }
}
