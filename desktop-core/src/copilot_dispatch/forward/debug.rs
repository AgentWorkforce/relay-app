//! Opt-in capture of hosted MCP exchanges, for diagnosing a refused call.
//! Off unless `AGENT_RELAY_MCP_DEBUG=1` is set on the process doing the
//! exchange; then each one is appended as a JSON line to
//! `$AGENT_RELAY_MCP_DEBUG_FILE`, else `mcp-debug.log` in the app's data
//! directory. The record holds the full request body, a chat's proof
//! included, but never the Authorization header. Nothing else changes.
use serde_json::{json, Map, Value};
use std::io::Write;
use std::path::PathBuf;

pub(crate) const VARIABLE: &str = "AGENT_RELAY_MCP_DEBUG";
pub(crate) const FILE_VARIABLE: &str = "AGENT_RELAY_MCP_DEBUG_FILE";

/// Where captures go, when capture is on.
pub(crate) fn file() -> Option<PathBuf> {
    file_from(&|key| std::env::var(key).ok())
}

fn file_from(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if env(VARIABLE).as_deref().map(str::trim) != Some("1") {
        return None;
    }
    Some(
        env(FILE_VARIABLE)
            .filter(|path| !path.trim().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| crate::history::retention::app_data_dir().join("mcp-debug.log")),
    )
}

/// `headers` as a JSON object, without Authorization.
pub(super) fn redacted(headers: &[(&str, String)]) -> Value {
    let kept: Map<String, Value> = headers
        .iter()
        .filter(|(name, _)| !name.eq_ignore_ascii_case("authorization"))
        .map(|(name, value)| ((*name).to_owned(), json!(value)))
        .collect();
    Value::Object(kept)
}

/// Appends one record; a capture that cannot be written is dropped.
pub(crate) fn append(path: &std::path::Path, record: Value) {
    let line = json!({"at": chrono::Utc::now().to_rfc3339(), "exchange": record});
    let opened = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path);
    if let Ok(mut out) = opened {
        let _ = writeln!(out, "{line}");
    }
}

/// One forwarded POST and what came back.
pub(super) fn record(
    endpoint: &str,
    headers: &[(&str, String)],
    body: &Value,
    status: u16,
    response: &[u8],
) -> Value {
    json!({
        "method": "POST",
        "url": endpoint,
        "headers": redacted(headers),
        "body": body,
        "status": status,
        "response": String::from_utf8_lossy(response),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_keeps_the_proof_but_never_the_authorization() {
        let headers = [
            ("Authorization", "Bearer secret-grant".to_owned()),
            ("Accept", "application/json".to_owned()),
            ("MCP-Protocol-Version", "2025-06-18".to_owned()),
        ];
        let body = json!({"params": {"arguments": {"_agent_relay_attestation": "a.b.c"}}});
        let record = record("https://site/mcp", &headers, &body, 400, b"{\"error\":1}");
        let text = record.to_string();
        assert!(!text.contains("secret-grant") && !text.contains("Authorization"));
        assert_eq!(record["headers"]["Accept"], "application/json");
        assert_eq!(record["body"], body, "the proof is kept");
        assert_eq!(record["status"], 400);
        assert_eq!(record["response"], "{\"error\":1}");
        assert_eq!(redacted(&[("authorization", "x".into())]), json!({}));
    }

    #[test]
    fn capture_is_off_unless_asked_and_goes_to_the_named_file() {
        let env = |pairs: &'static [(&str, &str)]| {
            move |key: &str| {
                pairs
                    .iter()
                    .find(|(name, _)| *name == key)
                    .map(|(_, value)| (*value).to_owned())
            }
        };
        assert_eq!(file_from(&env(&[])), None);
        assert_eq!(file_from(&env(&[(VARIABLE, "0")])), None);
        assert_eq!(
            file_from(&env(&[(VARIABLE, "1"), (FILE_VARIABLE, "/tmp/x.log")])),
            Some(PathBuf::from("/tmp/x.log"))
        );
        let default = file_from(&env(&[(VARIABLE, "1")])).unwrap();
        assert!(default.ends_with("mcp-debug.log"));
    }

    #[test]
    fn a_record_is_appended_as_one_json_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mcp-debug.log");
        append(&path, json!({"n": 1}));
        append(&path, json!({"n": 2}));
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1]["exchange"]["n"], 2);
    }
}
