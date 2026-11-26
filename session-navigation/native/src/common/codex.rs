//! Codex evidence: the `threads` row (or the rollout file without a
//! database), writer-lock holders (Codex facts A, C, D).
use session_navigation::evidence::{CodexEvidence, Fact, ProviderEvidence, ThreadInfo};
use session_navigation::parse::{codex, ids};

use super::{probe, Found};
use crate::common::config::CommonConfig;
use crate::common::parse;
use crate::system::{join, FsError, Run, System};

const ROLLOUT_PREFIX: usize = 1024 * 1024;
/// Bound on the rollout directory walk.
const MAX_DIRS: usize = 20_000;

/// The id as it may appear in SQL: a normalized UUIDv7 made of `[0-9a-f-]`
/// only. Anything else never reaches a query.
pub fn query_id(id: &str) -> Option<String> {
    ids::normalize_codex_thread_id(id).filter(|id| {
        id.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-')
    })
}

pub fn thread_query(id: &str) -> String {
    format!(
        "SELECT id, cwd, archived, originator, thread_source, source FROM threads WHERE id = '{id}';"
    )
}

pub fn thread_run(cfg: &CommonConfig, id: &str) -> Run {
    Run::with_args(
        &cfg.programs.sqlite3,
        vec![
            "-readonly".into(),
            "-json".into(),
            "-init".into(),
            "/dev/null".into(),
            cfg.paths.codex_state_db(),
            thread_query(id),
        ],
    )
    .timeout(cfg.timeouts.probe)
}

fn from_db<S: System>(
    sys: &S,
    cfg: &CommonConfig,
    id: &str,
) -> (Fact<Option<ThreadInfo>>, Option<String>) {
    let Some(output) = probe(sys, &thread_run(cfg, id)) else {
        return (Fact::Unknown, None);
    };
    let Some(rows) = parse::sqlite_rows(&output.stdout) else {
        return (Fact::Unknown, None);
    };
    match rows.as_slice() {
        [] => (Fact::Known(None), None),
        [row] => {
            let bytes = serde_json::Value::Object(row.clone()).to_string();
            match codex::parse_thread_row(bytes.as_bytes()) {
                Ok(r) if r.id == id => (
                    Fact::Known(Some(ThreadInfo {
                        archived: r.archived,
                        origin: r.origin,
                        subagent: r.subagent,
                    })),
                    r.cwd,
                ),
                _ => (Fact::Unknown, None),
            }
        }
        _ => (Fact::Unknown, None),
    }
}

/// Rollout files named for `id` under `sessions/YYYY/MM/DD/` (live) and the
/// flat `archived_sessions/` (archived, Codex facts 6).
fn rollouts<S: System>(sys: &S, cfg: &CommonConfig, id: &str) -> Option<Vec<(String, bool)>> {
    let named = |name: &str| codex::thread_id_from_rollout_name(name).as_deref() == Some(id);
    let mut found = Vec::new();
    let mut dirs = vec![(join(&cfg.paths.codex_home, "sessions"), 0usize)];
    let mut visited = 0;
    while let Some((dir, depth)) = dirs.pop() {
        visited += 1;
        if visited > MAX_DIRS {
            return None;
        }
        let names = match sys.list_dir(&dir) {
            Err(FsError::NotFound) if depth == 0 => continue,
            Err(_) => return None,
            Ok(names) => names,
        };
        for name in names {
            if depth < 3 {
                if name.bytes().all(|b| b.is_ascii_digit()) {
                    dirs.push((join(&dir, &name), depth + 1));
                }
            } else if named(&name) {
                found.push((join(&dir, &name), false));
            }
        }
    }
    let archived = join(&cfg.paths.codex_home, "archived_sessions");
    match sys.list_dir(&archived) {
        Err(FsError::NotFound) => {}
        Err(_) => return None,
        Ok(names) => found.extend(
            names
                .iter()
                .filter(|n| named(n))
                .map(|n| (join(&archived, n), true)),
        ),
    }
    Some(found)
}

/// Without `state_5.sqlite`: exactly one rollout file decides; its
/// directory says whether it is archived.
fn from_rollout<S: System>(
    sys: &S,
    cfg: &CommonConfig,
    id: &str,
) -> (Fact<Option<ThreadInfo>>, Option<String>) {
    let Some(found) = rollouts(sys, cfg, id) else {
        return (Fact::Unknown, None);
    };
    match found.as_slice() {
        [] => (Fact::Known(None), None),
        [(path, archived)] => {
            let meta = sys
                .read_prefix(path, ROLLOUT_PREFIX)
                .ok()
                .and_then(|b| codex::parse_rollout_first(&b).ok());
            match meta {
                Some(m) if m.id == id => (
                    Fact::Known(Some(ThreadInfo {
                        archived: *archived,
                        origin: m.origin,
                        subagent: m.subagent,
                    })),
                    m.cwd,
                ),
                _ => (Fact::Unknown, None),
            }
        }
        _ => (Fact::Unknown, None),
    }
}

pub fn lock_path(cfg: &CommonConfig, id: &str) -> String {
    join(&cfg.paths.codex_locks_dir(), &format!("{id}.lock"))
}

pub fn lock_run(cfg: &CommonConfig, id: &str) -> Run {
    Run::with_args(
        &cfg.programs.lsof,
        vec!["-Fp".into(), "--".into(), lock_path(cfg, id)],
    )
    .timeout(cfg.timeouts.probe)
}

/// Pids holding the writer lock. A missing lock file is `Known(empty)` only
/// when the lock directory itself is readable; lsof's exit 1 with no output
/// is `Known(empty)` only for a lock file that exists.
pub fn lock_holders<S: System>(sys: &S, cfg: &CommonConfig, id: &str) -> Fact<Vec<u32>> {
    match sys.metadata(&lock_path(cfg, id)) {
        Ok(meta) if meta.is_file => {}
        Ok(_) => return Fact::Unknown,
        Err(FsError::NotFound) => {
            return match sys.list_dir(&cfg.paths.codex_locks_dir()) {
                Ok(_) => Fact::Known(Vec::new()),
                Err(_) => Fact::Unknown,
            }
        }
        Err(_) => return Fact::Unknown,
    }
    let Ok(output) = sys.run(&lock_run(cfg, id)) else {
        return Fact::Unknown;
    };
    match output.status {
        Some(0) => parse::lsof_pids(&output.stdout).map_or(Fact::Unknown, Fact::Known),
        Some(1) if output.stdout.iter().all(u8::is_ascii_whitespace) => Fact::Known(Vec::new()),
        _ => Fact::Unknown,
    }
}

/// Evidence for an id that cannot be looked up.
pub fn unknown() -> ProviderEvidence {
    ProviderEvidence::Codex(CodexEvidence {
        thread: Fact::Unknown,
        lock_holders: Fact::Unknown,
        window_owner: Fact::Unknown,
    })
}

pub fn observe<S: System>(sys: &S, cfg: &CommonConfig, id: &str) -> Found {
    let Some(id) = query_id(id) else {
        return Found::unknown(unknown());
    };
    let (thread, cwd) = match sys.metadata(&cfg.paths.codex_state_db()) {
        Ok(meta) if meta.is_file => from_db(sys, cfg, &id),
        Err(FsError::NotFound) => from_rollout(sys, cfg, &id),
        _ => (Fact::Unknown, None),
    };
    let found = thread.known().is_some_and(Option::is_some);
    Found {
        evidence: ProviderEvidence::Codex(CodexEvidence {
            thread,
            lock_holders: lock_holders(sys, cfg, &id),
            // IPC `thread-owner-discovery` is not implemented: its protocol
            // version is unverified for this build (Codex facts 13).
            window_owner: Fact::Unknown,
        }),
        cwd,
        found,
    }
}
