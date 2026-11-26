//! OpenCode `service.json`, `/api/info`, session lookups, `session_v2` rows and
//! the TUI tab list (OpenCode facts A, B, D, F). The service password is never
//! read into a value.
use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use super::{ids, is_any_absolute_path, whole_json, ParseError};
use crate::evidence::OpenCodeSession;

const LIMIT: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceState {
    pub version: String,
    pub url: String,
    pub pid: u32,
}

#[derive(Deserialize)]
struct RawService {
    version: Option<String>,
    url: Option<String>,
    pid: Option<u64>,
}

fn pid(value: Option<u64>) -> Result<u32, ParseError> {
    value
        .and_then(|p| u32::try_from(p).ok())
        .filter(|p| *p > 1)
        .ok_or(ParseError::InvalidValue("pid"))
}

/// `$XDG_STATE_HOME/opencode/service.json`. Only loopback URLs are accepted.
pub fn parse_service_state(bytes: &[u8]) -> Result<ServiceState, ParseError> {
    let raw: RawService = whole_json(bytes, LIMIT)?;
    let url = raw.url.ok_or(ParseError::MissingField("url"))?;
    let port = url
        .strip_prefix("http://127.0.0.1:")
        .ok_or(ParseError::InvalidValue("url"))?;
    let port_ok = !port.is_empty()
        && port.len() <= 5
        && port.bytes().all(|b| b.is_ascii_digit())
        && port.parse::<u32>().is_ok_and(|p| (1..=65535).contains(&p));
    if !port_ok {
        return Err(ParseError::InvalidValue("url port"));
    }
    Ok(ServiceState {
        version: raw.version.ok_or(ParseError::MissingField("version"))?,
        url,
        pid: pid(raw.pid)?,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiInfo {
    pub version: String,
    pub pid: u32,
}

#[derive(Deserialize)]
struct RawInfo {
    version: Option<String>,
    pid: Option<u64>,
}

/// `GET /api/info`. Discovery requires a string version and an integer pid.
pub fn parse_api_info(bytes: &[u8]) -> Result<ApiInfo, ParseError> {
    let raw: RawInfo = whole_json(bytes, LIMIT)?;
    Ok(ApiInfo {
        version: raw.version.ok_or(ParseError::MissingField("version"))?,
        pid: pid(raw.pid)?,
    })
}

#[derive(Deserialize)]
struct RawTime {
    archived: Option<Value>,
}

#[derive(Deserialize)]
struct RawLocation {
    directory: Option<String>,
}

#[derive(Deserialize)]
struct RawSession {
    id: Option<String>,
    #[serde(rename = "parentID")]
    parent_id: Option<Value>,
    time: Option<RawTime>,
    location: Option<RawLocation>,
}

fn present(value: Option<&Value>) -> bool {
    !matches!(value, None | Some(Value::Null))
}

fn session_info(raw: RawSession, expected: &str) -> Result<OpenCodeSession, ParseError> {
    if raw.id.as_deref() != Some(expected) {
        return Err(ParseError::InvalidId("id"));
    }
    let directory = raw
        .location
        .and_then(|l| l.directory)
        .filter(|d| is_any_absolute_path(d))
        .ok_or(ParseError::MissingField("location.directory"))?;
    Ok(OpenCodeSession {
        archived: present(raw.time.as_ref().and_then(|t| t.archived.as_ref())),
        child: present(raw.parent_id.as_ref()),
        directory,
    })
}

#[derive(Deserialize)]
struct RawNotFound {
    #[serde(rename = "_tag")]
    tag: Option<String>,
    #[serde(rename = "sessionID")]
    session_id: Option<String>,
}

/// `GET /api/session/<id>`. `Ok(None)` only for a 404 whose body is exactly the
/// `SessionNotFoundError` for this id; any other failure is an error, so
/// absence is never inferred from a timeout or a 500.
pub fn parse_session_lookup(
    status: u16,
    bytes: &[u8],
    expected: &str,
) -> Result<Option<OpenCodeSession>, ParseError> {
    if !ids::is_opencode_session_id(expected) {
        return Err(ParseError::InvalidId("expected"));
    }
    match status {
        200 => {
            let value: Value = whole_json(bytes, LIMIT)?;
            let inner = match value {
                Value::Object(mut map) if map.contains_key("data") => {
                    map.remove("data").unwrap_or(Value::Null)
                }
                other => other,
            };
            let raw: RawSession = serde_json::from_value(inner).map_err(|_| ParseError::Json)?;
            session_info(raw, expected).map(Some)
        }
        404 => {
            let raw: RawNotFound = whole_json(bytes, LIMIT)?;
            let exact = raw.tag.as_deref() == Some("SessionNotFoundError")
                && raw.session_id.as_deref() == Some(expected);
            if exact {
                Ok(None)
            } else {
                Err(ParseError::InvalidValue("404 body"))
            }
        }
        _ => Err(ParseError::InvalidValue("status")),
    }
}

#[derive(Deserialize)]
struct RawRow {
    id: Option<String>,
    parent_id: Option<Value>,
    directory: Option<String>,
    time_archived: Option<Value>,
}

/// A `session_v2` row as JSON (read-only database guard, facts F).
pub fn parse_db_row(bytes: &[u8], expected: &str) -> Result<OpenCodeSession, ParseError> {
    let raw: RawRow = whole_json(bytes, LIMIT)?;
    if raw.id.as_deref() != Some(expected) || !ids::is_opencode_session_id(expected) {
        return Err(ParseError::InvalidId("id"));
    }
    Ok(OpenCodeSession {
        archived: present(raw.time_archived.as_ref()),
        child: present(raw.parent_id.as_ref()),
        directory: raw
            .directory
            .filter(|d| is_any_absolute_path(d))
            .ok_or(ParseError::MissingField("directory"))?,
    })
}

#[derive(Deserialize)]
struct RawActive {
    data: Option<BTreeMap<String, Value>>,
}

/// `GET /api/session/active`: ids with a running foreground drain.
pub fn parse_active(bytes: &[u8]) -> Result<Vec<String>, ParseError> {
    let raw: RawActive = whole_json(bytes, LIMIT)?;
    let data = raw.data.ok_or(ParseError::MissingField("data"))?;
    if data.keys().any(|k| !ids::is_opencode_session_id(k)) {
        return Err(ParseError::InvalidId("active"));
    }
    Ok(data.into_keys().collect())
}

#[derive(Deserialize)]
struct RawTab {
    #[serde(rename = "sessionID")]
    session_id: Option<String>,
}

#[derive(Deserialize)]
struct RawDirTabs {
    tabs: Option<Vec<RawTab>>,
}

#[derive(Deserialize)]
struct RawTuiTabs {
    cwd: Option<BTreeMap<String, RawDirTabs>>,
}

/// `~/.local/state/opencode/<channel>/tui/tabs.json`: (directory, session ids).
/// A hint that survives quit, never liveness.
pub fn parse_tui_tabs(bytes: &[u8]) -> Result<Vec<(String, Vec<String>)>, ParseError> {
    let raw: RawTuiTabs = whole_json(bytes, LIMIT)?;
    let mut out = Vec::new();
    for (dir, tabs) in raw.cwd.unwrap_or_default() {
        let ids_in_dir: Vec<String> = tabs
            .tabs
            .unwrap_or_default()
            .into_iter()
            .filter_map(|t| t.session_id)
            .filter(|id| ids::is_opencode_session_id(id))
            .collect();
        out.push((dir, ids_in_dir));
    }
    Ok(out)
}
