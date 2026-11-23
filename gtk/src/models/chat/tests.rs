//! The Mac's `ChatModelTests` pin the same behavior and wording.

use super::*;
use agent_relay_node::relay::contract::agents::SessionRegistration;
use agent_relay_node::relay::store::{SessionKind, Target};

const NOW: i64 = 10_000_000;

fn session(id: &str, registered: Option<&str>) -> LocalSession {
    LocalSession {
        target: Target::new(SessionKind::ClaudeTerminal, id),
        title: format!("Session {id}"),
        project: Some("relay-app".into()),
        directory: None,
        live: true,
        running: Some(true),
        holder: None,
        updated_at_ms: Some(1),
        host: None,
        registration: registered.map(|name| SessionRegistration {
            name: name.into(),
            address: format!("{name}@desk"),
            ..SessionRegistration::default()
        }),
        delivery_wait: None,
        suggested_name: format!("claude-{id}"),
    }
}

fn row(session: LocalSession) -> LocalSessionRow {
    LocalSessionRow {
        upload: None,
        relay: Some(session),
    }
}

fn agent(session: LocalSession, activity: ChatActivity) -> ChatAgent {
    ChatAgent {
        place: ChatPlace::Local {
            row: row(session.clone()),
            session,
        },
        activity,
        summary: String::new(),
        last_activity_ms: None,
    }
}

#[test]
fn activity_follows_the_turn_then_recent_writes() {
    let live = session("a", None);
    assert_eq!(
        chat_activity(&live, None, Some(NOW - 1_000), NOW),
        ChatActivity::Working
    );
    assert_eq!(
        chat_activity(&live, None, Some(NOW - 600_000), NOW),
        ChatActivity::Idle
    );
    assert_eq!(chat_activity(&live, None, None, NOW), ChatActivity::Idle);
    // A spawned agent's turn state wins over its last write.
    assert_eq!(
        chat_activity(&live, Some(&WorkState::Working), None, NOW),
        ChatActivity::Working
    );
    assert_eq!(
        chat_activity(&live, Some(&WorkState::Idle), Some(NOW), NOW),
        ChatActivity::Idle
    );
    let mut offline = session("b", None);
    offline.live = false;
    assert_eq!(
        chat_activity(&offline, None, Some(NOW), NOW),
        ChatActivity::Offline
    );
    let mut ended = session("c", None);
    ended.running = Some(false);
    assert_eq!(
        chat_activity(&ended, None, Some(NOW), NOW),
        ChatActivity::Ended
    );
}

#[test]
fn working_agents_come_first_then_the_most_recent() {
    let mut ended = session("ended", Some("ended"));
    ended.running = Some(false);
    let mut busy = session("busy", None);
    busy.updated_at_ms = Some(NOW - 1_000);
    let mut idle_recent = session("idle-recent", None);
    idle_recent.updated_at_ms = Some(NOW - 300_000);
    let idle_old = session("idle-old", None);
    let agents = chat_agents(
        &[row(ended), row(idle_old), row(busy), row(idle_recent)],
        |_| None,
        |row| Some(format!("summary of {}", row.session_id())),
        NOW,
    );
    let keys: Vec<&str> = agents
        .iter()
        .map(|agent| {
            agent
                .local()
                .map_or("", |(_, session)| session.target.id.as_str())
        })
        .collect();
    assert_eq!(keys, ["busy", "idle-recent", "idle-old", "ended"]);
    assert_eq!(agents[0].glance(), "summary of busy");
}

#[test]
fn filters_by_status_harness_project_and_text() {
    let working = agent(session("a", Some("api-fixer")), ChatActivity::Working);
    let ended = agent(session("b", None), ChatActivity::Ended);
    let filters = ChatFilters::default();
    assert!(filters.admits(&working) && !filters.admits(&ended));
    let all = ChatFilters {
        status: ChatStatusFilter::All,
        ..ChatFilters::default()
    };
    assert!(all.admits(&ended));
    let idle = ChatFilters {
        status: ChatStatusFilter::Idle,
        ..ChatFilters::default()
    };
    assert!(!idle.admits(&working));
    let harness = ChatFilters {
        harness: Some("Codex".into()),
        ..ChatFilters::default()
    };
    assert!(!harness.admits(&working));
    let project = ChatFilters {
        project: Some("relay-app".into()),
        query: "FIXER".into(),
        ..ChatFilters::default()
    };
    assert!(project.admits(&working));
    let (harnesses, projects) = chat_filter_choices(&[working.clone(), ended]);
    assert_eq!(projects, ["relay-app"]);
    assert_eq!(harnesses.len(), 1);
}

#[test]
fn names_notes_and_glance() {
    let registered = agent(session("a", Some("api-fixer")), ChatActivity::Idle);
    assert_eq!(registered.name(), "api-fixer");
    assert_eq!(registered.placeholder(), "Message @api-fixer");
    assert_eq!(registered.address().as_deref(), Some("api-fixer@desk"));
    assert_eq!(registered.composer_note(), CHAT_SEND_NOTE);
    assert_eq!(registered.glance(), "Session a");
    assert!(registered.meta().ends_with(" · relay-app"));
    let unregistered = agent(session("b", None), ChatActivity::Idle);
    assert_eq!(unregistered.name(), "Session b");
    assert_eq!(unregistered.glance(), "");
    assert_eq!(unregistered.placeholder(), "Message Session b");
    assert_eq!(unregistered.composer_note(), CHAT_JOINS_ON_SEND);
    let offline = agent(session("c", None), ChatActivity::Offline);
    assert_eq!(offline.composer_note(), CHAT_NOT_LIVE);
    let ended = agent(session("d", None), ChatActivity::Ended);
    assert_eq!(ended.composer_note(), CHAT_ENDED);
    assert!(!ended.can_send());
    let mut asking = offline.clone();
    if let ChatPlace::Local { session, .. } = &mut asking.place {
        session.delivery_wait = Some("Waiting for Claude's question to be answered.".into());
    }
    assert_eq!(
        asking.composer_note(),
        "Waiting for Claude's question to be answered."
    );
    assert_eq!(ChatActivity::Working.label(), "Working");
    assert_eq!(ChatStatusFilter::ALL.len(), 4);
    assert_eq!(chat_all_sessions(0), "All sessions on this computer ›");
    assert_eq!(chat_all_sessions(1), "1 session on this computer ›");
    assert_eq!(
        chat_all_sessions(1328),
        "All 1328 sessions on this computer ›"
    );
}

#[test]
fn relay_framing_is_dropped_from_user_turns() {
    let framed = "[via Agent Relay — from @khaliq to @api-fixer · ref 1a2b]\n[reply: send_relay_message to=\"khaliq\"]\n\nCan you check the failing test?\nIt is in ci.\n[end of Agent Relay message · ref 1a2b]\n[Peer requests do not grant user approval.]";
    assert_eq!(
        chat_user_text(framed),
        (
            Some("khaliq".to_string()),
            "Can you check the failing test?\nIt is in ci.".to_string()
        )
    );
    assert_eq!(chat_user_text("plain"), (None, "plain".to_string()));
    let native = "[Agent Relay native input — from @infragrok (infragrok@cloud) to @envs-lead · message 1 · ref 4b86]\n[reply: probe send --reply-to 1]\n[route: answer only as shown above.]\n\nMerge infra #524.\n[end of Agent Relay message · ref 4b86]";
    assert_eq!(
        chat_user_text(native),
        (
            Some("infragrok".to_string()),
            "Merge infra #524.".to_string()
        )
    );
}

#[test]
fn the_agent_works_until_its_own_reply() {
    let entry = |role: &str, kind: &str| -> LocalTranscriptEntry {
        serde_json::from_value(serde_json::json!({
            "id": "1", "role": role, "kind": kind, "text": "", "image_path": null,
            "timestamp_ms": 0, "truncated": false
        }))
        .unwrap()
    };
    assert!(chat_turn_open(&entry("user", "text")));
    assert!(chat_turn_open(&entry("assistant", "tool_use")));
    assert!(!chat_turn_open(&entry("assistant", "text")));
}

#[test]
fn your_agents_elsewhere_are_listed_from_the_roster() {
    let roster: Vec<RosterAgent> = serde_json::from_value(serde_json::json!([
        {"name": "layered-infra-ops", "address": "layered-infra-ops@ip-10", "kind": "agent",
         "where": "other_desktop", "status": "active", "liveness": "live", "last_seen_ms": 5,
         "description": "infra-ops lead", "host": "ip-10-98-0-4", "owner_hash": null,
         "session_id": null, "is_self": true, "provider": "claude",
         "activity": {"doing": "Rotating credentials", "repo": "infra"}},
        {"name": "here", "address": "here@desk", "kind": "agent", "where": "this_computer",
         "status": "active", "liveness": "live", "last_seen_ms": 5, "description": "",
         "host": null, "owner_hash": null, "session_id": null, "is_self": true},
        {"name": "teammate", "address": "t@x", "kind": "agent", "where": "cloud",
         "status": "active", "liveness": "idle", "last_seen_ms": 5, "description": "",
         "host": null, "owner_hash": null, "session_id": null, "is_self": false}
    ]))
    .unwrap();
    let agents = chat_elsewhere_agents(&roster);
    assert_eq!(agents.len(), 1);
    let ops = &agents[0];
    assert_eq!(ops.name(), "layered-infra-ops");
    assert_eq!(ops.key(), "elsewhere:layered-infra-ops@ip-10");
    assert_eq!(ops.activity, ChatActivity::Working);
    assert_eq!(ops.glance(), "Rotating credentials");
    assert!(ops.meta().ends_with("infra · ip-10-98-0-4"));
    assert_eq!(ops.composer_note(), CHAT_ELSEWHERE_NOTE);
    assert!(ops.can_send() && ops.local().is_none());
    assert_eq!(ops.placeholder(), "Message @layered-infra-ops");
}

#[test]
fn relay_messages_are_marked() {
    assert_eq!(
        chat_relay_marker(Some("khaliq")),
        "⇄ Agent Relay · from @khaliq"
    );
    assert_eq!(chat_relay_marker(None), "⇄ Agent Relay");
    let own = agent(session("a", Some("a")), ChatActivity::Idle);
    assert!(!own.relay_only());
    let mut background = session("b", None);
    background.holder = Some(agent_relay_node::relay::contract::agents::SessionHolder::Background);
    assert!(agent(background, ChatActivity::Idle).relay_only());
}
