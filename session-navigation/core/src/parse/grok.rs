//! Grok's `active_sessions.json` and `summary.json` (facts-grok 2, 3; relay
//! `grok.rs` `decode_active_sessions`, relayhistory `discover.rs`).
use serde_json::Value;

use super::lstart::{days_from_civil, days_in_month};
use super::{ids, is_any_absolute_path, whole_json, ParseError};
use crate::evidence::{GrokActive, GrokSession};

const LIMIT: usize = 1024 * 1024;

fn digits(text: &str, min: i64, max: i64) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<i64>().ok().filter(|v| (min..=max).contains(v))
}

/// RFC 3339 (`2026-11-11T10:00:00.123Z`, `…+01:00`) to epoch seconds,
/// fraction dropped. Anything else is `None`.
pub fn rfc3339(text: &str) -> Option<u64> {
    let (date, rest) = text.split_once(['T', 't', ' '])?;
    let mut d = date.split('-');
    let year = digits(d.next()?, 1970, 9999)?;
    let month = digits(d.next()?, 1, 12)?;
    let day = digits(d.next()?, 1, 31)?;
    if d.next().is_some() || day > days_in_month(year, month) {
        return None;
    }
    let at = rest.find(['Z', 'z', '+', '-'])?;
    let (time, offset) = (rest.get(..at)?, rest.get(at..)?);
    let time = time.split('.').next()?;
    let mut t = time.split(':');
    let hour = digits(t.next()?, 0, 23)?;
    let minute = digits(t.next()?, 0, 59)?;
    let second = digits(t.next()?, 0, 60)?;
    if t.next().is_some() {
        return None;
    }
    let shift = match offset {
        "Z" | "z" => 0,
        _ => {
            let sign = if offset.starts_with('-') { -1 } else { 1 };
            let (h, m) = offset.get(1..)?.split_once(':')?;
            sign * (digits(h, 0, 23)? * 3600 + digits(m, 0, 59)? * 60)
        }
    };
    let secs =
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - shift;
    u64::try_from(secs).ok()
}

/// `active_sessions.json`: a list of `{session_id, pid, cwd, opened_at}`.
/// A file that is not a list is an error (never "no live sessions"); a
/// record without a UUID id or a positive pid is skipped, as relay does: it
/// cannot name a valid session.
pub fn parse_active_sessions(bytes: &[u8]) -> Result<Vec<GrokActive>, ParseError> {
    let Value::Array(records) = whole_json::<Value>(bytes, LIMIT)? else {
        return Err(ParseError::InvalidValue("active_sessions is not a list"));
    };
    Ok(records
        .iter()
        .filter_map(|record| {
            let session_id = record
                .get("session_id")?
                .as_str()
                .filter(|id| ids::is_uuid(id))?
                .to_ascii_lowercase();
            let pid = record
                .get("pid")?
                .as_u64()
                .and_then(|p| u32::try_from(p).ok())
                .filter(|p| *p > 0)?;
            let cwd = record
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|c| is_any_absolute_path(c))
                .map(str::to_owned);
            let opened_at = record
                .get("opened_at")
                .and_then(Value::as_str)
                .and_then(rfc3339);
            Some(GrokActive {
                session_id,
                pid,
                cwd,
                opened_at,
            })
        })
        .collect())
}

/// `summary.json` of session `id`: `info.id` (when present) must be `id`,
/// `info.cwd` must be absolute; `session_kind == "subagent"` marks a child.
pub fn parse_summary(bytes: &[u8], id: &str) -> Result<GrokSession, ParseError> {
    let value: Value = whole_json(bytes, LIMIT)?;
    let info = value.get("info").ok_or(ParseError::MissingField("info"))?;
    if let Some(own) = info.get("id").and_then(Value::as_str) {
        if !own.eq_ignore_ascii_case(id) {
            return Err(ParseError::InvalidId("info.id"));
        }
    }
    let cwd = info
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|c| is_any_absolute_path(c))
        .ok_or(ParseError::MissingField("info.cwd"))?;
    let subagent = value.get("session_kind").and_then(Value::as_str) == Some("subagent");
    Ok(GrokSession {
        cwd: cwd.to_owned(),
        subagent,
    })
}

/// Grok's encoding of a working directory as one path component (relay
/// `grok.rs` `encode_component`): every byte outside `A-Za-z0-9-_.!~*'()`
/// becomes `%XX`.
pub fn encode_cwd(cwd: &str) -> String {
    let mut out = String::with_capacity(cwd.len() * 3);
    for byte in cwd.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
