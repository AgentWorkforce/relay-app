import Foundation

/// The Claude Code config folders whose sessions this computer finds and
/// uploads (`claude_config_dirs`), in Settings › Relay. The Linux app's
/// `models::settings::claude_folder_*` matches this word for word.
public enum ClaudeFolders {
    public static let title = "Claude Code folders"
    public static let detail = "Agent Relay finds Claude Code sessions and uploads their transcripts from each shared folder. A folder a running session uses through CLAUDE_CONFIG_DIR is added when it is seen. Turn a folder off to keep its sessions off the relay and out of uploads."
    /// The opt-in for claude.ai cloud sessions (`claude_cloud_sessions`).
    public static let cloudTitle = "Claude Code cloud sessions"
    /// The Linux app's line stops after the first sentence: Claude Code
    /// keeps its login in a file there, with no Keychain to ask.
    public static let cloudDetail = "Also upload your claude.ai cloud sessions, read with the Claude Code login in ~/.claude while that folder is shared. When you turn this on, macOS asks whether Agent Relay may use Claude Code's sign-in from your Keychain: choose Always Allow so uploads continue in the background."
    public static let addLabel = "Add Folder…"
    public static let addHelp = "Watch another Claude Code config folder, such as one you set as CLAUDE_CONFIG_DIR"
    public static let removeLabel = "Remove"
    public static let shareHelp = "Share this folder's sessions: put them on the relay and upload them"
    public static let notShared = "Not shared"

    /// Why a folder is watched, as the core reports it.
    public static func originLabel(_ origin: String) -> String {
        switch origin {
        case "default": return "Default"
        case "sibling": return "Another Claude login"
        case "environment": return "Set by AGENT_RELAY_CLAUDE_CONFIG_DIRS"
        case "added": return "Added here"
        case "running": return "Seen on a running session"
        default: return origin
        }
    }

    /// The line under a folder: why it is watched, and whether it is shared.
    public static func rowDetail(_ folder: ClaudeConfigDir) -> String {
        let origin = originLabel(folder.origin)
        return folder.shared ? origin : "\(origin) · \(notShared)"
    }

    /// Only a folder added here can be removed here.
    public static func isRemovable(_ folder: ClaudeConfigDir) -> Bool {
        folder.origin == "added"
    }

    /// `path` with the home folder written `~`.
    public static func displayPath(_ path: String, home: String) -> String {
        guard !home.isEmpty else { return path }
        if path == home { return "~" }
        let prefix = home.hasSuffix("/") ? home : home + "/"
        return path.hasPrefix(prefix) ? "~/" + String(path.dropFirst(prefix.count)) : path
    }

    /// The added folders with `path` added at the end.
    public static func adding(_ path: String, to folders: [ClaudeConfigDir]) -> [String] {
        let added = folders.filter(isRemovable).map(\.path)
        return added.contains(path) ? added : added + [path]
    }

    /// The added folders without `path`.
    public static func removing(_ path: String, from folders: [ClaudeConfigDir]) -> [String] {
        folders.filter(isRemovable).map(\.path).filter { $0 != path }
    }
}

/// The Claude cloud source's health as Settings words it. Linux and Windows
/// keep the same labels and sentences in `models::settings`.
public enum ClaudeCloudHealthCopy {
    public static func label(_ health: ClaudeCloudHealth?) -> String {
        switch health?.state {
        case .working?: return "Working"
        case .blocked?: return "Blocked"
        case .needsAttention?: return "Needs attention"
        case .checking?, nil: return "Checking"
        }
    }

    public static func detail(_ health: ClaudeCloudHealth?) -> String {
        var parts: [String] = []
        if let reason = health?.reason, !reason.isEmpty {
            parts.append(reason.hasSuffix(".") ? reason : "\(reason).")
        } else if health == nil || health?.state == .checking {
            parts.append("Waiting for the first Claude cloud-session pass.")
        }
        if let milliseconds = health?.lastSuccessMs, milliseconds > 0 {
            let date = Date(timeIntervalSince1970: Double(milliseconds) / 1_000)
            parts.append("Last successful pass: \(date.formatted(date: .abbreviated, time: .shortened)).")
        } else if health?.state != .working {
            parts.append("No successful pass has completed yet.")
        }
        return parts.joined(separator: " ")
    }
}
