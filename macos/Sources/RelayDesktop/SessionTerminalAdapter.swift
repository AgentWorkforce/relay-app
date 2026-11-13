import AppKit
import RelayDesktopCore

/// Host-specific operations create a new surface; no command is ever sent to
/// an existing session. Unsupported hosts remain visible as unavailable.
enum SessionTerminalAdapter: String, CaseIterable {
    case ghostty = "com.mitchellh.ghostty"
    case terminal = "com.apple.Terminal"
    case iterm = "com.googlecode.iterm2"
    case kitty = "net.kovidgoyal.kitty"
    case wezterm = "com.github.wez.wezterm"
    case alacritty = "org.alacritty"

    var usesAppleEvents: Bool { [.ghostty, .terminal, .iterm].contains(self) }

    static func appleString(_ value: String) -> String {
        "\"" + value.replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"") + "\""
    }

    /// The shared core captures only provider store paths. Verified absence
    /// must survive the new login shell too; never inherit a changed override.
    static let providerEnvironmentKeys: Set<String> = ["CLAUDE_CONFIG_DIR", "CODEX_HOME", "CODEX_SQLITE_HOME", "GROK_HOME"]

    static func validEnvironment(_ command: SessionResumeCommand) -> Bool {
        command.environment?.keys.allSatisfy(providerEnvironmentKeys.contains) ?? true
    }

    static func shellCommand(_ command: SessionResumeCommand) -> String {
        var invocation = [command.executable] + command.args
        if let environment = command.environment {
            invocation = ["/usr/bin/env"] + providerEnvironmentKeys.sorted().flatMap { ["-u", $0] }
                + environment.sorted(by: { $0.key < $1.key }).map { "\($0.key)=\($0.value)" } + invocation
        }
        return "cd -- \(SessionOpening.quote(command.cwd)) && exec "
            + invocation.map(SessionOpening.quote).joined(separator: " ")
    }

    func resumeScript(_ command: SessionResumeCommand) -> String? {
        let executable = "/bin/zsh -lic \(SessionOpening.quote(Self.shellCommand(command)))"
        let shell = "exec " + executable
        let body: String
        switch self {
        case .ghostty:
            // Ghostty.sdef: surface configuration command and initial working
            // directory are creation settings, not input to an existing pane.
            body = """
            set config to new surface configuration
            set initial working directory of config to \(Self.appleString(command.cwd))
            set command of config to \(Self.appleString(shell))
            new window with configuration config
            activate
            """
        case .terminal:
            // Omitting `in` creates a new window (Terminal's scripting contract).
            body = "do script \(Self.appleString(shell))\nactivate"
        case .iterm:
            // https://iterm2.com/documentation-scripting.html
            body = "create window with default profile command \(Self.appleString(executable))\nactivate"
        default: return nil
        }
        return "with timeout of 15 seconds\ntell application id \"\(rawValue)\"\n\(body)\nreturn \"dispatched\"\nend tell\nend timeout"
    }

    func arguments(_ command: SessionResumeCommand) -> [String]? {
        let shell = ["/bin/zsh", "-lic", Self.shellCommand(command)]
        switch self {
        // The regular GUI launch CLI avoids remote-control socket/config
        // assumptions and always creates a new native window.
        case .kitty: return ["--directory", command.cwd] + shell
        case .wezterm: return ["start", "--always-new-process", "--cwd", command.cwd, "--"] + shell
        case .alacritty: return ["--working-directory", command.cwd, "--command"] + shell
        default: return nil
        }
    }

    @MainActor func resume(_ command: SessionResumeCommand) async -> String? {
        guard let app = NSWorkspace.shared.urlForApplication(withBundleIdentifier: rawValue) else {
            return "The original terminal is unavailable. Install it to resume this session there."
        }
        if let script = resumeScript(command) {
            return await SessionLocationLauncher.execute(script, expected: "dispatched")
        }
        guard Bundle(url: app)?.executableURL != nil, let arguments = arguments(command) else {
            return "This terminal does not support resuming the original session."
        }
        let config = NSWorkspace.OpenConfiguration()
        config.activates = true
        config.createsNewApplicationInstance = true
        config.arguments = arguments
        do {
            // Launch Services resolves the installed app. Never execute
            // a historical transcript's host_executable as a program.
            try await NSWorkspace.shared.openApplication(at: app, configuration: config)
            return nil
        } catch { return error.localizedDescription }
    }
}
