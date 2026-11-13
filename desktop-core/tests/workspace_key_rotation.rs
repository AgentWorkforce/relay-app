//! The hosted relay key rotation against an in-process fake Cloud: the
//! request names the workspace's own route, and a workspace id that is not
//! one path segment is refused before anything is sent.

#[path = "support"]
mod support {
    pub mod fake_cloud;
}

use agent_relay_desktop_core::history::cloud::rotate_relay_workspace_key;
use serde_json::json;
use support::fake_cloud::{FakeCloud, WORKSPACE};

async fn rotate(api_url: &str, bearer: &str, workspace: &str) -> anyhow::Result<serde_json::Value> {
    let (api_url, bearer, workspace) = (
        api_url.to_string(),
        bearer.to_string(),
        workspace.to_string(),
    );
    tokio::task::spawn_blocking(move || {
        rotate_relay_workspace_key(&api_url, &bearer, &workspace, 60_000)
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn rotation_posts_to_the_workspace_route_and_returns_the_new_generation() {
    let cloud = FakeCloud::start().await;
    let (access, _) = cloud.issue();
    let api_url = format!("{}/cloud", cloud.site_url);

    let answer = rotate(&api_url, &access, WORKSPACE).await.unwrap();

    assert_eq!(answer, json!({"generation": 2, "graceMs": 60_000}));
    let path = format!("/cloud/api/v1/workspaces/{WORKSPACE}/relay-key/rotate");
    let requests = cloud.requests(&path);
    assert_eq!(
        requests.len(),
        1,
        "{:?}",
        cloud.state.lock().unwrap().requests
    );
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].bearer.as_deref(), Some(access.as_str()));
    assert_eq!(requests[0].body, json!({"graceMs": 60_000}));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invalid_workspace_is_refused_before_any_request() {
    let cloud = FakeCloud::start().await;
    let (access, _) = cloud.issue();
    let api_url = format!("{}/cloud", cloud.site_url);

    for workspace in [
        "",
        "/",
        "..",
        "../../admin/x",
        "ws/other",
        "ws x",
        &"a".repeat(129),
    ] {
        let error = rotate(&api_url, &access, workspace)
            .await
            .expect_err(&format!("{workspace:?} was accepted"));
        assert_eq!(error.to_string(), "invalid workspace identifier");
    }
    assert!(
        cloud.state.lock().unwrap().requests.is_empty(),
        "a refused workspace reached Cloud: {:?}",
        cloud.state.lock().unwrap().requests
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_rotation_names_the_cause_without_the_response_body() {
    let cloud = FakeCloud::start().await;
    let (access, _) = cloud.issue();
    let api_url = format!("{}/cloud", cloud.site_url);

    for (status, cause) in [
        (403, "Rotation needs owner or admin access."),
        (502, "Cloud could not finish the rotation at the relay."),
    ] {
        cloud.state.lock().unwrap().rotate_status = status;
        let message = rotate(&api_url, &access, WORKSPACE)
            .await
            .unwrap_err()
            .to_string();
        assert!(message.contains(cause), "{status}: {message}");
        assert!(
            !message.contains("secret-cloud-detail"),
            "{status}: {message}"
        );
    }
}
