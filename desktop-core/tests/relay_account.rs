//! Signing in, the kept Cloud session, the account workspace, sign-out and
//! the uploader, end to end over the bridge socket, against an in-process
//! fake Cloud and a fake probe. No broker, PTY runtime or agent harness is
//! started, and nothing reaches a real Cloud, Relaycast or PostHog.

#[path = "support"]
mod support {
    pub mod bridge;
    pub mod fake_cloud;
}

use agent_relay_desktop_core::account::{Account, Flow};
use agent_relay_node::relay::cloud_session::CloudAccess;
use agent_relay_node::relay::daemon::{self, BoundSocket, Config, Core, Plugins};
use agent_relay_node::relay::delivery::DeliveryTiming;
use agent_relay_node::relay::paths::{self, CorePaths};
use agent_relay_node::relay::push::PushTiming;
use agent_relay_node::relay::sessions::DiscoveredSession;
use agent_relay_node::relay::setup::{DirectDelivery, Setup};
use agent_relay_node::relay::state::AccountAction;
use agent_relay_node::relay::store::{SessionKind, Target};
use agent_relay_desktop_core::telemetry::{
    PostHogProject, Telemetry, TelemetryConfig, TelemetrySender,
};
use agent_relay_desktop_core::history::uploader::{InstallRun, ProbeRunner, Uploader, UploaderError};
use agent_relay_node::relay::workspace::Secret;
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use support::bridge::{eventually, Client};
use support::fake_cloud::{FakeCloud, ACCOUNT_KEY, USER, WORKSPACE};
use tokio::sync::mpsc;

// Setup preferences stay in this test's temporary core; no user settings change.
struct SetupDirect;
impl DirectDelivery for SetupDirect {
    fn delivers_directly(&self) -> bool {
        true
    }
    fn managed_value(&self) -> Option<String> {
        None
    }
    fn user_choice(&self) -> Option<String> {
        Some("accept".into())
    }
    fn set_direct_delivery(&self, _: bool) -> Result<(), String> {
        Ok(())
    }
}

/// The probe's commands, answered from memory.
#[derive(Default)]
struct FakeProbe {
    state: Mutex<ProbeState>,
}

#[derive(Default)]
struct ProbeState {
    installs: Vec<Value>,
    status: Value,
    /// Every command: its arguments and the bearer it was given.
    calls: Vec<(Vec<String>, Option<String>)>,
    /// `cloud install` fails with this sentence.
    install_error: Option<String>,
    /// `disconnect` fails while set.
    disconnect_fails: bool,
    cancelled: u32,
    /// `cloud install` connects this computer, then runs until cancelled.
    hold_install: bool,
    hold_before_connection: bool,
    held_events: Vec<mpsc::UnboundedSender<Value>>,
    /// `cloud install` connects this computer, then fails with this.
    fail_after_connected: Option<String>,
    /// `status` fails, as on an install whose setup did not finish.
    status_fails: bool,
    /// Held installs' completions, never sent.
    held: Vec<tokio::sync::oneshot::Sender<Result<(), UploaderError>>>,
}

impl FakeProbe {
    fn calls(&self, command: &str) -> Vec<(Vec<String>, Option<String>)> {
        let state = self.state.lock().unwrap();
        state
            .calls
            .iter()
            .filter(|(args, _)| args.first().map(String::as_str) == Some(command))
            .cloned()
            .collect()
    }

    fn connected(site: &str) -> Value {
        json!({"directory": "/tmp/x", "site_url": site, "account_id": USER, "workspace_id": WORKSPACE, "org_id": ORG,
               "account_email": "ada@example.com", "account_name": "Ada", "sharing_mode": "new",
               "running": true, "paused": false, "upgrade_required": false})
    }
}

impl ProbeRunner for FakeProbe {
    fn status(
        &self,
        _: Option<std::path::PathBuf>,
        _: agent_relay_desktop_core::history::uploader::HistoryReader,
    ) -> BoxFuture<'_, Result<Value, UploaderError>> {
        self.run(vec!["status".into()], None)
    }
    fn run(
        &self,
        args: Vec<String>,
        _: Option<Duration>,
    ) -> BoxFuture<'_, Result<Value, UploaderError>> {
        let mut state = self.state.lock().unwrap();
        state.calls.push((args.clone(), None));
        let answer = match args[0].as_str() {
            "installs" => Ok(json!({ "installs": state.installs })),
            "status" if state.status_fails => Err(UploaderError::new("No status yet.")),
            "status" => Ok(state.status.clone()),
            "start" => Ok(json!({"running": true, "started": true})),
            "disconnect" if state.disconnect_fails => {
                Err(UploaderError::new("This workspace’s uploader is busy."))
            }
            "disconnect" => {
                state.installs.clear();
                Ok(json!({"disconnected": true}))
            }
            "sharing" => Ok(
                json!({"sharing_mode": args[args.iter().position(|a| a == "--mode").unwrap() + 1], "regenerated": false}),
            ),
            "sessions" if args[1] == "list" => Ok(
                json!({"sharing_mode": "new", "sessions": [{"source": "codex", "session_id": "a", "title": "a",
                    "cwd": null, "git_branch": null, "first_activity_ms": null, "last_activity_ms": null,
                    "is_agent": false, "included": false, "status": "not_shared"}]}),
            ),
            "sessions" if args[1] == "read" => Ok(json!({"entries": [], "next_cursor": null})),
            "sessions" if args[1] == "preview" => {
                Ok(json!({"sharing_mode": "selected", "sessions": []}))
            }
            "sessions" => Ok(json!({"included": [], "regenerated": false})),
            "reclaim" => Ok(json!({})),
            "pause" | "resume" => Ok(json!({"paused": args[0] == "pause"})),
            _ => Err(UploaderError::new("unknown command")),
        };
        Box::pin(async move { answer })
    }

    fn install(
        &self,
        args: Vec<String>,
        bearer: Option<Secret>,
    ) -> Result<InstallRun, UploaderError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push((
            args.clone(),
            bearer.as_ref().map(|b| b.expose().to_string()),
        ));
        let (events, receiver) = mpsc::unbounded_channel();
        let (done, finished) = tokio::sync::oneshot::channel();
        let site = args[args.iter().position(|a| a == "--site-url").unwrap() + 1].clone();
        // The install has the mode setup asked for, and the setup reports
        // uploads it withheld for the operator policy.
        let mode = [
            ("--include-existing", "all"),
            ("--new-sessions-only", "new"),
            ("--selected-sessions-only", "selected"),
        ]
        .into_iter()
        .find(|(flag, _)| args.iter().any(|arg| arg == flag))
        .map_or("new", |(_, mode)| mode);
        let connected = |site: &str| {
            let mut install = FakeProbe::connected(site);
            install["sharing_mode"] = json!(mode);
            install
        };
        if args.iter().any(|arg| arg == "--withhold-uploads") {
            events.send(json!({"event": "withheld"})).unwrap();
        }
        match state.install_error.clone() {
            Some(message) => {
                let _ = done.send(Err(UploaderError::new(message)));
            }
            None if state.fail_after_connected.is_some() => {
                for event in [
                    json!({"event": "authenticated", "account_id": USER, "workspace_id": WORKSPACE}),
                    json!({"event": "connected", "account_id": USER, "workspace_id": WORKSPACE}),
                ] {
                    events.send(event).unwrap();
                }
                state.installs.push(connected(&site));
                let message = state.fail_after_connected.clone().unwrap();
                let _ = done.send(Err(UploaderError::new(message)));
            }
            None if state.hold_before_connection => {
                events.send(json!({"event":"authenticated", "account_id":USER, "workspace_id":WORKSPACE})).unwrap();
                state.held_events.push(events);
                state.held.push(done);
            }
            None if state.hold_install => {
                for event in [
                    json!({"event": "authenticated", "account_id": USER, "workspace_id": WORKSPACE}),
                    json!({"event": "connected", "account_id": USER, "workspace_id": WORKSPACE}),
                ] {
                    events.send(event).unwrap();
                }
                state.installs.push(connected(&site));
                state.held.push(done);
            }
            None => {
                for event in [
                    json!({"event": "authenticated", "account_id": USER, "workspace_id": WORKSPACE, "account_email": "ada@example.com"}),
                    json!({"event": "connected", "account_id": USER, "workspace_id": WORKSPACE}),
                    json!({"event": "progress", "progress": {"phase": "scanning", "source": "codex", "processedFiles": 1, "totalFiles": 2, "sessionsCaptured": 1}}),
                    json!({"event": "ready"}),
                ] {
                    events.send(event).unwrap();
                }
                state.installs.push(connected(&site));
                let _ = done.send(Ok(()));
            }
        }
        Ok(InstallRun {
            events: receiver,
            finished,
            cancel: Box::new(|| {}),
        })
    }

    fn cancel_all(&self) -> BoxFuture<'_, Result<(), UploaderError>> {
        self.state.lock().unwrap().cancelled += 1;
        Box::pin(async { Ok(()) })
    }
}

const ORG: &str = "org-1";

/// PostHog in memory: every event, accepted.
#[derive(Default)]
struct FakeAnalytics {
    events: Mutex<Vec<Value>>,
    delay: Mutex<Option<Duration>>,
}

impl TelemetrySender for FakeAnalytics {
    fn send(&self, batch: Vec<Value>) -> Result<(), String> {
        if let Some(delay) = *self.delay.lock().unwrap() {
            std::thread::sleep(delay);
        }
        self.events.lock().unwrap().extend(batch);
        Ok(())
    }
}

struct Harness {
    cloud: FakeCloud,
    analytics: Arc<FakeAnalytics>,
    telemetry: Arc<Telemetry>,
    probe: Arc<FakeProbe>,
    account: Arc<Account>,
    core: Arc<Core>,
    _temp: tempfile::TempDir,
    home: PathBuf,
    socket: PathBuf,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Harness {
    async fn start() -> Harness {
        Harness::start_with(|_, _| {}).await
    }

    async fn start_with(prepare: impl FnOnce(&FakeCloud, &Harnessing)) -> Harness {
        let cloud = FakeCloud::start().await;
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_path_buf();
        let probe = Arc::new(FakeProbe::default());
        let setup = Harnessing {
            home: home.clone(),
            probe: probe.clone(),
            supervise_every: Mutex::new(None),
            cloud_token: Mutex::new(None),
            env: Mutex::new(BTreeMap::new()),
            site_analytics: Mutex::new(false),
        };
        prepare(&cloud, &setup);
        let socket = home.join("run/core.sock");
        let mut env = BTreeMap::new();
        // Any Relaycast verification fails fast on a closed local port.
        env.insert(
            "RELAYCAST_URL".to_string(),
            cloud
                .state
                .lock()
                .unwrap()
                .relaycast_url
                .clone()
                .unwrap_or_else(|| "http://127.0.0.1:9".to_string()),
        );
        if let Some(token) = setup.cloud_token.lock().unwrap().clone() {
            env.insert("CLOUD_API_ACCESS_TOKEN".into(), token);
        }
        env.extend(setup.env.lock().unwrap().clone());
        let app_data_dir = home.join("app-data");
        let config = Config {
            paths: CorePaths::for_socket(socket.clone()).unwrap(),
            home: home.clone(),
            app_data_dir: app_data_dir.clone(),
            env: env.clone(),
            version: "0.0.0-test".into(),
            host_name: "Test Desk.local".into(),
            rediscover_every: Duration::from_secs(3600),
            push_timing: PushTiming::default(),
            delivery_timing: DeliveryTiming::default(),
        };
        paths::socket_directory(config.paths.directory()).unwrap();
        let bound = BoundSocket::bind(&config.paths).unwrap();
        let mut account = Account::new(&env, &app_data_dir, Arc::new(Uploader::new(probe.clone())))
            .with_site(&cloud.site_url)
            .unwrap();
        if let Some(every) = *setup.supervise_every.lock().unwrap() {
            account.supervise_every = every;
        }
        let analytics = Arc::new(FakeAnalytics::default());
        let telemetry = Telemetry::new(
            TelemetryConfig {
                app_data_dir: app_data_dir.clone(),
                home: home.clone(),
                env: env.clone(),
                version: "0.0.0-test".into(),
                project: if *setup.site_analytics.lock().unwrap() {
                    PostHogProject::site_collector("https://site.example")
                } else {
                    PostHogProject::new("phc_test", "https://analytics.example.com")
                },
                session: Default::default(),
                clock: Arc::new(std::time::SystemTime::now),
                ticks: Arc::new(std::time::Instant::now),
            },
            analytics.clone(),
        );
        account.telemetry = Some(telemetry.clone());
        let account = Arc::new(account);
        let local = agent_relay_node::relay::local::LocalConfig::from_env(
            &|key| env.get(key).cloned(),
            &home,
        );
        let installer = agent_relay_desktop_core::mcp_settings::Installer::new(
            home.clone(),
            local.codex,
            local.claude,
            local.opencode,
        )
        .with_choices_path(app_data_dir.join("tool-integrations.json"));
        let _ = account.mcp_config.set(Arc::new(installer));
        let plugins = Plugins {
            cloud: account.clone() as Arc<dyn CloudAccess>,
            extensions: vec![Arc::new(agent_relay_desktop_core::account::plugin::AccountMethods(account.clone()))],
            account: Some(account.clone().into_plugin()),
            ..Plugins::default()
        };
        let (core, signals) = Core::with_plugins(config, plugins);
        let task = tokio::spawn(daemon::run(core.clone(), bound, signals));
        Harness {
            cloud,
            analytics,
            telemetry,
            probe,
            account,
            core,
            _temp: temp,
            home,
            socket,
            task,
        }
    }

    async fn client(&self) -> Client {
        Client::connect(&self.socket).await
    }

    async fn setup_sign_in(
        &self,
        params: Value,
    ) -> Result<Value, agent_relay_node::relay::outbound::OutboundError> {
        self.setup_request("POST /setup/sign-in", params).await
    }

    async fn setup_request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, agent_relay_node::relay::outbound::OutboundError> {
        Arc::new(Setup::new(self.core.clone(), Arc::new(SetupDirect)))
            .handle(
                &DiscoveredSession {
                    target: Target {
                        kind: SessionKind::ClaudeTerminal,
                        id: "setup-test".into(),
                    },
                    title: "Setup test".into(),
                    directory: None,
                    updated_at_ms: None,
                    host: None,
                    live: true,
                    holder: None,
                },
                method,
                params.as_object().unwrap().clone(),
            )
            .await
    }

    /// What reached PostHog, once everything queued is sent.
    fn analytics(&self) -> Vec<Value> {
        assert!(self.telemetry.flush(Duration::from_secs(5)));
        self.analytics.events.lock().unwrap().clone()
    }

    fn account_state(&self) -> String {
        self.core.status().account.state_name()
    }

    fn store(&self) -> Value {
        serde_json::from_slice(
            &std::fs::read(self.home.join("app-data/injectable-agents.json")).unwrap_or_default(),
        )
        .unwrap_or(Value::Null)
    }

    async fn stop(self) {
        self.core.request_shutdown();
        let _ = self.task.await;
    }
}

/// What a test can arrange before the daemon starts.
struct Harnessing {
    home: PathBuf,
    probe: Arc<FakeProbe>,
    supervise_every: Mutex<Option<Duration>>,
    cloud_token: Mutex<Option<String>>,
    /// More of the core's environment.
    env: Mutex<BTreeMap<String, String>>,
    /// Telemetry is a site build's, for its analytics gateway.
    site_analytics: Mutex<bool>,
}

trait StateName {
    fn state_name(&self) -> String;
}

impl StateName for agent_relay_node::relay::state::AccountStatus {
    fn state_name(&self) -> String {
        serde_json::to_value(self.state)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }
}

#[tokio::test]
async fn a_device_sign_in_connects_the_workspace_and_the_uploader_with_one_approval() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    client
        .ok("subscribe", json!({"topics": ["sign_in", "status"]}))
        .await;
    let started = client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    assert_eq!(started["state"], "pending_approval");
    assert_eq!(started["phase"], "waiting_for_device_approval");
    assert_eq!(started["user_code"], "ABCD-EFGH");
    assert_eq!(
        started["verification_url_complete"],
        format!("{}/device?code=ABCD-EFGH", harness.cloud.site_url)
    );
    assert!(started.get("device_code").is_none());
    assert!(!started.to_string().contains("secret-device-code"));

    eventually("signed in", || harness.account_state() == "signed_in").await;

    // The probe connected with the approved session: no second approval.
    let installs = harness.probe.calls("cloud");
    assert_eq!(installs.len(), 1);
    let (args, bearer) = &installs[0];
    assert!(args.contains(&"--new-sessions-only".to_string()));
    // A reconnect keeps the install's saved sharing choice.
    assert!(args.contains(&"--keep-saved-sharing".to_string()));
    assert!(args.windows(2).any(|w| w == ["--workspace", WORKSPACE]));
    assert_eq!(bearer.as_deref(), Some("access-1"));

    // The session is kept, renewable, in the store; the account's Relaycast
    // workspace is resolved into it and discovered.
    let store = harness.store();
    assert_eq!(store["cloudSession"]["accessToken"], "access-1");
    assert_eq!(store["cloudSession"]["refreshToken"], "refresh-1");
    assert_eq!(store["cloud"]["key"], ACCOUNT_KEY);
    assert_eq!(store["cloud"]["accountWorkspaceID"], WORKSPACE);
    let status = client.ok("status.get", json!({})).await;
    assert_eq!(status["workspace"]["state"], "found");
    assert_eq!(status["account"]["workspace_id"], WORKSPACE);
    assert_eq!(status["account"]["workspace_name"], "Acme");
    assert_eq!(status["account"]["person"]["id"], USER);
    assert_eq!(status["account"]["person"]["email"], "ada@example.com");
    assert!(!status.to_string().contains("access-1"));

    // The CloudAccess the rest of the core uses is this account.
    let account = harness.account.account().unwrap();
    assert_eq!(account.workspace_id, WORKSPACE);
    assert_eq!(account.user_id.as_deref(), Some(USER));

    // Progress and the approval link were published on `sign_in`.
    client.ok("status.get", json!({})).await;
    let sign_in: Vec<&Value> = client
        .events
        .iter()
        .filter(|e| e["params"]["topic"] == "sign_in")
        .map(|e| &e["params"]["data"])
        .collect();
    assert!(sign_in.iter().any(|d| d["user_code"] == "ABCD-EFGH"));
    assert!(sign_in
        .iter()
        .any(|d| d["progress"]["sessions_captured"] == 1));
    assert!(sign_in.iter().any(|d| d["state"] == "signed_in"));
    harness.stop().await;
}

/// The account signs in to its fake Cloud, not the production site this
/// test binary was built for, so the first production sign-in's launch at
/// login does not run: it would write a real autostart entry (on Windows,
/// a Run value on the developer's own account).
#[tokio::test]
async fn a_sign_in_against_a_fake_cloud_leaves_launch_at_login_alone() {
    let harness = Harness::start().await;
    assert!(harness.account.launch_at_login_after_sign_in);
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    // The state turns signed-in before the attempt's remaining work, which
    // ends with the launch-at-login gate, so wait for all of it.
    eventually("sign-in finished", || harness.account.attempt_finished()).await;
    assert!(!harness
        .home
        .join("app-data/launch-at-login-configured")
        .exists());
    assert!(!harness
        .home
        .join(".config/autostart/agent-relay.desktop")
        .exists());
    harness.stop().await;
}

#[tokio::test]
async fn browser_continuation_preserves_the_requested_workspace() {
    for entry in ["default", "browser", "rejected_saved"] {
        let harness = Harness::start_with(move |cloud, setup| {
            if entry == "rejected_saved" {
                keep_live_session(cloud, setup);
                cloud.expire_access();
                cloud.state.lock().unwrap().refresh_tokens.clear();
            }
        })
        .await;
        let mut client = harness.client().await;
        let mut params = json!({"sharing": "selected", "workspace": WORKSPACE});
        if entry == "browser" {
            params["flow"] = json!("browser");
        }
        client.ok("account.sign_in", params).await;
        eventually("workspace browser link", || {
            harness.account_state() == "pending_approval"
        })
        .await;
        let report = client.ok("account.status", json!({})).await;
        let url = url::Url::parse(report["verification_url_complete"].as_str().unwrap()).unwrap();
        let login = if entry == "browser" {
            let next = url
                .query_pairs()
                .find(|(key, _)| key == "next")
                .unwrap()
                .1
                .into_owned();
            url.join(&next).unwrap()
        } else {
            url
        };
        assert_eq!(
            login
                .query_pairs()
                .find(|(key, _)| key == "workspace")
                .map(|(_, value)| value.into_owned()),
            Some(WORKSPACE.to_owned()),
            "{entry} must preserve the requested workspace through the browser handoff"
        );
        client.ok("account.sign_in.cancel", json!({})).await;
        harness.stop().await;
    }
}

#[tokio::test]
async fn a_browser_sign_in_returns_to_the_loopback_receiver_and_exchanges_with_pkce() {
    for entry in ["browser", "google", "microsoft", "default", "setup"] {
        let harness = Harness::start().await;
        let mut client = harness.client().await;
        let started =
            if entry == "setup" {
                let response = harness
                    .setup_sign_in(json!({"flow":"browser"}))
                    .await
                    .unwrap();
                assert!(["preparing", "pending_approval"]
                    .contains(&response["status"].as_str().unwrap()));
                eventually("browser link", || {
                    harness.account_state() == "pending_approval"
                })
                .await;
                harness
                    .setup_request("GET /setup/status", json!({}))
                    .await
                    .unwrap()
            } else {
                let params = if matches!(entry, "google" | "microsoft") {
                    json!({"sharing": "all", "flow": "browser", "provider": entry})
                } else if entry == "browser" {
                    json!({"sharing": "all", "flow": "browser"})
                } else {
                    json!({"sharing": "all"})
                };
                let response = client.ok("account.sign_in", params).await;
                assert!(["preparing", "pending_approval"]
                    .contains(&response["state"].as_str().unwrap()));
                eventually("browser link", || {
                    harness.account_state() == "pending_approval"
                })
                .await;
                let pending = client.ok("account.status", json!({})).await;
                assert_eq!(pending["phase"], "waiting_for_browser");
                pending
            };
        assert!(
            started["user_code"].is_null(),
            "{entry} must not request a device code"
        );
        assert!(harness
            .cloud
            .requests("/cloud/api/v1/auth/device/start")
            .is_empty());
        let link = started["verification_url_complete"]
            .as_str()
            .unwrap()
            .to_string();
        let path = if entry == "microsoft" {
            "/cloud/api/auth/microsoft/start?next="
        } else if matches!(entry, "browser" | "google") {
            "/cloud/api/auth/google/start?next="
        } else {
            "/cloud/api/v1/cli/login?"
        };
        assert!(link.starts_with(&format!("{}{path}", harness.cloud.site_url)));

        // What the browser does after Google: Cloud redirects to the receiver.
        let decoded = link.replace("%3D", "=").replace("%26", "&");
        let redirect = decoded
            .split("redirect_uri=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap()
            .to_string();
        let challenge = decoded
            .split("code_challenge=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap()
            .to_string();
        let (handoff, _) = harness.cloud.issue();
        let forged = reqwest_get(&format!(
            "{}?access_token=x&api_url=https://evil.example/cloud",
            redirect
        ))
        .await;
        assert!(forged.starts_with("HTTP/1.1 400"));
        let accepted = reqwest_get(&format!(
            "{redirect}?access_token={handoff}&api_url={}/cloud",
            harness.cloud.site_url
        ))
        .await;
        assert!(accepted.starts_with("HTTP/1.1 200"), "{accepted}");

        eventually("signed in", || harness.account_state() == "signed_in").await;
        let exchange = harness.cloud.requests("/cloud/api/v1/auth/desktop/session");
        assert_eq!(exchange.len(), 1);
        assert_eq!(exchange[0].bearer.as_deref(), Some(handoff.as_str()));
        let verifier = exchange[0].body["codeVerifier"].as_str().unwrap();
        let digest = base64_url(&Sha256::digest(verifier.as_bytes()));
        assert_eq!(digest, challenge);
        // The renewable session from the exchange is the one kept and used.
        let store = harness.store();
        assert_eq!(store["cloudSession"]["accessToken"], "access-2");
        assert_eq!(store["cloudSession"]["refreshToken"], "refresh-2");
        let (args, bearer) = &harness.probe.calls("cloud")[0];
        assert_eq!(
            args.contains(&"--include-existing".to_string()),
            entry != "setup"
        );
        assert_eq!(bearer.as_deref(), Some("access-2"));
        harness.stop().await;
    }
}

fn base64_url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// One GET over a raw socket, the way a browser reaches the receiver.
async fn reqwest_get(url: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let rest = url.strip_prefix("http://").unwrap();
    let (authority, path) = rest.split_at(rest.find('/').unwrap());
    let mut stream = tokio::net::TcpStream::connect(authority).await.unwrap();
    stream
        .write_all(format!("GET {path} HTTP/1.1\r\nHost: {authority}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer).await;
    answer
}

#[tokio::test]
async fn choosing_another_account_bypasses_a_valid_saved_login() {
    let harness = Harness::start_with(|cloud, setup| {
        let (access, refresh) = cloud.issue();
        std::fs::create_dir_all(setup.home.join("app-data")).unwrap();
        std::fs::write(
            setup.home.join("app-data/cloud-session.json"),
            json!({"apiUrl": format!("{}/cloud", cloud.site_url), "accessToken": access,
                   "accessTokenExpiresAt": "2099-01-01T00:00:00Z", "refreshToken": refresh})
            .to_string(),
        )
        .unwrap();
    })
    .await;
    let mut client = harness.client().await;
    let started = client
        .ok(
            "account.sign_in",
            json!({"flow":"browser", "sharing":"selected"}),
        )
        .await;
    assert_eq!(started["state"], "pending_approval");
    assert!(started["verification_url_complete"]
        .as_str()
        .unwrap()
        .starts_with(&format!(
            "{}/cloud/api/auth/google/start?next=",
            harness.cloud.site_url
        )));
    assert!(harness.probe.calls("cloud").is_empty());
    client.ok("account.sign_in.cancel", json!({})).await;
    harness.stop().await;
}

#[tokio::test]
async fn revoked_saved_credentials_continue_to_browser_in_the_same_attempt() {
    let harness = Harness::start_with(|cloud, setup| {
        let (access, refresh) = cloud.issue();
        std::fs::create_dir_all(setup.home.join("app-data")).unwrap();
        std::fs::write(
            setup.home.join("app-data/cloud-session.json"),
            json!({"apiUrl":format!("{}/cloud",cloud.site_url),"accessToken":access,
                "accessTokenExpiresAt":"2099-01-01T00:00:00Z","refreshToken":refresh})
            .to_string(),
        )
        .unwrap();
        cloud.expire_access();
        cloud.state.lock().unwrap().refresh_tokens.clear();
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"selected"}))
        .await;
    eventually("browser after saved rejection", || {
        harness.account_state() == "pending_approval"
    })
    .await;
    let report = client.ok("account.status", json!({})).await;
    let link = report["verification_url_complete"].as_str().unwrap();
    assert!(link.starts_with(&format!(
        "{}/cloud/api/v1/cli/login?",
        harness.cloud.site_url
    )));
    let redirect = link
        .split("redirect_uri=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap();
    let (handoff, _) = harness.cloud.issue();
    assert!(reqwest_get(&format!(
        "{redirect}?access_token={handoff}&api_url={}/cloud",
        harness.cloud.site_url
    ))
    .await
    .starts_with("HTTP/1.1 200"));
    eventually("signed in", || harness.account_state() == "signed_in").await;
    assert_eq!(harness.store()["cloudSession"]["refreshToken"], "refresh-3");
    assert_eq!(
        harness
            .cloud
            .requests("/cloud/api/v1/auth/token/refresh")
            .len(),
        1
    );
    harness.stop().await;
}

#[tokio::test]
async fn setup_status_exposes_a_browser_link_that_arrives_after_a_saved_login_is_rejected() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let response_gate = gate.clone();
    let harness = Harness::start_with(move |cloud, setup| {
        keep_live_session(cloud, setup);
        cloud.expire_access();
        cloud.state.lock().unwrap().refresh_tokens.clear();
        cloud.state.lock().unwrap().whoami_gate = Some(response_gate);
    })
    .await;
    let started = tokio::time::timeout(
        Duration::from_secs(1),
        harness.setup_sign_in(json!({"flow":"browser"})),
    )
    .await
    .expect("setup must return before Cloud answers")
    .unwrap();
    assert_eq!(started["status"], "preparing");
    gate.notify_one();
    eventually("browser continuation", || {
        harness.account_state() == "pending_approval"
    })
    .await;
    let status = harness
        .setup_request("GET /setup/status", json!({}))
        .await
        .unwrap();
    assert_eq!(status["sign_in"], "pending_approval");
    assert_eq!(status["sign_in_phase"], "waiting_for_browser");
    assert!(status["verification_url_complete"]
        .as_str()
        .unwrap()
        .starts_with(&format!(
            "{}/cloud/api/v1/cli/login?",
            harness.cloud.site_url
        )));
    assert!(status["user_code"].is_null());
    let mut client = harness.client().await;
    client.ok("account.sign_in.cancel", json!({})).await;
    harness.stop().await;
}

#[tokio::test]
async fn saved_session_refresh_does_not_block_sign_in_or_cancellation() {
    for (cancel, fresh) in [(false, false), (true, false), (true, true)] {
        let gate = Arc::new(tokio::sync::Notify::new());
        let response_gate = gate.clone();
        let harness = Harness::start_with(move |cloud, setup| {
            let (access, refresh) = cloud.issue();
            cloud.state.lock().unwrap().refresh_gate = Some(response_gate);
            if fresh { cloud.expire_access(); }
            std::fs::create_dir_all(setup.home.join("app-data")).unwrap();
            std::fs::write(
                setup.home.join("app-data/cloud-session.json"),
                json!({"apiUrl":format!("{}/cloud",cloud.site_url),"accessToken":access,
                    "accessTokenExpiresAt": if fresh { "2099-01-01T00:00:00Z" } else { "2000-01-01T00:00:00Z" },"refreshToken":refresh})
                .to_string(),
            )
            .unwrap();
        })
        .await;
        let mut client = harness.client().await;
        let started = tokio::time::timeout(
            Duration::from_secs(1),
            client.ok("account.sign_in", json!({"sharing":"selected"})),
        )
        .await
        .expect("sign-in must not wait for saved-session refresh");
        assert_eq!(started["state"], "preparing");
        eventually("refresh request", || {
            !harness
                .cloud
                .requests("/cloud/api/v1/auth/token/refresh")
                .is_empty()
        })
        .await;
        if cancel {
            client.ok("account.sign_in.cancel", json!({})).await;
            assert_eq!(harness.account_state(), "signed_out");
            assert_eq!(harness.store()["cloudSession"]["refreshToken"], "refresh-1");
        }
        gate.notify_one();
        eventually("rotated token retained", || {
            harness.store()["cloudSession"]["refreshToken"] == "refresh-2"
        })
        .await;
        if !cancel {
            eventually("signed in", || harness.account_state() == "signed_in").await;
        } else {
            assert_eq!(harness.account_state(), "signed_out");
            assert!(harness.probe.calls("cloud").is_empty());
        }
        harness.stop().await;
    }
}

#[tokio::test]
async fn cancelling_account_switch_restores_the_rotated_saved_session() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let response_gate = gate.clone();
    let harness = Harness::start_with(move |cloud, setup| {
        let (access, refresh) = cloud.issue();
        cloud.state.lock().unwrap().refresh_gate = Some(response_gate);
        setup.probe.state.lock().unwrap().hold_install = true;
        std::fs::create_dir_all(setup.home.join("app-data")).unwrap();
        std::fs::write(
            setup.home.join("app-data/cloud-session.json"),
            json!({"apiUrl":format!("{}/cloud",cloud.site_url),"accessToken":access,
                "accessTokenExpiresAt":"2000-01-01T00:00:00Z","refreshToken":refresh})
            .to_string(),
        )
        .unwrap();
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"selected"}))
        .await;
    eventually("refresh rotated remotely", || {
        !harness
            .cloud
            .requests("/cloud/api/v1/auth/token/refresh")
            .is_empty()
    })
    .await;
    client.ok("account.sign_in.cancel", json!({})).await;
    let report = client
        .ok(
            "account.sign_in",
            json!({"sharing":"selected","flow":"browser"}),
        )
        .await;
    let link = report["verification_url_complete"]
        .as_str()
        .unwrap()
        .replace("%3D", "=")
        .replace("%26", "&");
    let redirect = link
        .split("redirect_uri=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap();
    let (handoff, _) = harness.cloud.issue();
    assert!(reqwest_get(&format!(
        "{redirect}?access_token={handoff}&api_url={}/cloud",
        harness.cloud.site_url
    ))
    .await
    .starts_with("HTTP/1.1 200"));
    eventually("new account validated", || {
        !harness
            .cloud
            .requests("/cloud/api/v1/auth/whoami")
            .is_empty()
    })
    .await;
    // The new attempt must not capture an undo snapshot until the old refresh
    // returns. Without synchronization this observation completes immediately.
    let replaced_early = tokio::time::timeout(Duration::from_millis(200), async {
        while harness.store()["cloudSession"]["refreshToken"] != "refresh-4" {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(
        replaced_early.is_err(),
        "account switch raced the rotating saved credentials"
    );
    gate.notify_one();
    eventually("new account install", || {
        !harness.probe.calls("cloud").is_empty()
    })
    .await;
    assert_eq!(harness.store()["cloudSession"]["refreshToken"], "refresh-4");
    client.ok("account.sign_in.cancel", json!({})).await;
    assert_eq!(harness.store()["cloudSession"]["refreshToken"], "refresh-2");
    assert!(harness
        .cloud
        .state
        .lock()
        .unwrap()
        .refresh_tokens
        .contains_key("refresh-2"));
    harness.stop().await;
}

#[tokio::test]
async fn saved_login_validation_is_background_work_and_is_not_duplicated() {
    for status in [None, Some(503)] {
        let gate = Arc::new(tokio::sync::Notify::new());
        let response_gate = gate.clone();
        let harness = Harness::start_with(move |cloud, setup| {
            keep_live_session(cloud, setup);
            let mut state = cloud.state.lock().unwrap();
            state.whoami_gate = Some(response_gate);
            state.whoami_status = status;
        })
        .await;
        let mut client = harness.client().await;
        // Cloud cannot answer until we release the gate. The RPC must already
        // have returned, including when the eventual answer is a server error.
        let started = tokio::time::timeout(
            Duration::from_secs(1),
            client.ok("account.sign_in", json!({"sharing":"selected"})),
        )
        .await
        .expect("sign-in must not wait for Cloud's identity response");
        assert_eq!(started["state"], "preparing");
        eventually("identity request", || {
            !harness
                .cloud
                .requests("/cloud/api/v1/auth/whoami")
                .is_empty()
        })
        .await;
        gate.notify_one();
        let expected = if status.is_none() {
            "signed_in"
        } else {
            "error"
        };
        eventually(expected, || harness.account_state() == expected).await;
        assert_eq!(harness.cloud.requests("/cloud/api/v1/auth/whoami").len(), 1);
        if status.is_some() {
            assert!(
                client.ok("account.status", json!({})).await["verification_url_complete"].is_null()
            );
        }
        harness.stop().await;
    }
}

#[tokio::test]
async fn a_kept_session_is_reused_without_asking_the_person() {
    // The Linux app's former session file is imported into the store once.
    let issued = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let seen = issued.clone();
    let harness = Harness::start_with(move |cloud, setup| {
        let (access, refresh) = cloud.issue();
        *seen.lock().unwrap() = access.clone();
        std::fs::create_dir_all(setup.home.join("app-data")).unwrap();
        std::fs::write(
            setup.home.join("app-data/cloud-session.json"),
            json!({"apiUrl": format!("{}/cloud", cloud.site_url), "accessToken": access,
                       "accessTokenExpiresAt": "2099-01-01T00:00:00Z", "refreshToken": refresh})
            .to_string(),
        )
        .unwrap();
    })
    .await;
    let access = issued.lock().unwrap().clone();
    assert!(!harness.home.join("app-data/cloud-session.json").exists());
    assert_eq!(harness.store()["cloudSession"]["accessToken"], access);
    let mut client = harness.client().await;
    let started = client
        .ok("account.sign_in", json!({"sharing": "selected"}))
        .await;
    assert_eq!(started["state"], "preparing");
    eventually("signed in", || harness.account_state() == "signed_in").await;
    assert_eq!(
        client.ok("account.status", json!({})).await["reused_login"],
        true
    );
    assert!(harness
        .cloud
        .requests("/cloud/api/v1/auth/device/start")
        .is_empty());
    assert_eq!(
        harness.probe.calls("cloud")[0].1.as_deref(),
        Some(access.as_str())
    );
    harness.stop().await;
}

#[tokio::test]
async fn cloud_calls_refresh_a_refused_session_once_and_keep_the_rotated_one() {
    let harness = Harness::start_with(|_, setup| {
        setup.probe.state.lock().unwrap().installs = vec![];
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    // Cloud revokes every access token server-side; the session still
    // looks fresh here.
    harness.cloud.expire_access();
    let people = client.ok("people.list", json!({})).await;
    assert_eq!(people["people"], json!([]));
    let refreshes = harness.cloud.requests("/cloud/api/v1/auth/token/refresh");
    assert_eq!(refreshes.len(), 1);
    assert_eq!(refreshes[0].body["refreshToken"], "refresh-1");
    let members = harness.cloud.requests("/cloud/api/v1/members");
    assert_eq!(members.len(), 2);
    assert_eq!(members[1].bearer.as_deref(), Some("access-2"));
    let store = harness.store();
    assert_eq!(store["cloudSession"]["refreshToken"], "refresh-2");

    // A session Cloud refuses outright is dropped, and the person asked to
    // sign in again.
    harness.cloud.expire_access();
    harness.cloud.state.lock().unwrap().refresh_tokens.clear();
    let (kind, message) = client.error("people.list", json!({})).await;
    assert_eq!(kind, "not_signed_in");
    assert!(message.contains("sign in again"), "{message}");
    assert!(harness.store()["cloudSession"].is_null());
    harness.stop().await;
}

/// Polls `GET /setup/status` until `sign_in` reads `want`.
async fn sign_in_reads(harness: &Harness, want: &str) -> Value {
    let mut status = Value::Null;
    for _ in 0..300 {
        status = harness
            .setup_request("GET /setup/status", json!({}))
            .await
            .unwrap();
        if status["sign_in"] == want {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("sign_in stayed {}, not {want}", status["sign_in"]);
}

#[tokio::test]
async fn a_session_another_process_rotated_is_adopted_not_refreshed_again() {
    let harness = Harness::start_with(|_, setup| {
        setup.probe.state.lock().unwrap().installs = vec![];
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    // Another process keeping this store refreshes the session first: Cloud
    // retires this core's pair, and the new one is only on disk.
    let (access, refresh) = harness.cloud.issue();
    {
        let mut cloud = harness.cloud.state.lock().unwrap();
        cloud.refresh_tokens.remove("refresh-1");
        cloud.valid.retain(|token| *token == access);
    }
    let mut store = harness.store();
    store["cloudSession"]["accessToken"] = json!(access);
    store["cloudSession"]["refreshToken"] = json!(refresh);
    std::fs::write(
        harness.home.join("app-data/injectable-agents.json"),
        store.to_string(),
    )
    .unwrap();
    // Cloud refuses this core's stale access token: the renewal adopts the
    // rotated session instead of refreshing a retired token (which Cloud
    // would refuse, and which used to forget the session for good).
    let people = client.ok("people.list", json!({})).await;
    assert_eq!(people["people"], json!([]));
    assert!(harness
        .cloud
        .requests("/cloud/api/v1/auth/token/refresh")
        .is_empty());
    let members = harness.cloud.requests("/cloud/api/v1/members");
    assert_eq!(
        members.last().unwrap().bearer.as_deref(),
        Some(access.as_str())
    );
    assert_eq!(
        harness.store()["cloudSession"]["refreshToken"],
        json!(refresh)
    );
    let account = client.ok("account.status", json!({})).await;
    assert_ne!(account["needs_sign_in"], true);
    harness.stop().await;
}

#[tokio::test]
async fn concurrent_cloud_calls_refresh_an_expired_session_once() {
    let harness = Harness::start_with(|_, setup| {
        setup.probe.state.lock().unwrap().installs = vec![];
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    harness.cloud.expire_access();
    let mut second = harness.client().await;
    let (first, other) = tokio::join!(
        client.ok("people.list", json!({})),
        second.ok("people.list", json!({}))
    );
    assert_eq!(first["people"], json!([]));
    assert_eq!(other["people"], json!([]));
    // One rotation: the second call used the pair the first one saved.
    assert_eq!(
        harness
            .cloud
            .requests("/cloud/api/v1/auth/token/refresh")
            .len(),
        1
    );
    assert_eq!(harness.store()["cloudSession"]["refreshToken"], "refresh-2");
    harness.stop().await;
}

#[tokio::test]
async fn a_session_cloud_refuses_after_a_refresh_asks_to_sign_in_again() {
    let harness = Harness::start_with(|_, setup| {
        setup.probe.state.lock().unwrap().installs = vec![];
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    let account = client.ok("account.status", json!({})).await;
    assert_ne!(account["needs_sign_in"], true);
    harness.cloud.expire_access();
    harness.cloud.state.lock().unwrap().refresh_tokens.clear();
    let (kind, _) = client.error("people.list", json!({})).await;
    assert_eq!(kind, "not_signed_in");
    // The account no longer claims to be plainly signed in.
    let account = client.ok("account.status", json!({})).await;
    assert_eq!(account["state"], "signed_in");
    assert_eq!(account["needs_sign_in"], true);
    let status = sign_in_reads(&harness, "needs_sign_in").await;
    assert_eq!(status["signed_in"], false);
    harness.stop().await;
}

#[tokio::test]
async fn a_call_cloud_refuses_even_renewed_asks_to_sign_in_again_until_one_works() {
    let harness = Harness::start_with(|_, setup| {
        setup.probe.state.lock().unwrap().installs = vec![];
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    harness.cloud.state.lock().unwrap().members_refused = true;
    let (kind, _) = client.error("people.list", json!({})).await;
    assert_eq!(kind, "not_signed_in");
    // Renewed once, and the renewed session refused too: ask the person.
    assert_eq!(
        harness
            .cloud
            .requests("/cloud/api/v1/auth/token/refresh")
            .len(),
        1
    );
    let account = client.ok("account.status", json!({})).await;
    assert_eq!(account["needs_sign_in"], true);
    // A call Cloud answers again clears it.
    harness.cloud.state.lock().unwrap().members_refused = false;
    client.ok("people.list", json!({})).await;
    let account = client.ok("account.status", json!({})).await;
    assert_ne!(account["needs_sign_in"], true);
    harness.stop().await;
}

#[tokio::test]
async fn signing_in_again_after_cloud_refuses_a_renewed_session_asks_the_person() {
    let harness = Harness::start_with(|_, setup| {
        setup.probe.state.lock().unwrap().installs = vec![];
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    harness.cloud.state.lock().unwrap().members_refused = true;
    let (kind, _) = client.error("people.list", json!({})).await;
    assert_eq!(kind, "not_signed_in");
    let account = client.ok("account.status", json!({})).await;
    assert_eq!(account["needs_sign_in"], true);
    // The renewed access token still looks fresh, yet Sign in again asks.
    let started = client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    assert_eq!(started["state"], "pending_approval");
    assert_eq!(started["phase"], "waiting_for_device_approval");
    harness.stop().await;
}

#[tokio::test]
async fn capy_spawn_without_a_model_is_refused_before_cloud() {
    // Capy has no default model: on 2026-10-28 a spawn without one was
    // accepted and bound, then failed inside Capy with model_not_selected.
    let harness = Harness::start_with(|_, setup| {
        setup.probe.state.lock().unwrap().installs = vec![];
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    let spawns = format!("/cloud/api/v1/workspaces/{WORKSPACE}/capy/thread-spawns");
    let base = json!({"client_request_id": "relay-desktop-1", "project_id": "project_1",
                      "task": "Review pull request 455"});
    for model in [None, Some(json!(null)), Some(json!("")), Some(json!("  "))] {
        let mut params = base.clone();
        if let Some(model) = model {
            params["model"] = model;
        }
        let (kind, message) = client.error("capy.threads.spawn", params).await;
        assert_eq!(kind, "invalid_params");
        assert!(message.contains("\"model\""), "{message}");
        assert!(message.contains("anthropic/claude-sonnet-5-5"), "{message}");
    }
    assert!(harness.cloud.requests(&spawns).is_empty());

    let mut params = base.clone();
    params["model"] = json!("anthropic/claude-sonnet-5-5");
    let started = client.ok("capy.threads.spawn", params).await;
    assert_eq!(started["thread_id"], "jam_01THREAD");
    let sent = harness.cloud.requests(&spawns);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].body["model"], "anthropic/claude-sonnet-5-5");
    assert_eq!(sent[0].body["clientRequestId"], "relay-desktop-1");
    harness.stop().await;
}

#[tokio::test]
async fn sign_out_revokes_the_session_and_disconnects_the_uploader() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    client.ok("account.sign_out", json!({})).await;
    assert_eq!(harness.account_state(), "signed_out");
    assert!(harness.store()["cloudSession"].is_null());
    assert!(harness.probe.state.lock().unwrap().cancelled >= 1);
    let disconnect = harness.probe.calls("disconnect");
    assert_eq!(disconnect.len(), 1);
    assert!(disconnect[0].0.windows(2).any(|w| w == ["--account", USER]));
    eventually("revoked", || {
        harness
            .cloud
            .requests("/cloud/api/v1/auth/token/revoke")
            .iter()
            .any(|r| r.body["token"] == "refresh-1")
    })
    .await;
    assert!(!harness
        .cloud
        .requests(&format!(
            "/cloud/api/v1/workspaces/{WORKSPACE}/messaging/desktop-identities"
        ))
        .is_empty());
    eventually("desktop identity clear committed", || {
        harness.store()["desktopIdentityClearsPending"].is_null()
    })
    .await;
    let (kind, _) = client.error("uploader.status", json!({})).await;
    assert_eq!(kind, "not_signed_in");
    assert!(harness.account.account().is_none());
    harness.stop().await;
}

#[tokio::test]
async fn a_setup_the_probe_refuses_reports_what_the_person_can_do() {
    let harness = Harness::start_with(|_, setup| {
        setup.probe.state.lock().unwrap().install_error = Some(
            "Could not inspect older upload schedules. Check them, then retry with --acknowledge-uninspected-schedules."
                .into(),
        );
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("failed", || harness.account_state() == "error").await;
    let account = client.ok("account.status", json!({})).await;
    assert_eq!(account["action"], "acknowledge_uninspected_schedules");
    assert!(account["error"]
        .as_str()
        .unwrap()
        .contains("older upload schedules"));

    harness.probe.state.lock().unwrap().install_error = None;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device", "acknowledge_uninspected_schedules": true}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    let calls = harness.probe.calls("cloud");
    assert!(calls[1]
        .0
        .contains(&"--acknowledge-uninspected-schedules".to_string()));
    harness.stop().await;
}

#[tokio::test]
async fn a_declined_or_cancelled_device_sign_in_stops() {
    let harness = Harness::start_with(|cloud, _| {
        let mut state = cloud.state.lock().unwrap();
        state.pending_polls = 0;
        state.device_error = Some("access_denied");
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("declined", || harness.account_state() == "denied").await;
    assert!(harness.probe.calls("cloud").is_empty());

    harness.cloud.state.lock().unwrap().pending_polls = 100;
    let started = client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    assert_eq!(started["state"], "pending_approval");
    client.ok("account.sign_in.cancel", json!({})).await;
    assert_eq!(harness.account_state(), "signed_out");
    let after = client.ok("account.status", json!({})).await;
    assert!(after["verification_url"].is_null());
    harness.stop().await;
}

/// A site that expired the approval, as a password reset does
/// (`expired_token`), asks to sign in again in the shared words, and signing
/// in again works.
#[tokio::test]
async fn an_expired_device_sign_in_asks_to_sign_in_again() {
    let harness = Harness::start_with(|cloud, _| {
        let mut state = cloud.state.lock().unwrap();
        state.pending_polls = 0;
        state.device_error = Some("expired_token");
    })
    .await;
    let mut client = harness.client().await;
    let device = json!({"sharing": "new", "flow": "device"});
    client.ok("account.sign_in", device.clone()).await;
    eventually("expired", || harness.account_state() == "expired").await;
    let account = client.ok("account.status", json!({})).await;
    assert_eq!(account["error"], "Your sign-in expired. Sign in again.");
    assert!(harness.probe.calls("cloud").is_empty());

    harness.cloud.state.lock().unwrap().device_error = None;
    client.ok("account.sign_in", device).await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    harness.stop().await;
}

/// The sign-in screen reads the site's login config; a site without one
/// answers unavailable and the screen stays as it was. A site that fails to
/// answer fails the call, so the screen asks again next time.
#[tokio::test]
async fn the_login_config_comes_from_the_site_or_is_unavailable() {
    let answer = |harness: &Harness, status: u16, body: Value| {
        let route = "/cloud/api/v1/auth/login-config";
        let mut state = harness.cloud.state.lock().unwrap();
        state
            .response_overrides
            .insert((route.into(), String::new(), "GET".into()), (status, body));
    };
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    answer(&harness, 503, json!({"error": "unavailable"}));
    let (kind, _) = client.error("account.login_config", json!({})).await;
    assert_eq!(kind, "cloud_failed");

    answer(
        &harness,
        200,
        json!({"methods": ["email_link", "password"],
               "logo_url": "https://cdn.example.com/logo.png",
               "org_name": "Example Org"}),
    );
    let config = client.ok("account.login_config", json!({})).await;
    assert_eq!(config["available"], true);
    assert_eq!(config["logo_url"], "https://cdn.example.com/logo.png");
    assert_eq!(config["org_name"], "Example Org");
    assert_eq!(
        config["methods_line"],
        "Sign in with an email link or a password in your browser."
    );

    answer(&harness, 404, json!({"error": "not found"}));
    let config = client.ok("account.login_config", json!({})).await;
    assert_eq!(config["available"], false);
    assert!(config["methods_line"].is_null());
    harness.stop().await;
}

fn arrange_legacy_workspace(cloud: &FakeCloud, setup: &Harnessing) {
    let (access, refresh) = cloud.issue();
    std::fs::create_dir_all(setup.home.join("app-data")).unwrap();
    std::fs::write(
        setup.home.join("app-data/cloud-session.json"),
        json!({"apiUrl": format!("{}/cloud", cloud.site_url), "accessToken": access,
            "accessTokenExpiresAt": "2099-01-01T00:00:00Z", "refreshToken": refresh})
        .to_string(),
    )
    .unwrap();
    setup.probe.state.lock().unwrap().installs = vec![FakeProbe::connected(&cloud.site_url)];
    *setup.supervise_every.lock().unwrap() = Some(Duration::from_millis(20));
    cloud.state.lock().unwrap().resolve_status = 503;
}

#[tokio::test]
async fn legacy_workspace_name_loads_without_relaycast_and_is_not_refetched() {
    let harness = Harness::start_with(arrange_legacy_workspace).await;
    eventually("Cloud workspace name", || {
        harness.account.status().workspace_name.as_deref() == Some("Acme")
    })
    .await;
    assert_eq!(
        harness.account.status().workspace_id.as_deref(),
        Some(WORKSPACE)
    );
    assert!(harness.store()["cloud"].is_null());
    assert_eq!(harness.account.status().workspace_count, Some(1));
    assert_eq!(
        harness.account.status().workspace_avatar_url.as_deref(),
        Some("https://example.com/workspace.png")
    );
    let requests = harness.cloud.requests("/cloud/api/v1/auth/whoami").len();
    assert_eq!(requests, 1);
    // Let supervision run again: loading a label must not continually fetch it.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        harness.cloud.requests("/cloud/api/v1/auth/whoami").len(),
        requests
    );
    assert!(harness.probe.calls("cloud").is_empty());
    harness.stop().await;
}

#[tokio::test]
async fn existing_install_requests_cloud_profile_repair_without_client_pii() {
    let harness = Harness::start_with(arrange_legacy_workspace).await;
    eventually("authenticated profile bootstrap", || {
        harness.account.status().workspace_count == Some(1)
    })
    .await;
    let requests = harness.cloud.requests("/cloud/api/v1/auth/whoami");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].analytics.as_deref(), Some("enabled"));
    assert!(requests[0].bearer.is_some());
    assert_eq!(requests[0].body, Value::Null);
    for event in harness.analytics() {
        let properties = &event["properties"];
        assert!(properties.get("email").is_none());
        assert!(properties.get("name").is_none());
        assert!(properties.get("$set").is_none());
    }
    harness.stop().await;
}

#[tokio::test]
async fn opted_out_install_loads_account_without_profile_repair_or_events() {
    let harness = Harness::start_with(|cloud, setup| {
        arrange_legacy_workspace(cloud, setup);
        for directory in [
            setup.home.join("app-data"),
            setup.home.join(".config/com.agentrelay.desktop"),
        ] {
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                directory.join("preferences.json"),
                r#"{"analytics_enabled":false}"#,
            )
            .unwrap();
        }
    })
    .await;
    eventually("ordinary account bootstrap", || {
        harness.account.status().workspace_count == Some(1)
    })
    .await;
    assert!(!harness.telemetry.enabled());
    assert!(harness
        .cloud
        .requests("/cloud/api/v1/auth/whoami")
        .iter()
        .all(|r| r.analytics.is_none()));
    assert!(harness.analytics().is_empty());
    harness.stop().await;
}

#[tokio::test]
async fn whoami_preserves_canonical_id_and_retries_eligible_profile_request() {
    use agent_relay_node::relay::cloud_session::{whoami, whoami_with_analytics};
    let cloud = FakeCloud::start().await;
    let (access, _) = cloud.issue();
    let bearer = Secret::new(access);
    cloud.state.lock().unwrap().whoami_user = "usr_cloud_canonical".into();
    cloud.state.lock().unwrap().whoami_status = Some(503);
    assert!(whoami_with_analytics(&cloud.site_url, &bearer, true)
        .await
        .is_err());
    cloud.state.lock().unwrap().whoami_status = None;
    let identity = whoami_with_analytics(&cloud.site_url, &bearer, true)
        .await
        .unwrap();
    assert_eq!(identity.user_id, "usr_cloud_canonical");
    whoami(&cloud.site_url, &bearer).await.unwrap();
    let requests = cloud.requests("/cloud/api/v1/auth/whoami");
    assert_eq!(
        requests
            .iter()
            .map(|r| r.analytics.as_deref())
            .collect::<Vec<_>>(),
        [Some("enabled"), Some("enabled"), None]
    );
    assert!(requests
        .iter()
        .all(|r| r.bearer.as_deref() == Some(bearer.expose())));
}

#[tokio::test]
async fn workspace_avatar_metadata_reports_multiple_memberships() {
    let harness = Harness::start_with(|cloud, setup| {
        arrange_legacy_workspace(cloud, setup);
        cloud.state.lock().unwrap().whoami_workspace_count = 2;
    })
    .await;
    eventually("workspace membership metadata", || {
        harness.account.status().workspace_count == Some(2)
    })
    .await;
    assert_eq!(
        harness.account.status().workspace_avatar_url.as_deref(),
        Some("https://example.com/workspace.png")
    );
    harness.stop().await;
}

#[tokio::test]
async fn legacy_workspace_name_rejects_another_workspace_or_person() {
    for wrong_person in [false, true] {
        let harness = Harness::start_with(move |cloud, setup| {
            arrange_legacy_workspace(cloud, setup);
            let mut state = cloud.state.lock().unwrap();
            if wrong_person {
                state.whoami_user = "another-person".into();
            } else {
                state.whoami_workspace = "another-workspace".into();
            }
        })
        .await;
        eventually("name lookup", || {
            !harness
                .cloud
                .requests("/cloud/api/v1/auth/whoami")
                .is_empty()
        })
        .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(harness.account.status().workspace_name, None);
        assert_eq!(harness.account.status().workspace_count, None);
        assert_eq!(harness.account.status().workspace_avatar_url, None);
        assert_eq!(
            harness.account.status().workspace_id.as_deref(),
            Some(WORKSPACE)
        );
        harness.stop().await;
    }
}

#[tokio::test]
async fn an_existing_connection_is_found_at_start_and_the_uploader_answers() {
    let harness = Harness::start_with(|cloud, setup| {
        let mut state = setup.probe.state.lock().unwrap();
        state.installs = vec![FakeProbe::connected(&cloud.site_url)];
        state.status = json!({"running": false, "paused": false, "sharing_mode": "new",
            "delivery": {"bootstrap_complete": true, "pending_records": 0}, "sessions": {"shared": 0, "total": 0}});
    })
    .await;
    eventually("discovered", || harness.account_state() == "signed_in").await;
    let mut client = harness.client().await;
    let status = client.ok("uploader.status", json!({})).await;
    assert_eq!(status["stage"], "not_running");
    assert_eq!(status["needs_start"], true);
    // Where the install keeps its collector log.
    assert_eq!(status["directory"], "/tmp/x");
    // Supervision starts a stopped collector at launch, as the app did.
    eventually("supervised start", || {
        harness.probe.calls("start").len() == 1
    })
    .await;
    client.ok("uploader.start", json!({})).await;
    assert_eq!(harness.probe.calls("start").len(), 2);
    let sharing = client
        .ok("uploader.sharing.set", json!({"mode": "selected"}))
        .await;
    assert_eq!(sharing, json!({"sharing_mode": "selected"}));
    let sessions = client.ok("uploader.sessions.list", json!({})).await;
    assert_eq!(sessions["sessions"].as_array().unwrap().len(), 1);
    let updated = client
        .ok(
            "uploader.sessions.include",
            json!({"session_keys": ["codex:a"]}),
        )
        .await;
    assert_eq!(updated, json!({"updated": 1}));
    let (kind, _) = client
        .error(
            "uploader.sessions.exclude",
            json!({"session_keys": ["no-colon"]}),
        )
        .await;
    assert_eq!(kind, "invalid_params");
    let (kind, _) = client
        .error("uploader.sharing.set", json!({"mode": "everything"}))
        .await;
    assert_eq!(kind, "invalid_params");
    harness.stop().await;
}

#[tokio::test]
async fn connect_workspace_resolves_with_the_kept_session() {
    let harness = Harness::start_with(|cloud, setup| {
        let (access, refresh) = cloud.issue();
        std::fs::create_dir_all(setup.home.join("app-data")).unwrap();
        std::fs::write(
            setup.home.join("app-data/injectable-agents.json"),
            json!({"registrations": [], "tokens": {}, "cloudSession": {
                    "accessToken": access, "refreshToken": refresh, "siteURL": cloud.site_url}})
            .to_string(),
        )
        .unwrap();
    })
    .await;
    let mut client = harness.client().await;
    let (kind, _) = client
        .error("account.connect_workspace", json!({"workspace_id": "acme"}))
        .await;
    assert_eq!(kind, "invalid_params");
    let workspace = client
        .ok(
            "account.connect_workspace",
            json!({"workspace_id": WORKSPACE}),
        )
        .await;
    assert_eq!(workspace["state"], "found");
    assert_eq!(harness.store()["cloud"]["key"], ACCOUNT_KEY);
    harness.stop().await;
}

#[tokio::test]
async fn the_bridge_lists_every_account_method_and_the_sign_in_topic() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    let hello = client.ok("hello", json!({"bridge_version": 2})).await;
    let methods: Vec<&str> = hello["methods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect();
    for method in [
        "account.sign_in",
        "account.sign_in.cancel",
        "account.sign_out",
        "account.connect_workspace",
        "account.status",
        "uploader.status",
        "uploader.sessions.preview",
        "telemetry.record",
        "login_item.set",
        "login_item.status",
        "chat.send",
        "contractors.status",
    ] {
        assert!(methods.contains(&method), "{method}");
    }
    assert!(hello["topics"]
        .as_array()
        .unwrap()
        .contains(&json!("sign_in")));
    assert!(!methods.contains(&"credential_proxy.status"));
    let preview = client.ok("uploader.sessions.preview", json!({})).await;
    assert_eq!(preview, json!({"sessions": []}));
    harness.stop().await;
}

#[tokio::test]
async fn a_sign_out_whose_disconnect_fails_stays_signed_out_and_the_next_sign_in_retries_it() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    harness.probe.state.lock().unwrap().disconnect_fails = true;
    let (kind, message) = client.error("account.sign_out", json!({})).await;
    assert_eq!(kind, "refused");
    assert!(
        message.contains("Could not finish disconnecting"),
        "{message}"
    );
    assert!(message.contains("busy"), "{message}");
    assert_eq!(harness.account_state(), "signed_out");
    assert!(harness.store()["cloudSession"].is_null());

    // Still failing: the next sign-in does not start over a live connection.
    let (kind, _) = client
        .error(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    assert_eq!(kind, "refused");
    assert_eq!(harness.probe.calls("cloud").len(), 1);

    harness.probe.state.lock().unwrap().disconnect_fails = false;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("signed in again", || harness.account_state() == "signed_in").await;
    assert_eq!(harness.probe.calls("disconnect").len(), 3);
    assert_eq!(harness.probe.calls("cloud").len(), 2);
    harness.stop().await;
}

#[tokio::test]
async fn an_install_for_another_site_is_not_this_app_s_connection() {
    let harness = Harness::start_with(|_, setup| {
        setup.probe.state.lock().unwrap().installs =
            vec![FakeProbe::connected("https://agentrelay.com")];
    })
    .await;
    eventually("discovery ran", || {
        !harness.probe.calls("installs").is_empty()
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(harness.account_state(), "signed_out");
    assert!(harness.account.account().is_none());
    harness.stop().await;
}

#[tokio::test]
async fn a_browser_sign_in_whose_exchange_fails_keeps_the_handoff_session() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    let started = client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "browser"}),
        )
        .await;
    let link = started["verification_url_complete"]
        .as_str()
        .unwrap()
        .replace("%3D", "=")
        .replace("%26", "&");
    let redirect = link
        .split("redirect_uri=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_string();
    harness.cloud.state.lock().unwrap().exchange_fails = true;
    let (handoff, _) = harness.cloud.issue();
    let answer = reqwest_get(&format!(
        "{redirect}?access_token={handoff}&api_url={}/cloud&access_token_expires_at=2099-01-01T00:00:00Z",
        harness.cloud.site_url
    ))
    .await;
    assert!(answer.starts_with("HTTP/1.1 200"));
    eventually("signed in", || harness.account_state() == "signed_in").await;
    let store = harness.store();
    assert_eq!(store["cloudSession"]["accessToken"], handoff_of(&harness));
    assert!(store["cloudSession"]["refreshToken"].is_null());
    assert_eq!(
        harness.probe.calls("cloud")[0].1.as_deref(),
        Some(handoff_of(&harness).as_str())
    );
    harness.stop().await;
}

/// The last access token the fake Cloud issued.
fn handoff_of(harness: &Harness) -> String {
    format!("access-{}", harness.cloud.state.lock().unwrap().minted)
}

#[tokio::test]
async fn messaging_uses_the_account_workspace_over_this_computers_relay() {
    let harness = Harness::start_with(|_, setup| {
        let relay = setup.home.join(".agentworkforce/relay");
        std::fs::create_dir_all(&relay).unwrap();
        std::fs::write(
            relay.join("workspace-key.json"),
            json!({"workspaceKey": "rk_live_node_key_0001", "workspaceId": "rw_node"}).to_string(),
        )
        .unwrap();
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    eventually("the account workspace", || {
        harness.core.workspace().map(|w| w.key.expose().to_string()) == Some(ACCOUNT_KEY.into())
    })
    .await;
    let status = client.ok("status.get", json!({})).await;
    assert_eq!(status["workspace"]["workspace_id"], "rw_acme");
    assert!(status["workspace"].get("source").is_none());
    assert!(status["account"].get("workspace_mismatch").is_none());
    harness.stop().await;
}

#[tokio::test]
async fn a_rate_limited_refresh_keeps_the_session_for_a_later_try() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    harness.cloud.expire_access();
    for status in [429, 408] {
        harness.cloud.state.lock().unwrap().refresh_status = Some(status);
        let (kind, _) = client.error("people.list", json!({})).await;
        assert_ne!(kind, "not_signed_in", "HTTP {status}");
        assert_eq!(
            harness.store()["cloudSession"]["refreshToken"],
            "refresh-1",
            "HTTP {status}"
        );
    }
    harness.cloud.state.lock().unwrap().refresh_status = None;
    client.ok("people.list", json!({})).await;
    assert_eq!(harness.store()["cloudSession"]["refreshToken"], "refresh-2");
    harness.stop().await;
}

/// A live Cloud session for `cloud` from the environment
/// (`CLOUD_API_ACCESS_TOKEN`), possibly another person's.
fn borrow_env_session(cloud: &FakeCloud, setup: &Harnessing) -> String {
    let (access, _) = cloud.issue();
    *setup.cloud_token.lock().unwrap() = Some(access.clone());
    access
}

/// A live session for `cloud` kept by this app, imported from the Linux
/// app's former session file.
fn keep_live_session(cloud: &FakeCloud, setup: &Harnessing) -> String {
    let (access, refresh) = cloud.issue();
    std::fs::create_dir_all(setup.home.join("app-data")).unwrap();
    std::fs::write(
        setup.home.join("app-data/cloud-session.json"),
        json!({"apiUrl": format!("{}/cloud", cloud.site_url), "accessToken": access,
               "accessTokenExpiresAt": "2099-01-01T00:00:00Z", "refreshToken": refresh})
        .to_string(),
    )
    .unwrap();
    access
}

#[tokio::test]
async fn a_sign_in_while_connected_with_no_usable_session_keeps_one_and_leaves_the_uploader() {
    // Connected by the uploader with no session kept: Cloud calls as the
    // person fail.
    let harness = Harness::start_with(|cloud, setup| {
        setup.probe.state.lock().unwrap().installs = vec![FakeProbe::connected(&cloud.site_url)];
    })
    .await;
    eventually("discovered", || harness.account_state() == "signed_in").await;
    let mut client = harness.client().await;
    let (kind, _) = client.error("people.list", json!({})).await;
    assert_eq!(kind, "not_signed_in");

    // Signing in again asks the person, keeping who is connected meanwhile.
    let started = client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    assert_eq!(started["state"], "pending_approval");
    assert_eq!(started["phase"], "waiting_for_device_approval");
    assert_eq!(started["user_code"], "ABCD-EFGH");
    assert_eq!(started["person"]["id"], USER);

    eventually("kept", || {
        harness.store()["cloudSession"]["accessToken"].is_string()
    })
    .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    // The uploader's connection was left alone: no second install.
    assert!(harness.probe.calls("cloud").is_empty());
    let kept = harness.store()["cloudSession"]["accessToken"]
        .as_str()
        .unwrap()
        .to_string();
    client.ok("people.list", json!({})).await;
    let members = harness.cloud.requests("/cloud/api/v1/members");
    assert_eq!(
        members.last().unwrap().bearer.as_deref(),
        Some(kept.as_str())
    );

    // With a usable session, a sign-in while connected asks nothing.
    let again = client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    assert_eq!(again["state"], "signed_in");
    harness.stop().await;
}

#[tokio::test]
async fn a_kept_session_that_cannot_be_renewed_answers_not_signed_in() {
    let harness = Harness::start_with(|cloud, setup| {
        setup.probe.state.lock().unwrap().installs = vec![FakeProbe::connected(&cloud.site_url)];
        std::fs::create_dir_all(setup.home.join("app-data")).unwrap();
        std::fs::write(
            setup.home.join("app-data/cloud-session.json"),
            json!({"apiUrl": format!("{}/cloud", cloud.site_url), "accessToken": "access-old",
                   "accessTokenExpiresAt": "2000-01-01T00:00:00Z", "refreshToken": "refresh-unknown"})
            .to_string(),
        )
        .unwrap();
    })
    .await;
    eventually("discovered", || harness.account_state() == "signed_in").await;
    let mut client = harness.client().await;
    let (kind, _) = client.error("people.list", json!({})).await;
    assert_eq!(kind, "not_signed_in");
    assert!(harness.cloud.requests("/cloud/api/v1/members").is_empty());
    harness.stop().await;
}

fn operation<'a>(events: &'a [Value], operation: &str, outcome: &str) -> Option<&'a Value> {
    events.iter().rev().find(|event| {
        event["event"] == "desktop_operation"
            && event["properties"]["operation"] == operation
            && event["properties"]["outcome"] == outcome
    })
}

#[tokio::test]
async fn usage_actions_keep_properties_and_failures_without_content() {
    let harness = Harness::start_with(|cloud, setup| {
        setup.probe.state.lock().unwrap().installs = vec![FakeProbe::connected(&cloud.site_url)];
    })
    .await;
    eventually("discovered", || harness.account_state() == "signed_in").await;
    let mut client = harness.client().await;
    client
        .error(
            "agents.register",
            json!({"target":{"kind":"codex","id":"private-session"}, "name":"private-agent"}),
        )
        .await;
    client.error("messages.send", json!({"to":"private-agent", "address":"private@host", "text":"private-message", "reply_to":"private-reply"})).await;
    client
        .ok(
            "uploader.sessions.include",
            json!({"session_keys":["codex:private-session"]}),
        )
        .await;
    client
        .ok("uploader.sharing.set", json!({"mode":"selected"}))
        .await;
    client
        .ok(
            "uploader.sessions.delete",
            json!({"session_keys":["codex:private-session"]}),
        )
        .await;
    // A cursor-less background follow must not count as another open.
    for _ in 0..3 {
        client
            .ok(
                "sessions.transcript",
                json!({"source":"codex", "session_id":"private-session", "following":true}),
            )
            .await;
    }
    assert!(operation(&harness.analytics(), "transcript_open", "started").is_none());
    // Legacy initial opens and explicit refreshes retain their observations.
    for _ in 0..2 {
        client
            .ok(
                "sessions.transcript",
                json!({"source":"codex", "session_id":"private-session"}),
            )
            .await;
    }
    let events = harness.analytics();
    assert_eq!(
        events
            .iter()
            .filter(
                |event| event["properties"]["operation"] == "transcript_open"
                    && event["properties"]["outcome"] == "started"
            )
            .count(),
        2
    );
    assert!(operation(&events, "transcript_open", "succeeded").is_some());
    assert_eq!(
        operation(&events, "agent_register", "failed").unwrap()["properties"]["agent_kind"],
        "codex"
    );
    assert!(operation(&events, "agent_register", "started").is_some());
    assert_eq!(
        operation(&events, "message_send", "failed").unwrap()["properties"]["is_reply"],
        true
    );
    assert!(operation(&events, "message_send", "started").is_some());
    assert_eq!(
        operation(&events, "include_sessions", "succeeded").unwrap()["properties"]["session_count"],
        1
    );
    assert_eq!(
        operation(&events, "sharing_mode", "succeeded").unwrap()["properties"]["sharing_mode"],
        "selected"
    );
    assert_eq!(
        operation(&events, "delete_sessions", "succeeded").unwrap()["properties"]["session_count"],
        1
    );
    assert!(!serde_json::to_string(&events).unwrap().contains("private-"));
    harness.telemetry.set_enabled(false).unwrap();
    client
        .error(
            "messages.send",
            json!({"to":"private-agent", "address":"private@host", "text":"private-message"}),
        )
        .await;
    assert_eq!(harness.analytics().len(), events.len());
    harness.stop().await;
}

#[tokio::test]
async fn cancelling_sign_in_is_recorded_once_by_the_core() {
    let harness = Harness::start_with(|_, setup| {
        setup.probe.state.lock().unwrap().hold_install = true;
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"new", "flow":"device"}))
        .await;
    eventually("setup in flight", || {
        !harness.probe.calls("cloud").is_empty()
    })
    .await;
    client.ok("account.sign_in.cancel", json!({})).await;
    client.ok("account.sign_in.cancel", json!({})).await;
    let cancelled = harness
        .analytics()
        .into_iter()
        .filter(|event| {
            event["event"] == "desktop_operation"
                && event["properties"]["operation"] == "sign_in"
                && event["properties"]["outcome"] == "cancelled"
        })
        .count();
    assert_eq!(cancelled, 1);
    harness.stop().await;
}

#[tokio::test]
async fn disconnect_during_sign_in_renewal_does_not_record_sign_in_cancellation() {
    let harness = Harness::start_with(|cloud, setup| {
        // An existing uploader with no renewable desktop login.
        setup.probe.state.lock().unwrap().installs = vec![FakeProbe::connected(&cloud.site_url)];
    })
    .await;
    eventually("existing connection discovered", || {
        harness.account_state() == "signed_in"
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing":"new", "flow":"browser"}),
        )
        .await;
    assert_eq!(harness.account_state(), "pending_approval");
    assert!(harness.account.uploader.target().is_some());
    client.ok("uploader.disconnect", json!({})).await;
    let events = harness.analytics();
    assert!(operation(&events, "disconnect", "succeeded").is_some());
    assert!(operation(&events, "sign_in", "cancelled").is_none());
    harness.stop().await;
}

#[tokio::test]
async fn update_success_reaches_the_sender_before_the_rpc_allows_core_release() {
    let harness = Harness::start().await;
    *harness.analytics.delay.lock().unwrap() = Some(Duration::from_millis(100));
    let mut client = harness.client().await;
    client
        .ok(
            "telemetry.record",
            json!({"operation":"update_install", "outcome":"succeeded"}),
        )
        .await;
    // Do not use analytics(): its flush would mask an early RPC response.
    let events = harness.analytics.events.lock().unwrap().clone();
    assert!(operation(&events, "update_install", "succeeded").is_some());
    harness.stop().await;
}

#[tokio::test]
async fn analytics_belong_to_the_user_once_the_install_attaches_until_sign_out() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    client
        .ok(
            "telemetry.record",
            json!({"operation": "app_launch", "outcome": "started"}),
        )
        .await;
    let launch = harness.analytics()[0].clone();
    let anonymous = launch["distinct_id"].as_str().unwrap().to_string();
    assert!(anonymous.starts_with("desktop:"));
    assert_eq!(launch["properties"]["$process_person_profile"], false);

    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    let events = harness.analytics();
    let identifies: Vec<&Value> = events
        .iter()
        .filter(|e| e["event"] == "$identify")
        .collect();
    assert_eq!(identifies.len(), 1, "{events:?}");
    assert_eq!(identifies[0]["distinct_id"], USER);
    assert_eq!(identifies[0]["properties"]["$anon_distinct_id"], anonymous);
    let groups = json!({"organization": ORG, "workspace": WORKSPACE});
    assert_eq!(identifies[0]["properties"]["$groups"], groups);
    let ready = operation(&events, "history_setup", "succeeded").unwrap();
    assert_eq!(ready["distinct_id"], USER);
    assert_eq!(ready["properties"]["$groups"], groups);
    assert!(ready["properties"].get("$process_person_profile").is_none());

    client.ok("account.sign_out", json!({})).await;
    client
        .ok(
            "telemetry.record",
            json!({"operation": "sign_in", "outcome": "started"}),
        )
        .await;
    let events = harness.analytics();
    let after = operation(&events, "sign_in", "started").unwrap();
    let rotated = after["distinct_id"].as_str().unwrap();
    assert!(
        rotated.starts_with("desktop:") && rotated != anonymous,
        "{rotated}"
    );
    assert_eq!(after["properties"]["$process_person_profile"], false);
    harness.stop().await;
}

/// A harness whose telemetry is a site build's.
async fn site_build(prepare: impl FnOnce(&FakeCloud, &Harnessing)) -> Harness {
    Harness::start_with(|cloud, setup| {
        *setup.site_analytics.lock().unwrap() = true;
        prepare(cloud, setup);
    })
    .await
}

/// How many `kind` events reached the site's gateway.
fn site_events(harness: &Harness, kind: &str) -> usize {
    harness
        .analytics()
        .iter()
        .filter(|event| event["event"] == kind)
        .count()
}

#[tokio::test]
async fn a_site_build_records_a_sign_in_once_it_connects() {
    let harness = site_build(|_, _| {}).await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    assert_eq!(site_events(&harness, "desktop.sign_in_completed"), 1);
    assert_eq!(site_events(&harness, "desktop.app_started"), 1);
    harness.stop().await;
}

#[tokio::test]
async fn a_site_build_records_a_renewed_sign_in() {
    let harness = site_build(|cloud, setup| {
        setup.probe.state.lock().unwrap().installs = vec![FakeProbe::connected(&cloud.site_url)];
    })
    .await;
    eventually("discovered", || harness.account_state() == "signed_in").await;
    let mut client = harness.client().await;
    let started = client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    assert_eq!(started["phase"], "waiting_for_device_approval");
    eventually("kept", || {
        harness.store()["cloudSession"]["accessToken"].is_string()
    })
    .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    // Renewed, not reconnected.
    assert!(harness.probe.calls("cloud").is_empty());
    eventually("renewal recorded", || {
        site_events(&harness, "desktop.sign_in_completed") == 1
    })
    .await;
    harness.stop().await;
}

#[tokio::test]
async fn a_site_build_restart_that_restores_the_saved_session_records_no_sign_in() {
    let harness = site_build(|cloud, setup| {
        keep_live_session(cloud, setup);
        setup.probe.state.lock().unwrap().installs = vec![FakeProbe::connected(&cloud.site_url)];
    })
    .await;
    eventually("discovered", || harness.account_state() == "signed_in").await;
    eventually("started", || {
        site_events(&harness, "desktop.app_started") == 1
    })
    .await;
    assert_eq!(site_events(&harness, "desktop.sign_in_completed"), 0);
    harness.stop().await;
}

#[tokio::test]
async fn a_cancelled_setup_never_links_analytics() {
    let harness = Harness::start_with(|_, setup| {
        setup.probe.state.lock().unwrap().hold_install = true;
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("connected", || {
        harness.core.status().account.phase.as_deref() == Some("preparing_local_sessions")
    })
    .await;
    client.ok("account.sign_in.cancel", json!({})).await;
    let events = harness.analytics();
    assert!(!events.is_empty());
    assert!(
        events.iter().all(|e| e["event"] != "$identify"),
        "{events:?}"
    );
    assert!(events
        .iter()
        .all(|e| e["distinct_id"].as_str().unwrap().starts_with("desktop:")));
    harness.stop().await;
}

#[tokio::test]
async fn a_setup_cancelled_after_it_connected_disconnects_what_it_made() {
    let harness = Harness::start_with(|_, setup| {
        setup.probe.state.lock().unwrap().hold_install = true;
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("connected", || {
        harness.core.status().account.phase.as_deref() == Some("preparing_local_sessions")
    })
    .await;
    client.ok("account.sign_in.cancel", json!({})).await;
    assert_eq!(harness.account_state(), "signed_out");
    eventually("disconnected", || {
        harness
            .probe
            .calls("disconnect")
            .iter()
            .any(|(args, _)| args.windows(2).any(|w| w == ["--account", USER]))
    })
    .await;
    assert!(harness.probe.state.lock().unwrap().installs.is_empty());
    assert!(harness.account.uploader.install().is_none());
    assert_eq!(harness.account_state(), "signed_out");
    // Started signed out, so it ends signed out: the session and workspace
    // key this attempt kept are gone.
    assert!(harness.account.account().is_none());
    let store = harness.store();
    assert!(store["cloudSession"].is_null(), "{store}");
    assert!(store["cloud"].is_null(), "{store}");

    // Sign-out does the same for a setup still running.
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("connected again", || {
        harness.core.status().account.phase.as_deref() == Some("preparing_local_sessions")
    })
    .await;
    client.ok("account.sign_out", json!({})).await;
    assert_eq!(harness.probe.calls("disconnect").len(), 2);
    assert!(harness.probe.state.lock().unwrap().installs.is_empty());
    assert_eq!(harness.account_state(), "signed_out");
    harness.stop().await;
}

#[tokio::test]
async fn a_cancelled_setup_that_reused_a_kept_session_leaves_it_kept() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    client.ok("uploader.disconnect", json!({})).await;
    let account = harness.account.account();
    assert!(account.is_some());
    let kept = harness.store();

    harness.probe.state.lock().unwrap().hold_install = true;
    let started = client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    assert_eq!(started["reused_login"], true);
    eventually("connected", || {
        harness.core.status().account.phase.as_deref() == Some("preparing_local_sessions")
    })
    .await;
    client.ok("account.sign_in.cancel", json!({})).await;
    eventually("disconnected", || {
        harness.probe.state.lock().unwrap().installs.is_empty()
    })
    .await;
    assert_eq!(harness.account.account(), account);
    let store = harness.store();
    assert_eq!(store["cloudSession"], kept["cloudSession"]);
    assert_eq!(store["cloud"], kept["cloud"]);
    harness.stop().await;
}

#[tokio::test]
async fn a_sign_in_that_fails_after_connecting_is_attached() {
    let harness = Harness::start_with(move |_, setup| {
        let mut probe = setup.probe.state.lock().unwrap();
        probe.fail_after_connected = Some("RelayHistory is unavailable.".into());
        probe.status_fails = true;
        drop(probe);
        *setup.supervise_every.lock().unwrap() = Some(Duration::from_millis(100));
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("the setup failed", || {
        harness.account_state() == "error" || harness.account_state() == "signed_in"
    })
    .await;
    eventually("attached", || harness.account_state() == "signed_in").await;
    harness.stop().await;
}

fn keep_session(setup: &Harnessing, site: &str, access: &str, refresh: &str) {
    std::fs::create_dir_all(setup.home.join("app-data")).unwrap();
    std::fs::write(
        setup.home.join("app-data/injectable-agents.json"),
        json!({"registrations": [], "tokens": {}, "cloudSession": {
                "accessToken": access, "refreshToken": refresh, "siteURL": site}})
        .to_string(),
    )
    .unwrap();
}

/// An uploader whose job is blocked on `authentication_required`, as the
/// probe's `status --json` reports it.
fn blocked_on_sign_in() -> Value {
    json!({"running": true, "paused": false, "sharing_mode": "new",
        "delivery": {"bootstrap_complete": true, "pending_records": 3,
            "failure": "Uploads need you to sign in to Agent Relay again.",
            "failure_code": "authentication_required"},
        "sessions": {"shared": 1, "total": 1}})
}

/// A kept session whose access token expired long ago, as the legacy file
/// the store imports at start.
fn keep_expired_session(cloud: &FakeCloud, setup: &Harnessing) {
    let (access, refresh) = cloud.issue();
    std::fs::create_dir_all(setup.home.join("app-data")).unwrap();
    std::fs::write(
        setup.home.join("app-data/cloud-session.json"),
        json!({"apiUrl": format!("{}/cloud", cloud.site_url), "accessToken": access,
               "accessTokenExpiresAt": "2000-01-01T00:00:00Z", "refreshToken": refresh})
        .to_string(),
    )
    .unwrap();
}

/// The collector renews its upload session from the app's kept Cloud
/// session, which it accepts only while live. Uploads blocked on sign-in
/// refresh that session themselves, with no desktop identity sync (there is
/// no workspace binding here) or other Cloud call to do it.
#[tokio::test]
async fn uploads_blocked_on_sign_in_refresh_an_expired_kept_session() {
    let harness = Harness::start_with(|cloud, setup| {
        keep_expired_session(cloud, setup);
        let mut state = setup.probe.state.lock().unwrap();
        let mut install = FakeProbe::connected(&cloud.site_url);
        // Already named: isolate upload-driven renewal from the legacy name lookup.
        install["workspace_name"] = json!("Acme");
        state.installs = vec![install];
        let mut healthy = blocked_on_sign_in();
        healthy["delivery"]["failure"] = Value::Null;
        healthy["delivery"]["failure_code"] = Value::Null;
        state.status = healthy;
        *setup.supervise_every.lock().unwrap() = Some(Duration::from_millis(100));
    })
    .await;
    let refreshes = || harness.cloud.requests("/cloud/api/v1/auth/token/refresh");
    // With no workspace binding, linking the agents renews the kept session
    // once.
    eventually("renewed to bind the workspace", || {
        harness.store()["cloudSession"]["refreshToken"] == "refresh-2"
            && harness.store()["cloud"]["accountWorkspaceID"] == WORKSPACE
    })
    .await;
    assert_eq!(refreshes().len(), 1);
    assert_eq!(
        harness.store()["cloudSession"]["accessTokenExpiresAt"],
        "2099-01-01T00:00:00Z"
    );

    // Uploads blocked on sign-in find the session already fresh.
    harness.probe.state.lock().unwrap().status = blocked_on_sign_in();
    // A fresh session is left alone while the collector renews from it.
    let calls = harness.probe.calls("status").len();
    eventually("supervised again", || {
        harness.probe.calls("status").len() >= calls + 3
    })
    .await;
    assert_eq!(refreshes().len(), 1);
    harness.stop().await;
}

/// A refresh Cloud keeps failing backs off from one supervision period,
/// doubling, rather than asking on every tick while uploads stay blocked.
#[tokio::test]
async fn a_refresh_for_blocked_uploads_backs_off_while_cloud_fails_it() {
    let harness = Harness::start_with(|cloud, setup| {
        keep_expired_session(cloud, setup);
        cloud.state.lock().unwrap().refresh_status = Some(503);
        let mut state = setup.probe.state.lock().unwrap();
        state.installs = vec![FakeProbe::connected(&cloud.site_url)];
        state.status = blocked_on_sign_in();
        *setup.supervise_every.lock().unwrap() = Some(Duration::from_millis(100));
    })
    .await;
    let refreshes = || harness.cloud.requests("/cloud/api/v1/auth/token/refresh");
    eventually("first refresh", || !refreshes().is_empty()).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let ticks = harness.probe.calls("status").len();
    let asked = refreshes().len();
    // About 20 ticks; 0.2 + 0.4 + 0.8 s of backoff allows four asks.
    assert!(ticks >= 10, "{ticks} ticks");
    assert!(
        (2..=5).contains(&asked),
        "{asked} refreshes in {ticks} ticks"
    );
    // The kept session is kept for a later try.
    assert_eq!(harness.store()["cloudSession"]["refreshToken"], "refresh-1");
    harness.stop().await;
}

#[test]
fn desktop_default_is_google_browser_on_every_platform() {
    assert_eq!(Flow::platform(), Flow::Browser);
}

#[tokio::test]
async fn desktop_setup_preserves_device_approval_for_existing_clients() {
    let harness = Harness::start().await;
    for flow in [json!("unknown"), json!(true), Value::Null] {
        let error = harness
            .setup_sign_in(json!({"flow": flow}))
            .await
            .unwrap_err();
        assert_eq!(error.code, "invalid_request");
    }
    let started = harness.setup_sign_in(json!({})).await.unwrap();
    assert_eq!(started["status"], "pending_approval");
    assert_eq!(started["user_code"], "ABCD-EFGH");
    let mut client = harness.client().await;
    client.ok("account.sign_in.cancel", json!({})).await;
    harness.stop().await;
}

#[tokio::test]
async fn native_workspace_choice_waits_before_setup_and_can_be_cancelled() {
    let harness = Harness::start_with(|cloud, setup| {
        let (access, refresh) = cloud.issue();
        keep_session(setup, &cloud.site_url, &access, &refresh);
        cloud.state.lock().unwrap().desktop_workspaces = Some(json!([
            {"id": WORKSPACE, "name":"Acme", "organization_name":"Company"},
            {"id":"11111111-1111-4111-8111-111111111111", "name":"Other", "organization_name":null}
        ]));
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"selected"}))
        .await;
    eventually("native picker", || {
        harness.account.status().phase.as_deref() == Some("selecting_workspace")
    })
    .await;
    assert!(harness.probe.calls("cloud").is_empty());
    assert!(harness
        .cloud
        .requests(&format!("/cloud/api/v1/workspaces/{WORKSPACE}/resolve"))
        .is_empty());
    client
        .error(
            "account.select_workspace",
            json!({"workspace_id":"not-a-member"}),
        )
        .await;
    assert!(harness.probe.calls("cloud").is_empty());
    client.ok("account.sign_in.cancel", json!({})).await;
    eventually("cancelled picker", || {
        harness.account_state() == "signed_out"
    })
    .await;
    client
        .error(
            "account.select_workspace",
            json!({"workspace_id":WORKSPACE}),
        )
        .await;
    assert!(harness.probe.calls("cloud").is_empty());
    harness.stop().await;
}

#[tokio::test]
async fn native_workspace_choice_rebinds_the_session_before_setup() {
    let harness = Harness::start_with(|cloud, setup| {
        let (access, refresh) = cloud.issue();
        keep_session(setup, &cloud.site_url, &access, &refresh);
        let mut state = cloud.state.lock().unwrap();
        state.whoami_workspace = "11111111-1111-4111-8111-111111111111".into();
        state.desktop_workspaces = Some(json!([
            {"id": state.whoami_workspace, "name":"Other", "organization_name":null},
            {"id": WORKSPACE, "name":"Acme", "organization_name":"Company"}
        ]));
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"selected"}))
        .await;
    eventually("native picker", || {
        harness.account.status().phase.as_deref() == Some("selecting_workspace")
    })
    .await;
    client
        .ok(
            "account.select_workspace",
            json!({"workspace_id":WORKSPACE}),
        )
        .await;
    eventually("selected workspace connected", || {
        harness.account_state() == "signed_in"
    })
    .await;
    let selected = harness
        .cloud
        .requests("/cloud/api/v1/auth/desktop/workspaces");
    assert_eq!(
        selected.iter().find(|r| r.method == "POST").unwrap().body["workspaceId"],
        WORKSPACE
    );
    assert_eq!(
        harness.probe.calls("cloud")[0].1.as_deref(),
        Some("access-2")
    );
    assert_eq!(
        harness.account.status().workspace_id.as_deref(),
        Some(WORKSPACE)
    );
    harness.stop().await;
}

#[tokio::test]
async fn setup_uses_the_rotated_token_after_workspace_adoption() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let harness = Harness::start_with(|cloud, setup| {
        let (access, refresh) = cloud.issue();
        keep_session(setup, &cloud.site_url, &access, &refresh);
        let path = setup.home.join("app-data/injectable-agents.json");
        let mut stored: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        stored["cloud"] = json!({"key":ACCOUNT_KEY,"baseURL":"http://127.0.0.1:9","accountWorkspaceID":WORKSPACE});
        std::fs::write(path, stored.to_string()).unwrap();
        cloud.state.lock().unwrap().resolve_gate = Some(gate.clone());
    }).await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"selected"}))
        .await;
    eventually("workspace resolution waiting", || {
        !harness
            .cloud
            .requests(&format!("/cloud/api/v1/workspaces/{WORKSPACE}/resolve"))
            .is_empty()
    })
    .await;
    // Force exactly the rotation that a background 401 triggers while the
    // sign-in task still holds its original local copy of the credential.
    harness.cloud.expire_access();
    harness
        .account
        .request("GET", "/members".into(), None)
        .await
        .unwrap();
    let renewed = harness.store()["cloudSession"]["accessToken"]
        .as_str()
        .unwrap()
        .to_string();
    gate.notify_one();
    eventually("signed in with latest token", || {
        harness.account_state() == "signed_in"
    })
    .await;
    assert_eq!(
        harness.probe.calls("cloud")[0].1.as_deref(),
        Some(renewed.as_str())
    );
    harness.stop().await;
}

#[tokio::test]
async fn logout_then_browser_login_uses_a_new_session_and_the_native_picker() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing":"selected","flow":"device"}),
        )
        .await;
    eventually("first sign-in", || harness.account_state() == "signed_in").await;
    let old_token = harness.store()["cloudSession"]["accessToken"].clone();
    client.ok("account.sign_out", json!({})).await;
    assert_eq!(harness.account_state(), "signed_out");
    harness.cloud.state.lock().unwrap().desktop_workspaces = Some(json!([
        {"id":WORKSPACE,"name":"Acme","organization_name":null},
        {"id":"11111111-1111-4111-8111-111111111111","name":"Other","organization_name":null}
    ]));
    client
        .ok("account.sign_in", json!({"sharing":"selected"}))
        .await;
    eventually("browser return", || {
        harness.account_state() == "pending_approval"
    })
    .await;
    let report = client.ok("account.status", json!({})).await;
    let link = url::Url::parse(report["verification_url_complete"].as_str().unwrap()).unwrap();
    assert!(link
        .query_pairs()
        .any(|(key, value)| key == "workspace_selection" && value == "desktop"));
    let redirect = link
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .unwrap()
        .1
        .to_string();
    let (handoff, _) = harness.cloud.issue();
    assert!(reqwest_get(&format!(
        "{redirect}?access_token={handoff}&api_url={}/cloud",
        harness.cloud.site_url
    ))
    .await
    .starts_with("HTTP/1.1 200"));
    eventually("picker after re-login", || {
        harness.account.status().phase.as_deref() == Some("selecting_workspace")
    })
    .await;
    assert_eq!(harness.probe.calls("cloud").len(), 1);
    // The newly exchanged browser session is not saved yet. It must also
    // renew if the picker remains open past its access-token lifetime.
    harness.cloud.expire_access();
    client
        .ok(
            "account.select_workspace",
            json!({"workspace_id":WORKSPACE}),
        )
        .await;
    eventually("reconnected", || harness.account_state() == "signed_in").await;
    assert_ne!(harness.store()["cloudSession"]["accessToken"], old_token);
    assert_eq!(harness.probe.calls("cloud").len(), 2);
    harness.stop().await;
}

#[tokio::test]
async fn one_workspace_continues_automatically_and_no_workspaces_starts_no_setup() {
    for count in [0, 1] {
        let harness = Harness::start_with(|cloud, setup| {
            let (access, refresh) = cloud.issue();
            keep_session(setup, &cloud.site_url, &access, &refresh);
            cloud.state.lock().unwrap().desktop_workspaces = Some(if count == 0 {
                json!([])
            } else {
                json!([{"id":WORKSPACE,"name":"Acme","organization_name":null}])
            });
        })
        .await;
        let mut client = harness.client().await;
        client
            .ok("account.sign_in", json!({"sharing":"selected"}))
            .await;
        eventually("single workspace outcome", || {
            if count == 0 {
                harness.account.status().action == Some(AccountAction::CreateWorkspace)
            } else {
                harness.account_state() == "signed_in"
            }
        })
        .await;
        assert_eq!(harness.probe.calls("cloud").len(), count);
        assert!(harness.account.status().workspace_choices.is_none());
        if count == 0 {
            assert!(harness
                .account
                .status()
                .error
                .unwrap()
                .contains("no workspace yet"));
            assert_eq!(harness.account_state(), "error");
        }
        harness.stop().await;
    }
}

#[tokio::test]
async fn setup_holds_rotation_until_history_connects_but_signout_can_cancel_it() {
    let harness = Harness::start_with(|cloud, setup| {
        let (access, refresh) = cloud.issue();
        keep_session(setup, &cloud.site_url, &access, &refresh);
        setup.probe.state.lock().unwrap().hold_before_connection = true;
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"selected"}))
        .await;
    eventually("history exchange waiting", || {
        harness.account.status().phase.as_deref() == Some("preparing_local_sessions")
    })
    .await;
    harness.cloud.expire_access();
    let account = harness.account.clone();
    let read = tokio::spawn(async move { account.request("GET", "/members".into(), None).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !read.is_finished(),
        "a Cloud read must wait for the setup credential lease"
    );
    assert!(harness
        .cloud
        .requests("/cloud/api/v1/auth/token/refresh")
        .is_empty());
    tokio::time::timeout(Duration::from_secs(2), harness.account.sign_out())
        .await
        .expect("sign-out cancels setup before waiting for its lease")
        .unwrap();
    assert_eq!(harness.account_state(), "signed_out");
    assert!(harness.store()["cloudSession"].is_null());
    let _ = read.await;
    harness.stop().await;
}

#[tokio::test]
async fn headless_device_setup_with_multiple_workspaces_uses_current_workspace() {
    let harness = Harness::start_with(|cloud, _| {
        cloud.state.lock().unwrap().desktop_workspaces = Some(json!([
            {"id":WORKSPACE,"name":"Acme","organization_name":null},
            {"id":"11111111-1111-4111-8111-111111111111","name":"Other","organization_name":null}
        ]));
    })
    .await;
    harness.setup_sign_in(json!({})).await.unwrap();
    eventually(
        "headless device setup connects without native picker",
        || harness.account_state() == "signed_in",
    )
    .await;
    assert_eq!(
        harness.account.status().workspace_id.as_deref(),
        Some(WORKSPACE)
    );
    assert!(harness.account.status().workspace_choices.is_none());
    harness.stop().await;
}

#[tokio::test]
async fn picker_uses_renewed_credentials_after_a_long_wait() {
    for rotate_in_background in [false, true] {
        let harness = Harness::start_with(|cloud, setup| {
            let (access, refresh) = cloud.issue();
            keep_session(setup, &cloud.site_url, &access, &refresh);
            let path = setup.home.join("app-data/injectable-agents.json");
            let mut stored: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            stored["cloud"] = json!({"key":ACCOUNT_KEY,"baseURL":"http://127.0.0.1:9","accountWorkspaceID":WORKSPACE});
            std::fs::write(path, stored.to_string()).unwrap();
            let mut state = cloud.state.lock().unwrap();
            state.whoami_workspace = "11111111-1111-4111-8111-111111111111".into();
            state.desktop_workspaces = Some(json!([
                {"id":state.whoami_workspace,"name":"Other","organization_name":null},
                {"id":WORKSPACE,"name":"Acme","organization_name":null}
            ]));
        })
        .await;
        let mut client = harness.client().await;
        client
            .ok("account.sign_in", json!({"sharing":"selected"}))
            .await;
        eventually("native picker", || {
            harness.account.status().phase.as_deref() == Some("selecting_workspace")
        })
        .await;
        harness.cloud.expire_access();
        if rotate_in_background {
            harness
                .account
                .request("GET", "/members".into(), None)
                .await
                .unwrap();
        }
        client
            .ok(
                "account.select_workspace",
                json!({"workspace_id":WORKSPACE}),
            )
            .await;
        eventually("picker refreshes before binding", || {
            harness.account_state() == "signed_in"
        })
        .await;
        let requests = harness
            .cloud
            .requests("/cloud/api/v1/auth/desktop/workspaces");
        let binding = requests.iter().rev().find(|r| r.method == "POST").unwrap();
        assert_eq!(binding.bearer.as_deref(), Some("access-2"));
        assert_eq!(
            harness.probe.calls("cloud")[0].1.as_deref(),
            Some("access-3")
        );
        harness.stop().await;
    }
}

#[tokio::test]
async fn an_attached_install_with_only_an_environment_session_adopts_the_workspace_binding() {
    // A headless core attached by setup never ran an interactive sign-in:
    // it binds the account workspace itself, so its agents reach Cloud.
    let harness = Harness::start_with(|cloud, setup| {
        setup.probe.state.lock().unwrap().installs = vec![FakeProbe::connected(&cloud.site_url)];
        borrow_env_session(cloud, setup);
    })
    .await;
    eventually("workspace binding adopted", || {
        harness.store()["cloud"]["accountWorkspaceID"] == WORKSPACE
    })
    .await;
    eventually("identity sync sent", || {
        harness
            .cloud
            .state
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|r| r.method == "PUT" && r.path.ends_with("/messaging/desktop-identities"))
    })
    .await;
    let status = harness
        .setup_request("GET /setup/status", json!({}))
        .await
        .unwrap();
    assert_eq!(status["cloud_identity_sync"], "ready");
    harness.stop().await;
}

#[tokio::test]
async fn an_attached_install_with_no_session_reports_it() {
    let harness = Harness::start_with(|cloud, setup| {
        setup.probe.state.lock().unwrap().installs = vec![FakeProbe::connected(&cloud.site_url)];
    })
    .await;
    eventually("discovered", || harness.account_state() == "signed_in").await;
    let mut reported = Value::Null;
    for _ in 0..200 {
        reported = harness
            .setup_request("GET /setup/status", json!({}))
            .await
            .unwrap()["cloud_identity_sync"]
            .clone();
        if !reported.is_null() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(reported, "no_session");
    harness.stop().await;
}

/// Polls `GET /setup/status` until `cloud_identity_sync` reads `want`.
async fn identity_sync_reads(harness: &Harness, want: &str) {
    let mut reported = Value::Null;
    for _ in 0..300 {
        reported = harness
            .setup_request("GET /setup/status", json!({}))
            .await
            .unwrap()["cloud_identity_sync"]
            .clone();
        if reported == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("cloud_identity_sync stayed {reported}, not {want}");
}

fn identity_puts(harness: &Harness) -> usize {
    harness
        .cloud
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|r| r.method == "PUT" && r.path.ends_with("/messaging/desktop-identities"))
        .count()
}

#[tokio::test]
async fn another_person_s_environment_session_neither_binds_nor_syncs() {
    // The environment's session is someone else's: it must not carry this
    // computer's agent tokens to Cloud.
    let harness = Harness::start_with(|cloud, setup| {
        setup.probe.state.lock().unwrap().installs = vec![FakeProbe::connected(&cloud.site_url)];
        borrow_env_session(cloud, setup);
        cloud.state.lock().unwrap().whoami_user = "someone-else".into();
        *setup.supervise_every.lock().unwrap() = Some(Duration::from_millis(50));
    })
    .await;
    identity_sync_reads(&harness, "session_mismatch").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(harness.store()["cloud"].is_null());
    assert!(
        harness
            .cloud
            .requests("/cloud/api/v1/workspaces/")
            .is_empty()
            || harness
                .cloud
                .state
                .lock()
                .unwrap()
                .requests
                .iter()
                .all(|r| !r.path.ends_with("/resolve"))
    );
    assert_eq!(identity_puts(&harness), 0);
    harness.stop().await;
}

#[tokio::test]
async fn an_expired_kept_session_is_renewed_to_bind_the_workspace() {
    // This app's own kept session may be renewed: an expired one with a
    // valid refresh token still links the agents.
    let harness = Harness::start_with(|cloud, setup| {
        keep_expired_session(cloud, setup);
        setup.probe.state.lock().unwrap().installs = vec![FakeProbe::connected(&cloud.site_url)];
        *setup.supervise_every.lock().unwrap() = Some(Duration::from_millis(50));
    })
    .await;
    eventually("workspace binding adopted", || {
        harness.store()["cloud"]["accountWorkspaceID"] == WORKSPACE
    })
    .await;
    assert!(!harness
        .cloud
        .requests("/cloud/api/v1/auth/token/refresh")
        .is_empty());
    identity_sync_reads(&harness, "ready").await;
    harness.stop().await;
}

#[tokio::test]
async fn a_binding_that_cloud_refused_is_retried_and_recovers() {
    let harness = Harness::start_with(|cloud, setup| {
        setup.probe.state.lock().unwrap().installs = vec![FakeProbe::connected(&cloud.site_url)];
        borrow_env_session(cloud, setup);
        cloud.state.lock().unwrap().resolve_status = 503;
        *setup.supervise_every.lock().unwrap() = Some(Duration::from_millis(20));
    })
    .await;
    identity_sync_reads(&harness, "no_binding").await;
    harness.cloud.state.lock().unwrap().resolve_status = 200;
    eventually("recovered", || {
        harness.store()["cloud"]["accountWorkspaceID"] == WORKSPACE
    })
    .await;
    identity_sync_reads(&harness, "ready").await;
    harness.stop().await;
}

#[tokio::test]
async fn a_binding_resolved_after_the_sign_in_changed_is_dropped() {
    // Cloud answers the resolve only after the person signed out: the answer
    // belongs to the earlier sign-in, so it is neither saved nor counted as a
    // failure.
    let gate = Arc::new(tokio::sync::Notify::new());
    let held = gate.clone();
    let harness = Harness::start_with(move |cloud, setup| {
        setup.probe.state.lock().unwrap().installs = vec![FakeProbe::connected(&cloud.site_url)];
        borrow_env_session(cloud, setup);
        cloud.state.lock().unwrap().resolve_gate = Some(held);
    })
    .await;
    eventually("resolve asked", || {
        harness
            .cloud
            .state
            .lock()
            .unwrap()
            .requests
            .iter()
            .any(|r| r.path.ends_with("/resolve"))
    })
    .await;
    harness.cloud.state.lock().unwrap().resolve_gate = None;
    let mut client = harness.client().await;
    client.ok("account.sign_out", json!({})).await;
    gate.notify_one();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        harness.store()["cloud"].is_null(),
        "the answer for the earlier sign-in was saved"
    );
    harness.stop().await;
}

#[tokio::test]
async fn a_bound_computer_sends_no_agent_tokens_with_another_person_s_environment_session() {
    // Already bound by an earlier sign-in, now given an environment session
    // that is someone else's: the identity sync must not run with it.
    let harness = Harness::start_with(|cloud, setup| {
        setup.probe.state.lock().unwrap().installs = vec![FakeProbe::connected(&cloud.site_url)];
        borrow_env_session(cloud, setup);
        cloud.state.lock().unwrap().whoami_user = "someone-else".into();
        std::fs::create_dir_all(setup.home.join("app-data")).unwrap();
        std::fs::write(
            setup.home.join("app-data/injectable-agents.json"),
            json!({"registrations": [], "tokens": {}, "cloud": {"key": ACCOUNT_KEY,
                   "baseURL": "http://127.0.0.1:9", "accountWorkspaceID": WORKSPACE}})
            .to_string(),
        )
        .unwrap();
        *setup.supervise_every.lock().unwrap() = Some(Duration::from_millis(50));
    })
    .await;
    identity_sync_reads(&harness, "session_mismatch").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(identity_puts(&harness), 0);
    harness.stop().await;
}

#[tokio::test]
async fn a_whoami_that_fails_is_retried_with_backoff_and_not_called_a_mismatch() {
    let harness = Harness::start_with(|cloud, setup| {
        setup.probe.state.lock().unwrap().installs = vec![FakeProbe::connected(&cloud.site_url)];
        borrow_env_session(cloud, setup);
        // Cloud cannot say whose the environment's session is.
        cloud.state.lock().unwrap().whoami_status = Some(503);
        *setup.supervise_every.lock().unwrap() = Some(Duration::from_millis(20));
    })
    .await;
    identity_sync_reads(&harness, "no_binding").await;
    let before = harness.cloud.requests("/cloud/api/v1/auth/whoami").len();
    tokio::time::sleep(Duration::from_millis(600)).await;
    let asked = harness.cloud.requests("/cloud/api/v1/auth/whoami").len() - before;
    // About 30 passes; 20 + 40 + 80 + 160 + 320 ms of backoff allows ~5 asks.
    assert!(asked <= 7, "{asked} whoami calls without backing off");
    assert!(
        harness.store()["cloud"].is_null(),
        "bound on a whoami error"
    );
    harness.cloud.state.lock().unwrap().whoami_status = None;
    identity_sync_reads(&harness, "ready").await;
    harness.stop().await;
}

#[tokio::test]
async fn a_kept_session_that_cannot_be_renewed_settles_on_no_session() {
    let harness = Harness::start_with(|cloud, setup| {
        keep_expired_session(cloud, setup);
        cloud.state.lock().unwrap().refresh_status = Some(503);
        setup.probe.state.lock().unwrap().installs = vec![FakeProbe::connected(&cloud.site_url)];
        *setup.supervise_every.lock().unwrap() = Some(Duration::from_millis(20));
    })
    .await;
    identity_sync_reads(&harness, "no_session").await;
    for _ in 0..20 {
        let status = harness
            .setup_request("GET /setup/status", json!({}))
            .await
            .unwrap();
        assert_eq!(status["cloud_identity_sync"], "no_session");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    harness.stop().await;
}

#[tokio::test]
async fn a_renewal_the_uploads_already_failed_reports_no_session() {
    // Uploads blocked on sign-in tried to renew the kept session and Cloud
    // refused the refresh token: the person must sign in again.
    let harness = Harness::start_with(|cloud, setup| {
        keep_expired_session(cloud, setup);
        cloud.state.lock().unwrap().refresh_status = Some(401);
        let mut state = setup.probe.state.lock().unwrap();
        state.installs = vec![FakeProbe::connected(&cloud.site_url)];
        state.status = blocked_on_sign_in();
        *setup.supervise_every.lock().unwrap() = Some(Duration::from_millis(20));
    })
    .await;
    identity_sync_reads(&harness, "no_session").await;
    for _ in 0..20 {
        let status = harness
            .setup_request("GET /setup/status", json!({}))
            .await
            .unwrap();
        assert_eq!(status["cloud_identity_sync"], "no_session");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(harness.store()["cloud"].is_null());
    harness.stop().await;
}

#[tokio::test]
async fn auth_health_refreshes_revoked_access_and_reports_missing_or_unreachable_auth() {
    let harness = Harness::start_with(|cloud, _| {
        let mut state = cloud.state.lock().unwrap();
        state.relaycast_url = Some(cloud.site_url.clone());
        state.relaycast_status = 503;
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;

    harness.cloud.expire_access();
    harness
        .core
        .hub()
        .update(|status| status.account.auth_checked_at = None);
    harness.account.check_auth_health().await;
    assert_eq!(harness.store()["cloudSession"]["refreshToken"], "refresh-2");
    // A valid Cloud login alone cannot make the connection ready: this
    // fixture intentionally has no reachable Relaycast service.
    assert_ne!(
        harness.core.status().account.auth_health.as_deref(),
        Some("ready")
    );

    harness.cloud.expire_access();
    harness.cloud.state.lock().unwrap().refresh_status = Some(503);
    harness
        .core
        .hub()
        .update(|status| status.account.auth_checked_at = None);
    harness.account.check_auth_health().await;
    assert_eq!(
        harness.core.status().account.auth_health.as_deref(),
        Some("offline")
    );
    assert_eq!(harness.store()["cloudSession"]["refreshToken"], "refresh-2");

    {
        let mut cloud = harness.cloud.state.lock().unwrap();
        cloud.refresh_status = None;
        cloud.refresh_tokens.clear();
    }
    harness
        .core
        .hub()
        .update(|status| status.account.auth_checked_at = None);
    harness.account.check_auth_health().await;
    assert_eq!(
        harness.core.status().account.auth_health.as_deref(),
        Some("sign_in_required")
    );
    assert_eq!(harness.store()["cloudSession"]["refreshToken"], "refresh-2");
    assert!(harness.core.status().account.auth_checked_at.is_some());
    harness.stop().await;
}

#[tokio::test]
async fn auth_health_repairs_a_rejected_account_key_but_preserves_it_on_transient_failure() {
    let harness = Harness::start_with(|cloud, _| {
        cloud.state.lock().unwrap().relaycast_url = Some(cloud.site_url.clone());
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok(
            "account.sign_in",
            json!({"sharing": "new", "flow": "device"}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    harness.account.recheck_auth_health().await;
    assert_eq!(
        harness.core.status().account.auth_health.as_deref(),
        Some("ready")
    );

    let resolutions = harness
        .cloud
        .requests(&format!("/cloud/api/v1/workspaces/{WORKSPACE}/resolve"))
        .len();
    harness.cloud.state.lock().unwrap().relaycast_status = 503;
    harness.account.recheck_auth_health().await;
    assert_eq!(
        harness.core.status().account.auth_health.as_deref(),
        Some("offline")
    );
    assert_eq!(
        harness
            .cloud
            .requests(&format!("/cloud/api/v1/workspaces/{WORKSPACE}/resolve"))
            .len(),
        resolutions
    );
    assert_eq!(harness.core.workspace().unwrap().key.expose(), ACCOUNT_KEY);

    let rotated = "rk_live_rotated0000000000";
    {
        let mut cloud = harness.cloud.state.lock().unwrap();
        cloud.relaycast_status = 200;
        cloud.relaycast_key = rotated.into();
    }
    harness.account.recheck_auth_health().await;
    assert_eq!(
        harness.core.status().account.auth_health.as_deref(),
        Some("ready")
    );
    assert_eq!(harness.core.workspace().unwrap().key.expose(), rotated);
    assert!(
        harness
            .cloud
            .requests(&format!("/cloud/api/v1/workspaces/{WORKSPACE}/resolve"))
            .len()
            > resolutions
    );
    harness.stop().await;
}

#[tokio::test]
async fn slow_auth_checks_coalesce_without_delaying_uploader_supervision() {
    let harness = Harness::start_with(|cloud, setup| {
        cloud.state.lock().unwrap().relaycast_url = Some(cloud.site_url.clone());
        *setup.supervise_every.lock().unwrap() = Some(Duration::from_millis(20));
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"new", "flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    harness.account.recheck_auth_health().await;
    let before = harness.cloud.requests("/cloud/api/v1/auth/whoami").len();
    let gate = Arc::new(tokio::sync::Notify::new());
    harness.cloud.state.lock().unwrap().whoami_gate = Some(gate.clone());
    let first = {
        let account = harness.account.clone();
        tokio::spawn(async move { account.recheck_auth_health().await })
    };
    eventually("check in flight", || {
        harness.cloud.requests("/cloud/api/v1/auth/whoami").len() > before
    })
    .await;
    let second = {
        let account = harness.account.clone();
        tokio::spawn(async move { account.recheck_auth_health().await })
    };
    let statuses = harness.probe.calls("status").len();
    eventually(
        "uploader stays supervised during slow authentication",
        || harness.probe.calls("status").len() >= statuses + 3,
    )
    .await;
    harness.cloud.state.lock().unwrap().whoami_gate = None;
    gate.notify_waiters();
    first.await.unwrap();
    second.await.unwrap();
    assert_eq!(
        harness.cloud.requests("/cloud/api/v1/auth/whoami").len(),
        before + 1
    );
    assert_eq!(
        harness.core.status().account.auth_health.as_deref(),
        Some("ready")
    );
    harness.stop().await;
}

#[tokio::test]
async fn hosted_mcp_health_checks_discovery_without_creating_a_participant() {
    let harness = Harness::start_with(|cloud, _| {
        cloud.state.lock().unwrap().relaycast_url = Some(cloud.site_url.clone());
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"new", "flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    harness.account.recheck_auth_health().await;
    assert_eq!(
        harness.core.status().account.auth_health.as_deref(),
        Some("ready")
    );
    let probes = harness.cloud.requests("/cloud/api/v1/mcp/shared-sessions");
    assert!(probes.iter().any(|r| r.body["method"] == "initialize"));
    assert!(probes.iter().any(|r| r.body["method"] == "tools/list"));
    assert!(probes
        .iter()
        .all(|r| matches!(r.body["method"].as_str(), Some("initialize" | "tools/list"))));
    let session = harness.store()["cloudSession"].clone();
    assert!(probes
        .iter()
        .all(|r| r.bearer.as_deref() == session["accessToken"].as_str()));

    harness.cloud.state.lock().unwrap().mcp_status = 503;
    harness.account.recheck_auth_health().await;
    assert_eq!(
        harness.core.status().account.auth_health.as_deref(),
        Some("offline")
    );
    assert_eq!(harness.store()["cloudSession"], session);

    harness.cloud.state.lock().unwrap().mcp_status = 403;
    harness.account.recheck_auth_health().await;
    assert_eq!(
        harness.core.status().account.auth_health.as_deref(),
        Some("sign_in_required")
    );
    assert!(!harness.store()["cloudSession"].is_null());
    harness.stop().await;
}

#[tokio::test]
async fn hosted_mcp_missing_on_a_self_hosted_site_keeps_the_connection_ready_and_notices_once() {
    let harness = Harness::start_with(|cloud, _| {
        cloud.state.lock().unwrap().relaycast_url = Some(cloud.site_url.clone());
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"new", "flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    let mut events = harness.core.hub().subscribe();
    let mut notices = move || {
        let mut count = 0;
        while let Ok(event) = events.try_recv() {
            if event.topic == agent_relay_node::relay::protocol::Topic::Activity
                && event.data["message"]
                    .as_str()
                    .is_some_and(|m| m.starts_with("Hosted Agent Relay MCP is not available on"))
            {
                count += 1;
            }
        }
        count
    };
    let health = || harness.core.status().account.auth_health;

    // The fake Cloud is not production, so a 404 from the hosted MCP route
    // means this site has none: the connection stays verified and Activity
    // says so once, not on every check.
    harness.cloud.state.lock().unwrap().mcp_status = 404;
    harness.account.recheck_auth_health().await;
    assert_eq!(health().as_deref(), Some("ready"));
    assert_eq!(notices(), 1);
    harness.account.recheck_auth_health().await;
    assert_eq!(health().as_deref(), Some("ready"));
    assert_eq!(notices(), 0);
    // A provider chat that needs the hosted MCP is refused with the same
    // words, not reported as a Cloud failure to retry.
    let (kind, message) = client
        .error("account.mcp_headers", pinned_mcp_request(&harness))
        .await;
    assert_eq!(kind, "refused");
    assert!(
        message.starts_with("Hosted Agent Relay MCP is not available on"),
        "{message}"
    );

    // An outage in between does not make the notice repeat.
    harness.cloud.state.lock().unwrap().mcp_status = 503;
    harness.account.recheck_auth_health().await;
    assert_eq!(health().as_deref(), Some("offline"));
    harness.cloud.state.lock().unwrap().mcp_status = 404;
    harness.account.recheck_auth_health().await;
    assert_eq!(health().as_deref(), Some("ready"));
    assert_eq!(notices(), 0);

    // Only the route answering again forgets the finding; losing it after
    // that is a new one.
    harness.cloud.state.lock().unwrap().mcp_status = 200;
    harness.account.recheck_auth_health().await;
    assert_eq!(health().as_deref(), Some("ready"));
    assert_eq!(notices(), 0);

    // A route that answers initialize but not tools/list is a broken hosted
    // MCP, not an absent one: the connection goes offline and nothing is
    // said about the site.
    let bearer = harness.store()["cloudSession"]["accessToken"]
        .as_str()
        .unwrap()
        .to_string();
    let tools_list = (
        "/cloud/api/v1/mcp/shared-sessions".to_string(),
        bearer,
        "tools/list".to_string(),
    );
    harness
        .cloud
        .state
        .lock()
        .unwrap()
        .response_overrides
        .insert(tools_list.clone(), (404, json!({"error":"no such route"})));
    harness.account.recheck_auth_health().await;
    assert_eq!(health().as_deref(), Some("offline"));
    assert_eq!(notices(), 0);
    harness
        .cloud
        .state
        .lock()
        .unwrap()
        .response_overrides
        .remove(&tools_list);
    harness.cloud.state.lock().unwrap().mcp_status = 404;
    harness.account.recheck_auth_health().await;
    assert_eq!(health().as_deref(), Some("ready"));
    assert_eq!(notices(), 1);
    harness.stop().await;
}

fn pinned_mcp_request(harness: &Harness) -> Value {
    json!({"site_url":harness.cloud.site_url,"workspace_id":WORKSPACE,"account_id":USER})
}

#[tokio::test]
async fn hosted_headers_use_only_the_pinned_desktop_login_and_follow_rotation() {
    let harness = Harness::start_with(|cloud, _| {
        cloud.state.lock().unwrap().relaycast_url = Some(cloud.site_url.clone());
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"new","flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    let pins = pinned_mcp_request(&harness);
    let first = client.ok("account.mcp_headers", pins.clone()).await;
    assert_eq!(first["headers"]["Authorization"], "Bearer access-1");
    for (field, wrong) in [
        ("site_url", "https://another.example"),
        ("workspace_id", "other-workspace"),
        ("account_id", "other-person"),
    ] {
        let mut request = pins.clone();
        request[field] = json!(wrong);
        let (kind, message) = client.error("account.mcp_headers", request).await;
        assert_eq!(kind, "not_signed_in");
        assert!(!message.contains("access-"));
    }
    harness.cloud.expire_access();
    let rotated = client.ok("account.mcp_headers", pins.clone()).await;
    assert_eq!(rotated["headers"]["Authorization"], "Bearer access-2");
    let status = client.ok("status.get", json!({})).await.to_string();
    assert!(!status.contains("access-"));
    assert!(!status.contains("refresh-"));
    client.ok("account.sign_out", json!({})).await;
    let (kind, message) = client.error("account.mcp_headers", pins).await;
    assert_eq!(kind, "not_signed_in");
    assert!(!message.contains("access-"));
    harness.stop().await;
}

fn pinned_feed_request(harness: &Harness, profile: &str) -> Value {
    let mut request = pinned_mcp_request(harness);
    request["profile_id"] = json!(profile);
    request
}

#[tokio::test]
async fn feed_headers_carry_the_pinned_login_and_the_site_profile() {
    let harness = Harness::start().await;
    let mut client = harness.client().await;
    // Signed out: no headers, and a sentence to show.
    let (kind, message) = client
        .error(
            "account.feed_headers",
            pinned_feed_request(&harness, "x7k2q"),
        )
        .await;
    assert_eq!(kind, "not_signed_in");
    assert!(message.starts_with("Sign in"), "{message}");
    client
        .ok("account.sign_in", json!({"sharing":"new","flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    // A site without the hosted MCP still serves its release feed.
    harness.cloud.state.lock().unwrap().mcp_status = 404;
    let pins = pinned_feed_request(&harness, "x7k2q");
    let first = client.ok("account.feed_headers", pins.clone()).await;
    assert_eq!(
        first["headers"],
        json!({
            "Authorization": "Bearer access-1",
            "X-Agent-Relay-Desktop": "1",
            "X-Agent-Relay-Profile": "x7k2q",
        })
    );
    for (field, wrong) in [
        ("site_url", "https://another.example"),
        ("workspace_id", "other-workspace"),
        ("account_id", "other-person"),
    ] {
        let mut request = pins.clone();
        request[field] = json!(wrong);
        let (kind, message) = client.error("account.feed_headers", request).await;
        assert_eq!(kind, "not_signed_in");
        assert!(!message.contains("access-"));
    }
    for profile in ["", "X7K2Q", "x7-k2", &"z".repeat(33)] {
        let (kind, message) = client
            .error(
                "account.feed_headers",
                pinned_feed_request(&harness, profile),
            )
            .await;
        assert_eq!(kind, "invalid_params", "{profile}");
        assert!(!message.contains("access-"));
    }
    // Asked again after Cloud rejects the token, the headers carry a renewed one.
    harness.cloud.expire_access();
    let rotated = client.ok("account.feed_headers", pins.clone()).await;
    assert_eq!(rotated["headers"]["Authorization"], "Bearer access-2");
    client.ok("account.sign_out", json!({})).await;
    let (kind, message) = client.error("account.feed_headers", pins).await;
    assert_eq!(kind, "not_signed_in");
    assert!(!message.contains("access-"));
    harness.stop().await;
}

#[tokio::test]
async fn hosted_headers_never_fall_back_to_environment_credentials() {
    for environment in [false, true] {
        let harness = Harness::start_with(|cloud, setup| {
            setup.probe.state.lock().unwrap().installs =
                vec![FakeProbe::connected(&cloud.site_url)];
            if environment {
                borrow_env_session(cloud, setup);
            }
        })
        .await;
        eventually("attached", || harness.account_state() == "signed_in").await;
        let mut client = harness.client().await;
        let (kind, message) = client
            .error("account.mcp_headers", pinned_mcp_request(&harness))
            .await;
        assert_eq!(kind, "not_signed_in");
        assert!(!message.contains("access-"));
        harness.account.recheck_auth_health().await;
        assert_eq!(
            harness.core.status().account.auth_health.as_deref(),
            Some("sign_in_required")
        );
        harness.stop().await;
    }
}

#[tokio::test]
async fn conflicting_environment_credentials_cannot_report_healthy_or_export_headers() {
    for conflicting in [false, true] {
        let harness = Harness::start_with(|cloud, setup| {
            cloud.state.lock().unwrap().relaycast_url = Some(cloud.site_url.clone());
            setup.probe.state.lock().unwrap().installs =
                vec![FakeProbe::connected(&cloud.site_url)];
            let (access, refresh) = cloud.issue();
            keep_session(setup, &cloud.site_url, &access, &refresh);
            *setup.cloud_token.lock().unwrap() =
                Some(if conflicting { cloud.issue().0 } else { access });
        })
        .await;
        eventually("attached", || harness.account_state() == "signed_in").await;
        // Binding adoption is independent maintenance; wait until it is present.
        eventually("workspace bound", || !harness.store()["cloud"].is_null()).await;
        harness.account.recheck_auth_health().await;
        let mut client = harness.client().await;
        if conflicting {
            assert_eq!(
                harness.core.status().account.auth_health.as_deref(),
                Some("sign_in_required")
            );
            assert!(harness
                .core
                .status()
                .account
                .auth_error
                .as_deref()
                .unwrap_or_default()
                .contains("CLOUD_API_ACCESS_TOKEN"));
            client
                .error("account.mcp_headers", pinned_mcp_request(&harness))
                .await;
        } else {
            assert_eq!(
                harness.core.status().account.auth_health.as_deref(),
                Some("ready")
            );
            let headers = client
                .ok("account.mcp_headers", pinned_mcp_request(&harness))
                .await;
            assert_eq!(headers["headers"]["Authorization"], "Bearer access-1");
        }
        harness.stop().await;
    }
}

#[tokio::test]
async fn hosted_headers_require_native_identity_support_without_signing_the_account_out() {
    let harness = Harness::start_with(|cloud, _| {
        let mut state = cloud.state.lock().unwrap();
        state.relaycast_url = Some(cloud.site_url.clone());
        state.mcp_native_identity = false;
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"new","flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    harness.account.recheck_auth_health().await;
    let health = harness.core.status().account;
    assert_eq!(health.auth_health.as_deref(), Some("offline"));
    assert!(health
        .auth_error
        .as_deref()
        .unwrap_or_default()
        .contains("Hosted Agent Relay needs an update"));
    let (kind, message) = client
        .error("account.mcp_headers", pinned_mcp_request(&harness))
        .await;
    assert_eq!(kind, "cloud_failed");
    assert!(message.contains("needs an update"));
    assert!(!message.contains("access-"));
    assert_eq!(harness.account_state(), "signed_in");
    assert!(!harness.store()["cloudSession"].is_null());
    harness.cloud.state.lock().unwrap().mcp_native_identity = true;
    harness.account.recheck_auth_health().await;
    assert_eq!(
        harness.core.status().account.auth_health.as_deref(),
        Some("ready")
    );
    assert!(harness.core.status().account.auth_error.is_none());
    assert_eq!(
        client
            .ok("account.mcp_headers", pinned_mcp_request(&harness))
            .await["headers"]["Authorization"],
        "Bearer access-1"
    );
    let checks = harness
        .cloud
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|r| r.body["params"]["clientInfo"]["name"] == "agent-relay-desktop-capability")
        .count();
    client
        .ok("account.mcp_headers", pinned_mcp_request(&harness))
        .await;
    assert_eq!(
        harness
            .cloud
            .state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|r| r.body["params"]["clientInfo"]["name"] == "agent-relay-desktop-capability")
            .count(),
        checks
    );
    harness.stop().await;
}

#[tokio::test]
async fn auth_health_refreshes_workspace_names_and_preserves_them_on_blank_answers() {
    let harness = Harness::start_with(|cloud, _| {
        cloud.state.lock().unwrap().relaycast_url = Some(cloud.site_url.clone());
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"new", "flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    harness.account.recheck_auth_health().await;
    assert_eq!(
        harness.account.status().workspace_name.as_deref(),
        Some("Acme")
    );
    harness.cloud.state.lock().unwrap().whoami_workspace_name = "Renamed workspace".into();
    harness.account.recheck_auth_health().await;
    assert_eq!(
        harness.account.status().workspace_name.as_deref(),
        Some("Renamed workspace")
    );
    harness.cloud.state.lock().unwrap().whoami_workspace_name = "  ".into();
    harness.account.recheck_auth_health().await;
    assert_eq!(
        harness.account.status().workspace_name.as_deref(),
        Some("Renamed workspace")
    );
    harness.stop().await;
}

#[tokio::test]
async fn auth_health_retries_missing_workspace_binding_without_requesting_another_login() {
    let harness = Harness::start_with(|cloud, _| {
        let mut state = cloud.state.lock().unwrap();
        state.relaycast_url = Some(cloud.site_url.clone());
        state.resolve_status = 500;
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"new","flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    assert!(harness.store()["cloud"].is_null());
    assert!(!harness.store()["cloudSession"].is_null());

    harness.account.recheck_auth_health().await;
    let health = harness.account.status();
    assert_eq!(health.auth_health.as_deref(), Some("offline"));
    assert_eq!(
        health.auth_error.as_deref(),
        Some("Agent Relay could not verify your connection. Retrying automatically.")
    );
    assert_eq!(harness.account_state(), "signed_in");
    harness.stop().await;
}

#[tokio::test]
async fn auth_health_offers_sign_in_when_workspace_rejection_cannot_be_recovered() {
    let harness = Harness::start_with(|cloud, _| {
        cloud.state.lock().unwrap().relaycast_url = Some(cloud.site_url.clone());
    })
    .await;
    let mut client = harness.client().await;
    client
        .ok("account.sign_in", json!({"sharing":"new", "flow":"device"}))
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
    harness.account.recheck_auth_health().await;
    assert_eq!(
        harness.account.status().auth_health.as_deref(),
        Some("ready")
    );
    harness.cloud.state.lock().unwrap().relaycast_status = 401;
    harness.account.recheck_auth_health().await;
    assert_eq!(
        harness.account.status().auth_health.as_deref(),
        Some("sign_in_required")
    );
    assert!(harness
        .account
        .status()
        .auth_error
        .unwrap()
        .contains("Sign in again"));
    assert_eq!(harness.account_state(), "signed_in");
    harness.stop().await;
}

#[path = "support/provider_credentials.rs"]
mod provider_credentials;

#[path = "support/provider_connections.rs"]
mod provider_connections;

// The workspace operator's upload policy (`history::operator_policy`),
// read from a file the test rewrites in place of Cloud's `access/me`.

const ENFORCED: &str = "This setting is set by your organization.";
const WITHHELD: &str = "Uploads wait until your organization's upload policy can be checked.";

/// Starts a core whose operator policy is read from `home/operator.json`,
/// written with `policy` first (`None` leaves it missing: a failed read).
async fn with_operator_policy(policy: Option<Value>) -> Harness {
    Harness::start_with(|_, setup| {
        let file = setup.home.join("operator.json");
        if let Some(policy) = &policy {
            std::fs::write(&file, policy.to_string()).unwrap();
        }
        setup.env.lock().unwrap().insert(
            "RELAY_DESKTOP_OPERATOR_POLICY".into(),
            file.to_str().unwrap().into(),
        );
    })
    .await
}

fn enforcing(mode: &str, revision: i64) -> Value {
    json!({"version": 1, "revision": revision, "settings": {"sharing_mode": mode}})
}

impl Harness {
    fn set_operator_policy(&self, policy: Value) {
        std::fs::write(self.home.join("operator.json"), policy.to_string()).unwrap();
    }

    /// The cached policy entry for the workspace.
    fn cached_policy(&self) -> Value {
        let saved: Value =
            serde_json::from_slice(&std::fs::read(self.home.join("app-data/policy.json")).unwrap())
                .unwrap();
        saved["workspaces"][WORKSPACE].clone()
    }

    /// Each `sharing set` run's mode.
    fn sharing_sets(&self) -> Vec<String> {
        self.probe
            .calls("sharing")
            .into_iter()
            .map(|(args, _)| args[args.iter().position(|a| a == "--mode").unwrap() + 1].clone())
            .collect()
    }
}

async fn sign_in_sharing(harness: &Harness, client: &mut Client, sharing: &str) {
    client
        .ok(
            "account.sign_in",
            json!({"sharing": sharing, "flow": "device"}),
        )
        .await;
    eventually("signed in", || harness.account_state() == "signed_in").await;
}

/// A sign-in while the operator enforces a mode sets up with that mode and
/// keeps the person's own; lifting the policy gives it back.
#[tokio::test]
async fn a_sign_in_under_an_enforced_mode_gives_the_persons_mode_back_when_lifted() {
    let harness = with_operator_policy(Some(enforcing("all", 1))).await;
    let mut client = harness.client().await;
    sign_in_sharing(&harness, &mut client, "new").await;

    let installs = harness.probe.calls("cloud");
    assert_eq!(installs.len(), 1);
    let (args, _) = &installs[0];
    assert!(args.contains(&"--include-existing".to_string()), "{args:?}");
    // The policy was read: nothing is withheld.
    assert!(
        !args.contains(&"--withhold-uploads".to_string()),
        "{args:?}"
    );
    let cached = harness.cached_policy();
    assert_eq!(cached["user_settings"], json!({"sharing_mode": "new"}));
    assert_eq!(cached["withheld"], Value::Null);
    // The install already has the enforced mode: nothing to switch.
    assert!(harness.sharing_sets().is_empty());

    let status = client.ok("uploader.status", json!({})).await;
    assert_eq!(status["enforced_settings"], json!({"sharing_mode": "all"}));
    assert_eq!(status["uploads_withheld"], Value::Null);
    let (_, message) = client
        .error("uploader.sharing.set", json!({"mode": "new"}))
        .await;
    assert_eq!(message, ENFORCED);

    harness.set_operator_policy(Value::Null);
    client
        .ok(
            "account.connect_workspace",
            json!({"workspace_id": WORKSPACE}),
        )
        .await;
    assert_eq!(harness.sharing_sets(), ["new"]);
    assert_eq!(harness.cached_policy()["user_settings"], Value::Null);
    let status = client.ok("uploader.status", json!({})).await;
    assert_eq!(status["enforced_settings"], Value::Null);
    harness.stop().await;
}

/// A policy that arrives while signed in switches the install to the
/// enforced mode, refuses the person's changes, and gives their mode back
/// once lifted.
#[tokio::test]
async fn an_enforced_mode_replaces_the_persons_and_is_given_back_when_lifted() {
    let harness = with_operator_policy(Some(Value::Null)).await;
    let mut client = harness.client().await;
    sign_in_sharing(&harness, &mut client, "selected").await;
    let (args, _) = &harness.probe.calls("cloud")[0];
    assert!(
        args.contains(&"--selected-sessions-only".to_string()),
        "{args:?}"
    );
    assert!(
        !args.contains(&"--withhold-uploads".to_string()),
        "{args:?}"
    );
    assert!(harness.sharing_sets().is_empty());

    harness.set_operator_policy(enforcing("all", 1));
    client
        .ok(
            "account.connect_workspace",
            json!({"workspace_id": WORKSPACE}),
        )
        .await;
    assert_eq!(harness.sharing_sets(), ["all"]);
    assert_eq!(
        harness.cached_policy()["user_settings"],
        json!({"sharing_mode": "selected"})
    );
    let status = client.ok("uploader.status", json!({})).await;
    assert_eq!(status["enforced_settings"], json!({"sharing_mode": "all"}));
    let (_, message) = client
        .error("uploader.sharing.set", json!({"mode": "selected"}))
        .await;
    assert_eq!(message, ENFORCED);
    let (_, message) = client
        .error(
            "uploader.sessions.exclude",
            json!({"session_keys": ["codex:a"]}),
        )
        .await;
    assert_eq!(message, ENFORCED);

    harness.set_operator_policy(Value::Null);
    client
        .ok(
            "account.connect_workspace",
            json!({"workspace_id": WORKSPACE}),
        )
        .await;
    assert_eq!(harness.sharing_sets(), ["all", "selected"]);
    assert_eq!(harness.cached_policy()["user_settings"], Value::Null);
    harness.stop().await;
}

/// A sign-in whose first read fails, with nothing cached, sets up with
/// uploads withheld; the person cannot resume them, and the first read
/// that succeeds does.
#[tokio::test]
async fn uploads_withheld_at_sign_in_resume_once_the_policy_is_read() {
    let harness = with_operator_policy(None).await;
    let mut client = harness.client().await;
    sign_in_sharing(&harness, &mut client, "new").await;

    let (args, _) = &harness.probe.calls("cloud")[0];
    assert!(args.contains(&"--withhold-uploads".to_string()), "{args:?}");
    assert!(
        args.contains(&"--new-sessions-only".to_string()),
        "{args:?}"
    );
    let cached = harness.cached_policy();
    assert_eq!(cached["withheld"], true);
    assert_eq!(cached["fetched_at"], Value::Null);
    let status = client.ok("uploader.status", json!({})).await;
    assert_eq!(status["uploads_withheld"], true);
    assert_eq!(status["enforced_settings"], Value::Null);
    let (_, message) = client.error("uploader.resume", json!({})).await;
    assert_eq!(message, WITHHELD);
    assert!(harness.probe.calls("resume").is_empty());

    harness.set_operator_policy(Value::Null);
    client
        .ok(
            "account.connect_workspace",
            json!({"workspace_id": WORKSPACE}),
        )
        .await;
    assert_eq!(harness.probe.calls("resume").len(), 1);
    let cached = harness.cached_policy();
    assert_eq!(cached["withheld"], Value::Null);
    assert!(cached["fetched_at"].is_i64(), "{cached}");
    let status = client.ok("uploader.status", json!({})).await;
    assert_eq!(status["uploads_withheld"], Value::Null);
    client.ok("uploader.resume", json!({})).await;
    assert_eq!(harness.probe.calls("resume").len(), 2);
    harness.stop().await;
}

/// A failed read with a policy cached from an earlier sign-in does not
/// withhold uploads: the cached policy applies.
#[tokio::test]
async fn a_failed_read_with_a_cached_policy_does_not_withhold_uploads() {
    let harness = Harness::start_with(|_, setup| {
        let app_data = setup.home.join("app-data");
        std::fs::create_dir_all(&app_data).unwrap();
        std::fs::write(
            app_data.join("policy.json"),
            json!({"workspaces": {WORKSPACE: {"workspace_id": WORKSPACE, "revision": 1,
                "fetched_at": 1, "settings": {"sharing_mode": "all"}}}})
            .to_string(),
        )
        .unwrap();
        setup.env.lock().unwrap().insert(
            "RELAY_DESKTOP_OPERATOR_POLICY".into(),
            setup.home.join("missing.json").to_str().unwrap().into(),
        );
    })
    .await;
    let mut client = harness.client().await;
    sign_in_sharing(&harness, &mut client, "new").await;
    let (args, _) = &harness.probe.calls("cloud")[0];
    assert!(
        !args.contains(&"--withhold-uploads".to_string()),
        "{args:?}"
    );
    assert!(args.contains(&"--include-existing".to_string()), "{args:?}");
    assert_eq!(harness.cached_policy()["withheld"], Value::Null);
    harness.stop().await;
}
