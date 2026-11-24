//! `sessions.cloud_transcript`: one page of an uploaded session from Cloud's
//! session events (`GET …/reflex/sessions/:id/events`), newest first, as
//! transcript entries the apps already draw.

use super::*;
use crate::contract::cloud_transcript::{CloudTranscriptPage, CloudTranscriptParams};
use agent_relay_node::relay::contract::transcript::LocalTranscriptEntry;

/// The page size when none is asked for, and the most Cloud gives.
const DEFAULT_LIMIT: u32 = 200;
const MAX_LIMIT: u32 = 1000;

pub(super) async fn read(
    core: &Arc<Core>,
    request: CloudTranscriptParams,
) -> Result<CloudTranscriptPage, RpcError> {
    let path = session_path(
        core,
        &SessionOverviewParams {
            session_id: request.session_id.clone(),
        },
    )?;
    let limit = request.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let mut query = format!("{path}/events?order=desc&limit={limit}");
    if let Some(cursor) = request
        .cursor
        .as_deref()
        .filter(|cursor| !cursor.is_empty())
    {
        query.push_str("&cursor=");
        query.push_str(&segment(cursor));
    }
    let response = core
        .plugins
        .cloud
        .request("GET", query, None)
        .await
        .map_err(overview_error)?;
    page(&response)
}

/// Cloud's events page as transcript entries.
pub(super) fn page(response: &Value) -> Result<CloudTranscriptPage, RpcError> {
    let events = response["events"].as_array().ok_or_else(|| {
        RpcError::new(
            ErrorKind::Internal,
            "Cloud returned invalid session events.",
        )
    })?;
    Ok(CloudTranscriptPage {
        entries: events.iter().filter_map(entry).collect(),
        next_cursor: response["nextCursor"]
            .as_str()
            .filter(|cursor| !cursor.is_empty())
            .map(str::to_string),
    })
}

/// What Claude Code adds to a session as the person's turn without the
/// person writing it: a SessionStart hook's context.
const HOOK_CONTEXT: &str = "SessionStart hook additional context:";

/// One event as an entry: who wrote it, and whether it is text, thinking
/// or tool work. An event with nothing to show is left out, and so are
/// copies: Claude's prompt log (`history`) repeats each prompt the
/// transcript has, and a hook's context in the person's turn is not something they wrote.
fn entry(event: &Value) -> Option<LocalTranscriptEntry> {
    let id = event["eventId"].as_str()?.to_string();
    let kind_of = event["type"].as_str().unwrap_or_default();
    if kind_of == "history" {
        return None;
    }
    let persons = matches!(event["actorRole"].as_str(), Some("user" | "human"))
        || (event["actorRole"].is_null() && kind_of == "user");
    if persons
        && event["content"]
            .as_str()
            .is_some_and(|content| content.trim_start().starts_with(HOOK_CONTEXT))
    {
        return None;
    }
    let tool = event["toolName"].as_str().filter(|name| !name.is_empty());
    let kind = match kind_of {
        "tool_use" | "tool_call" => "tool_use",
        "tool_result" => "tool_result",
        "thinking" | "reasoning" => "thinking",
        _ if tool.is_some() => "tool_use",
        _ => "text",
    };
    let role = match (event["actorRole"].as_str(), kind_of) {
        (_, "tool_result") => "tool_result",
        (Some("user" | "human"), _) | (None, "user") => "user",
        (Some("system" | "developer"), _) => "system",
        _ => "assistant",
    };
    let content = event["content"].as_str().unwrap_or_default();
    let text = if content.trim().is_empty() {
        tool?.to_string()
    } else {
        content.to_string()
    };
    let timestamp_ms = event["ts"]
        .as_str()
        .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
        .map_or(0, |ts| ts.timestamp_millis());
    Some(LocalTranscriptEntry {
        id,
        role: role.into(),
        kind: kind.into(),
        text,
        image_path: None,
        timestamp_ms,
        truncated: event["contentTruncated"].as_bool().unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_become_entries_newest_first_with_the_next_cursor() {
        let response = json!({
            "sessionId": "s",
            "nextCursor": "c-2",
            "events": [
                {"eventId": "e3", "type": "assistant", "actorRole": "assistant",
                 "content": "Done.", "ts": "2026-11-22T10:00:02Z", "contentTruncated": false},
                {"eventId": "e2", "type": "tool_use", "actorRole": "assistant",
                 "content": null, "toolName": "Bash", "ts": "2026-11-22T10:00:01Z"},
                {"eventId": "e1", "type": "user", "actorRole": "user",
                 "content": "Fix CI", "ts": "2026-11-22T10:00:00Z"},
                {"eventId": "e0", "type": "assistant", "actorRole": "assistant",
                 "content": "  ", "ts": "2026-11-22T09:59:59Z"}
            ]
        });
        let page = page(&response).unwrap();
        assert_eq!(page.next_cursor.as_deref(), Some("c-2"));
        let shown: Vec<(&str, &str, &str, &str)> = page
            .entries
            .iter()
            .map(|entry| {
                (
                    entry.id.as_str(),
                    entry.role.as_str(),
                    entry.kind.as_str(),
                    entry.text.as_str(),
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                ("e3", "assistant", "text", "Done."),
                ("e2", "assistant", "tool_use", "Bash"),
                ("e1", "user", "text", "Fix CI"),
            ]
        );
        assert!(page.entries[0].timestamp_ms > page.entries[2].timestamp_ms);
    }

    #[test]
    fn prompt_log_copies_and_hook_context_are_left_out() {
        let page = page(&json!({"events": [
            {"eventId": "h1", "type": "history", "actorRole": "user", "content": "ping"},
            {"eventId": "u1", "type": "user", "actorRole": "user",
             "content": "SessionStart hook additional context: This session joins"},
            {"eventId": "u2", "type": "user", "actorRole": "user", "content": "ping"},
        ]}))
        .unwrap();
        let ids: Vec<&str> = page.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["u2"]);
        // An agent quoting the hook's words is still the agent's text.
        let quoted = super::page(&json!({"events": [
            {"eventId": "a1", "type": "assistant", "actorRole": "assistant",
             "content": "SessionStart hook additional context: is what you saw"},
        ]}))
        .unwrap();
        assert_eq!(quoted.entries.len(), 1);
    }

    #[test]
    fn a_page_without_events_is_refused() {
        assert!(page(&json!({"sessionId": "s"})).is_err());
        let end = page(&json!({"events": [], "nextCursor": null})).unwrap();
        assert!(end.entries.is_empty() && end.next_cursor.is_none());
    }
}
