//! Chat: every agent on this computer in one list, each with what it is
//! doing at a glance (Working, Idle, Offline and its session summary), the
//! filters over that list, and the words its conversation is drawn with.
//! The Mac's `ChatModel` matches this word for word.

use super::{source_name, LocalSessionRow, SpawnHandover};
use agent_relay_node::relay::contract::agents::{LocalSession, SessionHolder, SessionRegistration};
use agent_relay_node::relay::contract::transcript::LocalTranscriptEntry;
use agent_relay_node::relay::people::{Liveness, RosterAgent, RosterWhere};
use agent_relay_node::relay::state::WorkState;

/// How recently a session must have written to count as working without
/// a turn state from the core.
pub const CHAT_WORKING_WINDOW_MS: i64 = 120_000;

/// What an agent is doing now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChatActivity {
    /// On a turn, or wrote within the last two minutes.
    Working,
    /// Running and ready for a message.
    Idle,
    /// Its process is not taking messages now; they wait.
    Offline,
    /// Its session ended; its conversation can still be read.
    Ended,
}

impl ChatActivity {
    pub fn label(self) -> &'static str {
        match self {
            ChatActivity::Working => "Working",
            ChatActivity::Idle => "Idle",
            ChatActivity::Offline => "Offline",
            ChatActivity::Ended => "Ended",
        }
    }

    /// The CSS class of its badge and dot.
    pub fn class(self) -> &'static str {
        match self {
            ChatActivity::Working => "working",
            ChatActivity::Idle => "idle",
            ChatActivity::Offline | ChatActivity::Ended => "offline",
        }
    }

    fn rank(self) -> u8 {
        match self {
            ChatActivity::Working => 0,
            ChatActivity::Idle => 1,
            ChatActivity::Offline => 2,
            ChatActivity::Ended => 3,
        }
    }
}

/// The status filter's choices, in the order shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatStatusFilter {
    /// Everything still running: working, idle and offline.
    Running,
    Working,
    Idle,
    /// Ended sessions too: the whole history on this computer.
    All,
}

impl ChatStatusFilter {
    pub const ALL: [ChatStatusFilter; 4] = [
        ChatStatusFilter::Running,
        ChatStatusFilter::Working,
        ChatStatusFilter::Idle,
        ChatStatusFilter::All,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ChatStatusFilter::Running => "Running",
            ChatStatusFilter::Working => "Working",
            ChatStatusFilter::Idle => "Idle",
            ChatStatusFilter::All => "All sessions",
        }
    }

    pub fn admits(self, activity: ChatActivity) -> bool {
        match self {
            ChatStatusFilter::Running => activity != ChatActivity::Ended,
            ChatStatusFilter::Working => activity == ChatActivity::Working,
            ChatStatusFilter::Idle => activity == ChatActivity::Idle,
            ChatStatusFilter::All => true,
        }
    }
}

/// The list's filters: a status, and optionally one harness and one project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatFilters {
    pub status: ChatStatusFilter,
    pub harness: Option<String>,
    pub project: Option<String>,
    pub query: String,
}

impl Default for ChatFilters {
    fn default() -> Self {
        ChatFilters {
            status: ChatStatusFilter::Running,
            harness: None,
            project: None,
            query: String::new(),
        }
    }
}

impl ChatFilters {
    pub fn admits(&self, agent: &ChatAgent) -> bool {
        self.status.admits(agent.activity)
            && self
                .harness
                .as_ref()
                .is_none_or(|harness| *harness == agent.provider())
            && self
                .project
                .as_ref()
                .is_none_or(|project| *project == agent.project())
            && agent.matches(&self.query)
    }
}

/// Where an agent the Chat page lists runs.
#[derive(Clone)]
pub enum ChatPlace {
    /// A session on this computer: its transcript is read here.
    Local {
        row: LocalSessionRow,
        session: LocalSession,
    },
    /// A session in this computer's history the core no longer lists: its
    /// transcript can be read, and it takes no messages.
    History(LocalSessionRow),
    /// One of your agents on another computer or in Cloud, from the
    /// workspace roster: its direct messages are the conversation.
    Elsewhere(RosterAgent),
}

/// One agent the Chat page lists.
#[derive(Clone)]
pub struct ChatAgent {
    pub place: ChatPlace,
    pub activity: ChatActivity,
    /// What it is doing, from its Cloud session summary or, elsewhere, what
    /// its desktop reports; empty until one is read.
    pub summary: String,
    pub last_activity_ms: Option<i64>,
}

pub const CHAT_TITLE: &str = "Chat";
pub const CHAT_AGENT: &str = "Agent";
pub const CHAT_RELAY_MARK: &str = "⇄ Agent Relay";

/// The marker over a message that came through the relay: who it is from
/// when that is known.
pub fn chat_relay_marker(sender: Option<&str>) -> String {
    match sender.filter(|sender| !sender.is_empty()) {
        Some(sender) => format!("{CHAT_RELAY_MARK} · from @{sender}"),
        None => CHAT_RELAY_MARK.to_string(),
    }
}
pub const CHAT_ELSEWHERE: &str = "Elsewhere";
pub const CHAT_ELSEWHERE_SECTION: &str = "Your agents elsewhere";
pub const CHAT_ELSEWHERE_NOTE: &str =
    "Sent as a direct message through Agent Relay. Its replies to you show here.";
pub const CHAT_SUBTITLE: &str = "Every agent on this computer.";
pub const CHAT_SEARCH: &str = "Search agents and summaries";
pub const CHAT_ANY_HARNESS: &str = "Any harness";
pub const CHAT_ANY_PROJECT: &str = "Any project";
pub const CHAT_EMPTY_TITLE: &str = "Chat with any agent on this computer";
pub const CHAT_EMPTY_DETAIL: &str =
    "Pick an agent on the left. Its conversation follows live, and what you send arrives as a message in that session.";
pub const CHAT_NO_AGENTS: &str = "No agents are running on this computer.";
pub const CHAT_NO_MATCH: &str = "No agents match these filters.";
pub const CHAT_NO_MESSAGES: &str = "No messages yet. Say hello.";
pub const CHAT_LOADING: &str = "Loading conversation…";
pub const CHAT_SENDING: &str = "Sending…";
pub const CHAT_JOINING: &str = "Putting this session on the relay…";
pub const CHAT_SEND: &str = "Send";
pub const CHAT_WORKING: &str = "Working…";
pub const CHAT_DELIVERED: &str = "Delivered: your message is in the session.";
pub const CHAT_HIDE_LIST: &str = "Hide agent list";
pub const CHAT_SHOW_LIST: &str = "Show agent list";
pub const CHAT_RECEIVES: &str = "Receives messages";
pub const CHAT_RECEIVES_HELP: &str =
    "Receive messages from other agents. This also uploads the session to Cloud.";
pub const CHAT_BACK: &str = "‹ Back to chat";
pub const CHAT_BACK_HELP: &str = "Back to your agents";

pub const CHAT_NOT_LIVE: &str =
    "This session can't take a message right now. What you send waits until it can.";
pub const CHAT_ENDED: &str = "This session has ended. Its conversation stays readable here.";
pub const CHAT_JOINS_ON_SEND: &str =
    "Sending turns on Receives messages for this session, so it joins the relay under its suggested name.";
pub const CHAT_SEND_NOTE: &str =
    "Sent through Agent Relay; it arrives in the session as a message from you.";

/// What `session` is doing at `now_ms`, given the core's turn state for a
/// spawned agent (`work`) and its last write.
pub fn chat_activity(
    session: &LocalSession,
    work: Option<&WorkState>,
    last_activity_ms: Option<i64>,
    now_ms: i64,
) -> ChatActivity {
    if session.running == Some(false) {
        return ChatActivity::Ended;
    }
    if !session.live {
        return ChatActivity::Offline;
    }
    match work {
        Some(WorkState::Working | WorkState::Waiting) => return ChatActivity::Working,
        Some(WorkState::Idle | WorkState::Completed) => return ChatActivity::Idle,
        _ => {}
    }
    let recent = last_activity_ms.is_some_and(|at| now_ms - at <= CHAT_WORKING_WINDOW_MS);
    if recent {
        ChatActivity::Working
    } else {
        ChatActivity::Idle
    }
}

/// Every session on this computer, history included, working first, then idle, offline and
/// ended, most recently active first within each.
pub fn chat_agents(
    rows: &[LocalSessionRow],
    work: impl Fn(&LocalSession) -> Option<WorkState>,
    summary: impl Fn(&LocalSessionRow) -> Option<String>,
    now_ms: i64,
) -> Vec<ChatAgent> {
    let mut agents: Vec<ChatAgent> = rows
        .iter()
        .map(|row| {
            let Some(session) = row.relay.clone() else {
                // History only: the core no longer lists the session.
                return ChatAgent {
                    summary: summary(row).unwrap_or_default(),
                    last_activity_ms: row
                        .upload
                        .as_ref()
                        .and_then(|upload| upload.last_activity_ms),
                    place: ChatPlace::History(row.clone()),
                    activity: ChatActivity::Ended,
                };
            };
            let last_activity_ms = [
                session.updated_at_ms,
                row.upload
                    .as_ref()
                    .and_then(|upload| upload.last_activity_ms),
            ]
            .into_iter()
            .flatten()
            .max();
            let activity =
                chat_activity(&session, work(&session).as_ref(), last_activity_ms, now_ms);
            ChatAgent {
                summary: summary(row).unwrap_or_default(),
                place: ChatPlace::Local {
                    row: row.clone(),
                    session,
                },
                activity,
                last_activity_ms,
            }
        })
        .collect();
    sort_agents(&mut agents);
    agents
}

/// Working first, then idle, offline and ended, most recently active first
/// within each.
fn sort_agents(agents: &mut [ChatAgent]) {
    agents.sort_by(|a, b| {
        a.activity
            .rank()
            .cmp(&b.activity.rank())
            .then(b.last_activity_ms.cmp(&a.last_activity_ms))
            .then_with(|| a.name().to_lowercase().cmp(&b.name().to_lowercase()))
    });
}

/// Your agents on other computers and in Cloud, from the workspace roster:
/// agents you own that are not registered from this computer.
pub fn chat_elsewhere_agents(roster: &[RosterAgent]) -> Vec<ChatAgent> {
    let mut agents: Vec<ChatAgent> = roster
        .iter()
        .filter(|agent| {
            agent.is_self
                && agent.kind == "agent"
                && agent.where_runs != RosterWhere::ThisComputer
                && agent.address.is_some()
        })
        .map(|agent| ChatAgent {
            summary: agent
                .activity
                .as_ref()
                .and_then(|activity| activity.doing.clone())
                .unwrap_or_default(),
            activity: match agent.liveness {
                Liveness::Live => ChatActivity::Working,
                Liveness::Idle => ChatActivity::Idle,
                Liveness::Offline => ChatActivity::Offline,
            },
            last_activity_ms: agent.last_seen_ms,
            place: ChatPlace::Elsewhere(agent.clone()),
        })
        .collect();
    sort_agents(&mut agents);
    agents
}

/// The harnesses and projects the filters offer, sorted.
pub fn chat_filter_choices(agents: &[ChatAgent]) -> (Vec<String>, Vec<String>) {
    let mut harnesses: Vec<String> = agents.iter().map(ChatAgent::provider).collect();
    let mut projects: Vec<String> = agents
        .iter()
        .map(ChatAgent::project)
        .filter(|project| !project.is_empty())
        .collect();
    for list in [&mut harnesses, &mut projects] {
        list.sort_by_key(|value| value.to_lowercase());
        list.dedup();
    }
    (harnesses, projects)
}

/// A user turn as the person wrote it: a relay message's attribution,
/// reply hints and closing line are dropped. Returns the sender's name
/// when the turn came through the relay.
pub fn chat_user_text(text: &str) -> (Option<String>, String) {
    let trimmed = text.trim_start();
    if !["[via Agent Relay", "[Agent Relay native input"]
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
    {
        return (None, text.to_string());
    }
    let mut lines = trimmed.lines();
    let header = lines.next().unwrap_or_default();
    let sender = header
        .split("from @")
        .nth(1)
        .and_then(|rest| rest.split([' ', ']']).next())
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    let mut body = Vec::new();
    let mut started = false;
    for line in lines {
        let hint = line.starts_with('[') && line.trim_end().ends_with(']');
        if line.starts_with("[end of Agent Relay message") {
            break;
        }
        if !started && (hint || line.trim().is_empty()) {
            continue;
        }
        started = true;
        body.push(line);
    }
    (sender, body.join("\n").trim_end().to_string())
}

/// Whether the agent is still on its turn after `entry`: anything but its
/// own written reply (the person's message, tool work) leaves it working.
pub fn chat_turn_open(entry: &LocalTranscriptEntry) -> bool {
    !(entry.role == "assistant" && entry.kind == "text")
}

mod agent;

#[cfg(test)]
mod tests;
