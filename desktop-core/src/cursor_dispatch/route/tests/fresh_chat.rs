//! A new Cursor chat whose first action is an identity call: the intent
//! route puts it on the relay itself, but only for the process holding it
//! and only while Core's discovery lists it live.
use super::routes::{holding, HOOK_PID};
use super::*;
use crate::attestation_route::tests::{fixture_with, registration, CAST};
use agent_relay_node::relay::sessions::{CallerMiss, DiscoveredSession, SessionDirectory};
use agent_relay_node::relay::store::Registration;
use agent_relay_node::relay::workspace::Secret;
use std::sync::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// A chat no one has registered yet.
const FRESH: &str = "7e2b9c41-3d5a-4f6e-8b1c-2a9d0e4f6b13";

/// Core's session discovery listing `FRESH` live, as it does once Cursor's
/// `beforeSubmitPrompt` hook recorded the conversation, or not at all.
struct Discovery(bool);

impl SessionDirectory for Discovery {
    fn sessions(&self) -> Vec<DiscoveredSession> {
        self.0
            .then(|| DiscoveredSession {
                target: Target::new(SessionKind::CursorCli, FRESH),
                title: "Cursor".into(),
                directory: None,
                updated_at_ms: None,
                host: None,
                live: true,
                holder: None,
            })
            .into_iter()
            .collect()
    }
    fn caller(&self, _: i32) -> Result<DiscoveredSession, CallerMiss> {
        Err(CallerMiss::NotASession)
    }
}

/// Relaycast as registration sees it: `POST /v1/agents` creates the agent,
/// anything else is not found. Every request line is recorded.
struct Relaycast {
    url: String,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Relaycast {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Relaycast {
    async fn start() -> Relaycast {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let recorded = recorded.clone();
                tokio::spawn(async move {
                    let (read, mut write) = stream.into_split();
                    let mut reader = BufReader::new(read);
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
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
                    let request = line.split_whitespace().take(2).collect::<Vec<_>>();
                    let request = request.join(" ");
                    recorded.lock().unwrap().push(request.clone());
                    let (status, reply) = if request == "POST /v1/agents" {
                        let asked: Value = serde_json::from_slice(&body).unwrap();
                        (
                            "201 Created",
                            json!({"ok": true, "data": {"id": "agent-fresh",
                                "name": asked["name"], "token": "at_live_fresh"}}),
                        )
                    } else {
                        (
                            "404 Not Found",
                            json!({"ok": false, "error": {"code": "not_found", "message": "no"}}),
                        )
                    };
                    let reply = reply.to_string();
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                        reply.len()
                    );
                    let _ = write.write_all(response.as_bytes()).await;
                });
            }
        });
        Relaycast {
            url,
            requests,
            task,
        }
    }

    fn registrations(&self) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| *request == "POST /v1/agents")
            .count()
    }
}

/// Cursor attested, the workspace on `relaycast`, and Core's discovery
/// listing `FRESH` live when `listed`. Nothing has registered `FRESH`.
async fn fresh(listed: bool) -> (Fixture, Relaycast) {
    let relaycast = Relaycast::start().await;
    let f = fixture_with(|_| Arc::new(Discovery(listed)));
    let url = relaycast.url.clone();
    f.core
        .store
        .update(|store| store.cloud.as_mut().unwrap().base_url = url)
        .unwrap();
    f.core.rediscover_workspace();
    assert_eq!(
        f.core.workspace().map(|w| w.base_url),
        Some(relaycast.url.clone())
    );
    f.negotiate(json!({"native_session_identity":1,"cursor_session_attestation":1}))
        .await;
    let f = cursor_selected(f);
    assert!(attesting(&account(&f), &f.core));
    (f, relaycast)
}

fn cursor_registrations(f: &Fixture) -> Vec<Registration> {
    let target = Target::new(SessionKind::CursorCli, FRESH);
    f.core.store.read(|store| {
        store
            .registrations
            .iter()
            .filter(|registration| registration.target == target)
            .cloned()
            .collect()
    })
}

async fn admit(f: &Fixture, pid: i32, table: FakeProcesses) -> Result<(), &'static str> {
    let deadline = Instant::now() + crate::cursor_dispatch::ANNOUNCE_BUDGET;
    admitted(&account(f), &f.core, pid, FRESH, Arc::new(table), deadline).await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_chat_s_first_identity_call_puts_it_on_the_relay() {
    let (f, relaycast) = fresh(true).await;
    let account = account(&f);
    let hooks = holding(&f, FRESH);
    assert_eq!(
        accepted(&account, &f.core, HOOK_PID, FRESH, &hooks),
        Ok(Chat::Unregistered)
    );
    assert!(cursor_registrations(&f).is_empty());

    let started = Instant::now();
    assert_eq!(admit(&f, HOOK_PID, hooks).await, Ok(()));
    let took = started.elapsed();
    assert!(took < crate::cursor_dispatch::ANNOUNCE_BUDGET, "{took:?}");
    let registered = cursor_registrations(&f);
    assert_eq!(registered.len(), 1);
    assert_eq!(registered[0].base_url, relaycast.url);
    assert_eq!(relaycast.registrations(), 1);
    assert!(super::super::registered(&f.core, FRESH).is_some());
    assert_eq!(
        accepted(&account, &f.core, HOOK_PID, FRESH, &holding(&f, FRESH)),
        Ok(Chat::Registered)
    );

    // The announcement is kept, and the helper signs the call as the chat.
    let arguments = json!({"to":"synthetic-peer","text":"synthetic-message"});
    let input = json!({
        "conversation_id": FRESH, "tool_use_id": TUID,
        "tool_name": "send_relay_message", "tool_input": arguments.to_string(),
    });
    let generation = f.cloud.generation.load(std::sync::atomic::Ordering::SeqCst);
    let now = Instant::now();
    let intents = &account.copilot_dispatch.cursor_intents;
    keep(intents, announced(&input, generation).unwrap(), now, now).unwrap();
    let (reply, posted) = run(
        dispatch(&f, "send_relay_message", arguments),
        &helper_table(&f),
    );
    assert!(reply.get("error").is_none(), "{reply}");
    let proof = posted.unwrap().1["params"]["arguments"][FIELD]
        .as_str()
        .unwrap()
        .to_owned();
    let encoded = proof.split('.').nth(1).unwrap();
    let payload: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(encoded).unwrap()).unwrap();
    assert_eq!(payload["sid"], FRESH);

    // Announcing again answers from the registration: Relaycast is not asked.
    assert_eq!(admit(&f, HOOK_PID, holding(&f, FRESH)).await, Ok(()));
    assert_eq!(relaycast.registrations(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_caller_not_holding_the_new_chat_never_registers_it() {
    let (f, relaycast) = fresh(true).await;
    let account = account(&f);
    // A process outside the chat's `cursor-agent`, and the hook of another
    // chat announcing this one.
    for (pid, table) in [
        (4400 + 1, holding(&f, FRESH)),
        (HOOK_PID, holding(&f, &sid())),
    ] {
        assert_eq!(
            accepted(&account, &f.core, pid, FRESH, &table),
            Err("cursor_chat_unknown")
        );
        assert_eq!(admit(&f, pid, table).await, Err("cursor_chat_unknown"));
    }
    assert!(cursor_registrations(&f).is_empty());
    assert_eq!(relaycast.registrations(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_chat_discovery_does_not_list_or_another_workspace_holds_stays_unknown() {
    // Cursor never recorded the conversation (no `beforeSubmitPrompt`):
    // Core refuses to register a session that is not running.
    let (f, relaycast) = fresh(false).await;
    assert_eq!(
        admit(&f, HOOK_PID, holding(&f, FRESH)).await,
        Err("cursor_chat_unknown")
    );
    assert!(cursor_registrations(&f).is_empty());
    assert_eq!(relaycast.registrations(), 0);

    // The chat is on the relay in another workspace: that registration is
    // answered as it is, and is not this workspace's.
    let (f, relaycast) = fresh(true).await;
    let elsewhere = registration(
        "cursor-elsewhere",
        Target::new(SessionKind::CursorCli, FRESH),
    );
    assert_eq!(elsewhere.base_url, CAST);
    let added = f
        .core
        .store
        .update(|store| store.add(elsewhere, Secret::new("t-elsewhere")))
        .unwrap();
    assert!(added);
    assert_eq!(
        admit(&f, HOOK_PID, holding(&f, FRESH)).await,
        Err("cursor_chat_unknown")
    );
    let registered = cursor_registrations(&f);
    assert_eq!(registered.len(), 1);
    assert_eq!(registered[0].base_url, CAST);
    assert_eq!(relaycast.registrations(), 0);
}
