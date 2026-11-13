import Foundation

/// Wording for the trusted senders setting (`trusted_relay_senders`),
/// shown while OpenClaw or Hermes is on. The Linux app's `TRUSTED_SENDERS_*`
/// constants match these word for word.
public enum TrustedSendersCopy {
    public static let title = "Trusted senders"
    public static let detail = "Relay names or addresses trusted like you: their messages reach your own OpenClaw and Hermes, with their memory and tools. Everyone else gets an answer-only chat of their own. Separate them with commas."
    public static let placeholder = "teammate, name@machine"

    /// The `trust_workspace_members` toggle, Cloud's `trustWorkspaceMembers`.
    public static let membersToggle = "Trust people in your workspace"
    public static let membersDetail = "Messages they send as themselves reach your own OpenClaw and Hermes too. Their agents are not included: add the ones you trust below."
    /// The label beside each person listed from the workspace.
    public static let memberLabel = "Workspace member"
    public static let membersEmpty = "No people from your workspace to show yet."
    /// Above the field, while the people in the workspace are trusted.
    public static let othersLabel = "Other names and agents"

    /// A person listed from the workspace: their name in Agent Relay with
    /// their relay name, or the relay name alone.
    public static func memberTitle(name: String, displayName: String?) -> String {
        displayName.map { "\($0) (\(name))" } ?? name
    }

    /// The comma-separated field as the list `trusted_relay_senders` takes:
    /// trimmed, empty entries dropped.
    public static func senders(from text: String) -> [String] {
        text.split(separator: ",").map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.filter { !$0.isEmpty }
    }
}
