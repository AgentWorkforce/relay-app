import Foundation

// How the app presents the generated bridge contract types
// (`CoreContract.swift`): identities, labels and derived values. The types
// themselves come from the core's Rust registry.

public enum BridgeTime {
    /// Milliseconds since the epoch; zero, negative and missing mean never.
    public static func date(milliseconds: Int64?) -> Date? {
        guard let milliseconds, milliseconds > 0 else { return nil }
        return Date(timeIntervalSince1970: Double(milliseconds) / 1000.0)
    }
}

// MARK: - Snapshot

extension WorkspaceStatus {
    public var isFound: Bool { state == .found }
}

extension PushState {
    public var isLive: Bool { self == .live }
}

extension SignInState {
    /// The last attempt ended without a session.
    public var isFailure: Bool { self == .denied || self == .expired || self == .error }
}

extension AccountStatus {
    public static let signedOut = AccountStatus(state: .signedOut)

    /// Local history preparation follows authentication and workspace attachment.
    /// Choosing providers must not wait for that potentially long scan.
    public var canChooseProviders: Bool {
        state == .signedIn || (state == .preparing && phase == "preparing_local_sessions")
    }
}

extension ToolIntegration {
    public func onboardingDetail(account: AccountStatus) -> String {
        if account.state == .preparing && account.phase == "preparing_local_sessions" && state == "signed_out" {
            return "Choose this provider now. Setup will finish after your local sessions are prepared."
        }
        return detail
    }
}

extension SignInReport {
    /// The account part of the report, as `status.account` carries it.
    public var account: AccountStatus {
        AccountStatus(action: action, error: error, needsSignIn: needsSignIn, person: person, phase: phase, progress: progress,
                      reusedLogin: reusedLogin, siteUrl: siteUrl, state: state, workspaceId: workspaceId,
                      workspaceName: workspaceName)
    }

    /// The page to open: the complete link when there is one.
    public var link: String? { verificationUrlComplete ?? verificationUrl }
}

extension CaptureProgress {
    public var isCapturing: Bool { phase == "scanning" || phase == "capture_paused" }
    public var headline: String { phase == "capture_paused" ? "History preparation paused · Retrying" : "Reading local history" }
    public var detail: String {
        let count = totalFiles.map { "\(processedFiles.formatted()) of \($0.formatted()) files processed" }
            ?? "\(processedFiles.formatted()) files processed"
        return "\(source.isEmpty ? "Local history" : source.capitalized): \(count) · \(sessionsCaptured.formatted()) sessions captured locally."
    }
    public var fraction: Double? {
        guard let totalFiles, totalFiles > 0 else { return nil }
        return min(1, max(0, Double(processedFiles) / Double(totalFiles)))
    }
}

// MARK: - Sessions and agents on this computer

/// The app a person chats with a session in, when it is not the provider's
/// own: Conductor runs Claude Code and Codex sessions in its workspaces.
public enum SessionHost {
    /// The host's name, or nil for a session in the provider's own app.
    public static func name(_ host: String?) -> String? {
        switch host?.lowercased() {
        case "conductor": return "Conductor"
        case "paseo": return "Paseo"
        default: return nil
        }
    }

    /// A Claude Code on the web session's host, as the core names it.
    public static let claudeWeb = "claude-web"

    /// Whether a Claude Code web session id body uses the provider's shape.
    public static func isClaudeSessionBody(_ body: Substring) -> Bool {
        (8...64).contains(body.utf8.count) && body.utf8.allSatisfy { byte in
            (48...57).contains(byte) || (65...90).contains(byte) || (97...122).contains(byte)
        }
    }

    /// The provider-stable body shared by `session_` and `cse_` ids.
    public static func claudeSessionBody(_ id: String) -> Substring? {
        let body: Substring
        if id.hasPrefix("session_") { body = id.dropFirst("session_".count) }
        else if id.hasPrefix("cse_") { body = id.dropFirst("cse_".count) }
        else { return nil }
        return isClaudeSessionBody(body) ? body : nil
    }

    /// Where a Claude cloud session runs: on the web, else in the Claude app.
    public static func claudeCloudPlace(_ host: String?) -> String {
        host == claudeWeb ? "Claude Code on the web" : "Claude app"
    }

    /// The icon's tooltip: "Codex in Conductor", or the provider alone.
    public static func label(provider: String, host: String?) -> String {
        if host == claudeWeb { return claudeCloudPlace(host) }
        return name(host).map { "\(provider) in \($0)" } ?? provider
    }
}

extension SessionKind {
    public var label: String {
        switch self {
        case .codex: return "Codex"
        case .claudeTerminal: return "Claude Code"
        case .claudeCloud: return "Claude app"
        case .grok, .grokOpen: return "Grok"
        case .opencode, .opencodeOpen: return "OpenCode"
        case .museOpen: return "Muse Code"
        case .devin: return "Devin"
        case .cursorCli: return "Cursor CLI"
        case .copilot: return "Copilot"
        case .openclaw: return "OpenClaw"
        case .hermes: return "Hermes"
        case .pi: return "Pi"
        case .omp: return "Oh My Pi"
        case .unknown(let kind): return kind
        }
    }

    /// The icon and history source name.
    public var source: String {
        switch self {
        case .codex: return "codex"
        case .claudeTerminal, .claudeCloud: return "claude"
        case .grok, .grokOpen: return "grok"
        case .opencode, .opencodeOpen: return "opencode"
        case .museOpen: return "muse"
        case .devin: return "devin"
        case .cursorCli: return "cursor"
        case .copilot: return "copilot"
        case .openclaw: return "openclaw"
        case .hermes: return "hermes"
        case .pi: return "pi"
        case .omp: return "omp"
        case .unknown(let kind): return kind
        }
    }

    /// The source the session icon is drawn for: a Claude Code terminal
    /// session shows the Claude Code crab, a Claude app session the Claude mark.
    public var iconSource: String {
        switch self {
        case .claudeTerminal: return "claude-code"
        case .claudeCloud: return "claude"
        default: return source
        }
    }
}

extension LocalSession {
    /// The uploader row this discovered session belongs to. Claude Code on
    /// the web has a remote transcript under Claude's history source; a
    /// Claude app cloud session still has no transcript to join.
    public var uploadKey: String? {
        if target.kind == .claudeCloud, host == SessionHost.claudeWeb {
            let body = SessionHost.claudeSessionBody(target.id)
            let id = body.map { "cse_\($0)" } ?? target.id
            return "\(target.kind.source):\(id)"
        }
        return target.uploadKey
    }

    /// An OpenCode window that was open before Agent Relay's OpenCode plugin
    /// was on: running, listed, but unreachable until OpenCode restarts. A
    /// registered session whose window has closed is not running, and is
    /// hidden like any ended session.
    public var needsOpenCodeRestart: Bool { !live && running != false && target.kind == .opencodeOpen }
}

extension Target {
    /// `kind:id`, unique across kinds.
    public var key: String { "\(kind.rawValue):\(id)" }

    /// The `SOURCE:ID` key the uploader lists this session under when the
    /// target alone decides it. A host-specific session uses
    /// `LocalSession.uploadKey` instead.
    public var uploadKey: String? {
        switch kind {
        // Every route names a session by its harness's own id.
        case .codex, .claudeTerminal, .grok, .grokOpen, .opencode, .opencodeOpen, .museOpen, .devin, .copilot:
            return "\(kind.source):\(id)"
        // Cursor CLI transcripts are not collected yet.
        case .claudeCloud, .openclaw, .hermes, .pi, .omp, .cursorCli, .unknown: return nil
        }
    }
}

extension LocalSession: Identifiable {
    public var id: String { target.key }
    public var updatedAt: Date? { BridgeTime.date(milliseconds: updatedAtMs) }
    public var projectName: String? { project.flatMap { $0.isEmpty ? nil : $0 } }
}

extension LocalAgent: Identifiable {
    public var id: String { name }
}

// MARK: - Workspace directory and messaging

extension RosterAgent: Identifiable {
    public var id: String { address ?? name }
}

extension DirectMessage: Identifiable {}

// MARK: - Uploader

extension SharingMode: Identifiable {
    public var id: Self { self }
}

extension SharingMode {
    /// The order Settings offers the modes in.
    public static let displayOrder: [SharingMode] = [.selected, .new, .all]

    public var title: String {
        switch self {
        case .all: return "Automatically upload all sessions, past and future"
        case .new: return "Automatically upload new sessions"
        case .selected: return "Only sessions I choose"
        }
    }

    public var explanation: String {
        switch self {
        case .all:
            return "Existing and future local coding sessions upload automatically. Sessions you turned off stay off until you enable Upload session again."
        case .new:
            return "Upload new sessions automatically. Existing sessions stay local unless you choose to share them."
        case .selected:
            return "New sessions stay local until you turn on Upload session. Once enabled, those sessions keep uploading changes until you turn it off."
        }
    }

    public var confirmationTitle: String {
        switch self {
        case .selected: return "Upload only sessions you choose?"
        case .new: return "Automatically upload new sessions?"
        case .all: return "Automatically upload all sessions?"
        }
    }

    public var confirmationAction: String {
        switch self {
        case .selected: return "Use selected sessions only"
        case .new: return "Enable automatic uploads"
        case .all: return "Upload all past and future sessions"
        }
    }
}

extension UploadSession: Identifiable {
    /// `SOURCE:ID`.
    public var id: String { "\(source):\(sessionId)" }

    /// Project for old cores that did not resolve repository identity.
    public var legacyProject: String {
        guard let cwd, !cwd.isEmpty else { return "" }
        let trimmed = cwd.hasSuffix("/") && cwd.count > 1 ? String(cwd.dropLast()) : cwd
        let component = (trimmed as NSString).lastPathComponent
        return component == "/" ? "" : component
    }

    /// The core-resolved project, or the cwd folder for an older core.
    public var displayProject: String { project ?? legacyProject }

    /// The local title, else the id.
    public var displayTitle: String {
        let trimmed = title.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? sessionId : trimmed
    }

    public var lastActivity: Date? { BridgeTime.date(milliseconds: lastActivityMs) }

    /// What a confirmed include or exclude makes this row until the next listing.
    public func withInclusion(_ included: Bool) -> UploadSession {
        UploadSession(avatar: avatar, cwd: cwd, firstActivityMs: firstActivityMs, gitBranch: gitBranch, host: host, included: included,
                      isAgent: isAgent, lastActivityMs: lastActivityMs, models: models, originator: originator, project: project, reasoningEffort: reasoningEffort, sessionId: sessionId, source: source,
                      status: included ? .shared : .notShared, title: title)
    }
}

public extension RequestEntry {
    /// A Relay Connect room's calls are named for the room, so its
    /// workspace's limits are not read as the team's.
    var serviceName: String {
        guard service == "Relaycast" else { return service }
        return connect == true ? "Relay Connect" : "Agent Relay"
    }
    var failureSummary: String {
        if service == "Cloud", status == 403, endpoint.contains("/messaging/desktop-identities") {
            return "Agents couldn't link to Cloud"
        }
        return "\(serviceName) request failed"
    }
}
