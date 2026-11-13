//! Whether the session list shows a session as active: its Status column
//! and `is:active`. macOS keeps the same rule in
//! `RelayDesktopCore/SessionActivity.swift`; both are tested against
//! `shared/session-active-cases.json`.
use agent_relay_node::relay::contract::agents::LocalSession;
use agent_relay_node::relay::store::SessionKind;

/// How recently a Devin Cloud session must have been active to count as
/// active, and to join the relay by itself. A Devin Cloud row stays `live`
/// for as long as the org's catalog remembers it — a message is how a quiet
/// cloud session resumes — so `live` alone would call weeks-old sessions
/// active.
pub const DEVIN_CLOUD_ACTIVE_WINDOW_MS: i64 = 24 * 60 * 60 * 1000;

/// The longest the apps wait before judging activity again even when no
/// session is due to change: a sleep or a clock set forward can carry the
/// time past a change a timer was set for.
pub const ACTIVITY_RECHECK_MS: i64 = 15 * 60 * 1000;

/// Whether a Devin Cloud row last active at `last_activity_ms` was active
/// within [`DEVIN_CLOUD_ACTIVE_WINDOW_MS`] of `now_ms`. The time is the
/// provider's and `now_ms` this computer's, so a time up to one window
/// ahead (a local clock running slow) still counts — rejecting it would
/// take every automatic cloud agent off the relay whenever the clock lags.
/// A row with no activity time, or one dated further ahead (a wrong unit, a
/// clock far off), proves nothing and is not recent.
pub fn devin_cloud_recently_active(last_activity_ms: Option<i64>, now_ms: i64) -> bool {
    last_activity_ms.is_some_and(|at| {
        (-DEVIN_CLOUD_ACTIVE_WINDOW_MS..=DEVIN_CLOUD_ACTIVE_WINDOW_MS)
            .contains(&now_ms.saturating_sub(at))
    })
}

/// Whether `session` is a Devin Cloud session: listed from the org's
/// catalog, not a `devin` process on this computer.
pub fn is_devin_cloud(session: &LocalSession) -> bool {
    session.target.kind == SessionKind::Devin
        && session.host.as_deref() == Some(agent_relay_node::relay::devin_cloud::CLOUD_HOST)
}

/// Whether the session list shows `session` as active at `now_ms`: it runs,
/// can take a message now, and — for a Devin Cloud session, which can
/// always take one — was active within [`DEVIN_CLOUD_ACTIVE_WINDOW_MS`].
pub fn session_is_active(session: &LocalSession, now_ms: i64) -> bool {
    session.live
        && session.running != Some(false)
        && (!is_devin_cloud(session) || devin_cloud_recently_active(session.updated_at_ms, now_ms))
}

/// The first time after `now_ms` at which `session` changes between active
/// and not by time alone, or `None` when only new data can change it. A
/// Devin Cloud session becomes active once its time is no more than a
/// window ahead, and stops a window after it.
pub fn activity_change_after(session: &LocalSession, now_ms: i64) -> Option<i64> {
    if !session.live || session.running == Some(false) || !is_devin_cloud(session) {
        return None;
    }
    let at = session.updated_at_ms?;
    [
        at.saturating_sub(DEVIN_CLOUD_ACTIVE_WINDOW_MS),
        at.saturating_add(DEVIN_CLOUD_ACTIVE_WINDOW_MS)
            .saturating_add(1),
    ]
    .into_iter()
    .find(|&change| change > now_ms)
}

/// When the apps should judge activity again after `now_ms`: the first
/// change among `sessions`, and never later than [`ACTIVITY_RECHECK_MS`].
/// Caches of Status and `is:active` results are kept until then; nothing in
/// them can change by time alone sooner.
pub fn next_activity_check<'a>(
    sessions: impl IntoIterator<Item = &'a LocalSession>,
    now_ms: i64,
) -> i64 {
    sessions
        .into_iter()
        .filter_map(|session| activity_change_after(session, now_ms))
        .fold(now_ms.saturating_add(ACTIVITY_RECHECK_MS), i64::min)
}

/// What `sessions`' activity is at `now_ms`, as one value: it changes
/// exactly when some session's Status or `is:active` match does, so a cache
/// keyed by it is never stale and never rebuilt for nothing.
pub fn activity_revision<'a>(
    sessions: impl IntoIterator<Item = &'a LocalSession>,
    now_ms: i64,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    for session in sessions
        .into_iter()
        .filter(|session| is_devin_cloud(session))
    {
        (&session.target.id, session_is_active(session, now_ms)).hash(&mut hash);
    }
    hash.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::session_model::{merge_local_sessions, LocalSessionQuery};

    fn fixture() -> (i64, Vec<(String, LocalSession, bool, Option<i64>)>) {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/shared/session-active-cases.json"
        ))
        .unwrap();
        let cases = fixture["cases"].as_array().unwrap().iter().map(|case| {
            (
                case["name"].as_str().unwrap().to_string(),
                serde_json::from_value(case["session"].clone()).unwrap(),
                case["active"].as_bool().unwrap(),
                case["next_change_ms"].as_i64(),
            )
        });
        (fixture["now_ms"].as_i64().unwrap(), cases.collect())
    }

    #[test]
    fn activity_matches_shared_contract() {
        let (now, cases) = fixture();
        for (name, session, active, next_change) in cases {
            assert_eq!(session_is_active(&session, now), active, "{name}");
            assert_eq!(activity_change_after(&session, now), next_change, "{name}");
            if let Some(change) = next_change {
                assert_ne!(session_is_active(&session, change), active, "{name}: flips");
                assert_eq!(
                    session_is_active(&session, change - 1),
                    active,
                    "{name}: not before"
                );
            }
            let rows = merge_local_sessions(&[], std::slice::from_ref(&session));
            let query = |text: &str| LocalSessionQuery::new(text).at(now).matches(&rows[0], None);
            assert_eq!(query("is:active"), active, "is:active: {name}");
            assert_eq!(query("is:inactive"), !active, "is:inactive: {name}");
        }
    }

    #[test]
    fn the_next_check_is_the_first_change_and_never_past_the_recheck() {
        let (now, cases) = fixture();
        let sessions: Vec<LocalSession> = cases.iter().map(|case| case.1.clone()).collect();
        let first = cases.iter().filter_map(|case| case.3).min().unwrap();
        assert_eq!(
            next_activity_check(&sessions, now),
            first.min(now + ACTIVITY_RECHECK_MS)
        );
        assert_eq!(next_activity_check(&[], now), now + ACTIVITY_RECHECK_MS);
    }

    #[test]
    fn the_revision_moves_exactly_when_an_activity_changes() {
        let (now, cases) = fixture();
        let sessions: Vec<LocalSession> = cases.iter().map(|case| case.1.clone()).collect();
        let change = cases.iter().filter_map(|case| case.3).min().unwrap();
        assert_eq!(
            activity_revision(&sessions, now),
            activity_revision(&sessions, change - 1)
        );
        assert_ne!(
            activity_revision(&sessions, now),
            activity_revision(&sessions, change)
        );
    }
}
