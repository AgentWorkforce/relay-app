//! Claude desktop session records, the per-pid registry, the archived index
//! and transcript lines (Claude facts A, C).
use serde::Deserialize;

use super::{first_json, ids, is_any_absolute_path, lstart, whole_json, ParseError};
use crate::evidence::{ClaimKind, Entrypoint, Fact};

/// Registry files are ~1 KB.
pub const REGISTRY_LIMIT: usize = 64 * 1024;
/// Desktop records are ~480 KB, mostly prompt snapshots.
pub const DESKTOP_LIMIT: usize = 8 * 1024 * 1024;
const LINE_LIMIT: usize = 4 * 1024 * 1024;

#[derive(Deserialize)]
struct RawRegistry {
    pid: Option<u64>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "procStart")]
    proc_start: Option<String>,
    entrypoint: Option<String>,
    #[serde(rename = "hostSessionId")]
    host_session_id: Option<String>,
    spare: Option<bool>,
    kind: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryRecord {
    pub pid: u32,
    pub session_id: String,
    pub cwd: Option<String>,
    pub proc_start: u64,
    pub entrypoint: Entrypoint,
    pub host_session_id: Option<String>,
    pub spare: bool,
    pub background: bool,
}

pub fn entrypoint(value: Option<&str>) -> Entrypoint {
    match value {
        Some("cli") => Entrypoint::Cli,
        Some("claude-desktop") => Entrypoint::Desktop,
        Some("sdk-cli") | Some("sdk-ts") | Some("sdk-py") => Entrypoint::Sdk,
        _ => Entrypoint::Other,
    }
}

/// `<config>/sessions/<pid>.json`. Only the first JSON value counts.
pub fn parse_registry(bytes: &[u8]) -> Result<RegistryRecord, ParseError> {
    let raw: RawRegistry = first_json(bytes, REGISTRY_LIMIT)?;
    let pid = raw.pid.ok_or(ParseError::MissingField("pid"))?;
    let pid = u32::try_from(pid)
        .ok()
        .filter(|p| *p > 1)
        .ok_or(ParseError::InvalidValue("pid"))?;
    let session_id = raw
        .session_id
        .ok_or(ParseError::MissingField("sessionId"))?;
    if !ids::is_claude_cli_id(&session_id) {
        return Err(ParseError::InvalidId("sessionId"));
    }
    let proc_start = lstart::parse(
        raw.proc_start
            .as_deref()
            .ok_or(ParseError::MissingField("procStart"))?,
    )?;
    if let Some(host) = &raw.host_session_id {
        if !ids::is_claude_local_id(host) {
            return Err(ParseError::InvalidId("hostSessionId"));
        }
    }
    let cwd = raw.cwd.filter(|c| is_any_absolute_path(c));
    Ok(RegistryRecord {
        pid,
        session_id,
        cwd,
        proc_start,
        entrypoint: entrypoint(raw.entrypoint.as_deref()),
        host_session_id: raw.host_session_id,
        spare: raw.spare.unwrap_or(false),
        background: raw.kind.as_deref() == Some("bg"),
    })
}

#[derive(Deserialize)]
struct RawDesktop {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    #[serde(rename = "cliSessionId")]
    cli_session_id: Option<String>,
    #[serde(rename = "unarchivedCliSessionId")]
    unarchived: Option<String>,
    #[serde(rename = "preClearCliSessionId")]
    pre_clear: Option<String>,
    #[serde(rename = "priorCliSessionIds")]
    prior: Option<Vec<String>>,
    #[serde(rename = "overflowCliSessionIds")]
    overflow: Option<Vec<String>>,
    #[serde(rename = "isArchived")]
    is_archived: Option<bool>,
    cwd: Option<String>,
    /// Absent → `Unknown`; `null` → never focused; a number → epoch ms.
    #[serde(rename = "lastFocusedAt", default, deserialize_with = "present")]
    last_focused_at: Option<Option<serde_json::Value>>,
}

/// Distinguish a present `null` from an absent field.
fn present<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<serde_json::Value>>, D::Error> {
    Option::<serde_json::Value>::deserialize(deserializer).map(Some)
}

/// The identity, archive and alias fields of one desktop session record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopRecord {
    pub session_id: String,
    pub current: Option<String>,
    pub unarchived: Option<String>,
    pub pre_clear: Option<String>,
    pub prior: Vec<String>,
    pub overflow: Vec<String>,
    pub archived: bool,
    pub cwd: Option<String>,
    /// `lastFocusedAt`: the success signal of a focusing link (native N1).
    pub last_focused: Fact<Option<u64>>,
}

/// `lastFocusedAt` is epoch milliseconds or `null`; anything else (a string,
/// a negative or fractional number) is not a timestamp and stays unknown.
fn last_focused(value: Option<Option<serde_json::Value>>) -> Fact<Option<u64>> {
    match value {
        None => Fact::Unknown,
        Some(None) | Some(Some(serde_json::Value::Null)) => Fact::Known(None),
        Some(Some(value)) => value
            .as_u64()
            .map_or(Fact::Unknown, |ms| Fact::Known(Some(ms))),
    }
}

fn optional_cli(value: Option<String>, field: &'static str) -> Result<Option<String>, ParseError> {
    match value {
        Some(id) if !ids::is_claude_cli_id(&id) => Err(ParseError::InvalidId(field)),
        other => Ok(other),
    }
}

fn cli_list(value: Option<Vec<String>>, field: &'static str) -> Result<Vec<String>, ParseError> {
    let list = value.unwrap_or_default();
    if list.len() > 5000 || list.iter().any(|id| !ids::is_claude_cli_id(id)) {
        return Err(ParseError::InvalidId(field));
    }
    Ok(list)
}

/// `claude-code-sessions/<account>/<org>/local_<uuid>.json`. `isArchived` is
/// the source of truth, so a record without it is rejected, not defaulted.
pub fn parse_desktop_record(bytes: &[u8]) -> Result<DesktopRecord, ParseError> {
    let raw: RawDesktop = whole_json(bytes, DESKTOP_LIMIT)?;
    let session_id = raw
        .session_id
        .ok_or(ParseError::MissingField("sessionId"))?;
    if !ids::is_claude_local_id(&session_id) {
        return Err(ParseError::InvalidId("sessionId"));
    }
    Ok(DesktopRecord {
        session_id,
        current: optional_cli(raw.cli_session_id, "cliSessionId")?,
        unarchived: optional_cli(raw.unarchived, "unarchivedCliSessionId")?,
        pre_clear: optional_cli(raw.pre_clear, "preClearCliSessionId")?,
        prior: cli_list(raw.prior, "priorCliSessionIds")?,
        overflow: cli_list(raw.overflow, "overflowCliSessionIds")?,
        archived: raw
            .is_archived
            .ok_or(ParseError::MissingField("isArchived"))?,
        cwd: raw.cwd.filter(|c| is_any_absolute_path(c)),
        last_focused: last_focused(raw.last_focused_at),
    })
}

impl DesktopRecord {
    /// How this record claims `cli_id`, strongest first (`cliHandles` and
    /// `cliSessionIdsClaimedBy`, facts A).
    /// Ids compare case-insensitively.
    ///
    /// Only the *current* conversation, `cliSessionId ?? unarchivedCliSessionId`
    /// (facts A, resume order at asar@16099366), is `Live`. An
    /// `unarchivedCliSessionId` next to a different `cliSessionId` is a stale
    /// snapshot of an earlier conversation and counts as `Prior`.
    pub fn claim_for(&self, cli_id: &str) -> Option<ClaimKind> {
        let is = |v: &Option<String>| v.as_deref().is_some_and(|v| v.eq_ignore_ascii_case(cli_id));
        let current = self.current.as_ref().or(self.unarchived.as_ref());
        if current.is_some_and(|c| c.eq_ignore_ascii_case(cli_id)) {
            return Some(ClaimKind::Live);
        }
        let stem = self.session_id.strip_prefix("local_");
        if stem.is_some_and(|s| s.eq_ignore_ascii_case(cli_id)) {
            return Some(ClaimKind::Stem);
        }
        let in_list = |list: &[String]| list.iter().any(|id| id.eq_ignore_ascii_case(cli_id));
        if is(&self.unarchived)
            || is(&self.pre_clear)
            || in_list(&self.prior)
            || in_list(&self.overflow)
        {
            return Some(ClaimKind::Prior);
        }
        None
    }
}

#[derive(Deserialize)]
struct RawArchivedIndex {
    v: Option<u64>,
    archived: Option<Vec<String>>,
}

/// `archived-sessions.idx`: a hint only; the record's `isArchived` wins.
pub fn parse_archived_index(bytes: &[u8]) -> Result<Vec<String>, ParseError> {
    let raw: RawArchivedIndex = whole_json(bytes, DESKTOP_LIMIT)?;
    if raw.v != Some(1) {
        return Err(ParseError::InvalidValue("v"));
    }
    let list = raw.archived.ok_or(ParseError::MissingField("archived"))?;
    if list.iter().any(|id| !ids::is_claude_local_id(id)) {
        return Err(ParseError::InvalidId("archived"));
    }
    Ok(list)
}

#[derive(Deserialize)]
struct RawLine {
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    cwd: Option<String>,
    entrypoint: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptLine {
    pub session_id: String,
    pub cwd: Option<String>,
    pub entrypoint: Option<Entrypoint>,
}

/// One line of `<config>/projects/<slug>/<id>.jsonl`. Queue records carry no
/// `entrypoint`, so it is optional here.
pub fn parse_transcript_line(bytes: &[u8]) -> Result<TranscriptLine, ParseError> {
    let raw: RawLine = whole_json(bytes, LINE_LIMIT)?;
    let session_id = raw
        .session_id
        .ok_or(ParseError::MissingField("sessionId"))?;
    if !ids::is_claude_cli_id(&session_id) {
        return Err(ParseError::InvalidId("sessionId"));
    }
    Ok(TranscriptLine {
        session_id,
        cwd: raw.cwd.filter(|c| is_any_absolute_path(c)),
        entrypoint: raw.entrypoint.as_deref().map(|e| entrypoint(Some(e))),
    })
}
