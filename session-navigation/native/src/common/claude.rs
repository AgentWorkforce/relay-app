//! Claude evidence: the per-pid registry, desktop records and the transcript
//! (Claude facts A, C, D).
use session_navigation::evidence::{
    ClaimKind, ClaudeEvidence, DesktopClaim, Entrypoint, Exe, Fact, ProcessInfo, ProviderEvidence,
    RegistryClaim, Transcript,
};
use session_navigation::parse::claude;

use super::Found;
use crate::common::config::CommonConfig;
use crate::common::parse;
use crate::system::{join, FsError, System};

/// How much of a transcript is read to find its first record.
const TRANSCRIPT_PREFIX: usize = 1024 * 1024;

fn live_claude(table: &Fact<Vec<ProcessInfo>>, pid: u32) -> bool {
    match table {
        Fact::Known(table) => table
            .iter()
            .any(|p| p.identity.pid == pid && p.exe == Exe::ClaudeCli),
        // Cannot tell whether the unreadable file belongs to a live process.
        Fact::Unknown => true,
    }
}

/// Every `<pid>.json`. An unreadable or unparsable file is harmless only if
/// its pid is not a live Claude process (same rule as the simulator).
pub fn registry<S: System>(
    sys: &S,
    cfg: &CommonConfig,
    boot_time: Option<u64>,
    table: &Fact<Vec<ProcessInfo>>,
) -> Fact<Vec<RegistryClaim>> {
    let dir = cfg.paths.claude_sessions_dir();
    let names = match sys.list_dir(&dir) {
        Err(FsError::NotFound) => return Fact::Known(Vec::new()),
        Err(_) => return Fact::Unknown,
        Ok(names) => names,
    };
    let mut claims = Vec::new();
    for (name, pid) in names
        .iter()
        .filter_map(|n| parse::registry_file_pid(n).map(|p| (n, p)))
    {
        let path = join(&dir, name);
        let record = sys
            .read_file(&path, claude::REGISTRY_LIMIT)
            .ok()
            .and_then(|b| claude::parse_registry(&b).ok())
            .filter(|r| r.pid == pid);
        let Some(record) = record else {
            if live_claude(table, pid) {
                return Fact::Unknown;
            }
            continue;
        };
        let written_this_boot = match (sys.metadata(&path), boot_time) {
            (Ok(meta), Some(boot)) => Fact::Known(meta.mtime >= boot),
            _ => Fact::Unknown,
        };
        claims.push(RegistryClaim {
            pid: record.pid,
            proc_start: record.proc_start,
            written_this_boot,
            session_id: record.session_id,
            host_session: record.host_session_id,
            spare: record.spare,
            entrypoint: record.entrypoint,
        });
    }
    Fact::Known(claims)
}

fn subdirs<S: System>(sys: &S, dir: &str) -> Result<Vec<String>, FsError> {
    let mut out = Vec::new();
    for name in sys.list_dir(dir)? {
        if name.starts_with('.') {
            continue;
        }
        if sys.metadata(&join(dir, &name))?.is_dir {
            out.push(name);
        }
    }
    Ok(out)
}

/// `(claims, account)`: records under `<account>/<org>/local_*.json` that
/// claim `id`, and the single `<account>/<org>` when exactly one exists. No
/// verified way to read the app's *current* account exists (Claude facts F),
/// so several directories make the account `Unknown`.
pub fn desktop<S: System>(
    sys: &S,
    cfg: &CommonConfig,
    id: &str,
) -> (Fact<Vec<DesktopClaim>>, Fact<String>) {
    let root = cfg.paths.claude_desktop_sessions_dir();
    let accounts = match subdirs(sys, &root) {
        Err(FsError::NotFound) => return (Fact::Known(Vec::new()), Fact::Unknown),
        Err(_) => return (Fact::Unknown, Fact::Unknown),
        Ok(accounts) => accounts,
    };
    let mut claims = Vec::new();
    let mut places = Vec::new();
    for account in accounts {
        let account_dir = join(&root, &account);
        let Ok(orgs) = subdirs(sys, &account_dir) else {
            return (Fact::Unknown, Fact::Unknown);
        };
        for org in orgs {
            let place = format!("{account}/{org}");
            let org_dir = join(&account_dir, &org);
            let Ok(names) = sys.list_dir(&org_dir) else {
                return (Fact::Unknown, Fact::Unknown);
            };
            for name in names
                .iter()
                .filter(|n| n.starts_with("local_") && n.ends_with(".json"))
            {
                let record = sys
                    .read_file(&join(&org_dir, name), claude::DESKTOP_LIMIT)
                    .ok()
                    .and_then(|b| claude::parse_desktop_record(&b).ok());
                let Some(record) = record else {
                    return (Fact::Unknown, Fact::Unknown);
                };
                if let Some(kind) = record.claim_for(id) {
                    claims.push(DesktopClaim {
                        current: record.current.clone().or(record.unarchived.clone()),
                        local_id: record.session_id,
                        archived: record.archived,
                        kind,
                        cwd: record.cwd,
                        account: place.clone(),
                        last_focused: record.last_focused,
                    });
                }
            }
            places.push(place);
        }
    }
    let account = match places.as_slice() {
        [one] => Fact::Known(one.clone()),
        _ => Fact::Unknown,
    };
    (Fact::Known(claims), account)
}

/// `<claude home>/projects/*/<id>.jsonl`, found by listing (the slug is
/// not recomputed, facts F), then the first record within a bounded prefix.
pub fn transcript<S: System>(sys: &S, cfg: &CommonConfig, id: &str) -> Fact<Option<Transcript>> {
    let projects = cfg.paths.claude_projects_dir();
    let dirs = match sys.list_dir(&projects) {
        Err(FsError::NotFound) => return Fact::Known(None),
        Err(_) => return Fact::Unknown,
        Ok(dirs) => dirs,
    };
    let file = format!("{id}.jsonl");
    let mut found_file = false;
    for dir in dirs.iter().filter(|d| !d.starts_with('.')) {
        let path = join(&join(&projects, dir), &file);
        match sys.metadata(&path) {
            Err(FsError::NotFound) => continue,
            Err(_) => return Fact::Unknown,
            Ok(meta) if !meta.is_file => continue,
            Ok(_) => {}
        }
        found_file = true;
        let head = sys
            .read_prefix(&path, TRANSCRIPT_PREFIX)
            .ok()
            .and_then(|b| parse::transcript_head(&b, id));
        if let Some(line) = head {
            return Fact::Known(Some(Transcript {
                cwd: line.cwd,
                entrypoint: line.entrypoint.unwrap_or(Entrypoint::Other),
            }));
        }
    }
    if found_file {
        Fact::Unknown
    } else {
        Fact::Known(None)
    }
}

/// Evidence for an id that cannot be looked up.
pub fn unknown() -> ProviderEvidence {
    ProviderEvidence::Claude(ClaudeEvidence {
        registry: Fact::Unknown,
        desktop: Fact::Unknown,
        transcript: Fact::Unknown,
        account: Fact::Unknown,
    })
}

pub fn observe<S: System>(
    sys: &S,
    cfg: &CommonConfig,
    id: &str,
    boot_time: Option<u64>,
    table: &Fact<Vec<ProcessInfo>>,
) -> Found {
    let registry = registry(sys, cfg, boot_time, table);
    let (desktop, account) = desktop(sys, cfg, id);
    let transcript = transcript(sys, cfg, id);
    let desktop_cwd = desktop.known().and_then(|claims| {
        claims
            .iter()
            .find(|c| c.kind != ClaimKind::Prior)
            .and_then(|c| c.cwd.clone())
    });
    let transcript_cwd = transcript.known().cloned().flatten().and_then(|t| t.cwd);
    let found = transcript.known().is_some_and(Option::is_some)
        || desktop.known().is_some_and(|c| !c.is_empty())
        || registry
            .known()
            .is_some_and(|r| r.iter().any(|c| c.session_id.eq_ignore_ascii_case(id)));
    Found {
        evidence: ProviderEvidence::Claude(ClaudeEvidence {
            registry,
            desktop,
            transcript,
            account,
        }),
        cwd: desktop_cwd.or(transcript_cwd),
        found,
    }
}
