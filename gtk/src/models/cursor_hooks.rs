//! The `cursor_direct_delivery` row in Settings → Relay: Agent Relay's
//! hooks in `~/.cursor/hooks.json`, which the Cursor editor's agent and
//! Cursor CLI both run; an editor chat whose turn has ended gets its
//! messages as they arrive. The Mac's `CursorHooks` is the reference, word
//! for word.

pub const CURSOR_HOOKS_TITLE: &str = "Cursor sessions";
pub const CURSOR_HOOKS_ON_DETAIL: &str = "Agent Relay's hooks are in ~/.cursor/hooks.json. A Cursor editor or Cursor CLI session gets its messages at its next tool call or when its turn ends. A Cursor editor chat that has finished its turn gets them as soon as they arrive, for up to an hour; a Cursor CLI session waiting for you gets them during the next turn you start. Start Cursor CLI sessions that were open before this was on again.";
pub const CURSOR_HOOKS_OFF_DETAIL: &str = "Without Agent Relay's hooks, messages for a Cursor session wait in its inbox, unheard. Turn this on to add them to ~/.cursor/hooks.json, so a Cursor editor or Cursor CLI session you put on the relay gets its messages at its next tool call or when its turn ends, and a Cursor editor chat that has finished its turn gets them as soon as they arrive.";
/// Shown whether the hooks are on or off; the Mac says "This Mac".
pub const CURSOR_HOOKS_LOCAL_ONLY_NOTE: &str = "Only a local Cursor agent (This PC) can use Agent Relay. Cloud agents, Projects and Remote Control (My Machines) chats cannot, because Cursor does not load ~/.cursor/hooks.json or ~/.cursor/mcp.json for them. Pick This PC in the selector under the chat box, or make it the default in Cursor Settings > Agents > Conversation > Default Environment.";
pub const CURSOR_HOOKS_TURN_ON_HELP: &str = "Add Agent Relay's hooks to Cursor";
pub const CURSOR_HOOKS_TURN_OFF_HELP: &str = "Remove Agent Relay's hooks from Cursor";

/// Why the last change of each Settings › Relay hook row failed, in the
/// core's words: Devin's lifecycle hooks (also the Devin card's), Cursor
/// CLI's hooks, Claude Code's SessionStart hook and Agent Relay's Codex hook.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HookRowErrors {
    pub devin: Option<String>,
    pub cursor: Option<String>,
    /// Claude Code's SessionStart hook.
    pub claude_session: Option<String>,
    /// Agent Relay's Codex hook (`codex_stop_hook`).
    pub codex: Option<String>,
    /// Question escalation (`question_escalation`).
    pub question_escalation: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_hooks_wording_matches_the_mac() {
        assert_eq!(CURSOR_HOOKS_TITLE, "Cursor sessions");
        assert_eq!(
            CURSOR_HOOKS_ON_DETAIL,
            "Agent Relay's hooks are in ~/.cursor/hooks.json. A Cursor editor or Cursor CLI session gets its messages at its next tool call or when its turn ends. A Cursor editor chat that has finished its turn gets them as soon as they arrive, for up to an hour; a Cursor CLI session waiting for you gets them during the next turn you start. Start Cursor CLI sessions that were open before this was on again."
        );
        assert_eq!(
            CURSOR_HOOKS_OFF_DETAIL,
            "Without Agent Relay's hooks, messages for a Cursor session wait in its inbox, unheard. Turn this on to add them to ~/.cursor/hooks.json, so a Cursor editor or Cursor CLI session you put on the relay gets its messages at its next tool call or when its turn ends, and a Cursor editor chat that has finished its turn gets them as soon as they arrive."
        );
        assert_eq!(
            CURSOR_HOOKS_LOCAL_ONLY_NOTE,
            "Only a local Cursor agent (This PC) can use Agent Relay. Cloud agents, Projects and Remote Control (My Machines) chats cannot, because Cursor does not load ~/.cursor/hooks.json or ~/.cursor/mcp.json for them. Pick This PC in the selector under the chat box, or make it the default in Cursor Settings > Agents > Conversation > Default Environment."
        );
        assert_eq!(
            CURSOR_HOOKS_TURN_ON_HELP,
            "Add Agent Relay's hooks to Cursor"
        );
        assert_eq!(
            CURSOR_HOOKS_TURN_OFF_HELP,
            "Remove Agent Relay's hooks from Cursor"
        );
    }
}
