import AppKit
import RelayDesktopCore
import SwiftUI

/// Sign in first; choose what to share in the Sessions window.
struct SignInView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.windowIsVisible) private var windowIsVisible
    @State private var startupMessage = Int.random(in: StartupPresentation.messages.indices)
    @State private var startupShown = Date()
    private let foreground = Color(red: 0.929, green: 0.957, blue: 0.984)
    private let muted = Color(red: 0.49, green: 0.54, blue: 0.60)

    var body: some View {
        GeometryReader { geometry in
            HStack(spacing: 0) {
                splash.frame(width: min(geometry.size.width * 1.3 / 2.3, max(0, geometry.size.width - 440)))
                ScrollView {
                    VStack(alignment: .leading, spacing: 0) {
                        Spacer(minLength: 24)
                        VStack(alignment: .leading, spacing: 0) {
                            HStack(spacing: 12) {
                                BrandMarkView(width: 42)
                                BrandWordmarkView(height: 28)
                            }
                            .accessibilityElement(children: .ignore)
                            .accessibilityLabel("Sign in to Agent Relay")
                            .zoomPadding(.bottom, siteBrand == nil ? 32 : 12)
                            // A self-host site's logo and organisation name.
                            if let config = siteBrand {
                                HStack(spacing: 8) {
                                    if let logo = config.logoUrl { SiteLogoView(url: logo) }
                                    if let org = config.orgName {
                                        Text(org).zoomFont(.callout).foregroundStyle(muted).lineLimit(1)
                                    }
                                }
                                .accessibilityElement(children: .combine)
                                .zoomPadding(.bottom, 32)
                            }
                            content
                            if model.phase != .launching, let error = model.signInError ?? model.coreProblem ?? model.lastError {
                                Label(error, systemImage: "exclamationmark.circle")
                                    .zoomFont(.callout).foregroundStyle(.red)
                                    .fixedSize(horizontal: false, vertical: true)
                                    .zoomPadding(.top, 24)
                            }
                            if model.phase == .signedOut { worksWith }
                            // A core for another site than production says
                            // so, and which: the core's site once it reported
                            // one, else this build's own.
                            if let site = model.developmentBadge {
                                Text("Development")
                                    .zoomFont(.callout).foregroundStyle(muted)
                                    .frame(maxWidth: .infinity)
                                    .zoomPadding(.top, 24)
                                    .help(site)
                            }
                        }
                        .zoomFrame(maxWidth: 360)
                        .frame(maxWidth: .infinity)
                        Spacer(minLength: 24)
                    }
                    .zoomPadding(.horizontal, 40)
                    .zoomPadding(.vertical, 32)
                    .frame(maxWidth: .infinity, minHeight: geometry.size.height)
                }
                .frame(maxWidth: .infinity)
                .background {
                    Color(red: 0.027, green: 0.063, blue: 0.086)
                    RadialGradient(colors: [Color(red: 0.29, green: 0.56, blue: 0.76).opacity(0.12), .clear],
                                   center: .leading, startRadius: 0, endRadius: 300)
                }
                .environment(\.colorScheme, .dark)
            }
            .foregroundStyle(foreground)
        }
    }

    private var splash: some View {
        LoginRibbonsView()
    }

    private var worksWith: some View {
        VStack(alignment: .leading, spacing: 0) {
            Path { path in path.move(to: .zero); path.addLine(to: CGPoint(x: 360, y: 0)) }
                .stroke(muted.opacity(0.35), style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
                .zoomFrame(height: 1)
            Text("Works with all your agents")
                .zoomFont(size: 11, design: .monospaced).tracking(0.4)
                .foregroundStyle(muted).zoomPadding(.top, 24)
            if let image = LoginArtwork.image("login-agents", extension: "svg") {
                Image(nsImage: image).resizable().scaledToFit()
                    .zoomFrame(maxWidth: 302, maxHeight: 22)
                    .opacity(0.72).zoomPadding(.top, 14)
                    .accessibilityLabel("Claude, Codex, OpenCode, Antigravity, Copilot, Cursor, Grok and Muse")
            }
        }
        .zoomPadding(.top, 40)
    }

    @ViewBuilder
    private var content: some View {
        switch model.phase {
        case .launching:
            startup
        case .waitingForBrowser, .signingIn:
            if model.account.phase == "selecting_workspace" {
                workspacePicker
            } else {
                SignInProgressView(
                    waitingForBrowser: model.phase == .waitingForBrowser,
                    canOpenBrowser: model.signInLink != nil,
                    openBrowser: model.openApprovalPageAgain,
                    cancel: model.cancelSignIn
                )
            }
        default:
            signIn
        }
    }

    @ViewBuilder
    private var workspacePicker: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 6) {
                Text("Choose your workspace")
                    .zoomFont(size: 28, weight: .semibold).tracking(-0.6)
                if let email = model.accountEmail {
                    Text("Signed in as \(email)")
                        .zoomFont(.callout).foregroundStyle(Color(red: 0.66, green: 0.72, blue: 0.78))
                }
            }
            VStack(spacing: 10) {
                ForEach(model.account.workspaceChoices ?? [], id: \.id) { workspace in
                    Button { model.selectWorkspace(workspace.id) } label: {
                        HStack(spacing: 12) {
                            Image(systemName: "square.stack.3d.up")
                                .foregroundStyle(Color(red: 0.45, green: 0.72, blue: 0.89))
                            VStack(alignment: .leading, spacing: 4) {
                                Text(workspace.name).fontWeight(.semibold)
                                if let organization = workspace.organizationName {
                                    Text(organization).zoomFont(.caption)
                                        .foregroundStyle(Color(red: 0.66, green: 0.72, blue: 0.78))
                                }
                            }
                            Spacer()
                            Image(systemName: "chevron.right")
                                .foregroundStyle(Color(red: 0.66, green: 0.72, blue: 0.78))
                        }
                        .foregroundStyle(foreground)
                        .zoomPadding(14).frame(maxWidth: .infinity, alignment: .leading)
                        .background(Color(red: 0.075, green: 0.125, blue: 0.17), in: RoundedRectangle(cornerRadius: 8))
                        .overlay(RoundedRectangle(cornerRadius: 8).stroke(foreground.opacity(0.18)))
                    }.buttonStyle(.plain)
                }
            }
            Button("Cancel", action: model.cancelSignIn).buttonStyle(.link)
        }
    }

    /// The site's login configuration when it names a logo or an organisation.
    private var siteBrand: LoginConfig? {
        model.siteLoginConfig.flatMap { $0.logoUrl == nil && $0.orgName == nil ? nil : $0 }
    }

    @ViewBuilder
    private var signIn: some View {
        // From the click until the core's snapshot shows the attempt, no
        // second one starts.
        VStack(alignment: .leading, spacing: 12) {
            // A self-host site's own page in the browser, naming the methods
            // it allows; a password is typed there, never here.
            if let config = model.siteLoginConfig {
                // Microsoft, when the site has it configured, opens the
                // site's own Microsoft sign-in, above the site's page.
                if let microsoft = config.microsoftName {
                    ProviderSignInButton(provider: .microsoft, name: microsoft) {
                        model.signIn(provider: .microsoft)
                    }
                }
                ProviderSignInButton(provider: nil) { model.signIn() }
                if let methods = config.methodsLine {
                    Text(methods).zoomFont(.callout).foregroundStyle(muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
            } else {
                ProviderSignInButton(provider: .google) { model.signIn(provider: .google) }
                ProviderSignInButton(provider: .microsoft) { model.signIn(provider: .microsoft) }
            }
        }
        .disabled(model.phase == .launching || model.startingSignIn)
        if model.account.action == .acknowledgeUninspectedSchedules {
            Button("Continue anyway") { model.signIn(acknowledgeUninspectedSchedules: true) }
                .buttonStyle(MenuPanelButtonStyle(kind: .secondary, controlHeight: 38))
                .help("Sign in although an earlier uploader's schedules could not be checked")
                .disabled(model.startingSignIn)
        }
        if model.account.action == .createWorkspace {
            HStack(spacing: 12) {
                Button("Open dashboard") { model.openWorkspaceSettings() }
                    .buttonStyle(AgentRelayPrimaryButtonStyle(height: 38, fontSize: 13))
                Button("Try again") { model.signIn() }
                    .buttonStyle(MenuPanelButtonStyle(kind: .secondary, controlHeight: 38))
                    .disabled(model.core == nil || model.startingSignIn)
            }
        }
        if model.coreNeedsApproval {
            Button("Open Login Items") { model.openLoginItemsSettings() }
                .buttonStyle(MenuPanelButtonStyle(kind: .secondary, controlHeight: 44))
        }
    }

    private var startup: some View {
        VStack(alignment: .leading, spacing: 24) {
            Group {
                if model.coreProblem != nil {
                    Image(systemName: model.coreNeedsInstallation ? "folder" :
                          model.coreNeedsApproval ? "lock.shield" : "arrow.triangle.2.circlepath")
                        .zoomFont(size: 25, weight: .medium)
                        .foregroundStyle(Color(red: 0.25, green: 0.43, blue: 0.56))
                        .zoomFrame(width: 60, height: 60)
                        .background(Color(red: 0.90, green: 0.94, blue: 0.96), in: RoundedRectangle(cornerRadius: 16))
                }
            }
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 12) {
                    Text(model.coreNeedsInstallation ? "Finish installation." :
                            model.coreNeedsApproval ? "One quick permission." :
                            model.coreProblem != nil ? "Let’s get you connected." : StartupPresentation.title)
                        .zoomFont(size: 28, weight: .semibold).tracking(-0.6)
                        .lineLimit(1)
                        .minimumScaleFactor(0.75)
                    if model.coreProblem == nil {
                        Spacer(minLength: 0)
                        RelayLoadingBars(label: "Starting Agent Relay", scale: 0.5)
                    }
                }
                if let problem = model.coreProblem {
                    startupText(problem)
                } else if reduceMotion || !windowIsVisible {
                    startupText(StartupPresentation.messages[startupMessage])
                } else {
                    // The messages take turns while the window shows them.
                    TimelineView(.periodic(from: startupShown, by: StartupPresentation.interval)) { context in
                        let turns = Int(context.date.timeIntervalSince(startupShown) / StartupPresentation.interval)
                        let index = (startupMessage + max(0, turns)) % StartupPresentation.messages.count
                        startupText(StartupPresentation.messages[index])
                            .animation(.easeInOut(duration: 0.25), value: index)
                    }
                }
            }
            if model.coreProblem != nil {
                VStack(spacing: 10) {
                    if model.coreNeedsInstallation {
                        Button("Open Applications") {
                            NSWorkspace.shared.open(URL(fileURLWithPath: "/Applications", isDirectory: true))
                        }
                        .buttonStyle(AgentRelayPrimaryButtonStyle(height: 44, expands: true))
                    } else if model.coreNeedsApproval {
                        Button("Open Login Items") { model.openLoginItemsSettings() }
                            .buttonStyle(MenuPanelButtonStyle(kind: .primary, controlHeight: 44))
                    }
                    if !model.coreNeedsInstallation {
                        Button {
                            Task { await model.retryBackgroundService() }
                        } label: {
                            Label(model.isRecoveringCore ? "Restarting service…" : "Try again", systemImage: "arrow.clockwise")
                                .frame(maxWidth: .infinity)
                        }
                        .buttonStyle(MenuPanelButtonStyle(kind: model.coreNeedsApproval ? .secondary : .primary, controlHeight: 44))
                        .disabled(model.isRecoveringCore)
                    }
                    if !model.coreNeedsInstallation && !model.coreNeedsApproval {
                        Button("Open Login Items") { model.openLoginItemsSettings() }
                            .buttonStyle(MenuPanelButtonStyle(kind: .secondary, controlHeight: 44))
                    }
                }
            }
            if model.coreProblem != nil {
                Button("Copy diagnostics") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(
                        DesktopDiagnostics.report(DesktopDiagnostics.sections(for: model)), forType: .string
                    )
                }
                .buttonStyle(.link)
            }
        }
    }

    private func startupText(_ text: String) -> some View {
        Text(text)
            .zoomFont(size: 15).foregroundStyle(.secondary)
            .lineSpacing(4)
            .fixedSize(horizontal: false, vertical: true)
            .zoomFrame(minHeight: 48, alignment: .topLeading)
            .contentTransition(.opacity)
    }

}

/// Fixed-size content keeps the login panel still as the loading bars move.
/// The window while the account leaves, from Log out until the core's
/// snapshot no longer shows it; the Linux app's signing-out screen says the
/// same.
struct SigningOutView: View {
    var body: some View {
        VStack(spacing: 16) {
            RelayLoadingBars()
            Text("Disconnecting this computer…")
                .zoomFont(size: 16, weight: .medium)
                .foregroundStyle(Color.primary)
            Text("Uploads are stopping safely.")
                .zoomFont(size: 13)
                .foregroundStyle(CloudTheme.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .combine)
    }
}

struct SignInProgressView: View {
    var waitingForBrowser: Bool
    var canOpenBrowser = false
    var openBrowser: () -> Void = {}
    var cancel: () -> Void

    var body: some View {
        VStack(spacing: 24) {
            VStack(spacing: 16) {
                RelayLoadingBars()
                    .zoomPadding(.top, 8)
                Text(waitingForBrowser ? "Continue in your browser" : "Connecting…")
                    .zoomFont(size: 16, weight: .medium)
                    .foregroundStyle(Color.primary)
            }
            .frame(maxWidth: .infinity)

            HStack(spacing: 10) {
                Button("Cancel", action: cancel)
                    .buttonStyle(MenuPanelButtonStyle(kind: .secondary, controlHeight: 44))
                    .keyboardShortcut(.cancelAction)
                if waitingForBrowser {
                    Button(action: openBrowser) {
                        Label("Open browser", systemImage: "arrow.up.right")
                    }
                    .buttonStyle(MenuPanelButtonStyle(kind: .primary, controlHeight: 44))
                    .disabled(!canOpenBrowser)
                }
            }
        }
    }
}

/// A quiet travelling wave in the homepage's primary blue. The surrounding
/// layout never changes size; Reduce Motion gets a static set of bars.
struct RelayLoadingBars: View {
    var label = "Connecting"
    var scale: CGFloat = 1
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.windowIsVisible) private var windowIsVisible
    @State private var start = Date()
    @State private var visible = false

    var body: some View {
        Group {
            if reduceMotion {
                bars(at: 0)
            } else {
                // The clock runs only while the bars are on screen.
                TimelineView(.animation(minimumInterval: 1 / 30, paused: !visible || !windowIsVisible)) { context in
                    bars(at: context.date.timeIntervalSince(start))
                }
            }
        }
        .scaleEffect(scale)
        .zoomFrame(width: 88 * scale, height: 40 * scale)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label)
        .onAppear { start = Date(); visible = true }
        .onDisappear { visible = false }
    }

    private func bars(at time: TimeInterval) -> some View {
        HStack(alignment: .center, spacing: 5) {
            ForEach(0..<9) { index in
                RelayLoadingBar(wave: wave(at: time, index: index))
            }
        }
        .zoomFrame(width: 88, height: 40)
    }

    private func wave(at time: TimeInterval, index: Int) -> Double {
        let cycle = time * 2 * Double.pi / 1.8
        let phase = cycle - Double(index) * 0.55
        let normalized = (sin(phase) + 1) / 2
        return normalized * normalized
    }
}

private struct RelayLoadingBar: View {
    let wave: Double

    var body: some View {
        Capsule()
            .fill(Color(red: 45 / 255, green: 106 / 255, blue: 156 / 255))
            .zoomFrame(width: 4, height: CGFloat(8 + 24 * wave))
            .opacity(0.3 + 0.7 * wave)
    }
}

/// Both identity providers use the website’s authentication controls.
/// A provider's sign-in button, or with no provider the site's own page.
/// The GTK app's `sign_in_methods::sign_in_label` matches.
struct ProviderSignInButton: View {
    var provider: SignInProvider?
    /// A site's own name for the provider's sign-in, shown in place of the
    /// provider's.
    var name: String? = nil
    var action: () -> Void

    static func label(_ provider: SignInProvider?, name: String? = nil) -> String {
        if provider != nil, let name { return "Continue with \(name)" }
        switch provider {
        case .google: return "Continue with Google"
        case .microsoft: return "Continue with Microsoft"
        case nil: return "Continue in browser"
        }
    }

    private var label: String { Self.label(provider, name: name) }

    var body: some View {
        Button(action: action) {
            HStack(spacing: 12) {
                if provider == .google { GoogleGlyph(length: 18) }
                else if provider == .microsoft { MicrosoftGlyph(length: 18) }
                Text(label)
            }
        }
        .buttonStyle(ProviderSignInStyle())
        .accessibilityLabel(label)
    }
}

/// Branded authentication controls match agentrelay.com/login.
private struct ProviderSignInStyle: ButtonStyle {
    @Environment(\.isEnabled) private var enabled
    @State private var hovered = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .zoomFont(size: 15, weight: .semibold)
            .foregroundStyle(Color(red: 0.051, green: 0.102, blue: 0.153))
            .zoomFrame(maxWidth: .infinity, minHeight: 50)
            .background(hovered ? Color(red: 0.933, green: 0.957, blue: 0.976) : .white,
                        in: UnevenRoundedRectangle(topLeadingRadius: 12, bottomLeadingRadius: 4,
                                                   bottomTrailingRadius: 12, topTrailingRadius: 12))
            .overlay(UnevenRoundedRectangle(topLeadingRadius: 12, bottomLeadingRadius: 4,
                                             bottomTrailingRadius: 12, topTrailingRadius: 12)
                .stroke(Color.black.opacity(0.14), lineWidth: 1))
            .shadow(color: .black.opacity(0.12), radius: 2, y: 1)
            .opacity(enabled ? 1 : 0.5)
            .scaleEffect(configuration.isPressed ? 0.99 : 1)
            .onHover { hovered = $0 }
            .actionCursor()
    }
}

private enum LoginArtwork {
    static func image(_ name: String, extension ext: String) -> NSImage? {
        let resources = Bundle.main.url(forResource: "RelayDesktop_RelayDesktop", withExtension: "bundle")
            .flatMap { Bundle(url: $0) } ?? .module
        return resources.url(forResource: name, withExtension: ext).flatMap { NSImage(contentsOf: $0) }
    }
}

struct MicrosoftGlyph: View {
    var length: CGFloat = 18

    var body: some View {
        VStack(spacing: 2) {
            HStack(spacing: 2) {
                Color(red: 0.953, green: 0.314, blue: 0.133)
                Color(red: 0.498, green: 0.729, blue: 0)
            }
            HStack(spacing: 2) {
                Color(red: 0, green: 0.643, blue: 0.937)
                Color(red: 1, green: 0.725, blue: 0)
            }
        }
        .zoomFrame(width: length, height: length)
        .accessibilityHidden(true)
    }
}

/// A plain four-arc approximation of the Google mark: no artwork files.
struct GoogleGlyph: View {
    var length: CGFloat = 18

    private var lineWidth: CGFloat { length * 0.24 }

    private var blue: Color { Color(red: 0.259, green: 0.522, blue: 0.957) }
    private var green: Color { Color(red: 0.204, green: 0.659, blue: 0.325) }
    private var yellow: Color { Color(red: 0.984, green: 0.737, blue: 0.020) }
    private var red: Color { Color(red: 0.918, green: 0.263, blue: 0.208) }

    var body: some View {
        ZStack {
            Circle()
                .trim(from: 0.00, to: 0.16)
                .stroke(blue, lineWidth: lineWidth)
            Circle()
                .trim(from: 0.16, to: 0.45)
                .stroke(green, lineWidth: lineWidth)
            Circle()
                .trim(from: 0.45, to: 0.66)
                .stroke(yellow, lineWidth: lineWidth)
            Circle()
                .trim(from: 0.66, to: 0.95)
                .stroke(red, lineWidth: lineWidth)
            Rectangle()
                .fill(blue)
                .frame(width: length * 0.30, height: lineWidth)
                .offset(x: length * 0.22)
        }
        .padding(lineWidth / 2)
        .frame(width: length, height: length)
        .accessibilityHidden(true)
    }
}
