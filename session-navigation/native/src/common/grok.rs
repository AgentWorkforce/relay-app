//! Grok evidence: `<grok home>/active_sessions.json` (every running TUI) and
//! the session's `sessions/<encoded cwd>/<id>/summary.json` (facts-grok 2,
//! 3). Grok is never run.
//!
//! The layout follows the two reference readers (relay
//! `crates/relay-node/src/relay/grok.rs`, relayhistory
//! `crates/ai-hist/src/ingest/grok.rs` and `discover.rs`):
//!
//! - the home is `GROK_HOME` (trimmed, `~` expanded), else `~/.grok`
//!   ([`home_from_env`]); a session found under a home other than the
//!   default is recorded under `GROK_HOME=<that home>` ([`environment`]), so
//!   a resume opens the same store;
//! - each session directory is `sessions/<percent-encoded cwd>/<id>/`, and
//!   the encoded name must decode to the summary's own `info.cwd`
//!   ([`decode_component`]); a disagreement is conflicting evidence
//!   (`Unknown`), never a guess;
//! - relayhistory falls back to the folder name and `git_root_dir` when
//!   `summary.json` is missing; this observer does not: a launch needs
//!   `summary.json` to confirm the id (`grok_resume_missing_creates`).
//!
//! UNVERIFIED: Grok was not installed where this was written (facts-grok);
//! every shape here comes from those readers, not from a captured store.
use std::collections::BTreeMap;

use session_navigation::evidence::{Fact, GrokActive, GrokEvidence, GrokSession, ProviderEvidence};
use session_navigation::parse::{grok, ids, is_any_absolute_path};

use super::config::ProviderPaths;
use super::Found;
use crate::system::{join, FsError, System};

/// The core parsers refuse anything larger.
const LIMIT: usize = 1024 * 1024;

/// The environment key that selects Grok's home.
pub const HOME_ENV: &str = "GROK_HOME";

/// Evidence for an id that cannot be looked up.
pub fn unknown() -> ProviderEvidence {
    ProviderEvidence::Grok(GrokEvidence {
        active: Fact::Unknown,
        session: Fact::Unknown,
    })
}

/// `<home>/.grok`: the home without `GROK_HOME`.
pub fn default_home(home: &str) -> String {
    join(home.trim_end_matches('/'), ".grok")
}

/// Grok's home from the `GROK_HOME` value (relay `GrokConfig::from_env`):
/// trimmed; empty or unset is the default; `~` and `~/…` expand to `home`.
/// Any other value is returned as is; a relative one makes every Grok fact
/// `Unknown` (it would resolve against Grok's own cwd, which is not known).
pub fn home_from_env(value: Option<&str>, home: &str) -> String {
    let home = home.trim_end_matches('/');
    match value.map(str::trim).filter(|v| !v.is_empty()) {
        None => default_home(home),
        Some("~") => home.to_owned(),
        Some(v) => match v.strip_prefix("~/") {
            Some(rest) => join(home, rest),
            None => v.to_owned(),
        },
    }
}

/// The provider-home selector a session found under `grok_home` was
/// recorded with: none for the default home, else `GROK_HOME`.
pub fn environment(grok_home: &str, default: &str) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    if grok_home.trim_end_matches('/') != default.trim_end_matches('/') {
        env.insert(HOME_ENV.to_owned(), grok_home.to_owned());
    }
    env
}

/// Undo Grok's cwd encoding (relay `encode_component`: every byte outside
/// `A-Za-z0-9-_.!~*'()` is `%XX`). `None` for a malformed escape, a byte
/// that should have been escaped but was not, or a result that is not UTF-8.
pub fn decode_component(name: &str) -> Option<String> {
    let bytes = name.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b);
            i += 1;
        } else {
            return None;
        }
    }
    String::from_utf8(out).ok()
}

fn home_ok(paths: &ProviderPaths) -> bool {
    is_any_absolute_path(&paths.grok_home)
}

/// Every record. No file is no running TUI; an unreadable or unparsable one
/// is `Unknown`, never "none".
pub fn active<S: System>(sys: &S, paths: &ProviderPaths) -> Fact<Vec<GrokActive>> {
    if !home_ok(paths) {
        return Fact::Unknown;
    }
    match sys.read_file(&paths.grok_active_sessions(), LIMIT) {
        Err(FsError::NotFound) => Fact::Known(Vec::new()),
        Err(_) => Fact::Unknown,
        Ok(bytes) => grok::parse_active_sessions(&bytes).map_or(Fact::Unknown, Fact::Known),
    }
}

/// The session's summary, found by listing the cwd directories (the
/// encoding is not recomputed, only checked). `Known(None)` only when every
/// directory was read and none holds `id`; two holders, a missing or bad
/// summary, a directory name that does not decode to `info.cwd`, or any
/// read error is `Unknown`. `id` must already be a valid Grok id.
pub fn session<S: System>(sys: &S, paths: &ProviderPaths, id: &str) -> Fact<Option<GrokSession>> {
    if !home_ok(paths) {
        return Fact::Unknown;
    }
    let root = paths.grok_sessions_dir();
    let entries = match sys.list_dir(&root) {
        Err(FsError::NotFound) => return Fact::Known(None),
        Err(_) => return Fact::Unknown,
        Ok(entries) => entries,
    };
    let mut found = None;
    for entry in entries.iter().filter(|e| !e.starts_with('.')) {
        let cwd_dir = join(&root, entry);
        match sys.metadata(&cwd_dir) {
            Ok(meta) if meta.is_dir => {}
            Ok(_) => continue,
            Err(_) => return Fact::Unknown,
        }
        let session_dir = join(&cwd_dir, id);
        match sys.metadata(&session_dir) {
            Err(FsError::NotFound) => continue,
            Err(_) => return Fact::Unknown,
            Ok(meta) if !meta.is_dir => continue,
            Ok(_) => {}
        }
        let summary = sys
            .read_file(&join(&session_dir, "summary.json"), LIMIT)
            .ok()
            .and_then(|bytes| grok::parse_summary(&bytes, id).ok())
            .filter(|s| decode_component(entry).as_deref() == Some(s.cwd.as_str()));
        match (summary, &found) {
            (Some(summary), None) => found = Some(summary),
            _ => return Fact::Unknown,
        }
    }
    Fact::Known(found)
}

pub fn observe<S: System>(sys: &S, paths: &ProviderPaths, id: &str) -> Found {
    if !ids::is_grok_session_id(id) || !home_ok(paths) {
        return Found::unknown(unknown());
    }
    // Grok writes lower-case UUIDs; ids compare in lower case.
    let id = id.to_ascii_lowercase();
    let session = session(sys, paths, &id);
    let cwd = session.known().cloned().flatten().map(|s| s.cwd);
    Found {
        evidence: ProviderEvidence::Grok(GrokEvidence {
            active: active(sys, paths),
            session,
        }),
        found: cwd.is_some(),
        cwd,
    }
}
