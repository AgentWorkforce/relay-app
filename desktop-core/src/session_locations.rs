//! Keep verified session origins locally, independently of disposable history indexes.
//! Window focusing stays in the UI process so macOS attributes Automation permission
//! to the app the person clicked, not a background service.
use agent_relay_node::relay::{
    local::LocalConfig,
    process::{ProcessInfo, ProcessTable, MAX_ANCESTRY},
    sessions::DiscoveredSession,
};
use rusqlite::{params, Connection};
use std::collections::{BTreeSet, HashSet};
use std::path::Path;

mod contract;
mod extension;
pub use contract::*;
mod hosts;
pub mod planner;
mod providers;
mod store;
pub use extension::{watch, SessionLocations};
use hosts::from_process;
use providers::{claude_link, owners};
use store::{database, history, origins, save};

pub fn report(
    config: &LocalConfig,
    root: &Path,
    table: &dyn ProcessTable,
    source: &str,
    id: &str,
) -> Result<SessionLocationReport, String> {
    let owners = owners(config, table, source, &BTreeSet::from([id.to_owned()]));
    let running = !owners.is_empty();
    let current = locations(
        table,
        config.uid,
        owners.into_iter().map(|(_, process)| process),
    );
    let db = database(root)?;
    save(&db, source, id, &current)?;
    let (original, latest) = origins(&db, source, id)?;
    let previous: Vec<SessionLocation> = history(&db, source, id)?
        .into_iter()
        .filter(|old| {
            !current.iter().any(|now| {
                now.pid == old.pid
                    && now.process_start == old.process_start
                    && now.boot_id == old.boot_id
            })
        })
        .collect();
    let desktop_link = (source == "claude")
        .then(|| claude_link(config, id))
        .flatten();
    let owner_state = if running {
        "running"
    } else {
        providers::owner_state(config, table, source, id, &previous)
    };
    Ok(SessionLocationReport {
        supported: true,
        running,
        current,
        previous,
        desktop_link,
        reason: (owner_state == "unknown").then(|| "Local process or provider ownership could not be fully inspected. The session may still be running; no new session was started.".into()),
        original,
        latest,
        owner_state: Some(owner_state.into()),
    })
}

/// Called on each session-list change (`watch`), so sessions need not be clicked to be remembered.
pub fn observe(
    config: &LocalConfig,
    root: &Path,
    table: &dyn ProcessTable,
    sessions: &[DiscoveredSession],
) {
    let mut db = match database(root) {
        Ok(db) => db,
        Err(error) => {
            eprintln!("[session-locations] Could not open observation store: {error}");
            return;
        }
    };
    let tx = match db.transaction() {
        Ok(tx) => tx,
        Err(error) => {
            eprintln!("[session-locations] Could not begin observations: {error}");
            return;
        }
    };
    for source in ["codex", "claude", "grok"] {
        let ids: BTreeSet<_> = sessions
            .iter()
            .filter_map(|session| session.target.upload_key())
            .filter_map(|key| key.strip_prefix(&format!("{source}:")).map(str::to_owned))
            .collect();
        if ids.is_empty() {
            continue;
        }
        let owners = owners(config, table, source, &ids);
        for id in ids {
            let mut locations = locations(
                table,
                config.uid,
                owners
                    .iter()
                    .filter(|(key, _)| key == &id)
                    .map(|(_, process)| *process),
            );
            for location in &mut locations {
                if location.cwd.is_none() {
                    location.cwd = sessions
                        .iter()
                        .find(|session| {
                            session.target.upload_key().as_deref()
                                == Some(&format!("{source}:{id}"))
                        })
                        .and_then(|s| s.directory.clone());
                }
            }
            if locations.is_empty() {
                continue;
            }
            if let Err(error) = save(&tx, source, &id, &locations) {
                eprintln!("[session-locations] Could not retain observation: {error}");
            }
        }
    }
    if let Err(error) = tx.commit() {
        eprintln!("[session-locations] Could not commit observations: {error}");
    }
}

#[cfg(test)]
mod tests;

pub fn unavailable() -> SessionLocationReport {
    SessionLocationReport {
        supported: false,
        running: false,
        current: vec![],
        previous: vec![],
        desktop_link: None,
        reason: Some(
            "Local process observations are unavailable. No new session was started.".into(),
        ),
        original: None,
        latest: None,
        owner_state: Some("unknown".into()),
    }
}

fn locations(
    table: &dyn ProcessTable,
    uid: u32,
    processes: impl Iterator<Item = ProcessInfo>,
) -> Vec<SessionLocation> {
    let mut found = vec![];
    for process in processes {
        if table.process(process.pid) != Some(process) {
            continue;
        }
        if let Some(location) =
            from_process(table, uid, process.pid).filter(|l| l.process_start == process.start)
        {
            if !found.iter().any(|old: &SessionLocation| {
                (old.pid == location.pid && old.process_start == location.process_start)
                    || ((old.tty.is_some() || old.pane_id.is_some())
                        && old.tty == location.tty
                        && old.pane_id == location.pane_id
                        && old.host_pid == location.host_pid
                        && old.host_process_start == location.host_process_start
                        && old.provider_environment == location.provider_environment
                        && old.provider_environment_known == location.provider_environment_known)
            }) {
                found.push(location);
            }
        }
    }
    found
}
