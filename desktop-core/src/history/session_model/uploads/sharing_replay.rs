//! The question asked before a sharing switch that re-sends the whole local
//! history (`uploader.sharing.preview`'s `replays_history`), in place of the
//! mode's own. macOS says the same (`SharingMode.replayConfirmation*`).

/// The question.
pub const SHARING_REPLAY_TITLE: &str = "Share all sessions?";
/// Why the switch is asked about.
pub const SHARING_REPLAY_DETAIL: &str = "Sharing all sessions re-sends your whole local history to Agent Relay Cloud. That can take hours, and new sessions wait behind it.";
/// The button that confirms it.
pub const SHARING_REPLAY_ACTION: &str = "Share all sessions";
