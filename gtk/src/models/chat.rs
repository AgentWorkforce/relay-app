//! Chat: every agent on this computer in one list, each with what it is
//! doing at a glance (Working, Idle, Offline and its session summary), the
//! filters over that list, and the words its conversation is drawn with.
//! The Mac's `ChatModel` matches this word for word.

use super::{
    cloud_session_url, session_key, source_name, uploaded_session_url, LocalSessionRow,
    SpawnHandover,
};
use agent_relay_node::relay::activity::AgentTurn;
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
pub const CHAT_OPEN_FALLBACK: &str = "Open";
pub const CHAT_AGENT: &str = "Agent";
pub const CHAT_RELAY_MARK: &str = "⇄ Agent Relay";

/// The marker over a relay message the agent sent: to whom, when known.
pub fn chat_relay_sent_marker(recipient: Option<&str>) -> String {
    match recipient.filter(|to| !to.is_empty()) {
        Some(channel) if channel.starts_with('#') => format!("{CHAT_RELAY_MARK} · to {channel}"),
        Some(to) => format!("{CHAT_RELAY_MARK} · to @{to}"),
        None => format!("{CHAT_RELAY_MARK} · sent"),
    }
}

/// The marker over a message that came through the relay: who it is from
/// when that is known.
pub fn chat_relay_marker(sender: Option<&str>) -> String {
    match sender.filter(|sender| !sender.is_empty()) {
        Some(sender) => format!("{CHAT_RELAY_MARK} · from @{sender}"),
        None => CHAT_RELAY_MARK.to_string(),
    }
}
pub const CHAT_ELSEWHERE: &str = "Elsewhere";
/// The tooltip on an agent's unread count.
pub fn chat_unread_label(count: u64) -> String {
    match count {
        1 => "1 unread message".to_string(),
        n => format!("{n} unread messages"),
    }
}

/// A notification's link back to a conversation: `<prefix><peer name>`.
pub const CHAT_LINK: &str = "agent-relay-chat:";
/// An agent elsewhere someone else in the workspace owns.
pub const CHAT_NOT_YOURS: &str = "Not yours";
pub const CHAT_CLOUD: &str = "Cloud";
pub const CHAT_ELSEWHERE_SECTION: &str = "Agents elsewhere";
/// Above a long conversation's newest messages.
pub const CHAT_EARLIER: &str = "Show earlier messages";
/// The arrow over a conversation scrolled away from its end.
pub const CHAT_JUMP_LATEST: &str = "Jump to the latest messages";
/// The head of the Chat list: agents with messages you have not opened.
pub const CHAT_UNREAD_SECTION: &str = "New messages";
/// The rest of the list, under the agents with new messages.
pub const CHAT_ALL_SECTION: &str = "All agents";
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
pub const CHAT_ATTACH: &str = "Attach files";
pub const CHAT_ATTACH_TITLE: &str = "Attach files to the message";
pub const CHAT_REMOVE_ATTACHMENT: &str = "Remove attachment";
pub const CHAT_FILES_NEED_CORE: &str =
    "Sent without its files: attaching them needs the updated background service.";

/// A chip's label for an attached file: its name and size, as in Pear.
pub fn chat_attachment_label(path: &std::path::Path) -> String {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    match std::fs::metadata(path).map(|meta| meta.len()) {
        Ok(bytes) if bytes >= 1_048_576 => format!("{name} · {:.1} MB", bytes as f64 / 1_048_576.0),
        Ok(bytes) if bytes >= 1024 => format!("{name} · {:.1} KB", bytes as f64 / 1024.0),
        Ok(bytes) => format!("{name} · {bytes} B"),
        Err(_) => name,
    }
}
pub const CHAT_WORKING: &str = "Working…";
pub const CHAT_DELIVERED: &str = "Delivered: your message is in the session.";
pub const CHAT_HIDE_LIST: &str = "Hide agent list";
pub const CHAT_SHOW_LIST: &str = "Show agent list";
pub const CHAT_NAME: &str = "Agent name";
pub const CHAT_MESSAGING: &str = "Messaging";
pub const CHAT_DETAILS: &str = "Details";
pub const CHAT_VIEW_WEB: &str = "View on web";
pub const CHAT_VIEW_WEB_HELP: &str = "Open this session in Cloud in your browser";
pub const CHAT_NAME_HELP: &str =
    "The name other agents see on Agent Relay. Press Return to rename; your session title stays the same.";
pub const CHAT_UPLOAD: &str = "Upload";
pub const CHAT_UPLOAD_HELP: &str =
    "Upload this session to Cloud. Turning it off asks before deleting the cloud copy.";
pub const CHAT_RECEIVES: &str = "Receives messages";
pub const CHAT_RECEIVES_HELP: &str =
    "Receive messages from other agents. This also uploads the session to Cloud.";

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

/// The tag on an agent elsewhere in the sessions list, where a local
/// session shows this computer's details: where it runs.
pub fn chat_elsewhere_tag(agent: &ChatAgent) -> String {
    match &agent.place {
        ChatPlace::Elsewhere(remote) => format!(
            "{} · {}",
            if remote.is_self {
                CHAT_ELSEWHERE
            } else {
                CHAT_NOT_YOURS
            },
            remote.host.clone().unwrap_or_else(|| CHAT_CLOUD.into())
        ),
        _ => String::new(),
    }
}

/// The link that opens the session of `row` in its own app instead of a
/// conversation here, for a harness that is an app of its own: a Grok Bot
/// chat opens in Grok Bot on that bot. Grok Bot keeps one session per bot,
/// under the bot's id, which its `agent` route opens; a thread link with
/// that id is refused and leaves Grok Bot behind.
pub fn chat_app_link(row: &LocalSessionRow) -> Option<String> {
    chat_app_link_for(row.source(), &row.session_id())
}

/// [`chat_app_link`] for a session of `source` with id `id`.
fn chat_app_link_for(source: &str, id: &str) -> Option<String> {
    let source = source.to_lowercase().replace(['-', '_', ' '], "");
    if source != "grokbot" {
        return None;
    }
    // Grok Bot's agent ids: letters, digits, `_` and `-`, at most 128.
    let agent = !id.is_empty()
        && id.len() <= 128
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    Some(if agent {
        format!("grokbot://app/v1/agent?id={id}")
    } else {
        "grokbot://app/v1/open".to_string()
    })
}

/// Where `agent`'s session is on the web, at `site`: an uploaded session
/// here, or the session an agent elsewhere uploads.
pub fn chat_web_url(agent: &ChatAgent, site: &str) -> Option<String> {
    match &agent.place {
        ChatPlace::Elsewhere(remote) => remote
            .session_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .and_then(|id| cloud_session_url(site, id)),
        _ => agent
            .row()
            .and_then(|row| row.upload.as_ref())
            .and_then(|upload| uploaded_session_url(site, upload)),
    }
}

/// The Chat page's key for the agent of a sessions-table `row`.
pub fn chat_key(row: &LocalSessionRow) -> String {
    match &row.relay {
        Some(session) => session.target.key(),
        None => format!("history:{}", row.key()),
    }
}

/// How coarsely recency orders agents. Recency is when an agent last did
/// something (a session event, a turn change), never a heartbeat; the
/// bucket keeps a burst of events from reshuffling neighbours.
pub const CHAT_ORDER_BUCKET_MS: i64 = 300_000;

/// `ms` rounded down to its ordering bucket.
pub fn chat_order_bucket(ms: Option<i64>) -> Option<i64> {
    ms.map(|ms| ms.div_euclid(CHAT_ORDER_BUCKET_MS))
}

/// Working first, then idle, offline and ended; within each, the most
/// recently active five-minute bucket first, then by name, so a heartbeat
/// does not move a row.
fn sort_agents(agents: &mut [ChatAgent]) {
    agents.sort_by(|a, b| {
        a.activity
            .rank()
            .cmp(&b.activity.rank())
            .then(chat_order_bucket(b.last_activity_ms).cmp(&chat_order_bucket(a.last_activity_ms)))
            .then_with(|| a.name().to_lowercase().cmp(&b.name().to_lowercase()))
    });
}

/// Whether the sessions list already has a row for the roster `agent`: its
/// uploaded session, a session registered under its name, or a session
/// titled with its name. Such an agent is that row, not another one.
fn listed(agent: &RosterAgent, rows: &[LocalSessionRow]) -> bool {
    rows.iter().any(|row| {
        agent.session_id.as_deref() == Some(row.session_id().as_str())
            || row
                .relay
                .as_ref()
                .and_then(|session| session.registration.as_ref())
                .is_some_and(|registration| {
                    registration.name == agent.name
                        || registration.alias.as_deref() == Some(agent.name.as_str())
                })
            || row.title() == agent.name
    })
}

/// What an agent elsewhere is doing: the turn its desktop reports when it
/// reports one (working only while a turn runs), else its liveness.
fn elsewhere_activity(agent: &RosterAgent) -> ChatActivity {
    if agent.liveness == Liveness::Offline {
        return ChatActivity::Offline;
    }
    match agent
        .activity
        .as_ref()
        .and_then(|activity| activity.turn.as_ref())
    {
        Some(AgentTurn::Working) => ChatActivity::Working,
        Some(AgentTurn::Idle) => ChatActivity::Idle,
        None if agent.liveness == Liveness::Live => ChatActivity::Working,
        None => ChatActivity::Idle,
    }
}

/// Your agents on other computers and in Cloud, from the workspace roster:
/// agents you own that are not registered from this computer and that no
/// row of the sessions list (`rows`) already shows.
///
/// Other people's agents in the workspace are many: one is listed only when
/// `others` admits it (a search that matches it, or the one shown).
pub fn chat_elsewhere_agents(
    roster: &[RosterAgent],
    rows: &[LocalSessionRow],
    now_ms: i64,
    others: impl Fn(&ChatAgent) -> bool,
) -> Vec<ChatAgent> {
    let mut agents: Vec<ChatAgent> = roster
        .iter()
        .filter(|agent| !listed(agent, rows))
        .filter(|agent| {
            agent.kind == "agent"
                && agent.where_runs != RosterWhere::ThisComputer
                && agent.address.is_some()
        })
        .map(|agent| ChatAgent {
            summary: agent
                .activity
                .as_ref()
                .and_then(|activity| activity.doing.clone())
                .unwrap_or_default(),
            activity: elsewhere_activity(agent),
            // When its turn or what it is doing last changed, not when it
            // was last seen: a heartbeat must not reorder the list. Its
            // computer stamps that time, so a fast clock is held to now.
            last_activity_ms: agent
                .activity
                .as_ref()
                .and_then(|activity| activity.updated_ms)
                .map(|at| at.min(now_ms)),
            place: ChatPlace::Elsewhere(agent.clone()),
        })
        .filter(|agent| agent.is_yours() || others(agent))
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
mod inbox;
mod order;
mod pasted;
mod relay_lines;
pub use agent::chat_row_names;
pub use inbox::{ChatInbox, ChatNotice, Unread};
pub use order::{chat_sort_elsewhere, chat_sorts_before};
pub use pasted::{chat_attached_images, chat_attachment_lines, chat_pasted_images};
pub use relay_lines::{chat_relay_incoming, chat_relay_outgoing, RelayIncoming, RelayOutgoing};

#[cfg(test)]
mod tests;
