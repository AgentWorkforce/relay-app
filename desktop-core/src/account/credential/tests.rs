//! `CloudAccess::device_token` against an in-process fake Cloud that only
//! answers the session refresh; no real tokens, daemons or providers.
use crate::account::Account;
use crate::history::uploader::{ProcessRunner, Uploader};
use agent_relay_node::relay::cloud_session::CloudAccess;
use agent_relay_node::relay::daemon::{Config, Core, Plugins};
use agent_relay_node::relay::delivery::DeliveryTiming;
use agent_relay_node::relay::paths::CorePaths;
use agent_relay_node::relay::push::PushTiming;
use agent_relay_node::relay::store::{CloudSession, CloudWorkspace};
use agent_relay_node::relay::workspace::Secret;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

const FRESH: &str = "2099-01-01T00:00:00Z";
const EXPIRED: &str = "2000-01-01T00:00:00Z";

/// Answers `POST …/auth/token/refresh` with `status` (a renewed pair on
/// 200) and counts the refreshes; anything else gets a 404.
struct FakeCloud {
    site: String,
    status: Arc<AtomicU16>,
    refreshes: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeCloud {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeCloud {
    async fn start() -> FakeCloud {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let site = format!("http://{}", listener.local_addr().unwrap());
        let status = Arc::new(AtomicU16::new(200));
        let refreshes = Arc::new(AtomicUsize::new(0));
        let (answer, count) = (status.clone(), refreshes.clone());
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (answer, count) = (answer.clone(), count.clone());
                tokio::spawn(async move {
                    let (read, mut write) = stream.into_split();
                    let mut reader = BufReader::new(read);
                    let mut line = String::new();
                    reader.read_line(&mut line).await.unwrap();
                    let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                    let mut length = 0;
                    loop {
                        let mut header = String::new();
                        reader.read_line(&mut header).await.unwrap();
                        if header.trim().is_empty() {
                            break;
                        }
                        if let Some((name, value)) = header.trim().split_once(':') {
                            if name.eq_ignore_ascii_case("content-length") {
                                length = value.trim().parse().unwrap();
                            }
                        }
                    }
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).await.unwrap();
                    let (status, body) = if path.ends_with("/auth/token/refresh") {
                        count.fetch_add(1, Ordering::SeqCst);
                        let status = answer.load(Ordering::SeqCst);
                        let body = match status {
                            200 => json!({"accessToken": "renewed-access",
                                "refreshToken": "renewed-refresh",
                                "accessTokenExpiresAt": FRESH}),
                            _ => json!({"error": "refused"}),
                        };
                        (status, body)
                    } else {
                        (404, json!({}))
                    };
                    let body = body.to_string();
                    let response = format!(
                        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = write.write_all(response.as_bytes()).await;
                });
            }
        });
        FakeCloud {
            site,
            status,
            refreshes,
            task,
        }
    }

    fn refreshes(&self) -> usize {
        self.refreshes.load(Ordering::SeqCst)
    }
}

struct Harness {
    cloud: FakeCloud,
    core: Arc<Core>,
    account: Arc<Account>,
    _temp: tempfile::TempDir,
}

impl Harness {
    /// An account attached to a core whose workspace is the account's;
    /// signed in with `(access expiry, refresh token)`, or signed out.
    async fn new(session: Option<(&str, Option<&str>)>) -> Harness {
        Harness::with_env(session, BTreeMap::new()).await
    }

    /// `new`, with `env` as the account's and the core's environment.
    async fn with_env(
        session: Option<(&str, Option<&str>)>,
        env: BTreeMap<String, String>,
    ) -> Harness {
        let cloud = FakeCloud::start().await;
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_path_buf();
        let app_data_dir = home.join("app");
        let account = Arc::new(
            Account::new(
                &env,
                &app_data_dir,
                Arc::new(Uploader::new(Arc::new(ProcessRunner::new(
                    home.join("unused-probe"),
                )))),
            )
            .with_site(&cloud.site)
            .unwrap(),
        );
        let config = Config {
            paths: CorePaths::for_socket(home.join("core.sock")).unwrap(),
            home,
            app_data_dir,
            env,
            version: "test".into(),
            host_name: "Test".into(),
            rediscover_every: Duration::from_secs(3600),
            push_timing: PushTiming::default(),
            delivery_timing: DeliveryTiming::default(),
        };
        let plugins = Plugins {
            account: Some(account.clone().into_plugin()),
            cloud: account.clone() as Arc<dyn CloudAccess>,
            ..Plugins::default()
        };
        let (core, _) = Core::with_plugins(config, plugins);
        let site = cloud.site.clone();
        core.store
            .update(|s| {
                s.cloud = Some(CloudWorkspace {
                    key: Secret::new(""),
                    base_url: site.clone(),
                    workspace_id: Some("rw_gate".into()),
                    account_workspace_id: Some("0f8fad5b-d9cb-469f-a165-70867728950e".into()),
                    name: None,
                    relayfile_workspace_id: None,
                    gate_url: None,
                });
                s.cloud_session = session.map(|(expires, refresh)| CloudSession {
                    access_token: Secret::new("kept-access"),
                    refresh_token: refresh.map(Secret::new),
                    site_url: site.clone(),
                    access_token_expires_at: Some(expires.into()),
                    refresh_token_expires_at: None,
                });
            })
            .unwrap();
        Harness {
            cloud,
            core,
            account,
            _temp: temp,
        }
    }

    async fn device_token(&self) -> Option<String> {
        tokio::time::timeout(Duration::from_secs(10), self.account.device_token())
            .await
            .expect("device_token must not hang")
            .map(|token| token.expose().to_string())
    }

    fn needs_sign_in(&self) -> bool {
        self.account.status().needs_sign_in
    }

    fn kept_access(&self) -> Option<String> {
        self.core.store.read(|s| {
            s.cloud_session
                .as_ref()
                .map(|session| session.access_token.expose().to_string())
        })
    }
}

#[tokio::test]
async fn signed_in_with_a_fresh_session_gives_its_access_token_unrefreshed() {
    let h = Harness::new(Some((FRESH, Some("kept-refresh")))).await;
    assert_eq!(h.device_token().await.as_deref(), Some("kept-access"));
    assert_eq!(h.cloud.refreshes(), 0);
}

#[tokio::test]
async fn an_expired_access_token_is_renewed_and_the_renewed_one_kept() {
    let h = Harness::new(Some((EXPIRED, Some("kept-refresh")))).await;
    assert_eq!(h.device_token().await.as_deref(), Some("renewed-access"));
    assert_eq!(h.cloud.refreshes(), 1);
    assert_eq!(h.kept_access().as_deref(), Some("renewed-access"));
    // The renewed token is fresh now: no second refresh.
    assert_eq!(h.device_token().await.as_deref(), Some("renewed-access"));
    assert_eq!(h.cloud.refreshes(), 1);
}

#[tokio::test]
async fn the_environment_token_is_given_before_the_kept_session() {
    let env = BTreeMap::from([("CLOUD_API_ACCESS_TOKEN".into(), "env-token".into())]);
    let h = Harness::with_env(Some((EXPIRED, Some("kept-refresh"))), env).await;
    assert_eq!(h.device_token().await.as_deref(), Some("env-token"));
    assert_eq!(h.cloud.refreshes(), 0);
}

#[tokio::test]
async fn signed_out_gives_none() {
    let h = Harness::new(None).await;
    assert_eq!(h.device_token().await, None);
    assert_eq!(h.cloud.refreshes(), 0);
}

#[tokio::test]
async fn no_account_workspace_gives_none() {
    let h = Harness::new(Some((FRESH, Some("kept-refresh")))).await;
    h.core.store.update(|s| s.cloud = None).unwrap();
    assert_eq!(h.device_token().await, None);
}

#[tokio::test]
async fn a_refused_refresh_gives_none_never_the_stale_token() {
    let h = Harness::new(Some((EXPIRED, Some("kept-refresh")))).await;
    h.cloud.status.store(401, Ordering::SeqCst);
    assert_eq!(h.device_token().await, None);
    assert_eq!(h.cloud.refreshes(), 1);
    assert!(h.needs_sign_in());
}

#[tokio::test]
async fn a_refresh_cloud_could_not_answer_gives_none() {
    let h = Harness::new(Some((EXPIRED, Some("kept-refresh")))).await;
    h.cloud.status.store(503, Ordering::SeqCst);
    assert_eq!(h.device_token().await, None);
    assert_eq!(h.cloud.refreshes(), 1);
    assert!(!h.needs_sign_in());
    // Kept for a later try, which succeeds once Cloud answers again.
    assert_eq!(h.kept_access().as_deref(), Some("kept-access"));
    h.cloud.status.store(200, Ordering::SeqCst);
    assert_eq!(h.device_token().await.as_deref(), Some("renewed-access"));
}

#[tokio::test]
async fn an_expired_session_with_nothing_to_refresh_with_gives_none() {
    let h = Harness::new(Some((EXPIRED, None))).await;
    assert_eq!(h.device_token().await, None);
    assert_eq!(h.cloud.refreshes(), 0);
}
