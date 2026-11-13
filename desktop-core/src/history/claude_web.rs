//! Claude Code on the web (claude.ai/code) sessions, as the history catalogs
//! list them. The probe's `claude-web` connector lists every claude.ai/code
//! session into each install's history catalog as a remote row, once the
//! person turns Claude Code cloud sessions on; those rows are this listing,
//! which the core merges into its Claude sessions
//! (`agent_relay_node::relay::claude_web`).

use agent_relay_node::relay::claude::ClaudeConfig;
use agent_relay_node::relay::claude_web::{same_session, HOST};
use agent_relay_node::relay::sessions::DiscoveredSession;
use agent_relay_node::relay::store::{is_claude_session_id, SessionKind, Target};
use ai_hist::{CatalogQuery, SessionScope, SessionStore, Source, StoreOptions};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// A web session idle longer than this is left off the list: claude.ai
/// keeps every session a person ever started, and an old one is archived
/// or forgotten more often than not.
pub const RECENT_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// The claude.ai/code sessions the probe installs' history catalogs list as
/// remote, as discovered rows, while the person has Claude Code cloud
/// sessions on and `~/.claude` shared: the consent the connector that lists
/// and uploads them asks for, since it reads Claude Code's login there
/// whatever `CLAUDE_CONFIG_DIR` says. A row is `live` while the `claude`
/// command line that sends into it is installed. A session several
/// installs know is listed once.
pub fn discover(config: &ClaudeConfig, probe_root: &Path) -> Vec<DiscoveredSession> {
    if !agent_relay_node::relay::claude_dirs::cloud_sessions_shared_here() {
        return Vec::new();
    }
    discover_at(probe_root, config.executable.is_some(), now_ms())
}

/// [`discover`] past its opt-in, at `now_ms`.
pub fn discover_at(probe_root: &Path, live: bool, now_ms: i64) -> Vec<DiscoveredSession> {
    let mut found: BTreeMap<String, DiscoveredSession> = BTreeMap::new();
    for install in installs(probe_root) {
        for session in remote_sessions(&install.join("history.db"), live, now_ms) {
            // The install that saw the session last has its latest title.
            let key = same_session(&session.target.id).to_string();
            match found.get(&key) {
                Some(kept) if kept.updated_at_ms >= session.updated_at_ms => {}
                _ => {
                    found.insert(key, session);
                }
            }
        }
    }
    found.into_values().collect()
}

/// The probe installs under `probe_root`: a live one carries a
/// `config.json`; a decommissioned one's catalog is nobody's to refresh.
fn installs(probe_root: &Path) -> Vec<PathBuf> {
    let mut installs: Vec<PathBuf> = std::fs::read_dir(probe_root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && path.join("config.json").exists())
        .collect();
    installs.sort();
    installs
}

/// One install's recent remote Claude rows with a claude.ai session id.
fn remote_sessions(catalog_db: &Path, live: bool, now_ms: i64) -> Vec<DiscoveredSession> {
    let mut options = StoreOptions::default().db_path(catalog_db);
    options.read_only = true;
    let Ok(store) = SessionStore::open(options) else {
        return Vec::new();
    };
    let mut query = CatalogQuery::default();
    query.scope = SessionScope::Remote;
    query.sources = Some(vec![Source::Claude]);
    store
        .sessions(query)
        .filter_map(|row| row.ok())
        .filter(|row| {
            row.locations.iter().any(|location| location == "remote")
                && is_claude_session_id(&row.session_id)
                && row
                    .last_activity_ms
                    .is_some_and(|at| now_ms.saturating_sub(at) <= RECENT_MS)
        })
        .map(|row| DiscoveredSession {
            target: Target::new(SessionKind::ClaudeCloud, row.session_id.clone()),
            title: row.title.unwrap_or_else(|| row.session_id.clone()),
            directory: row.cwd,
            updated_at_ms: row.last_activity_ms,
            host: Some(HOST.to_string()),
            live,
            // A web session is not a transcript one local process holds.
            holder: None,
        })
        .collect()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_MS: i64 = 24 * 60 * 60 * 1000;

    /// Writes `rows` (id, title, last activity) as one install's remote
    /// `claude-web` observations under `root`.
    pub(crate) fn catalog(root: &Path, install: &str, rows: &[(&str, &str, i64)]) {
        let dir = root.join(install);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), "{}").unwrap();
        let store =
            SessionStore::open(StoreOptions::default().db_path(dir.join("history.db"))).unwrap();
        store
            .apply_source_observations(ai_hist::SourceObservationBatch::new(
                "claude-web",
                "default",
                ai_hist::SessionLocation::Remote,
                rows.iter()
                    .map(|(id, title, at)| ai_hist::SourceObservation {
                        session: ai_hist::ShallowSession {
                            source: "claude".into(),
                            session_id: (*id).into(),
                            title: Some((*title).into()),
                            cwd: Some("/home/user/relay-desktop".into()),
                            last_activity_ms: Some(*at),
                            source_stamp: Some(format!("web:{at}:{id}")),
                            ..Default::default()
                        },
                        raw_locator: Some(format!("v1/sessions/{id}")),
                    })
                    .collect(),
            ))
            .unwrap();
    }

    #[test]
    fn recent_web_sessions_are_listed_once_as_claude_cloud_rows() {
        let root = tempfile::tempdir().unwrap();
        let now = 100 * DAY_MS;
        let web = "cse_01AbCdEfGhIjKlMn";
        catalog(
            root.path(),
            "install-a",
            &[(web, "Wire relay", now - DAY_MS)],
        );
        catalog(
            root.path(),
            "install-b",
            &[(web, "Wire relay", now - DAY_MS)],
        );
        let found = discover_at(root.path(), true, now);
        assert_eq!(found.len(), 1);
        let row = &found[0];
        assert_eq!(row.target, Target::new(SessionKind::ClaudeCloud, web));
        assert_eq!(row.title, "Wire relay");
        assert_eq!(row.host.as_deref(), Some(HOST));
        assert_eq!(row.directory.as_deref(), Some("/home/user/relay-desktop"));
        assert!(row.live);
    }

    #[test]
    fn the_install_that_saw_a_session_last_names_it() {
        let root = tempfile::tempdir().unwrap();
        let now = 100 * DAY_MS;
        let web = "cse_01AbCdEfGhIjKlMn";
        catalog(
            root.path(),
            "install-a",
            &[(web, "Old title", now - 2 * DAY_MS)],
        );
        catalog(
            root.path(),
            "install-b",
            &[(web, "New title", now - DAY_MS)],
        );
        let found = discover_at(root.path(), true, now);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "New title");
        assert_eq!(found[0].updated_at_ms, Some(now - DAY_MS));
    }

    #[test]
    fn both_forms_of_one_session_id_are_one_session() {
        let root = tempfile::tempdir().unwrap();
        let now = 100 * DAY_MS;
        catalog(
            root.path(),
            "install-a",
            &[("session_01AbCdEfGh", "Old", now - DAY_MS)],
        );
        catalog(root.path(), "install-b", &[("cse_01AbCdEfGh", "New", now)]);
        let found = discover_at(root.path(), true, now);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "New");
    }

    #[test]
    fn old_and_untagged_sessions_are_left_off() {
        let root = tempfile::tempdir().unwrap();
        let now = 100 * DAY_MS;
        catalog(
            root.path(),
            "install-a",
            &[
                ("session_01Old0000000000", "Old", now - 8 * DAY_MS),
                ("0b6f2d0e-8a51-4a5c-9c3e-5d1f0a2b3c4d", "Container id", now),
            ],
        );
        assert!(discover_at(root.path(), true, now).is_empty());
    }

    #[test]
    fn without_the_claude_command_line_a_row_is_not_live() {
        let root = tempfile::tempdir().unwrap();
        let now = 100 * DAY_MS;
        catalog(
            root.path(),
            "install-a",
            &[("session_01AbCdEfGh", "t", now)],
        );
        assert!(!discover_at(root.path(), false, now)[0].live);
    }

    #[test]
    fn a_decommissioned_install_lists_nothing() {
        let root = tempfile::tempdir().unwrap();
        let now = 100 * DAY_MS;
        catalog(
            root.path(),
            "install-a",
            &[("session_01AbCdEfGh", "t", now)],
        );
        std::fs::remove_file(root.path().join("install-a/config.json")).unwrap();
        assert!(discover_at(root.path(), true, now).is_empty());
    }
}
