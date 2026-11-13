import Foundation

/// Matches the GTK unified mobile continuation setting word for word. One
/// switch, on unless turned off, sets both routes the core reports.
public enum MobileNativeContinuationCopy {
    public static let title = "Continue agents from mobile"
    public static let unavailable = "Not available on this platform yet. A supported native session connection is required."
    public static let detail = "On unless turned off. Your messages from the phone continue a Claude Code, Pi, OpenCode or Codex app session as your own turns, with its permissions unchanged. Claude Code's tool approvals come to your phone, and compatible Codex app windows send command and file approval requests to your phone; other prompts stay on your computer. Needs Claude Code 2.1.310 or later in the 2.1 series; Agent Relay installs its plugin in each shared Claude login folder, and a Claude session already open needs a restart or /reload-plugins once. Reading a session sends no message. Keep the computer awake and the session running. Messages already accepted by an agent may finish after you turn this off."
    /// Shown under the row until this computer's ownership is verified.
    public static let waiting = "Waiting for this computer's ownership to be verified for your signed-in account. Turn on Accept spawns from Agent Relay first."

    public static func isEnabled(_ settings: SettingsValues?) -> Bool {
        settings?.mobileClaudeHandoff == true || settings?.mobileNativeContinuation == true
    }

    public static func isSupported(_ settings: SettingsValues?) -> Bool {
        settings?.mobileClaudeHandoffSupported == true || settings?.mobileNativeContinuationSupported == true
    }

    public static func change(enabled: Bool, settings: SettingsValues?) -> SettingsChange {
        SettingsChange(
            mobileClaudeHandoff: settings?.mobileClaudeHandoff == nil ? nil : enabled,
            mobileNativeContinuation: settings?.mobileNativeContinuation == nil ? nil : enabled
        )
    }
}
