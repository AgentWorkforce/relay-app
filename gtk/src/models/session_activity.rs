//! Look up activity without scanning the roster for every table sort comparison.
use super::{session_is_active, LocalSessionRow};
use agent_relay_node::relay::contract::agents::ArchivedAgent;
use agent_relay_node::relay::people::{Liveness, RosterAgent};
use agent_relay_node::relay::store::Target;
use std::collections::HashSet;

pub struct SessionActivityIndex {
    archived_targets: HashSet<Target>,
    archived_uploads: HashSet<String>,
    idle_names: HashSet<String>,
    /// The time a Devin Cloud session's last activity is judged against.
    now_ms: i64,
}

impl SessionActivityIndex {
    pub fn new(archived: &[ArchivedAgent], roster: &[RosterAgent]) -> Self {
        Self {
            archived_targets: archived.iter().map(|agent| agent.target.clone()).collect(),
            archived_uploads: archived
                .iter()
                .filter_map(|agent| agent.target.upload_key())
                .collect(),
            idle_names: roster
                .iter()
                .filter(|agent| agent.liveness == Liveness::Idle)
                .map(|agent| agent.name.clone())
                .collect(),
            now_ms: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// This index judged at `now_ms` instead of when it was made.
    pub fn at(mut self, now_ms: i64) -> Self {
        self.now_ms = now_ms;
        self
    }

    pub fn with_archive_snapshot(
        mut self,
        keys: Option<&std::collections::BTreeSet<String>>,
    ) -> Self {
        if let Some(keys) = keys {
            self.archived_targets.clear();
            self.archived_uploads = keys.iter().cloned().collect();
        }
        self
    }

    pub fn activity(&self, session: &LocalSessionRow) -> &'static str {
        if self.archived_uploads.contains(&session.key())
            || session
                .relay
                .as_ref()
                .is_some_and(|local| self.archived_targets.contains(&local.target))
        {
            return "Archived";
        }
        match session.relay.as_ref() {
            Some(local) if local.running != Some(false) => {
                let idle = local.registration.as_ref().is_some_and(|registration| {
                    self.idle_names.contains(registration.name.as_str())
                });
                if session_is_active(local, self.now_ms) && !idle {
                    "Active"
                } else {
                    "Idle"
                }
            }
            _ => "Finished",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_relay_node::relay::contract::agents::LocalSession;
    use serde_json::json;

    fn local() -> LocalSession {
        serde_json::from_value(json!({"target": {"kind": "codex", "id": "a"},
            "title": "Local", "live": true, "registration": {
                "name": "worker", "address": "worker@relay", "added_by_session": false},
            "suggested_name": "worker"}))
        .unwrap()
    }

    /// A Devin Cloud session can always take a message, so its Status is
    /// Active only while it was active within a day; the same rule as
    /// `is:active` and macOS.
    #[test]
    fn a_devin_cloud_session_is_active_only_while_recently_active() {
        const DAY: i64 = 24 * 60 * 60 * 1000;
        let now = 1_800_000_000_000;
        let row = |updated: i64| LocalSessionRow {
            upload: None,
            relay: Some(
                serde_json::from_value(json!({
                    "target": {"kind": "devin", "id": "0123456789abcdef0123456789abcdef"},
                    "title": "Cloud", "live": true, "host": "Devin Cloud",
                    "updated_at_ms": updated, "suggested_name": "devin"}))
                .unwrap(),
            ),
        };
        let index = SessionActivityIndex::new(&[], &[]).at(now);
        assert_eq!(index.activity(&row(now - DAY / 2)), "Active");
        assert_eq!(index.activity(&row(now - 49 * DAY)), "Idle");
    }

    #[test]
    fn activity_refreshes_with_roster_archive_and_running_changes() {
        let mut row = LocalSessionRow {
            upload: None,
            relay: Some(local()),
        };
        assert_eq!(SessionActivityIndex::new(&[], &[]).activity(&row), "Active");
        let mut roster: Vec<RosterAgent> = serde_json::from_value(json!([{
            "name": "worker", "kind": "agent", "where": "this_computer",
            "status": "idle", "liveness": "idle", "description": "", "is_self": true
        }]))
        .unwrap();
        assert_eq!(
            SessionActivityIndex::new(&[], &roster).activity(&row),
            "Idle"
        );
        roster[0].liveness = Liveness::Live;
        assert_eq!(
            SessionActivityIndex::new(&[], &roster).activity(&row),
            "Active"
        );
        row.relay.as_mut().unwrap().running = Some(false);
        assert_eq!(
            SessionActivityIndex::new(&[], &roster).activity(&row),
            "Finished"
        );
        let archived = vec![ArchivedAgent {
            name: "worker".into(),
            address: "worker@relay".into(),
            target: local().target,
            title: "Local".into(),
            archived_at: "2026-10-14T00:00:00Z".into(),
        }];
        assert_eq!(
            SessionActivityIndex::new(&archived, &roster).activity(&row),
            "Archived"
        );
        assert_eq!(
            SessionActivityIndex::new(&archived, &roster)
                .with_archive_snapshot(Some(&Default::default()))
                .activity(&row),
            "Finished",
            "A retained page's explicit empty archive snapshot overrides newer recent-agent state"
        );
        row.upload = Some(
            serde_json::from_value(json!({"source": "codex", "session_id": "a",
            "title": "Upload", "included": false, "is_agent": false, "status": "not_shared"}))
            .unwrap(),
        );
        row.relay = None;
        assert_eq!(
            SessionActivityIndex::new(&archived, &[]).activity(&row),
            "Archived"
        );
        assert_eq!(
            SessionActivityIndex::new(&[], &[]).activity(&row),
            "Finished"
        );
    }
}
