import Foundation

/// Wording for the Claude Code SessionStart hook setting. The hook is on by default after sign-in. It tells a session that is not on the relay how to
/// activate with POST /register, and it never registers the session. The
/// Linux app's `CLAUDE_SESSION_HOOK_*` constants match these word for word.
public enum ClaudeSessionHookCopy {
    public static let title = "Ask Claude chats to join"
    public static let turnedOn = "Ask at the beginning of the session if it should go on the relay."
    public static let turnedOff = "Ask at the beginning of the session if it should go on the relay."
    public static let turnOnHelp = "Add the session-start prompt and permissions for Agent Relay's own read and reply commands. Registration still needs approval."
    public static let turnOffHelp = "Remove the session-start prompt and its Agent Relay command permissions"
}
