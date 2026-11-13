import Foundation

/// Wording for Agent Relay's Codex hook setting (`codex_stop_hook`). The hook
/// runs after each tool call and as a turn ends, so a message for a Codex
/// session that is working reaches it inside the running turn. The Linux
/// app's `CODEX_HOOK_*` constants match these word for word.
public enum CodexHookCopy {
    public static let title = "Codex mid-turn delivery"
    public static let turnedOn = "Agent Relay's Codex hook is in ~/.codex/hooks.json. A message for a Codex session that is working reaches it at the session's next tool call or as its turn ends, instead of waiting for the turn to finish. Its rules in ~/.codex/rules/agent-relay.rules let the session read and answer relayed messages through Agent Relay without asking."
    public static let turnedOff = "A message for a Codex session that is working waits until its turn ends, however long that takes. Turn this on to add Agent Relay's Codex hook to ~/.codex/hooks.json, so the message reaches the session at its next tool call or as its turn ends. Its rules in ~/.codex/rules/agent-relay.rules let the session read and answer relayed messages through Agent Relay without asking."
    public static let turnOnHelp = "Add Agent Relay's hook to Codex"
    public static let turnOffHelp = "Remove Agent Relay's hook from Codex"
}
