import SwiftUI
import AppKit
import RelayDesktopCore

/// The same full-width provider action in both the location and handover paths.
struct SessionOpenActionLabel: View {
    let title: String
    let source: String
    var terminal = false
    var showsArrow = true

    var body: some View {
        HStack(spacing: 6) {
            if terminal {
                Image(systemName: "terminal")
            } else {
                AgentIconView(source: source, size: 18)
            }
            Text(title)
            if showsArrow && !terminal {
                Image(systemName: "arrow.up.right")
                    .font(.system(size: 10, weight: .semibold))
            }
        }.frame(maxWidth: .infinity)
    }
}

private struct SessionOpenEngineKey: EnvironmentKey {
    static let defaultValue = SessionOpenEngine.live
}

extension EnvironmentValues {
    /// The helper the Open button asks; tests give it a fake runner.
    var sessionOpenEngine: SessionOpenEngine {
        get { self[SessionOpenEngineKey.self] }
        set { self[SessionOpenEngineKey.self] = newValue }
    }
}

struct SessionOpenButton: View {
    let row: LocalSessionRow
    /// The row's session as last read, when newer than the row's.
    var session: LocalSession? = nil
    @Environment(AppModel.self) private var model
    @Environment(\.sessionOpenEngine) private var engine
    private let applications = SessionApplications.shared
    @AppStorage("sessionOpening.claude") private var claudePreference: SessionOpenPreference = .automatic
    @AppStorage("sessionOpening.codex") private var codexPreference: SessionOpenPreference = .automatic

    /// The handover path (`resumeBody`) reads this directly.
    private var preference: SessionOpenPreference { row.source == "claude" ? claudePreference : codexPreference }
    /// "Open with: Terminal", sent to the helper as `prefer_terminal`.
    /// Claude and Codex have the setting; other providers never prefer.
    private var preferTerminal: Bool {
        switch row.source {
        case "claude": return claudePreference == .terminal
        case "codex": return codexPreference == .terminal
        default: return false
        }
    }
    @State private var error: String?
    @State private var opening = false

    @State private var location: SessionLocationReport?
    @State private var opener: SessionOpenModel?
    /// The sources the helper's `session-open` opens.
    static let sources: Set<String> = ["claude", "codex", "grok", "opencode"]
    private var tracksLocation: Bool {
        Self.sources.contains(row.source)
            && row.relay?.target.kind != .claudeCloud
            && SpawnHandover.of(session ?? row.relay) == nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if tracksLocation {
                if !engine.isAvailable {
                    Text(SessionOpen.Strings.fallback.helperMissing)
                        .zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
                } else if let opener {
                    SessionOpenControls(open: opener, source: row.source)
                } else {
                    Text(SessionOpen.Strings.fallback.checking).zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
                }
            } else if SpawnHandover.of(session ?? row.relay) != nil {
                resumeBody
            }
            if tracksLocation, let location {
                if !location.current.isEmpty {
                    Text("Running in " + location.current.map(\.application).joined(separator: ", "))
                        .zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
                } else if let previous = location.previous.first {
                    Text("Last seen in \(previous.application) · \(Date(timeIntervalSince1970: Double(previous.observedAtMs) / 1000).formatted(date: .abbreviated, time: .shortened))")
                        .zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .task(id: row.id) {
            opener = nil
            location = nil
            error = nil
            applications.refresh()
            guard tracksLocation, engine.isAvailable else { return }
            let current = SessionOpenModel(source: row.source, sessionId: row.sessionID, directory: row.directory,
                                           appDataDir: AppInfo.dataDirectory.path, preferTerminal: preferTerminal,
                                           engine: engine)
            opener = current
            // A look when the row appears, then on a slow timer while the
            // app is in front; the task ends when the row is no longer
            // shown. Coming to the front and each open or release (their
            // own answers) look too.
            await look(current)
            while !Task.isCancelled {
                do { try await Task.sleep(for: SessionOpenModel.pollInterval) } catch { return }
                if NSApplication.shared.isActive { await look(current) }
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            guard let opener else { return }
            Task { await look(opener) }
        }
        .onChange(of: preferTerminal) { _, prefers in
            guard let opener else { return }
            opener.preferTerminal = prefers
            Task { await opener.refresh() }
        }
    }

    /// One look at the button and where the core last saw the session.
    @MainActor private func look(_ current: SessionOpenModel) async {
        await current.refresh()
        guard !Task.isCancelled, opener === current else { return }
        location = try? await model.client.sessionsLocation(SessionTitleIdentity(sessionId: row.sessionID, source: row.source))
    }

    @ViewBuilder private var resumeBody: some View {
        let sessionOpening = SessionOpening(row: row, relay: session)
        if let action = sessionOpening {
            let destination = action.destination(preference: preference, desktopAvailable: applications.desktopAvailable(for: row.source))
            let title = destination == .desktop || action.liveOnly ? action.title : destination == .terminal ? "Open in Terminal" : "Set up session opening"
            Button {
                Task { await open(action) }
            } label: {
                HStack(spacing: 6) {
                    if destination == .desktop || action.liveOnly {
                        AgentIconView(source: row.source, size: 18)
                        Text(title)
                        Image(systemName: "arrow.up.right")
                            .zoomFont(size: 10, weight: .semibold)
                    } else {
                        Label(title, systemImage: destination == .terminal ? "terminal" : "arrow.up.right")
                    }
                }.frame(maxWidth: .infinity)
            }.buttonStyle(CloudControlStyle(height: 38, horizontalPadding: 12)).disabled(opening)
                .help(title).accessibilityLabel(title)
            .onAppear { applications.refresh() }
            .alert("Couldn’t open session", isPresented: Binding(get: { error != nil }, set: { if !$0 { error = nil } })) {
                if action.command != nil {
                    Button("Resume in new Terminal") { Task { await open(action, inTerminal: true) } }
                }
                Button("Opening settings") { model.page = Self.settingsDestination(source: row.source, tools: model.tools) }
                Button("Cancel", role: .cancel) { error = nil }
            } message: { Text(error ?? "") }
        }
        if let note = SpawnHandover.hostedNote(session ?? row.relay, opening: sessionOpening)
            ?? SessionOpening.heldNote(session) {
            Text(note).zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    @MainActor private func open(_ action: SessionOpening, inTerminal: Bool = false) async {
        guard !opening else { return }
        opening = true
        defer { opening = false }
        error = nil
        applications.refresh()
        let destination = action.destination(preference: inTerminal ? .terminal : preference,
                                             desktopAvailable: applications.desktopAvailable(for: row.source))
        if destination == .unavailable && action.liveOnly {
            error = SpawnHandover.codexMissing
            return
        }
        if destination == .unavailable {
            model.page = Self.settingsDestination(source: row.source, tools: model.tools)
            return
        }
        error = await Self.run(action, destination: destination)
    }

    @MainActor static func settingsDestination(source: String, tools: ToolsModel) -> DesktopPage {
        tools.showProvider(source)
        return .settings(.tools)
    }

    /// Opens `action` where `destination` says; why it failed, if it did.
    @MainActor static func run(_ action: SessionOpening, destination: SessionOpenDestination) async -> String? {
        if destination == .desktop {
            if let command = action.desktopCommand {
                return await launchDesktop(command)
            }
            if let url = action.deepLink {
                return NSWorkspace.shared.open(url) ? nil : "Couldn’t open the exact session link. No alternative session was started."
            }
        }
        guard let command = action.command else {
            return "Install Codex to open this session. Its working directory is unavailable for terminal resume."
        }
        guard let terminal = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.apple.Terminal") else {
            return "Terminal is unavailable. Check your session-opening preferences in Settings."
        }
        // A private, self-removing command file avoids Apple Events permission
        // and never interpolates transcript metadata into AppleScript.
        let file = FileManager.default.temporaryDirectory.appendingPathComponent("relay-resume-\(UUID().uuidString).command")
        do {
            let script = "#!/bin/sh\n/bin/rm -- \"$0\"\nexec /bin/zsh -lic \(SessionOpening.quote(command))\n"
            try Data(script.utf8).write(to: file, options: .atomic)
            try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: file.path)
            let config = NSWorkspace.OpenConfiguration()
            config.activates = true
            try await NSWorkspace.shared.open([file], withApplicationAt: terminal, configuration: config)
            return nil
        } catch {
            try? FileManager.default.removeItem(at: file)
            return "\(error.localizedDescription) Check your session-opening preferences in Settings."
        }
    }

    /// The CLI opens Desktop and exits; no terminal window or prompt is created.
    static func launchDesktop(_ command: String) async -> String? {
        await SessionDesktopLauncher.launch(command)
    }
}

/// The helper's answer for one session: the button (or the choose menu),
/// its reason, the outcome of the last open, and the follow-ups it offers.
struct SessionOpenControls: View {
    @Bindable var open: SessionOpenModel
    let source: String

    private var confirming: Binding<Bool> {
        Binding(get: { if case .confirm = open.prompt { return true } else { return false } },
                set: { if !$0, case .confirm = open.prompt { open.prompt = nil } })
    }
    private var takingOver: Binding<Bool> {
        Binding(get: { if case .takeOver = open.prompt { return true } else { return false } },
                set: { if !$0, case .takeOver = open.prompt { open.prompt = nil } })
    }
    private var resolvingLaunch: Binding<Bool> {
        Binding(get: { open.prompt == .launchGone }, set: { if !$0, open.prompt == .launchGone { open.prompt = nil } })
    }
    private var question: String {
        switch open.prompt {
        case .confirm(let message), .takeOver(let message): return message
        case .launchGone, nil: return ""
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if let plan = open.plan {
                if let choices = open.choices {
                    Menu {
                        ForEach(choices) { choice in
                            Button(choice.label) { Task { await open.choose(choice.id) } }
                        }
                    } label: {
                        SessionOpenActionLabel(title: open.title, source: source, showsArrow: false)
                    }
                    .buttonStyle(CloudControlStyle(height: 38, horizontalPadding: 12))
                    .disabled(!open.enabled)
                } else {
                    Button { Task { await open.press() } } label: {
                        SessionOpenActionLabel(title: open.title, source: source, terminal: open.showsTerminal)
                    }
                    .buttonStyle(CloudControlStyle(height: 38, horizontalPadding: 12))
                    .help(plan.title)
                    .accessibilityLabel(open.title)
                    .disabled(!open.enabled)
                }
                if let reason = open.reason {
                    Text(reason).zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
                }
            } else {
                Text(open.strings.checking).zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
            }
            if let error = open.error, error != open.reason {
                Text(error).zoomFont(.caption).foregroundStyle(CloudTheme.secondary).textSelection(.enabled)
            }
            if let notice = open.notice { Text(notice).zoomFont(.caption).foregroundStyle(CloudTheme.secondary) }
            if open.offersLaunchGone {
                Button(open.strings.resolveLaunch) { open.askLaunchGone() }.disabled(open.opening || open.releasing)
            }
            if open.offersDefaultResume {
                Button { Task { await open.resumeInDefaultTerminal() } } label: {
                    SessionOpenActionLabel(title: open.strings.resumeDefault, source: source, terminal: true)
                }
                .buttonStyle(CloudControlStyle(height: 38, horizontalPadding: 12))
                .disabled(open.opening || open.releasing)
            }
            if let readError = open.readError {
                Text(readError).zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
            }
        }
        .confirmationDialog(open.plan?.title ?? "", isPresented: confirming, titleVisibility: .visible) {
            Button(open.plan?.title ?? "") { Task { await open.confirmNotOpenElsewhere() } }
            Button(open.strings.cancel, role: .cancel) {}
        } message: { Text(question) }
        .confirmationDialog(open.plan?.title ?? "", isPresented: takingOver, titleVisibility: .visible) {
            Button(open.plan?.title ?? "", role: .destructive) { Task { await open.confirmTakeOver() } }
            Button(open.strings.cancel, role: .cancel) {}
        } message: { Text(question) }
        .confirmationDialog(open.strings.launchQuestion, isPresented: resolvingLaunch, titleVisibility: .visible) {
            Button(open.strings.launchClosed) { Task { await open.confirmLaunchGone() } }
            Button(open.strings.launchWait, role: .cancel) {}
        } message: {
            Text(open.strings.launchDetail)
        }
    }
}
