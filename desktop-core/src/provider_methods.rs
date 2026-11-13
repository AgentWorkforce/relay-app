//! Bridge methods over Cloud's provider routes and session summaries: Cursor
//! Cloud agents, Replicas, Capy and `sessions.overview.*`.

mod capy;
mod cursor_cloud;
mod replicas;

use crate::contract::method;
use agent_relay_node::relay::cloud_session::{segment, CloudError};
use agent_relay_node::relay::contract::answer;
use agent_relay_node::relay::contract::directory::SessionOverviewParams;
use agent_relay_node::relay::daemon::Core;
use agent_relay_node::relay::protocol::{ErrorKind, RpcError};
use serde_json::{json, Value};
use std::sync::Arc;

/// The Cloud route of `session_id` in `workspace_id`.
pub fn session_path_for(workspace_id: &str, session_id: &str) -> Result<String, RpcError> {
    let session_id = session_id.trim();
    if session_id.is_empty() || session_id.len() > 512 {
        return Err(RpcError::invalid_params(
            "Give a session id of at most 512 characters.",
        ));
    }
    Ok(format!(
        "/workspaces/{}/reflex/sessions/{}",
        segment(workspace_id),
        segment(session_id)
    ))
}

fn session_path(core: &Arc<Core>, request: &SessionOverviewParams) -> Result<String, RpcError> {
    let account = core.plugins.cloud.account().ok_or_else(|| {
        RpcError::new(
            ErrorKind::NotSignedIn,
            "Sign in to Agent Relay Cloud to view session summaries.",
        )
    })?;
    session_path_for(&account.workspace_id, &request.session_id)
}

/// Past `session_path` the account is connected, so a refusal means the
/// session the core calls Cloud with expired, not that nobody signed in.
fn overview_error(error: CloudError) -> RpcError {
    match error {
        CloudError::SignedOut | CloudError::Rejected => RpcError::new(
            ErrorKind::NotSignedIn,
            "Your Agent Relay sign-in expired. Sign in again to view session summaries.",
        ),
        other => RpcError::cloud(&other),
    }
}

pub(super) fn decode_cloud<T: serde::de::DeserializeOwned>(
    value: Value,
    what: &str,
) -> Result<T, RpcError> {
    serde_json::from_value(value).map_err(|_| {
        RpcError::new(
            ErrorKind::Internal,
            format!("Cloud returned invalid {what}."),
        )
    })
}

/// The bridge accepts 1 MiB frames. Cloud events can include large tool
/// payloads, while the shared goal parser needs only these three fields.
fn project_goal_events(response: Value) -> Result<Value, RpcError> {
    let session_id = response["sessionId"].as_str().ok_or_else(|| {
        RpcError::new(
            ErrorKind::Internal,
            "Cloud returned invalid session events.",
        )
    })?;
    let events = response["events"].as_array().ok_or_else(|| {
        RpcError::new(
            ErrorKind::Internal,
            "Cloud returned invalid session events.",
        )
    })?;
    let events: Vec<Value> = events
        .iter()
        .take(50)
        .map(|event| {
            let content = event["content"]
                .as_str()
                .map(|text| text.chars().take(1_000).collect::<String>());
            json!({"kind": event["kind"], "actorRole": event["actorRole"], "content": content})
        })
        .collect();
    Ok(json!({"sessionId": session_id, "events": events}))
}

#[cfg(test)]
mod overview_tests {
    use super::{project_goal_events, session_path_for};
    use serde_json::{json, Value};

    #[test]
    fn session_id_stays_in_one_path_segment() {
        assert_eq!(
            session_path_for("workspace", "session/other").unwrap(),
            "/workspaces/workspace/reflex/sessions/session%2Fother"
        );
        assert!(session_path_for("workspace", " ").is_err());
        assert!(session_path_for("workspace", &"a".repeat(513)).is_err());
    }

    #[test]
    fn history_projection_bounds_the_bridge_response() {
        let events: Vec<Value> = (0..60)
            .map(|_| json!({"kind":"history", "actorRole":"user", "content":"x".repeat(10_000), "private":"unused"}))
            .collect();
        let value = project_goal_events(json!({"sessionId":"session", "events":events})).unwrap();
        assert_eq!(value["events"].as_array().unwrap().len(), 50);
        assert_eq!(value["events"][0]["content"].as_str().unwrap().len(), 1_000);
        assert!(value["events"][0].get("private").is_none());
    }
}

/// Answers `method`, or `None` when this module has no such method.
pub async fn call(core: &Arc<Core>, method: &str, raw: &Value) -> Option<Result<Value, RpcError>> {
    if let Some(result) = cursor_cloud::call(core, method, raw).await {
        return Some(result);
    }
    if let Some(result) = replicas::call(core, method, raw).await {
        return Some(result);
    }
    if let Some(result) = capy::call(core, method, raw).await {
        return Some(result);
    }
    Some(match method {
        method::SessionsOverviewGet::NAME => {
            answer::<method::SessionsOverviewGet, _>(raw.clone(), |request| async move {
                let path = format!("{}/analysis", session_path(core, &request)?);
                core.plugins
                    .cloud
                    .request("GET", path, None)
                    .await
                    .map_err(overview_error)
            })
            .await
        }
        method::SessionsOverviewRequest::NAME => {
            answer::<method::SessionsOverviewRequest, _>(raw.clone(), |request| async move {
                let path = format!("{}/analysis", session_path(core, &request)?);
                core.plugins
                    .cloud
                    .request("POST", path, None)
                    .await
                    .map_err(overview_error)
            })
            .await
        }
        method::SessionsOverviewEvents::NAME => {
            answer::<method::SessionsOverviewEvents, _>(raw.clone(), |request| async move {
                let path = format!("{}/events", session_path(core, &request)?);
                let response = core
                    .plugins
                    .cloud
                    .request("GET", path, None)
                    .await
                    .map_err(overview_error)?;
                project_goal_events(response)
            })
            .await
        }
        _ => return None,
    })
}
