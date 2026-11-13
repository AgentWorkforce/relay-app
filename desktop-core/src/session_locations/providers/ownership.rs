use super::*;

pub(super) fn state(
    config: &LocalConfig,
    table: &dyn ProcessTable,
    source: &str,
    id: &str,
    previous: &[SessionLocation],
) -> &'static str {
    if !previous_owners_gone(table, previous) {
        return "unknown";
    }
    let Some(known_pids) = registry_owners(config, table, source, id) else {
        return "unknown";
    };
    inventory_state(config, table, source, id, previous, &known_pids)
}

fn previous_owners_gone(table: &dyn ProcessTable, previous: &[SessionLocation]) -> bool {
    let boot = hosts::boot_id();
    for old in previous {
        if old.boot_id.is_some() && boot.is_some() && old.boot_id != boot {
            continue;
        }
        match table.process(old.pid) {
            Some(now) if now.start != old.process_start || table.zombie(old.pid) => {}
            Some(_) => return false,
            None if table.process_gone(old.pid) => {}
            None => return false,
        }
    }
    true
}

fn registry_owners(
    config: &LocalConfig,
    table: &dyn ProcessTable,
    source: &str,
    id: &str,
) -> Option<HashSet<i32>> {
    let mut known_pids = HashSet::new();
    if source == "claude" {
        let registry = agent_relay_node::relay::claude::registry(&config.claude, table);
        if !registry.unreadable.is_empty() || registry.sessions.iter().any(|s| s.session_id == id) {
            return None;
        }
        known_pids.extend(registry.sessions.iter().map(|s| s.pid));
        if unproven_claude_record(config, table, id) {
            return None;
        }
    }
    if source == "grok" {
        let sessions =
            agent_relay_node::relay::grok::open_sessions(&config.grok, table, config.uid);
        if sessions.iter().any(|s| s.session_id == id) {
            return None;
        }
        known_pids.extend(sessions.iter().map(|s| s.pid));
    }
    Some(known_pids)
}

fn unproven_claude_record(config: &LocalConfig, table: &dyn ProcessTable, id: &str) -> bool {
    // A readable registry record with no process proof still blocks resume.
    for directory in config.claude.registries() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let file = entry.path();
            if file.extension().is_none_or(|s| s != "json") {
                continue;
            }
            let Some(session) = std::fs::read(file)
                .ok()
                .and_then(|b| agent_relay_node::relay::claude::TerminalSession::decode(&b))
            else {
                continue;
            };
            if session.session_id == id
                && table.process(session.pid).is_none()
                && !table.process_gone(session.pid)
            {
                return true;
            }
        }
    }
    false
}

fn inventory_state(
    config: &LocalConfig,
    table: &dyn ProcessTable,
    source: &str,
    id: &str,
    previous: &[SessionLocation],
    known_pids: &HashSet<i32>,
) -> &'static str {
    // Recorded owners were positively checked above. Protected unrelated OS
    // processes (for example Windows SYSTEM PIDs) cannot undo that evidence.
    // Without recorded provenance, an incomplete inventory remains unknown.
    let pids = table.all_pids();
    if pids.is_empty() {
        return "unknown";
    }
    let mut observed_user_process = false;
    for pid in pids {
        let Some(process) = table.process(pid) else {
            if previous.is_empty() {
                return "unknown";
            }
            continue;
        };
        if process.uid != config.uid || table.zombie(pid) {
            continue;
        }
        observed_user_process = true;
        let Some(exe) = table.executable(pid) else {
            return "unknown";
        };
        let stem = exe.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if stem == source && !known_pids.contains(&pid) {
            if source == "codex" && codex_owns_other_session(table, pid, id) {
                continue;
            }
            return "unknown";
        }
    }
    if previous.is_empty() && !observed_user_process {
        "unknown"
    } else {
        "stopped"
    }
}

fn codex_owns_other_session(table: &dyn ProcessTable, pid: i32, id: &str) -> bool {
    let arguments = table.arguments(pid);
    let locks = agent_relay_node::relay::process::codex_locked_threads(table, pid);
    let resume = agent_relay_node::relay::process::resume_argument(&arguments);
    (!locks.is_empty() && !locks.contains(id)) || resume.is_some_and(|other| other != id)
}
