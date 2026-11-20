//! The `coordination-advisories/1` rows and the note each becomes.

use super::*;

/// One advisory as `GET /v1/coordination/advisories` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advisory {
    pub id: String,
    /// `high` or `low`.
    pub urgency: &'static str,
    pub message: String,
    pub peer_source: String,
    pub peer_session_id: String,
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

pub(super) fn advisory(row: &Value) -> Option<Advisory> {
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
        message,
        peer_source: peer_source.to_string(),
        peer_session_id: peer_session_id.to_string(),
    })
}

/// A listing's advisories: at most [`MAX_ROWS`], each once, oldest first as
/// listed. `None` when the body is not this contract's answer.
pub fn parse(body: &Value) -> Option<Vec<Advisory>> {
    let listed: Listed = serde_json::from_value(body.clone()).ok()?;
    if listed.contract.as_deref().is_some_and(|c| c != CONTRACT) {
        return None;
    }
    let mut seen = HashSet::new();
    Some(
        listed
            .advisories
            .iter()
            .take(MAX_ROWS)
            .filter_map(advisory)
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
