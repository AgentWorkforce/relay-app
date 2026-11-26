//! Codex rollout first records, `threads` rows and rollout file names
//! (Codex facts A).
use serde::Deserialize;
use serde_json::Value;

use super::{first_json, ids, is_any_absolute_path, whole_json, ParseError};
use crate::evidence::CodexOrigin;

const RECORD_LIMIT: usize = 1024 * 1024;

/// `originator` is the reliable CLI/desktop discriminator; `source` is
/// `"vscode"` for both in 0.167 (VERIFIED, facts A3).
pub fn origin(originator: Option<&str>) -> CodexOrigin {
    match originator {
        Some("Codex Desktop") => CodexOrigin::Desktop,
        Some("codex-tui") => CodexOrigin::Tui,
        Some("codex_exec") | Some("codex-exec") => CodexOrigin::Exec,
        _ => CodexOrigin::Other,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RolloutMeta {
    pub id: String,
    pub cwd: Option<String>,
    pub origin: CodexOrigin,
    pub cli_version: Option<String>,
    pub parent_thread_id: Option<String>,
    pub subagent: bool,
}

#[derive(Deserialize)]
struct RawRecord {
    #[serde(rename = "type")]
    kind: Option<String>,
    payload: Option<RawPayload>,
}

#[derive(Deserialize)]
struct RawPayload {
    id: Option<String>,
    cwd: Option<String>,
    originator: Option<String>,
    cli_version: Option<String>,
    parent_thread_id: Option<String>,
    source: Option<Value>,
    thread_source: Option<String>,
}

fn is_subagent(source: Option<&Value>, thread_source: Option<&str>, parent: bool) -> bool {
    let object_source = matches!(source, Some(Value::Object(map)) if map.contains_key("subagent"));
    let user = thread_source == Some("user");
    object_source || parent || !user
}

/// The first line of `rollout-…-<uuid>.jsonl`: `type: session_meta`.
pub fn parse_rollout_first(bytes: &[u8]) -> Result<RolloutMeta, ParseError> {
    let raw: RawRecord = first_json(bytes, RECORD_LIMIT)?;
    if raw.kind.as_deref() != Some("session_meta") {
        return Err(ParseError::InvalidValue("type"));
    }
    let payload = raw.payload.ok_or(ParseError::MissingField("payload"))?;
    let id = payload
        .id
        .as_deref()
        .and_then(ids::normalize_codex_thread_id)
        .ok_or(ParseError::InvalidId("id"))?;
    let parent = match payload.parent_thread_id.as_deref() {
        Some(p) => Some(ids::normalize_codex_thread_id(p).ok_or(ParseError::InvalidId("parent"))?),
        None => None,
    };
    Ok(RolloutMeta {
        subagent: is_subagent(
            payload.source.as_ref(),
            payload.thread_source.as_deref(),
            parent.is_some(),
        ),
        id,
        cwd: payload.cwd.filter(|c| is_any_absolute_path(c)),
        origin: origin(payload.originator.as_deref()),
        cli_version: payload.cli_version,
        parent_thread_id: parent,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadRow {
    pub id: String,
    pub cwd: Option<String>,
    pub archived: bool,
    pub origin: CodexOrigin,
    pub subagent: bool,
}

#[derive(Deserialize)]
struct RawRow {
    id: Option<String>,
    cwd: Option<String>,
    archived: Option<i64>,
    originator: Option<String>,
    thread_source: Option<String>,
    source: Option<Value>,
}

/// A `threads` row from `state_5.sqlite`, as JSON. `archived` must be 0 or 1.
pub fn parse_thread_row(bytes: &[u8]) -> Result<ThreadRow, ParseError> {
    let raw: RawRow = whole_json(bytes, RECORD_LIMIT)?;
    let id = raw
        .id
        .as_deref()
        .and_then(ids::normalize_codex_thread_id)
        .ok_or(ParseError::InvalidId("id"))?;
    let archived = match raw.archived {
        Some(0) => false,
        Some(1) => true,
        Some(_) => return Err(ParseError::InvalidValue("archived")),
        None => return Err(ParseError::MissingField("archived")),
    };
    // In the row, `source` may be a JSON string holding the subagent object.
    let source = match raw.source {
        Some(Value::String(text)) => serde_json::from_str(&text).ok(),
        other => other,
    };
    Ok(ThreadRow {
        subagent: is_subagent(source.as_ref(), raw.thread_source.as_deref(), false),
        id,
        cwd: raw.cwd.filter(|c| is_any_absolute_path(c)),
        archived,
        origin: origin(raw.originator.as_deref()),
    })
}

/// The thread id is the last 36 characters of the rollout file stem.
pub fn thread_id_from_rollout_name(name: &str) -> Option<String> {
    let stem = name.strip_prefix("rollout-")?.strip_suffix(".jsonl")?;
    let start = stem.len().checked_sub(36)?;
    let id = stem.get(start..)?;
    let separator = stem.get(..start)?;
    if !separator.ends_with('-') {
        return None;
    }
    ids::normalize_codex_thread_id(id)
}
