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

struct SessionOpenButton: View {
    let row: LocalSessionRow
    /// The row's session as last read, when newer than the row's.
    var session: LocalSession? = nil
    @Environment(AppModel.self) private var model
    private let applications = SessionApplications.shared
    @AppStorage("sessionOpening.claude") private var claudePreference: SessionOpenPreference = .automatic
    @AppStorage("sessionOpening.codex") private var codexPreference: SessionOpenPreference = .automatic

    private var preference: SessionOpenPreference { row.source == "claude" ? claudePreference : codexPreference }
    @State private var error: String?
    @State private var opening = false

    @State private var location: SessionLocationReport?
    @State private var plan: SessionOpenPlan?
    @State private var notice: String?
    @State private var permitsDefaultTerminal = false
    @State private var resolvingPendingResume = false
    @State private var locationReadError: String?
    private var sessionKey: String { "\(row.source):\(row.sessionID)" }
    private var resumePending: Bool { SessionPlanExecutor.shared.pendingResumes.contains(sessionKey) }
    private var resumeStorageError: String? { SessionPlanExecutor.shared.resumeStorageError }
    private var tracksLocation: Bool {
        ["codex", "claude", "grok"].contains(row.source)
            && model.client.offers(CoreMethod.sessionsOpenPlan)
            && row.relay?.target.kind != .claudeCloud
            && SpawnHandover.of(session ?? row.relay) == nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if tracksLocation {
                if let plan {
                    if plan.action == .choose {
                        Menu {
                            ForEach(Array(plan.choices.enumerated()), id: \.offset) { _, target in
                                Button("\(target.application) · \(target.tty ?? String(target.pid))") {
                                    Task { await openShared(selected: target) }
                                }
                            }
                        } label: {
                            SessionOpenActionLabel(title: "Open running session", source: row.source, showsArrow: false)
                        }
                        .buttonStyle(CloudControlStyle(height: 38, horizontalPadding: 12))
                        .disabled(opening)
                    } else {
                        Button { Task { await openShared() } } label: {
                            SessionOpenActionLabel(
                                title: opening ? "Opening session…" : SessionLocationNavigation.title(plan),
                                source: row.source,
                                terminal: (plan.action == .resume && plan.command?.args.contains("--desktop") != true)
                                    || ((plan.action == .focus || plan.action == .showApplication) && plan.location?.tty != nil)
                            )
                        }
                            .buttonStyle(CloudControlStyle(height: 38, horizontalPadding: 12))
                            .help(SessionLocationNavigation.title(plan))
                            .accessibilityLabel(opening ? "Opening session…" : SessionLocationNavigation.title(plan))
                            .disabled(opening || plan.action == .unavailable || ((resumePending || resumeStorageError != nil) && plan.action == .resume))
                    }
                    if let reason = plan.reason {
                        Text(reason).zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
                    }
                } else {
                    Text("Checking session location…").zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
                }
                if let error { Text(error).zoomFont(.caption).foregroundStyle(CloudTheme.secondary).textSelection(.enabled) }
                if let resumeStorageError, resumeStorageError != error {
                    Text(resumeStorageError).zoomFont(.caption).foregroundStyle(CloudTheme.secondary).textSelection(.enabled)
                }
                if let notice, !resumePending { Text(notice).zoomFont(.caption).foregroundStyle(CloudTheme.secondary) }
                if resumePending {
                    Text(SessionPlanExecutor.pendingResumeMessage).zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
                    Button("Resolve earlier launch…") { resolvingPendingResume = true }.disabled(opening || resumeStorageError != nil)
                }
                if !resumePending, resumeStorageError == nil, let plan, SessionLocationNavigation.offersDefaultResume(plan, afterFailure: permitsDefaultTerminal) {
                    Button { Task { await openShared(useDefaultTerminal: true) } } label: {
                        SessionOpenActionLabel(title: "Resume in default terminal", source: row.source, terminal: true)
                    }
                        .buttonStyle(CloudControlStyle(height: 38, horizontalPadding: 12)).disabled(opening)
                }
            } else if SpawnHandover.of(session ?? row.relay) != nil {
                resumeBody
            } else if ["codex", "claude", "grok"].contains(row.source), row.relay?.target.kind != .claudeCloud {
                Text("Session opening requires an updated local core. Update Agent Relay to return to the original application.")
                    .zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
            }
            if let locationReadError {
                Text(locationReadError).zoomFont(.caption).foregroundStyle(CloudTheme.secondary)
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
        .confirmationDialog("Did you close or stop the earlier launch?", isPresented: $resolvingPendingResume, titleVisibility: .visible) {
            Button("I closed or stopped it") {
                if SessionPlanExecutor.shared.resolvePendingResume(key: sessionKey) {
                    error = nil
                    notice = nil
                }
            }
            Button("Keep waiting", role: .cancel) {}
        } message: {
            Text("The earlier terminal or application may still be starting the provider. Close or stop that launch before allowing another resume request.")
        }
        .task(id: row.id) {
            plan = nil
            location = nil
            locationReadError = nil
            error = nil
            notice = nil
            permitsDefaultTerminal = false
            resolvingPendingResume = false
            applications.refresh()
            guard tracksLocation else { return }
            repeat {
                do {
                    let fresh = try await readPlan()
                    guard !Task.isCancelled else { return }
                    plan = fresh
                    SessionPlanExecutor.shared.observe(key: sessionKey, plan: fresh)
                    locationReadError = nil
                    location = try? await model.client.sessionsLocation(SessionTitleIdentity(sessionId: row.sessionID, source: row.source))
                } catch {
                    guard !Task.isCancelled else { return }
                    locationReadError = "Couldn’t verify this session’s location. \(error.localizedDescription)"
                }
                do { try await Task.sleep(for: .seconds(5)) } catch { return }
            } while !Task.isCancelled
        }
    }

    private func readPlan(selected: SessionLocation? = nil, useDefaultTerminal: Bool = false) async throws -> SessionOpenPlan {
        try await model.client.sessionsOpenPlan(SessionOpenRequest(directory: row.directory,
            preferTerminal: preference == .terminal, selected: selected,
            sessionId: row.sessionID, source: row.source, useDefaultTerminal: useDefaultTerminal))
    }

    @MainActor private func openShared(selected: SessionLocation? = nil, useDefaultTerminal: Bool = false) async {
        guard !opening else { return }
        opening = true
        defer { opening = false }
        error = nil
        notice = nil
        permitsDefaultTerminal = false
        do {
            let fresh = try await readPlan(selected: selected, useDefaultTerminal: useDefaultTerminal)
            plan = fresh
            if useDefaultTerminal, fresh.action != .resume || fresh.location != nil {
                error = fresh.reason ?? "This session’s state changed. Refresh before resuming it in a default terminal."
                return
            }
            let outcome = await SessionPlanExecutor.shared.execute(key: sessionKey, plan: fresh) {
                try await readPlan(selected: selected, useDefaultTerminal: useDefaultTerminal)
            }
            switch outcome {
            case .focused: break
            case .dispatched(let message): notice = message
            case .failed(let message):
                error = message
                permitsDefaultTerminal = fresh.canResumeDefault
            }
        } catch { self.error = error.localizedDescription }
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
