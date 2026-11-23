//! Chat: every agent on this computer in one list, each with what it is
//! doing at a glance (Working, Idle, Offline and its session summary), the
//! filters over that list, and the words its conversation is drawn with.
//! The Mac's `ChatModel` matches this word for word.

use super::{source_name, LocalSessionRow};
use agent_relay_node::relay::contract::agents::LocalSession;
use agent_relay_node::relay::contract::transcript::LocalTranscriptEntry;
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
            ChatStatusFilter::All => "All, with ended",
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
                .is_none_or(|project| *project == agent.row.project())
            && agent.matches(&self.query)
    }
}

/// One agent the Chat page lists.
#[derive(Clone)]
pub struct ChatAgent {
    pub row: LocalSessionRow,
    pub session: LocalSession,
    pub activity: ChatActivity,
    /// What it is doing, from its Cloud session summary; empty until one
    /// is read.
    pub summary: String,
    pub last_activity_ms: Option<i64>,
}

impl ChatAgent {
    pub fn key(&self) -> String {
        self.session.target.key()
    }

    /// Its relay name, else the session's title.
    pub fn name(&self) -> String {
        match &self.session.registration {
            Some(registration) => registration
                .alias
                .clone()
                .unwrap_or_else(|| registration.name.clone()),
            None => self.row.title(),
        }
    }

    pub fn title(&self) -> String {
        self.row.title()
    }

    pub fn provider(&self) -> String {
        source_name(&self.row.icon_source())
    }

    /// Whether a message can reach it now.
    pub fn live(&self) -> bool {
        self.session.live
    }

    /// The exact address it is messaged at, once it is on the relay.
    pub fn address(&self) -> Option<String> {
        self.session
            .registration
            .as_ref()
            .map(|registration| registration.address.clone())
    }

    /// What it is doing at a glance: its summary, else its title when that
    /// is not already its name.
    pub fn glance(&self) -> String {
        if !self.summary.trim().is_empty() {
            return self.summary.trim().to_string();
        }
        let title = self.title();
        if title != self.name() {
            title
        } else {
            String::new()
        }
    }

    /// The line under its name: `Claude Code · relay-app`.
    pub fn meta(&self) -> String {
        let mut parts = vec![self.provider()];
        let project = self.row.project();
        if !project.is_empty() {
            parts.push(project);
        }
        parts.join(" · ")
    }

    /// The composer's placeholder.
    pub fn placeholder(&self) -> String {
        match &self.session.registration {
            Some(_) => format!("Message @{}", self.name()),
            None => format!("Message {}", self.title()),
        }
    }

    /// The note under the composer before anything is sent.
    pub fn composer_note(&self) -> String {
        // The core's reason a message would wait (a question the session
        // is asking, a busy turn) comes first: it is why a queue stalls.
        if let Some(wait) = self
            .session
            .delivery_wait
            .as_deref()
            .filter(|wait| !wait.trim().is_empty())
        {
            return wait.to_string();
        }
        match self.activity {
            ChatActivity::Ended => return CHAT_ENDED.into(),
            ChatActivity::Offline => return CHAT_NOT_LIVE.into(),
            _ => {}
        }
        match &self.session.registration {
            None => CHAT_JOINS_ON_SEND.into(),
            Some(_) => CHAT_SEND_NOTE.into(),
        }
    }

    /// Whether the composer can send to it.
    pub fn can_send(&self) -> bool {
        self.activity != ChatActivity::Ended
    }

    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        query.is_empty()
            || [self.name(), self.title(), self.meta(), self.summary.clone()]
                .iter()
                .any(|field| field.to_lowercase().contains(&query))
    }
}

pub const CHAT_TITLE: &str = "Chat";
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
pub const CHAT_OPEN_SESSION: &str = "Open in Sessions";
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

/// Every session on this computer, working first, then idle, offline and
/// ended, most recently active first within each.
pub fn chat_agents(
    rows: &[LocalSessionRow],
    work: impl Fn(&LocalSession) -> Option<WorkState>,
    summary: impl Fn(&LocalSessionRow) -> Option<String>,
    now_ms: i64,
) -> Vec<ChatAgent> {
    let mut agents: Vec<ChatAgent> = rows
        .iter()
        .filter_map(|row| {
            let session = row.relay.clone()?;
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
            Some(ChatAgent {
                summary: summary(row).unwrap_or_default(),
                row: row.clone(),
                session,
                activity,
                last_activity_ms,
            })
        })
        .collect();
    agents.sort_by(|a, b| {
        a.activity
            .rank()
            .cmp(&b.activity.rank())
            .then(b.last_activity_ms.cmp(&a.last_activity_ms))
            .then_with(|| a.name().to_lowercase().cmp(&b.name().to_lowercase()))
    });
    agents
}

/// The harnesses and projects the filters offer, sorted.
pub fn chat_filter_choices(agents: &[ChatAgent]) -> (Vec<String>, Vec<String>) {
    let mut harnesses: Vec<String> = agents.iter().map(ChatAgent::provider).collect();
    let mut projects: Vec<String> = agents
        .iter()
        .map(|agent| agent.row.project())
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

#[cfg(test)]
mod tests;
