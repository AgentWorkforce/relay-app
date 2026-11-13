//! The audit (`docs/service-callers.md` section 9): one JSON line per
//! decision, with the caller id, the method, the outcome class and a count.
//! Never a secret, hash, token, name of a folder, or text.

use super::registry::Binding;
use agent_relay_node::platform::fs as private_fs;
use serde::Serialize;
use std::io::Write;

/// The audit file is rotated past this size, keeping one older file.
const ROTATE_AT: u64 = 1024 * 1024;

#[derive(Serialize)]
struct Line<'a> {
    at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    caller_id: Option<&'a str>,
    method: &'a str,
    outcome: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    count: Option<u64>,
}

/// Appends one line. A failure to audit is logged by class and is not
/// itself an error for the call.
pub(crate) fn write(
    binding: &Binding,
    now_ms: i64,
    caller_id: Option<&str>,
    method: &str,
    outcome: &str,
    count: Option<u64>,
) -> bool {
    let line = Line {
        at: super::rfc3339(now_ms),
        caller_id,
        method,
        outcome,
        count,
    };
    let Ok(mut bytes) = serde_json::to_vec(&line) else {
        return false;
    };
    bytes.push(b'\n');
    let path = binding.audit();
    if private_fs::create_private_dir_all(&binding.folder).is_err() {
        return false;
    }
    if std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.len() > ROTATE_AT) {
        let _ = std::fs::rename(&path, path.with_extension("jsonl.1"));
    }
    let opened = private_fs::no_follow(private_fs::private(
        std::fs::OpenOptions::new().create(true).append(true),
    ))
    .open(&path);
    match opened {
        Ok(mut file) => file.write_all(&bytes).is_ok(),
        Err(_) => false,
    }
}
