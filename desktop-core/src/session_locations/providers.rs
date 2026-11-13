use super::*;

mod ownership;
pub(super) fn owners(
    config: &LocalConfig,
    table: &dyn ProcessTable,
    source: &str,
    ids: &BTreeSet<String>,
) -> Vec<(String, ProcessInfo)> {
    let candidates: Vec<(String, i32, Option<u64>)> = match source {
        "codex" => {
            agent_relay_node::relay::codex::processes::process_sessions(table, config.uid, ids)
                .into_iter()
                .filter(|s| {
                    table
                        .process(s.pid)
                        .is_some_and(|p| p.start == s.start && !table.zombie(p.pid))
                })
                .map(|s| (s.thread, s.pid, Some(s.start)))
                .collect()
        }
        "claude" => agent_relay_node::relay::claude::terminal_sessions(&config.claude, table)
            .into_iter()
            .filter(|s| ids.contains(&s.session_id))
            .filter_map(|s| {
                let process = table.process(s.pid)?;
                if process.uid != config.uid
                    || table.zombie(s.pid)
                    || !agent_relay_node::relay::process::claude_start_matches(
                        table.start_clock(),
                        s.proc_start.as_deref()?,
                        &process,
                    )
                {
                    return None;
                }
                Some((s.session_id, s.pid, Some(process.start)))
            })
            .collect(),
        "grok" => agent_relay_node::relay::grok::open_sessions(&config.grok, table, config.uid)
            .into_iter()
            .filter(|s| ids.contains(&s.session_id))
            .filter_map(|s| {
                let process = table.process(s.pid)?;
                // The TUI registry lacks procStart. Its observation must be newer
                // than this incarnation, or a recycled Grok PID could steal it.
                let opened = u64::try_from(s.opened_at_ms?).ok()?;
                if process_start_ms(table.start_clock(), process.start)
                    .is_none_or(|start| start > opened)
                {
                    return None;
                }
                Some((s.session_id, s.pid, Some(process.start)))
            })
            .collect(),
        _ => vec![],
    };
    candidates
        .into_iter()
        .filter_map(|(id, pid, expected)| {
            let process = table.process(pid).filter(|p| {
                p.uid == config.uid
                    && !table.zombie(pid)
                    && expected.is_none_or(|start| start == p.start)
            })?;
            Some((id, process))
        })
        .collect()
}

/// Claude Desktop's local_* identity differs from its cliSessionId. Only the
/// provider's record establishes that mapping; titles and cwd never do.
pub(super) fn claude_link(config: &LocalConfig, id: &str) -> Option<String> {
    let roots = claude_metadata_roots(config, planner::Platform::current(), &|key| {
        std::env::var(key).ok()
    });
    let matches: HashSet<_> = roots
        .iter()
        .filter_map(|root| claude_desktop_id(root, id))
        .collect();
    if matches.len() != 1 {
        return None;
    }
    matches
        .into_iter()
        .next()
        .map(|local| format!("claude://code/continue?session={local}"))
}

pub(super) fn claude_desktop_id(root: &std::path::Path, id: &str) -> Option<String> {
    fn entries(path: &std::path::Path) -> Vec<std::path::PathBuf> {
        std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect()
    }
    let mut matches = HashSet::new();
    for file in entries(root)
        .iter()
        .flat_map(|org| entries(org))
        .flat_map(|account| entries(&account))
    {
        if file.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(&file) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() > 1_048_576 {
            continue;
        }
        let Some(value) = std::fs::read(&file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        else {
            continue;
        };
        if value["cliSessionId"].as_str() != Some(id) || value["isArchived"].as_bool() == Some(true)
        {
            continue;
        }
        let Some(local) = value["sessionId"].as_str() else {
            continue;
        };
        if local.strip_prefix("local_").is_some_and(|rest| {
            !rest.is_empty()
                && rest.len() <= 64
                && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        }) {
            matches.insert(local.to_owned());
        }
    }
    (matches.len() == 1)
        .then(|| matches.into_iter().next())
        .flatten()
}

/// A missing lookup is not proof of exit. Preserve uncertainty instead of
/// turning an unreadable process or partially written registry into a resume.
pub(super) fn owner_state(
    config: &LocalConfig,
    table: &dyn ProcessTable,
    source: &str,
    id: &str,
    previous: &[SessionLocation],
) -> &'static str {
    ownership::state(config, table, source, id, previous)
}

fn process_start_ms(
    clock: agent_relay_node::relay::process::StartClock,
    start: u64,
) -> Option<u64> {
    use agent_relay_node::relay::process::StartClock;
    match clock {
        StartClock::Micros | StartClock::Windows => Some(start / 1000),
        StartClock::Ticks => {
            #[cfg(target_os = "linux")]
            {
                let stat = std::fs::read_to_string("/proc/stat").ok()?;
                let boot = stat
                    .lines()
                    .find_map(|l| l.strip_prefix("btime "))?
                    .parse::<u64>()
                    .ok()?;
                // SAFETY: sysconf accepts the constant _SC_CLK_TCK and neither
                // dereferences caller pointers nor retains borrowed memory. Its
                // failure value is rejected by the checked positive conversion.
                let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
                let ticks = u64::try_from(ticks).ok().filter(|n| *n > 0)?;
                Some(
                    boot.checked_mul(1000)?
                        .checked_add(start.checked_mul(1000)? / ticks)?,
                )
            }
            #[cfg(not(target_os = "linux"))]
            {
                None
            }
        }
    }
}

/// The explicit provider configuration always wins. On Windows the public
/// messaging core has no app_sessions path, so private navigation resolves the
/// Electron data directory itself. Linux reads only an existing XDG record;
/// finding it does not claim that a desktop URL handler is installed.
pub(super) fn claude_metadata_roots(
    config: &LocalConfig,
    platform: planner::Platform,
    env: &dyn Fn(&str) -> Option<String>,
) -> Vec<std::path::PathBuf> {
    if let Some(root) = config.claude.app_sessions.as_ref().and_then(|p| p.parent()) {
        return vec![root.join("claude-code-sessions")];
    }
    let root = match platform {
        planner::Platform::Windows => env("APPDATA")
            .map(std::path::PathBuf::from)
            .map(|p| p.join("Claude")),
        planner::Platform::Linux => env("XDG_CONFIG_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| env("HOME").map(|p| Path::new(&p).join(".config")))
            .map(|p| p.join("Claude")),
        planner::Platform::MacOS => None,
    };
    root.map(|p| vec![p.join("claude-code-sessions")])
        .unwrap_or_default()
}
