//! Where a conversation's entries come from: a session's transcript on
//! this computer (or a headless spawned agent's own output), and for an
//! agent elsewhere its uploaded session from Cloud merged by time with
//! your direct messages with it.

use super::super::app::App;
use crate::models::{ChatAgent, ChatPlace, SpawnHandover};
use agent_relay_desktop_core::contract::cloud_transcript::CloudTranscriptParams;
use agent_relay_desktop_core::contract::method as desktop_method;
use agent_relay_node::relay::contract::directory::HistoryParams;
use agent_relay_node::relay::contract::method;
use agent_relay_node::relay::contract::transcript::{
    LocalTranscriptCursor, LocalTranscriptEntry, LocalTranscriptPage, LocalTranscriptParams,
};
use agent_relay_node::relay::messaging::DirectMessage;
use agent_relay_node::relay::people::RosterAgent;
use std::collections::HashSet;

/// Cloud records a tool call twice, as the call and as assistant text with
/// the same words: the text copy goes.
fn without_tool_echoes(entries: Vec<LocalTranscriptEntry>) -> Vec<LocalTranscriptEntry> {
    let calls: HashSet<String> = entries
        .iter()
        .filter(|entry| entry.kind == "tool_use")
        .map(|entry| entry.text.clone())
        .collect();
    entries
        .into_iter()
        .filter(|entry| {
            entry.kind != "text" || entry.role == "user" || !calls.contains(&entry.text)
        })
        .collect()
}

/// The uploaded session of an agent elsewhere, when the core reads Cloud
/// transcripts and the roster names one.
fn cloud_session(app: &App, remote: &RosterAgent) -> Option<String> {
    let session = remote.session_id.clone().filter(|id| !id.is_empty())?;
    app.bridge()
        .offers(desktop_method::SessionsCloudTranscript::NAME)
        .then_some(session)
}

/// The newest events of an uploaded session from Cloud, in reading order: a
/// whole recent stretch when it opens, the latest few while following.
fn read_cloud(
    app: &App,
    session_id: String,
    endpoint: String,
    following: bool,
    done: impl FnOnce(&App, Result<LocalTranscriptPage, crate::bridge::CoreError>) + 'static,
) {
    app.view_call::<desktop_method::SessionsCloudTranscript>(
        CloudTranscriptParams {
            session_id,
            cursor: None,
            limit: Some(if following { 50 } else { 500 }),
        },
        move |app, result| {
            // Cloud's copy can lag hours behind (or be empty): your direct
            // messages with it fill in, merged by time; one in both is
            // drawn once.
            let cloud = result
                .map(|page| without_tool_echoes(page.entries.into_iter().rev().collect()))
                .unwrap_or_default();
            read_direct(app, endpoint, move |app, direct| {
                let mut entries = cloud;
                let failed = match direct {
                    Ok(page) => {
                        entries.extend(page.entries);
                        None
                    }
                    Err(error) => Some(error),
                };
                entries.sort_by_key(|entry| entry.timestamp_ms);
                let result = match failed {
                    Some(error) if entries.is_empty() && !following => Err(error),
                    _ => Ok(LocalTranscriptPage {
                        entries,
                        next_cursor: None,
                        follow_cursor: None,
                    }),
                };
                done(app, result)
            });
        },
    );
}

/// The id prefix of a direct message's entry, telling it from a
/// transcript's.
pub(super) const DIRECT: &str = "dm:";

/// A direct message as a conversation entry: yours as the person's turn,
/// the agent's as its reply.
fn direct_entry(message: &DirectMessage) -> LocalTranscriptEntry {
    LocalTranscriptEntry {
        id: format!("{DIRECT}{}", message.id),
        role: if message.mine { "user" } else { "assistant" }.into(),
        kind: "text".into(),
        text: message.text.clone(),
        image_path: None,
        timestamp_ms: message
            .created_at
            .as_deref()
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
            .map_or(0, |at| at.timestamp_millis()),
        truncated: false,
    }
}

/// Where a read of `agent`'s conversation comes from: a headless spawned
/// agent's own output, the session's transcript, or, for an agent
/// elsewhere, your direct messages with it.
pub(super) fn read_page(
    app: &App,
    agent: &ChatAgent,
    cursor: Option<LocalTranscriptCursor>,
    following: bool,
    done: impl FnOnce(&App, Result<LocalTranscriptPage, crate::bridge::CoreError>) + 'static,
) {
    let (row, session) = match &agent.place {
        ChatPlace::Local { row, session } => (row, Some(session)),
        ChatPlace::History(row) => (row, None),
        ChatPlace::Elsewhere(remote) => {
            // Its uploaded session, from Cloud, when the core can read it.
            let endpoint = agent.address().unwrap_or_else(|| agent.name());
            if let Some(session_id) = cloud_session(app, remote) {
                return read_cloud(app, session_id, endpoint, following, done);
            }
            read_direct(app, endpoint, done);
            return;
        }
    };
    let params = LocalTranscriptParams {
        source: row.source().into(),
        session_id: row.session_id(),
        cursor,
        following,
    };
    app.transcript_page(SpawnHandover::transcript_agent(session), params, done);
}

/// Your direct messages with an agent elsewhere, as its conversation.
fn read_direct(
    app: &App,
    endpoint: String,
    done: impl FnOnce(&App, Result<LocalTranscriptPage, crate::bridge::CoreError>) + 'static,
) {
    app.view_call::<method::MessagesHistory>(
        HistoryParams {
            endpoint,
            before: None,
        },
        move |app, result| {
            done(
                app,
                result.map(|history| LocalTranscriptPage {
                    entries: history.messages.iter().map(direct_entry).collect(),
                    next_cursor: None,
                    follow_cursor: None,
                }),
            )
        },
    );
}

/// The source a conversation's entries are drawn for: the session's, or
/// for an agent elsewhere its name, which its replies are marked with.
pub(super) fn entry_source(agent: &ChatAgent) -> String {
    match &agent.place {
        ChatPlace::Local { row, .. } | ChatPlace::History(row) => row.source().to_string(),
        ChatPlace::Elsewhere(remote) => remote.name.clone(),
    }
}
