//! An uploaded session's transcript read back from Agent Relay Cloud: how
//! the apps show the full conversation of one of your agents on another
//! computer, from the session it uploads.

use agent_relay_node::relay::contract::transcript::LocalTranscriptEntry;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `sessions.cloud_transcript`'s parameters.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema, Serialize)]
pub struct CloudTranscriptParams {
    /// The uploaded session, as the roster names it.
    pub session_id: String,
    /// Continues from where an earlier page ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// At most this many events, newest first; 200 when absent, 1000 at most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

/// One page of an uploaded session, newest entries first, as the local
/// reader's entries.
#[derive(Clone, Debug, PartialEq, Serialize, JsonSchema, Deserialize)]
pub struct CloudTranscriptPage {
    pub entries: Vec<LocalTranscriptEntry>,
    /// Where the next, older page starts; absent at the session's start.
    pub next_cursor: Option<String>,
}
