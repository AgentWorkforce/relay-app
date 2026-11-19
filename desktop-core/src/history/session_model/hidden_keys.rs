//! The keys of the saved hidden-session list: keys saved under an older row
//! key, retired hides that wait for their exclusion, and Claude web aliases.
use agent_relay_node::relay::contract::agents::LocalSession;
use std::collections::BTreeSet;

/// Row keys a relay-only session had before it joined its transcript by
/// upload key; hidden sessions saved under them stay hidden.
const RENAMED_ROW_KEYS: &[(&str, &str)] = &[
    ("grok-open:", "grok:"),
    ("opencode-open:", "opencode:"),
    ("muse-open:", "muse:"),
];

/// A hide saved under the key Cursor rows had before 2026.11.16, and the
/// key the row has now. Such a retired hide keeps the chat hidden until one
/// exclusion of the current key has succeeded; then it is dropped, so the chat
/// shows again with sharing off. Before that, a hide under the old key never
/// stored an exclusion.
const RETIRED_HIDDEN_PREFIX: (&str, &str) = ("cursor-cli:", "cursor:");

/// The current row key for a hidden session saved under an older one.
pub fn current_hidden_key(key: &str) -> String {
    RENAMED_ROW_KEYS
        .iter()
        .find_map(|(old, new)| key.strip_prefix(old).map(|id| format!("{new}{id}")))
        .unwrap_or_else(|| key.to_owned())
}

/// The current row key of a hide saved under a retired key.
pub fn retired_hidden_key(saved: &str) -> Option<String> {
    let (old, new) = RETIRED_HIDDEN_PREFIX;
    saved.strip_prefix(old).map(|id| format!("{new}{id}"))
}

/// The current row keys of a saved hidden list, retired hides included.
pub fn current_hidden_keys(saved: &[String]) -> BTreeSet<String> {
    saved
        .iter()
        .map(|key| retired_hidden_key(key).unwrap_or_else(|| current_hidden_key(key)))
        .collect()
}

/// The current row keys of the retired hides in a saved hidden list that
/// still wait for their exclusion. One saved beside its current key is an
/// ordinary hide.
pub fn retired_hidden_keys(saved: &[String]) -> BTreeSet<String> {
    saved
        .iter()
        .filter_map(|key| retired_hidden_key(key))
        .filter(|key| !saved.contains(key))
        .collect()
}

/// The list to save for `hidden`, given the list saved now: a retired hide
/// still waiting keeps its old key, so it is not saved as a new hide.
pub fn saved_hidden_keys(hidden: &BTreeSet<String>, saved: &[String]) -> Vec<String> {
    let retired = retired_hidden_keys(saved);
    let (old, new) = RETIRED_HIDDEN_PREFIX;
    let mut keys: Vec<_> = hidden
        .iter()
        .map(|key| match key.strip_prefix(new) {
            Some(id) if retired.contains(key) => format!("{old}{id}"),
            _ => key.clone(),
        })
        .collect();
    keys.sort();
    keys
}

/// The host-aware key added beside an older hidden target key. Keeping both
/// makes the preference stable if web discovery is temporarily unavailable.
pub fn additional_hidden_key_for_sessions(key: &str, sessions: &[LocalSession]) -> Option<String> {
    let current = current_hidden_key(key);
    let session = (current == key).then(|| {
        sessions.iter().find(|session| {
            session.target.kind == agent_relay_node::relay::store::SessionKind::ClaudeCloud
                && session.host.as_deref() == Some(agent_relay_node::relay::claude_web::HOST)
                && agent_relay_node::relay::claude_web::web_session_id(&session.target).is_some()
                && (session.target.key() == key || session.upload_key().as_deref() == Some(key))
        })
    })??;
    let target = session.target.key();
    let upload = session.upload_key()?;
    if target == key {
        Some(upload)
    } else if upload == key {
        Some(target)
    } else {
        None
    }
}

/// Whether two persisted row keys are aliases for one Claude web session.
/// Both are retained while hidden so a discovery outage cannot reveal it;
/// unhiding either identity removes the complete alias set.
pub fn same_hidden_session(left: &str, right: &str) -> bool {
    left == right
        || claude_hidden_body(left)
            .zip(claude_hidden_body(right))
            .is_some_and(|(left, right)| left == right)
}

fn claude_hidden_body(key: &str) -> Option<&str> {
    let id = key
        .strip_prefix("claude-cloud:")
        .or_else(|| key.strip_prefix("claude:"))?;
    let body = id
        .strip_prefix("session_")
        .or_else(|| id.strip_prefix("cse_"))?;
    agent_relay_node::relay::store::is_claude_session_id(id).then_some(body)
}
#[cfg(test)]
mod tests {
    use super::super::merge_local_sessions;
    use super::*;
    use serde_json::json;
    fn local(kind: &str, id: &str) -> LocalSession {
        serde_json::from_value(json!({"target": {"kind": kind, "id": id}, "title": "Local work", "live": true, "project": "desktop", "registration": null, "suggested_name": "session", "updated_at_ms": 200})).unwrap()
    }
    #[test]
    fn hidden_sessions_saved_under_a_relay_row_key_stay_hidden() {
        let id = "01a15bca-891b-7ce2-9318-d0683ee0163a";
        let row = merge_local_sessions(&[], &[local("grok-open", id)]).remove(0);
        assert_eq!(current_hidden_key(&format!("grok-open:{id}")), row.key());
        for (kind, id) in [("opencode-open", "ses_1"), ("muse-open", "m1")] {
            let row = merge_local_sessions(&[], &[local(kind, id)]).remove(0);
            assert_eq!(current_hidden_key(&format!("{kind}:{id}")), row.key());
        }
        assert_eq!(current_hidden_key("codex:same"), "codex:same");
        assert_eq!(
            current_hidden_key("claude-cloud:cse_1"),
            "claude-cloud:cse_1"
        );
    }
    #[test]
    fn hides_saved_under_the_old_cursor_key_wait_for_their_exclusion() {
        let row = merge_local_sessions(&[], &[local("cursor-cli", "cursor-1")]).remove(0);
        assert_eq!(retired_hidden_key("cursor-cli:cursor-1"), Some(row.key()));
        assert_eq!(retired_hidden_key("cursor:cursor-1"), None);
        assert_eq!(current_hidden_key("cursor-cli:x"), "cursor-cli:x");
        let saved = ["cursor-cli:x", "cursor:kept", "grok-open:g1", "codex:c1"].map(String::from);
        let hidden = current_hidden_keys(&saved);
        assert_eq!(
            hidden,
            BTreeSet::from(["codex:c1", "cursor:kept", "cursor:x", "grok:g1"].map(String::from))
        );
        assert_eq!(
            retired_hidden_keys(&saved),
            BTreeSet::from(["cursor:x".to_owned()])
        );
        let mut more = hidden.clone();
        more.insert("codex:new".into());
        assert_eq!(
            saved_hidden_keys(&more, &saved),
            [
                "codex:c1",
                "codex:new",
                "cursor-cli:x",
                "cursor:kept",
                "grok:g1"
            ]
        );
        more.remove("cursor:x");
        assert!(!saved_hidden_keys(&more, &saved)
            .iter()
            .any(|key| key.ends_with(":x")));
    }
    #[test]
    fn a_cursor_hide_saved_beside_the_old_one_is_an_ordinary_hide() {
        let saved = ["cursor-cli:x", "cursor:x"].map(String::from);
        assert_eq!(
            current_hidden_keys(&saved),
            BTreeSet::from(["cursor:x".to_owned()])
        );
        assert!(retired_hidden_keys(&saved).is_empty());
        assert_eq!(
            saved_hidden_keys(&current_hidden_keys(&saved), &saved),
            ["cursor:x"]
        );
    }
}
