//! Pure, OS-independent parsers for command output and provider files. Each
//! returns `None` (or an error) for anything malformed, oversized or hostile;
//! callers turn that into `Fact::Unknown`. None of them panics.

use std::collections::BTreeMap;

use serde_json::Value;
use session_navigation::parse::{ids, is_absolute_path};

use crate::system::Secret;

/// The first token of `--version` output that looks like a version:
/// `codex-cli 0.167.0` → `0.167.0`, `2.1.321 (Claude Code)` → `2.1.321`.
pub fn tool_version(bytes: &[u8]) -> Option<String> {
    if bytes.len() > 4096 {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let first = text.lines().find(|l| !l.trim().is_empty())?;
    first.split_whitespace().find_map(|token| {
        let token = token.strip_prefix('v').unwrap_or(token);
        let ok = token.len() <= 64
            && token.bytes().next().is_some_and(|b| b.is_ascii_digit())
            && token.contains('.')
            && token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'));
        ok.then(|| token.to_owned())
    })
}

/// `lsof -F…` output: `p<pid>` starts a process, `n<name>` names its file,
/// `f<fd>` is ignored. Anything else fails the parse.
pub fn lsof_names(bytes: &[u8]) -> Option<BTreeMap<u32, Vec<String>>> {
    if bytes.len() > 1024 * 1024 {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut out: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    let mut current = None;
    for line in text.lines().filter(|l| !l.is_empty()) {
        let mut chars = line.chars();
        let tag = chars.next();
        let value = chars.as_str();
        match tag {
            Some('p') => {
                let ok = !value.is_empty()
                    && value.len() <= 10
                    && value.bytes().all(|b| b.is_ascii_digit());
                let pid: u32 = if ok { value.parse().ok()? } else { return None };
                out.entry(pid).or_default();
                current = Some(pid);
            }
            Some('n') => out.get_mut(&current?)?.push(value.to_owned()),
            Some('f') => {}
            _ => return None,
        }
    }
    Some(out)
}

/// The process cwd from `lsof -a -d cwd -Fpn`: exactly one absolute name.
pub fn lsof_cwds(bytes: &[u8]) -> Option<BTreeMap<u32, String>> {
    let names = lsof_names(bytes)?;
    Some(
        names
            .into_iter()
            .filter_map(|(pid, names)| match names.as_slice() {
                [one] if is_absolute_path(one) => Some((pid, one.clone())),
                _ => None,
            })
            .collect(),
    )
}

/// The holders from `lsof -Fp -- <file>`.
pub fn lsof_pids(bytes: &[u8]) -> Option<Vec<u32>> {
    lsof_names(bytes).map(|m| m.into_keys().collect())
}

/// `bytes` without leading and trailing ASCII whitespace.
pub fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(start, |e| e + 1);
    bytes.get(start..end).unwrap_or_default()
}

/// `sqlite3 -json` output: nothing for zero rows, else one JSON array of
/// objects.
pub fn sqlite_rows(bytes: &[u8]) -> Option<Vec<serde_json::Map<String, Value>>> {
    if bytes.len() > 16 * 1024 * 1024 {
        return None;
    }
    let trimmed = trim_ascii(bytes);
    if trimmed.is_empty() {
        return Some(Vec::new());
    }
    let rows: Vec<Value> = serde_json::from_slice(trimmed).ok()?;
    rows.into_iter()
        .map(|row| match row {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .collect()
}

/// curl stdout written with `-w '\n%{http_code}'`: (status, body).
pub fn curl_response(bytes: &[u8]) -> Option<(u16, &[u8])> {
    let split = bytes.iter().rposition(|b| *b == b'\n')?;
    let code = std::str::from_utf8(bytes.get(split + 1..)?).ok()?.trim();
    if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let status: u16 = code.parse().ok()?;
    (100..=599)
        .contains(&status)
        .then(|| (status, bytes.get(..split).unwrap_or_default()))
}

/// The service password from OpenCode's `service.json`, straight into a
/// [`Secret`]. Control characters and quotes-only oddities are refused.
pub fn service_password(bytes: &[u8]) -> Option<Secret> {
    if bytes.len() > 64 * 1024 {
        return None;
    }
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let password = value.get("password")?.as_str()?;
    let ok =
        !password.is_empty() && password.len() <= 1024 && !password.chars().any(char::is_control);
    ok.then(|| Secret::new(password.as_bytes().to_vec()))
}

/// One `state.value` of OpenCode's `drafts.sqlite` for key `tabs`: a JSON
/// array, possibly JSON-encoded once more as a string. Session ids only.
pub fn desktop_tabs(value: &str) -> Option<Vec<String>> {
    let parsed: Value = serde_json::from_str(value).ok()?;
    let parsed = match parsed {
        Value::String(inner) => serde_json::from_str(&inner).ok()?,
        other => other,
    };
    let Value::Array(tabs) = parsed else {
        return None;
    };
    let mut out = Vec::new();
    for tab in tabs {
        if tab.get("type").and_then(Value::as_str) != Some("session") {
            continue;
        }
        let id = tab.get("sessionId").and_then(Value::as_str)?;
        if !ids::is_opencode_session_id(id) {
            return None;
        }
        out.push(id.to_owned());
    }
    Some(out)
}

/// `<pid>.json` in the Claude registry directory.
pub fn registry_file_pid(name: &str) -> Option<u32> {
    let stem = name.strip_suffix(".json")?;
    let ok = !stem.is_empty() && stem.len() <= 10 && stem.bytes().all(|b| b.is_ascii_digit());
    if !ok {
        return None;
    }
    stem.parse().ok()
}

/// The first transcript line (within a bounded prefix) that is a record of
/// `id`, preferring one that names its entrypoint (queue records do not).
pub fn transcript_head(
    prefix: &[u8],
    id: &str,
) -> Option<session_navigation::parse::claude::TranscriptLine> {
    let mut first = None;
    for line in prefix.split(|b| *b == b'\n') {
        let Ok(parsed) = session_navigation::parse::claude::parse_transcript_line(line) else {
            continue;
        };
        if !parsed.session_id.eq_ignore_ascii_case(id) {
            continue;
        }
        if parsed.entrypoint.is_some() {
            return Some(parsed);
        }
        first.get_or_insert(parsed);
    }
    first
}

/// The version in a Grok executable's canonical file name,
/// `grok-<X.Y.Z>-<os>-<arch>` (optionally `.exe`): relay reads it from there
/// and never runs Grok. Anything else is `None`.
pub fn grok_version_from_name(name: &str) -> Option<String> {
    if name.len() > 128 {
        return None;
    }
    let name = name.strip_suffix(".exe").unwrap_or(name);
    let mut parts = name.strip_prefix("grok-")?.split('-');
    let version = parts.next()?;
    let os = parts.next()?;
    let arch = parts.next()?;
    let word = |w: &str| !w.is_empty() && w.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    let numbers: Vec<&str> = version.split('.').collect();
    let version_ok = numbers.len() >= 2
        && numbers
            .iter()
            .all(|n| !n.is_empty() && n.len() <= 9 && n.bytes().all(|b| b.is_ascii_digit()));
    (version_ok && word(os) && word(arch) && parts.next().is_none()).then(|| version.to_owned())
}
