import Foundation

/// The `cursor_direct_delivery` setting's wording, shared with Linux and
/// Windows (`agent_relay_app::models::CURSOR_HOOKS_*`). Agent Relay's hooks in
/// `~/.cursor/hooks.json`, which the Cursor editor's agent and Cursor CLI
/// both run, hand a session its messages at its next tool call or when its
/// turn ends; an editor chat whose turn has ended gets them as they arrive.
/// Off unless turned on.
public enum CursorHooks {
    public static let title = "Cursor sessions"
    public static let onExplanation = "Agent Relay's hooks are in ~/.cursor/hooks.json. A Cursor editor or Cursor CLI session gets its messages at its next tool call or when its turn ends. A Cursor editor chat that has finished its turn gets them as soon as they arrive, for up to an hour; a Cursor CLI session waiting for you gets them during the next turn you start. Start Cursor CLI sessions that were open before this was on again."
    public static let offExplanation = "Without Agent Relay's hooks, messages for a Cursor session wait in its inbox, unheard. Turn this on to add them to ~/.cursor/hooks.json, so a Cursor editor or Cursor CLI session you put on the relay gets its messages at its next tool call or when its turn ends, and a Cursor editor chat that has finished its turn gets them as soon as they arrive."
    /// Shown on the page whether the hooks are on or off: Cursor runs Cloud
    /// agents, Projects and Remote Control chats in its cloud, where neither
    /// file is read. Linux and Windows say "This PC".
    public static let localOnlyNote = "Only a local Cursor agent (This Mac) can use Agent Relay. Cloud agents, Projects and Remote Control (My Machines) chats cannot, because Cursor does not load ~/.cursor/hooks.json or ~/.cursor/mcp.json for them. Pick This Mac in the selector under the chat box, or make it the default in Cursor Settings > Agents > Conversation > Default Environment."
    public static let turnOnHelp = "Add Agent Relay's hooks to Cursor"
    public static let turnOffHelp = "Remove Agent Relay's hooks from Cursor"
}
