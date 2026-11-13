import AppKit
import RelayDesktopCore
import SwiftUI

struct MainWindowView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.uiZoom) private var zoom
    private let updater = AppUpdater.shared

    var body: some View {
        Group {
            switch model.phase {
            case .attached, .preparingSessions:
                if model.signingOut { SigningOutView() } else { connected }
            default:
                SignInView()
            }
        }
        .buttonStyle(NativeActionButtonStyle(base: DefaultButtonStyle()))
        .foregroundStyle(CloudTheme.foreground)
        .tint(CloudTheme.primary).accentColor(CloudTheme.primary)
        .background(CloudTheme.background)
        // The window's smallest size grows with the zoom, up to the screen.
        .frame(minWidth: minimumSize.width, minHeight: minimumSize.height)
        // SwiftUI can route links to this scene before the app delegate.
        .onOpenURL { model.openConnection($0) }
        // Everything that runs for the window runs only while it is on screen.
        .tracksWindowVisibility { model.windowVisibilityChanged($0) }
        .onDisappear { model.windowVisibilityChanged(false) }
        .onChange(of: model.phase) { if $0 == .signedOut { model.tools.reset() } }
        .task(id: model.phase) {
            if model.phase == .attached || model.phase == .preparingSessions { await model.refreshVisibleState() }
        }
    }

    /// 980 by 650 at 100%; `MIN_WINDOW` in `gtk/src/ui/zoom.rs`.
    static let actualMinimumSize = CGSize(width: 980, height: 650)

    private var minimumSize: CGSize { zoom.fitted(Self.actualMinimumSize, in: UIZoom.usableMainScreen) }

    private var connected: some View {
        ToolsOnboardingHost(
            tools: model.tools,
            identity: "\(model.siteURL)|\(model.accountID ?? "")|\(model.workspaceID ?? "")",
            isCoreConnected: model.isCoreConnected,
            isSignedIn: model.account.canChooseProviders
        ) {
            WorkspaceShell(
                relay: model.relay,
                updater: updater
            ) {
                VStack(alignment: .leading, spacing: 0) {
                    switch model.page {
                    case .sessions: LocalSessionsView(relay: model.relay)
                    case .settings(let pane): SettingsPaneView(pane: pane)
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            }
            .messageComposer(model.relay, model: model)
        }
    }
}
