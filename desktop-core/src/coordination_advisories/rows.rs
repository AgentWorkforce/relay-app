//! The listed rows (`coordination-advisories/1`, or [`CONTRACT_V3`] with an
//! incarnation per row) and the note each becomes.

use super::*;

/// One advisory as `GET /v1/coordination/advisories` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advisory {
    pub id: String,
    /// `high` or `low`.
    pub urgency: &'static str,
    /// Additive M3-7 ranking, supplied by the pipeline; absent/unknown is
    /// never promoted to an urgent local injection or a system notice.
    pub priority: Option<u8>,
    /// An older server-side DM already accepted this advisory.
    pub dm_delivered: bool,
    /// Server-side evaluation gate. Missing is false, including old APIs.
    pub notify_eligible: bool,
    pub message: String,
    pub peer_source: String,
    pub peer_session_id: String,
    /// The row's incarnation (a lower-case UUID), listed only under
    /// [`CONTRACT_V3`]; `None` is a legacy row, keyed and acked by its id.
    pub incarnation: Option<String>,
    /// What this side keys the row by ([`instance::key`]): its id for a
    /// legacy row; set for its account and session as it is cached.
    pub(super) key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Listed {
    #[serde(default)]
    pub(super) contract: Option<String>,
    pub(super) advisories: Vec<Value>,
}

/// An id safe in a path and a note: the pipeline's ids are short hex.
pub(super) fn plain_id(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

/// `value` on one line, at most `max` characters.
pub(super) fn one_line(value: &str, max: usize) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max)
        .collect()
}

/// One listed row; `versioned` under [`CONTRACT_V3`] only, else any
/// incarnation it carries is ignored and the row is legacy.
pub(super) fn advisory(row: &Value, versioned: bool) -> Option<Advisory> {
    let incarnation = if versioned {
        instance::incarnation(&row["incarnation"]).ok()?
    } else {
        None
    };
    let id = row["advisoryId"].as_str().filter(|id| plain_id(id))?;
    let peer_source = row["peer"]["source"].as_str().filter(|s| plain_id(s))?;
    let peer_session_id = row["peer"]["sessionId"].as_str().filter(|s| plain_id(s))?;
    let message = one_line(row["message"].as_str()?, MAX_MESSAGE);
    if message.is_empty() {
        return None;
    }
    let urgency = match row["urgency"].as_str() {
        Some(urgency) if urgency.eq_ignore_ascii_case("high") => "high",
        _ => "low",
    };
    Some(Advisory {
        id: id.to_string(),
        urgency,
        priority: row["priority"]
            .as_u64()
            .filter(|p| (1..=3).contains(p))
            .map(|p| p as u8),
        dm_delivered: row["delivery"]["dm"].as_bool().unwrap_or(false),
        notify_eligible: row["notifyEligible"].as_bool().unwrap_or(false),
        message,
        peer_source: peer_source.to_string(),
        peer_session_id: peer_session_id.to_string(),
        incarnation,
        key: id.to_string(),
    })
}

impl Advisory {
    /// Priority one and high, on an instance row: a legacy row (any listing
    /// but [`CONTRACT_V3`], or a null incarnation) is never urgent, so no
    /// note is handed to an agent mid-turn under a key its id could reuse.
    pub fn urgent(&self) -> bool {
        self.priority_one() && self.incarnation.is_some()
    }

    /// Priority one and high, whatever the contract: what a build before
    /// instances handed to agents mid-turn, so a receipt it saved may hold it.
    pub(super) fn priority_one(&self) -> bool {
        self.priority == Some(1) && self.urgency == "high"
    }
}

/// A listing's advisories: at most [`MAX_ROWS`], each once, oldest first as
/// listed. `None` when the body is not a listing. Exactly [`CONTRACT_V3`]
/// lists instance rows; [`CONTRACT`], no contract or any other value is
/// read as legacy, as today: its rows reach the turn end and none is urgent.
pub fn parse(body: &Value) -> Option<Vec<Advisory>> {
    let listed: Listed = serde_json::from_value(body.clone()).ok()?;
    let versioned = listed.contract.as_deref() == Some(CONTRACT_V3);
    let mut seen = HashSet::new();
    Some(
        listed
            .advisories
            .iter()
            .take(MAX_ROWS)
            .filter_map(|row| advisory(row, versioned))
            .filter(|advisory| seen.insert(advisory.id.clone()))
            .collect(),
    )
}

/// One advisory's line in the note.
pub fn line(advisory: &Advisory) -> String {
    format!(
        "[Agent Relay coordination advisory: {}] {} (peer {}:{}, advisory {})",
        advisory.urgency,
        advisory.message,
        advisory.peer_source,
        advisory.peer_session_id,
        advisory.id
    )
}
