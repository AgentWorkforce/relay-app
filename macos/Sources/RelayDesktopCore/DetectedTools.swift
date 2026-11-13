import Foundation

/// The sign-in screen's pre-checked line naming the tools found on this
/// computer (`detected_tools`) that signing in connects to the relay. Its
/// state is the core's `connect_detected_tools`: nothing is written to a
/// tool before sign-in, and unticking it keeps every tool as it is. The
/// Linux app's `models::detected_tools_label` matches this word for word.
public enum DetectedTools {
    public static let detail = "After you sign in, Agent Relay adds its settings to each one's config so its sessions can get relay messages. Turn any of them off later in Settings."
    /// Sign-in waits for the settings, which name the tools it connects.
    public static let unreadSettings = "Agent Relay could not read its settings, so it cannot show which tools signing in connects. Try again."
    /// Sign-in waits for the tools choice, whose save the core restarted
    /// under twice. Mac only: the GTK app's bridge answers such a save with
    /// an error, which it shows instead.
    public static let unsavedChoice = "Agent Relay restarted before it saved your choice about connecting tools. Check the line again, then sign in."

    /// The line, or `nil` when there is nothing to connect.
    public static func label(_ settings: SettingsValues?) -> String? {
        guard let tools = settings?.detectedTools, let last = tools.last else { return nil }
        let named = tools.count == 1 ? last : "\(tools.dropLast().joined(separator: ", ")) and \(last)"
        return "Connect \(named) to Agent Relay"
    }
}

/// Why Claude Code direct delivery is the organization's choice.
public enum ManagedDirectDelivery {
    public static func detail(_ managed: String) -> String {
        if managed == "refuse" {
            return "Your organization's managed settings set `crossSessionInbound` to `refuse`, so Claude Code drops relayed messages. Ask your admin to change it."
        }
        return "Your organization's managed settings set `crossSessionInbound` to `\(managed)`, so Claude Code asks before showing each message. Ask your admin to change it."
    }
}
