//! The route against a synthetic Cloud and session directory: no broker,
//! harness or Claude process runs.
use super::*;
use agent_relay_node::relay::cloud_session::{AgentOwner, CloudAccess, CloudAccount, CloudError};
use agent_relay_node::relay::daemon::{Config, Core, Plugins};
use agent_relay_node::relay::sessions::{CallerMiss, DiscoveredSession, SessionDirectory};
use agent_relay_node::relay::setup::DirectDelivery;
use agent_relay_node::relay::store::{iso8601_from_seconds, CloudWorkspace, Registration};
use agent_relay_node::relay::workspace::Secret;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use futures_util::future::BoxFuture;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) const WORKSPACE: &str = "50587328-441d-4acb-b8f3-dbe1b3c5de99";
const CAST: &str = "https://cast.agentrelay.com";
const KEY: &str = "rk_live_cloudcloud";
pub(crate) const REGISTERED: &str = "0C6B1F0E-2F2A-4B7E-9D3C-5A1E8F7B6C4D";
const UNREGISTERED: &str = "11111111-2222-4333-8444-555555555555";
/// The hook process below the registered chat, and one below another chat.
pub(crate) const IN_REGISTERED: i32 = 4100;
const IN_UNREGISTERED: i32 = 4200;

#[derive(Default)]
pub(crate) struct FakeCloud {
    calls: Mutex<Vec<(String, String, Option<Value>)>>,
    capabilities: Mutex<Value>,
    pub(crate) generation: std::sync::atomic::AtomicU64,
    /// Every request fails while set.
    offline: std::sync::atomic::AtomicBool,
}

impl CloudAccess for FakeCloud {
    fn account(&self) -> Option<CloudAccount> {
        Some(CloudAccount {
            site_url: "https://agentrelay.com".into(),
            workspace_id: WORKSPACE.into(),
            user_id: Some("user-1".into()),
        })
    }
    fn agent_owner(&self) -> Option<AgentOwner> {
        None
    }
    fn identity_sync_generation(&self) -> u64 {
        self.generation.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn request(
        &self,
        method: &'static str,
        path: String,
        body: Option<Value>,
    ) -> BoxFuture<'_, Result<Value, CloudError>> {
        self.calls.lock().unwrap().push((method.into(), path, body));
        if self.offline.load(std::sync::atomic::Ordering::SeqCst) {
            return Box::pin(async { Err(CloudError::Unreachable) });
        }
        let capabilities = self.capabilities.lock().unwrap().clone();
        Box::pin(async move { Ok(json!({"identities": [], "capabilities": capabilities})) })
    }
}

struct Directory;
impl SessionDirectory for Directory {
    fn sessions(&self) -> Vec<DiscoveredSession> {
        Vec::new()
    }
    fn caller(&self, pid: i32) -> Result<DiscoveredSession, CallerMiss> {
        let id = match pid {
            IN_REGISTERED => REGISTERED.to_ascii_lowercase(),
            IN_UNREGISTERED => UNREGISTERED.into(),
            _ => return Err(CallerMiss::NotASession),
        };
        Ok(DiscoveredSession {
            target: Target::new(SessionKind::ClaudeTerminal, id),
            title: "Claude".into(),
            directory: None,
            updated_at_ms: None,
            host: None,
            live: true,
            holder: None,
        })
    }
}

struct NoDirect;
impl DirectDelivery for NoDirect {
    fn delivers_directly(&self) -> bool {
        false
    }
    fn managed_value(&self) -> Option<String> {
        None
    }
    fn user_choice(&self) -> Option<String> {
        None
    }
    fn set_direct_delivery(&self, _: bool) -> Result<(), String> {
        Ok(())
    }
}

fn registration(name: &str, target: Target) -> Registration {
    serde_json::from_value(json!({
        "name": name, "agentID": format!("agent-{name}"), "target": target,
        "title": name, "baseURL": CAST, "workspaceKey": KEY,
        "registeredAt": iso8601_from_seconds(0.0), "machinePrefix": "test-desk",
    }))
    .unwrap()
}

pub(crate) struct Fixture {
    _home: tempfile::TempDir,
    pub(crate) core: Arc<Core>,
    pub(crate) cloud: Arc<FakeCloud>,
    pub(crate) outbound: Outbound,
    pub(crate) installer: Arc<crate::mcp_settings::Installer>,
    pub(crate) dispatch: Arc<crate::copilot_dispatch::Registry>,
}

fn fixture() -> Fixture {
    fixture_with(|_| Arc::new(Directory))
}

/// The fixture with the session directory `sessions` builds for its home.
fn fixture_with(sessions: impl FnOnce(&std::path::Path) -> Arc<dyn SessionDirectory>) -> Fixture {
    let home = tempfile::tempdir().unwrap();
    let local = agent_relay_node::relay::local::LocalConfig::from_env(&|_| None, home.path());
    let helper = crate::probe_copy::destination(home.path());
    std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
    std::fs::write(&helper, "synthetic helper, never executed").unwrap();
    agent_relay_node::platform::fs::set_mode(&helper, 0o700).unwrap();
    let installer = Arc::new(
        crate::mcp_settings::Installer::new(
            home.path().into(),
            local.codex,
            local.claude,
            local.opencode,
        )
        .with_copilot_executable(Some(helper)),
    );
    let config = Config {
        paths: agent_relay_node::relay::paths::CorePaths::for_socket(home.path().join("core.sock"))
            .unwrap(),
        home: home.path().into(),
        app_data_dir: home.path().join("app-data"),
        env: Default::default(),
        version: "test".into(),
        host_name: "Test Desk".into(),
        rediscover_every: Duration::from_secs(3600),
        push_timing: Default::default(),
        delivery_timing: Default::default(),
    };
    let cloud = Arc::new(FakeCloud::default());
    let mut account = crate::account::Account::new(
        &Default::default(),
        &home.path().join("app-data"),
        Arc::new(crate::history::uploader::Uploader::new(Arc::new(
            crate::history::uploader::ProcessRunner::new(home.path().join("no-probe")),
        ))),
    );
    account.copilot_dispatch = Arc::new(crate::copilot_dispatch::Registry::with_verifier(
        Arc::new(copilot::NativeFixture),
    ));
    let dispatch = account.copilot_dispatch.clone();
    let account = Arc::new(account);
    let journal = installer
        .copilot_native_config()
        .sessions()
        .join(REGISTERED.to_ascii_lowercase())
        .join("events.jsonl");
    std::fs::create_dir_all(journal.parent().unwrap()).unwrap();
    std::fs::write(journal, b"").unwrap();
    let _ = account.mcp_config.set(installer.clone());
    let plugins = Plugins {
        cloud: cloud.clone(),
        extensions: vec![Arc::new(crate::account::plugin::AccountMethods(
            account.clone(),
        ))],
        account: Some(account.clone().into_plugin()),
        sessions: sessions(home.path()),
        ..Plugins::default()
    };
    let (core, _) = Core::with_plugins(config, plugins);
    core.store
        .update(|store| {
            store.cloud = Some(CloudWorkspace {
                key: Secret::new(KEY),
                base_url: CAST.into(),
                workspace_id: Some("rw_cast".into()),
                account_workspace_id: Some(WORKSPACE.into()),
                name: None,
                relayfile_workspace_id: None,
            });
            let claude = Target::new(SessionKind::ClaudeTerminal, REGISTERED.to_ascii_lowercase());
            assert!(store.add(registration("claude-chat", claude), Secret::new("t1")));
            let codex = Target::new(SessionKind::Codex, UNREGISTERED);
            assert!(store.add(registration("codex-chat", codex), Secret::new("t2")));
        })
        .unwrap();
    let outbound = Outbound::new(core.clone(), Arc::new(NoDirect));
    Fixture {
        _home: home,
        core,
        cloud,
        outbound,
        installer,
        dispatch,
    }
}

impl Fixture {
    async fn negotiate(&self, capabilities: Value) {
        *self.cloud.capabilities.lock().unwrap() = capabilities;
        crate::desktop_identity::Identities::new(self.core.clone()).refresh_mcp_links().await;
    }

    async fn ask(&self, pid: i32, body: Value) -> Result<Value, OutboundError> {
        let request = Request {
            method: "POST".into(),
            path: "/claude/attestation".into(),
            body: serde_json::to_vec(&body).unwrap(),
            ..Request::default()
        };
        let uid = agent_relay_node::relay::peer::current_uid();
        self.outbound.handle(pid, uid, &request).await
    }

    fn last_put(&self) -> Value {
        let calls = self.cloud.calls.lock().unwrap();
        let (_, _, body) = calls.iter().rev().find(|(m, _, _)| m == "PUT").unwrap();
        body.clone().unwrap()
    }
}

fn call(session_id: &str, tool_name: &str) -> Value {
    json!({"session_id": session_id, "tool_name": tool_name, "tool_use_id": "toolu_01ABC"})
}

fn decode(part: &str) -> Value {
    serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).unwrap()).unwrap()
}

#[tokio::test]
async fn named_joins_never_register_or_rename_before_command_permission() {
    let f = fixture();
    f.negotiate(json!({"claude_session_attestation":1})).await;
    for (pid, sid, name) in [
        (IN_UNREGISTERED, UNREGISTERED, "claude-assistant"),
        (IN_REGISTERED, REGISTERED, "another-name"),
    ] {
        let mut join = call(sid, "mcp__agent-relay__join_relay");
        join["registration"] = json!({"name":name});
        assert_eq!(f.ask(pid, join).await.unwrap_err().code, "registration_required");
    }
    assert_eq!(f.core.store.read(|store| store.registrations.len()), 2);
    let target = Target::new(SessionKind::ClaudeTerminal, REGISTERED.to_ascii_lowercase());
    assert_eq!(f.core.store.read(|store| store.registration_for(&target).unwrap().display_name().to_string()), "claude-chat");
    assert!(f.cloud.calls.lock().unwrap().iter().all(|(method,_,_)| method != "PUT"));

    let mut matching = call(REGISTERED, "mcp__agent-relay__join_relay");
    matching["registration"] = json!({"name":"claude-chat"});
    let result = f.ask(IN_REGISTERED, matching).await.unwrap();
    assert_eq!(result["native_join"], true);
    assert!(result["attestation"].is_string());
    assert_eq!(f.core.store.read(|store| store.registrations.len()), 2);
}

#[tokio::test]
async fn unregistered_named_joins_keep_caller_and_self_registration_checks() {
    let f = fixture();
    f.negotiate(json!({"claude_session_attestation":1})).await;
    let mut forged = call(REGISTERED, "mcp__agent-relay__join_relay");
    forged["registration"] = json!({"name":"claude-assistant"});
    assert_eq!(f.ask(IN_UNREGISTERED, forged).await.unwrap_err().code, OutboundError::not_a_session().code);
    f.core.set_settings(json!({"allow_self_registration":false}).as_object().unwrap()).unwrap();
    let mut join = call(UNREGISTERED, "mcp__agent-relay__join_relay");
    join["registration"] = json!({"name":"claude-assistant"});
    assert_eq!(f.ask(IN_UNREGISTERED, join).await.unwrap_err().code, "not_allowed");
    assert_eq!(f.core.store.read(|store| store.registrations.len()), 2);
}

#[tokio::test]
async fn the_sync_registers_the_key_and_claude_sessions_only_once_cloud_opts_in() {
    let f = fixture();
    f.negotiate(json!({"native_session_identity": 1})).await;
    crate::desktop_identity::Identities::new(f.core.clone()).sync_desktop_identities().await;
    let before = f.last_put();
    assert!(before.get("attestation_key").is_none(), "{before}");
    assert!(before["identities"]
        .as_array()
        .unwrap()
        .iter()
        .all(|identity| identity["native_session"]["kind"] != "claude"));

    f.negotiate(json!({"native_session_identity": 1, "claude_session_attestation": 1}))
        .await;
    crate::desktop_identity::Identities::new(f.core.clone()).sync_desktop_identities().await;
    let body = f.last_put();
    let key = f.core.plugins.account.as_ref().unwrap().clone().as_any().downcast::<crate::account::Account>().unwrap().attestation_key.get(&f.core.config.app_data_dir).unwrap();
    assert_eq!(body["attestation_key"], key.registration());
    assert_eq!(body["attestation_key"]["alg"], "Ed25519");
    let identities = body["identities"].as_array().unwrap();
    let claude = identities
        .iter()
        .find(|identity| identity["name"] == "claude-chat")
        .unwrap();
    assert_eq!(
        claude["native_session"],
        json!({"kind": "claude", "id": REGISTERED.to_ascii_lowercase()})
    );
    let codex = identities
        .iter()
        .find(|identity| identity["name"] == "codex-chat")
        .unwrap();
    assert_eq!(codex["native_session"]["kind"], "codex");
    assert!(
        !body.to_string().contains("PRIVATE"),
        "no private key material"
    );

    // Cloud replaces its stored key with each sync's, so every later sync
    // after negotiation carries the same key again.
    f.core
        .store
        .update(|store| {
            let target = Target::new(SessionKind::ClaudeTerminal, UNREGISTERED);
            assert!(store.add(registration("second-chat", target), Secret::new("t3")));
        })
        .unwrap();
    crate::desktop_identity::Identities::new(f.core.clone()).sync_desktop_identities().await;
    let later = f.last_put();
    assert_ne!(
        later["identities"], body["identities"],
        "a new sync was sent"
    );
    assert_eq!(later["attestation_key"], key.registration());
    assert_eq!(later["source_id"], body["source_id"]);
}

#[tokio::test]
async fn a_registered_chat_gets_a_proof_its_installation_key_verifies() {
    let f = fixture();
    let refused = f
        .ask(
            IN_REGISTERED,
            call(REGISTERED, "mcp__agent-relay__send_message"),
        )
        .await
        .unwrap_err();
    assert_eq!(
        refused.code, "attestation_unavailable",
        "before Cloud opts in"
    );

    f.negotiate(json!({"claude_session_attestation": 1})).await;
    let answer = f
        .ask(
            IN_REGISTERED,
            call(REGISTERED, "mcp__plugin_relay_agent-relay__send_message"),
        )
        .await
        .unwrap();
    let proof = answer["attestation"].as_str().unwrap();
    let parts: Vec<&str> = proof.split('.').collect();
    let key = f.core.plugins.account.as_ref().unwrap().clone().as_any().downcast::<crate::account::Account>().unwrap().attestation_key.get(&f.core.config.app_data_dir).unwrap();
    assert_eq!(decode(parts[0])["kid"], key.kid());
    let payload = decode(parts[1]);
    let source = f
        .core
        .store
        .read(|store| store.desktop_sync_source_id.clone().unwrap());
    assert_eq!(payload["sid"], REGISTERED.to_ascii_lowercase());
    assert_eq!(payload["src"], source);
    // Cloud resolves the chat only among identities synced under `src`.
    crate::desktop_identity::Identities::new(f.core.clone()).sync_desktop_identities().await;
    assert_eq!(f.last_put()["source_id"], payload["src"]);
    assert_eq!(payload["tuid"], "toolu_01ABC");
    assert_eq!(payload["tool"], "send_message");
    assert_eq!(payload["kind"], "claude");
    assert_eq!(
        payload["exp"].as_u64().unwrap() - payload["iat"].as_u64().unwrap(),
        60
    );
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key.public_key())
        .verify(
            format!("{}.{}", parts[0], parts[1]).as_bytes(),
            &URL_SAFE_NO_PAD.decode(parts[2]).unwrap(),
        )
        .unwrap();
}

#[tokio::test]
async fn nothing_is_signed_for_another_chat_an_unregistered_one_or_another_server() {
    let f = fixture();
    f.negotiate(json!({"claude_session_attestation": 1})).await;
    let tool = "mcp__agent-relay__send_message";
    // A chat that is not registered here.
    let error = f
        .ask(IN_UNREGISTERED, call(UNREGISTERED, tool))
        .await
        .unwrap_err();
    assert_eq!(error.code, OutboundError::not_registered().code);
    // A process naming a chat it does not run inside.
    let error = f
        .ask(IN_UNREGISTERED, call(REGISTERED, tool))
        .await
        .unwrap_err();
    assert_eq!(error.code, OutboundError::not_a_session().code);
    let error = f.ask(9, call(REGISTERED, tool)).await.unwrap_err();
    assert_eq!(error.code, OutboundError::not_a_session().code);
    // Not a hosted Agent Relay tool, or a malformed call.
    for body in [
        call(REGISTERED, "mcp__github__create_issue"),
        call("not-a-session", tool),
        json!({"session_id": REGISTERED, "tool_name": tool, "tool_use_id": "bad id"}),
        json!({"session_id": REGISTERED, "tool_name": tool}),
    ] {
        let error = f.ask(IN_REGISTERED, body.clone()).await.unwrap_err();
        assert_eq!(error.status, 400, "{body}");
    }
}

#[tokio::test]
async fn providers_and_signing_forget_the_capability_with_its_generation() {
    let f = fixture();
    assert!(
        !f.installer.claude_attestation(),
        "before any identity read"
    );
    f.negotiate(json!({"claude_session_attestation": 1})).await;
    assert!(f.installer.claude_attestation());
    let tool = "mcp__agent-relay__send_message";
    assert!(f.ask(IN_REGISTERED, call(REGISTERED, tool)).await.is_ok());

    // A new sign-in or workspace starts a new generation, and its identity
    // read fails: neither Providers nor signing keep the old answer.
    f.cloud
        .generation
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    f.cloud
        .offline
        .store(true, std::sync::atomic::Ordering::SeqCst);
    crate::desktop_identity::Identities::new(f.core.clone()).refresh_mcp_links().await;
    assert!(!f.installer.claude_attestation());
    let refused = f
        .ask(IN_REGISTERED, call(REGISTERED, tool))
        .await
        .unwrap_err();
    assert_eq!(refused.code, "attestation_unavailable");

    // The new generation's own read restores both together.
    f.cloud
        .offline
        .store(false, std::sync::atomic::Ordering::SeqCst);
    f.negotiate(json!({"claude_session_attestation": 1})).await;
    assert!(f.installer.claude_attestation());
    assert!(f.ask(IN_REGISTERED, call(REGISTERED, tool)).await.is_ok());
}

/// The real caller lookup: a fake process table (Claude 300, its hook shell
/// 310, the hook 320) and Claude's own `<claude home>/sessions/300.json`.
#[tokio::test]
async fn after_resume_the_hook_proves_only_the_chat_its_claude_now_holds() {
    use agent_relay_node::relay::process::{FakeProcess, FakeProcesses, StartClock};
    let uid = agent_relay_node::relay::peer::current_uid();
    let table = Arc::new(FakeProcesses {
        clock: Some(StartClock::Ticks),
        ..FakeProcesses::default()
    });
    let process = |parent, start| FakeProcess {
        parent,
        uid,
        start,
        executable: Some("/bin/zsh".into()),
        ..FakeProcess::default()
    };
    table.insert(300, process(1, 100));
    table.insert(310, process(300, 110));
    table.insert(320, process(310, 120));
    let registry = std::sync::Mutex::new(None);
    let f = fixture_with(|home| {
        let mut config = agent_relay_node::relay::local::LocalConfig::from_env(&|_| None, home);
        config.uid = uid;
        config.claude.dirs = Default::default();
        *registry.lock().unwrap() = Some(config.claude.home.join("sessions"));
        Arc::new(agent_relay_node::relay::local::LocalSessions::new(
            config,
            table.clone(),
        ))
    });
    let registry = registry.lock().unwrap().clone().unwrap();
    std::fs::create_dir_all(&registry).unwrap();
    let record = |session: &str| {
        std::fs::write(
            registry.join("300.json"),
            json!({"sessionId": session, "pid": 300, "procStart": "100", "peerProtocol": 1,
                   "messagingSocketPath": registry.join("peer.sock"), "cwd": "/tmp/work"})
            .to_string(),
        )
        .unwrap();
    };
    let resumed = UNREGISTERED;
    f.core
        .store
        .update(|store| {
            let target = Target::new(SessionKind::ClaudeTerminal, resumed);
            assert!(store.add(registration("resumed-chat", target), Secret::new("t3")));
        })
        .unwrap();
    f.negotiate(json!({"claude_session_attestation": 1})).await;
    let tool = "mcp__agent-relay__send_message";

    record(&REGISTERED.to_ascii_lowercase());
    assert!(f.ask(320, call(REGISTERED, tool)).await.is_ok());

    // In-process `/resume`: Claude Code rewrites its own record.
    record(resumed);
    let proof = f.ask(320, call(resumed, tool)).await.unwrap();
    let payload = proof["attestation"]
        .as_str()
        .unwrap()
        .split('.')
        .nth(1)
        .unwrap();
    assert_eq!(decode(payload)["sid"], resumed);
    let refused = f.ask(320, call(REGISTERED, tool)).await.unwrap_err();
    assert_eq!(refused.code, OutboundError::not_a_session().code);
}

pub(crate) mod copilot;
