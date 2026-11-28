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
    let here = ChatFilters {
        machine: Some(CHAT_THIS_COMPUTER.into()),
        ..ChatFilters::default()
    };
    assert!(here.admits(&working));
    let away = ChatFilters {
        machine: Some("studio-mac".into()),
        ..ChatFilters::default()
    };
    assert!(!away.admits(&working));
    let (harnesses, projects, machines) = chat_filter_choices(&[working.clone(), ended]);
    assert_eq!(projects, ["relay-app"]);
    assert_eq!(harnesses.len(), 1);
    assert_eq!(machines, [CHAT_THIS_COMPUTER]);
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
        {"name": "layered-infra-ops", "address": "layered-infra-ops@desk.example.test", "kind": "agent",
         "where": "other_desktop", "status": "active", "liveness": "live", "last_seen_ms": 5,
         "description": "infra-ops lead", "host": "build.example.test", "owner_hash": null,
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
    let agents = chat_elsewhere_agents(&roster, &[], i64::MAX, |_| false);
    assert_eq!(agents.len(), 1);
    let ops = &agents[0];
    assert_eq!(ops.name(), "layered-infra-ops");
    assert_eq!(ops.key(), "elsewhere:layered-infra-ops@desk.example.test");
    assert_eq!(ops.activity, ChatActivity::Working);
    assert_eq!(ops.glance(), "Rotating credentials");
    assert!(ops.meta().ends_with("infra · build.example.test"));
    assert_eq!(ops.composer_note(), CHAT_ELSEWHERE_NOTE);
    assert!(ops.can_send() && ops.local().is_none());
    assert_eq!(ops.placeholder(), "Message @layered-infra-ops");
    assert_eq!(chat_elsewhere_tag(ops), "Elsewhere · build.example.test");
    // Someone else's agent is listed when a search finds it, marked.
    let found = chat_elsewhere_agents(&roster, &[], i64::MAX, |agent| agent.matches("teammate"));
    assert_eq!(found.len(), 2);
    let theirs = found
        .iter()
        .find(|agent| agent.name() == "teammate")
        .unwrap();
    assert!(!theirs.is_yours());
    assert_eq!(chat_elsewhere_tag(theirs), "Not yours · Cloud");
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

#[test]
fn agents_elsewhere_are_tagged_with_where_they_run() {
    assert_eq!(
        chat_elsewhere_tag(&agent(session("a", None), ChatActivity::Idle)),
        ""
    );
}

#[test]
fn an_agent_the_sessions_list_shows_is_not_listed_elsewhere_too() {
    let roster: Vec<RosterAgent> = serde_json::from_value(serde_json::json!([
        {"name": "custsuccessgrok", "address": "custsuccessgrok@cloud", "kind": "agent",
         "where": "cloud", "status": "active", "liveness": "live", "last_seen_ms": 5,
         "description": "", "host": null, "owner_hash": null, "session_id": null,
         "is_self": true},
        {"name": "other", "address": "other@cloud", "kind": "agent", "where": "cloud",
         "status": "active", "liveness": "live", "last_seen_ms": 5, "description": "",
         "host": null, "owner_hash": null, "session_id": "s-9", "is_self": true}
    ]))
    .unwrap();
    let mut titled = session("a", None);
    titled.title = "custsuccessgrok".into();
    assert!(chat_elsewhere_agents(&roster, &[row(titled)], i64::MAX, |_| false).len() == 1);
    let uploaded = session("s-9", None);
    assert!(chat_elsewhere_agents(&roster, &[row(uploaded)], i64::MAX, |_| false).len() == 1);
    let named = session("b", Some("other"));
    let left = chat_elsewhere_agents(&roster, &[row(named)], i64::MAX, |_| false);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].name(), "custsuccessgrok");
}

#[test]
fn a_heartbeat_does_not_reorder_agents() {
    let mut early = session("b-agent", None);
    early.updated_at_ms = Some(NOW - 10_000);
    let mut late = session("a-agent", None);
    late.updated_at_ms = Some(NOW - 9_000);
    let first = chat_agents(
        &[row(early.clone()), row(late.clone())],
        |_| None,
        |_| None,
        NOW,
    );
    // The other one heartbeats a few seconds later: the order holds.
    early.updated_at_ms = Some(NOW - 1_000);
    let second = chat_agents(&[row(early), row(late)], |_| None, |_| None, NOW);
    let names =
        |agents: &[ChatAgent]| -> Vec<String> { agents.iter().map(ChatAgent::name).collect() };
    assert_eq!(names(&first), names(&second));
    assert_eq!(
        chat_order_bucket(Some(CHAT_ORDER_BUCKET_MS * 3 + 5)),
        Some(3)
    );
}

#[test]
fn an_agent_elsewhere_works_only_while_its_turn_runs() {
    let roster: Vec<RosterAgent> = serde_json::from_value(serde_json::json!([
        {"name": "busy", "address": "busy@desk.example.test", "kind": "agent", "where": "other_desktop",
         "status": "active", "liveness": "live", "last_seen_ms": 5, "description": "", "host": null,
         "owner_hash": null, "session_id": null, "is_self": true, "activity": {"turn": "working"}},
        {"name": "waiting", "address": "waiting@desk.example.test", "kind": "agent", "where": "other_desktop",
         "status": "active", "liveness": "live", "last_seen_ms": 5, "description": "", "host": null,
         "owner_hash": null, "session_id": null, "is_self": true, "activity": {"turn": "idle"}},
        {"name": "gone", "address": "gone@desk.example.test", "kind": "agent", "where": "other_desktop",
         "status": "idle", "liveness": "offline", "last_seen_ms": 5, "description": "", "host": null,
         "owner_hash": null, "session_id": null, "is_self": true, "activity": {"turn": "working"}}
    ]))
    .unwrap();
    let agents = chat_elsewhere_agents(&roster, &[], i64::MAX, |_| false);
    let by = |name: &str| {
        agents
            .iter()
            .find(|agent| agent.name() == name)
            .map(|agent| agent.activity)
    };
    assert_eq!(by("busy"), Some(ChatActivity::Working));
    assert_eq!(by("waiting"), Some(ChatActivity::Idle));
    assert_eq!(by("gone"), Some(ChatActivity::Offline));
}

#[test]
fn a_heartbeat_does_not_reorder_agents_elsewhere() {
    // Two idle agents that last changed in the same five minutes, seen a
    // heartbeat apart across a bucket boundary: names decide, either way.
    let roster = |seen_a: i64, seen_b: i64| -> Vec<RosterAgent> {
        serde_json::from_value(serde_json::json!([
            {"name": "beta", "address": "beta@desk.example.test", "kind": "agent", "where": "other_desktop",
             "status": "active", "liveness": "live", "last_seen_ms": seen_b, "description": "", "host": null,
             "owner_hash": null, "session_id": null, "is_self": true,
             "activity": {"turn": "idle", "updated_ms": CHAT_ORDER_BUCKET_MS + 10}},
            {"name": "alpha", "address": "alpha@desk.example.test", "kind": "agent", "where": "other_desktop",
             "status": "active", "liveness": "live", "last_seen_ms": seen_a, "description": "", "host": null,
             "owner_hash": null, "session_id": null, "is_self": true,
             "activity": {"turn": "idle", "updated_ms": CHAT_ORDER_BUCKET_MS + 20}}
        ]))
        .unwrap()
    };
    let names = |roster: Vec<RosterAgent>| -> Vec<String> {
        chat_elsewhere_agents(&roster, &[], i64::MAX, |_| false)
            .iter()
            .map(ChatAgent::name)
            .collect()
    };
    let boundary = CHAT_ORDER_BUCKET_MS * 4;
    assert_eq!(names(roster(boundary - 2, boundary + 2)), ["alpha", "beta"]);
    assert_eq!(names(roster(boundary + 2, boundary - 2)), ["alpha", "beta"]);
}

#[test]
fn a_fast_clock_elsewhere_is_held_to_now() {
    let roster: Vec<RosterAgent> = serde_json::from_value(serde_json::json!([
        {"name": "ahead", "address": "ahead@desk.example.test", "kind": "agent", "where": "other_desktop",
         "status": "active", "liveness": "live", "last_seen_ms": 5, "description": "", "host": null,
         "owner_hash": null, "session_id": null, "is_self": true,
         "activity": {"turn": "idle", "updated_ms": 9_000_000}}
    ]))
    .unwrap();
    let agents = chat_elsewhere_agents(&roster, &[], 1_000, |_| false);
    assert_eq!(agents[0].last_activity_ms, Some(1_000));
}

#[test]
fn a_grok_bot_chat_opens_in_grok_bot() {
    // The link is built from the row's source and session id alone.
    let link = |source: &str, id: &str| chat_app_link_for(source, id).unwrap_or_default();
    assert_eq!(
        link("grok-bot", "2f29a52e-a042-477f-90d5-715b037cce0e"),
        "grokbot://app/v1/agent?id=2f29a52e-a042-477f-90d5-715b037cce0e"
    );
    assert_eq!(link("Grok Bot", "bot_1"), "grokbot://app/v1/agent?id=bot_1");
    assert_eq!(link("grokbot", "not a bot id"), "grokbot://app/v1/open");
    assert_eq!(link("claude", "0123456789"), "");
}
