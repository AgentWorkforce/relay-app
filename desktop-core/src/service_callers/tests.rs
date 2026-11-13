//! The acceptance tests of `docs/service-callers.md` section 10, against a
//! stand-in core: its processes, clock, spawn registrations and Relaycast.
//! Every Relaycast request the plugin makes is counted, so "before any
//! Relaycast request" is checked, not assumed.

#![cfg(not(windows))]

use super::host::{Attestation, Host, Incoming, Registered, Spawned};
use super::{Class, ServiceCallers};
use crate::contract::service_callers::{CallerErrorClass, InboxSettled};
use agent_relay_node::relay::cloud_session::CloudAccount;
use agent_relay_node::relay::contract::spawn_control::{
    SpawnControlAction, SpawnControlInvocation, SpawnInvocationParams, SpawnInvokeParams,
    SpawnReleaseParams,
};
use agent_relay_node::relay::daemon::{
    CallContext, ConnectionCaller, ExternalCaller, Permit, SpawnCall, SpawnCallers, SpawnRequest,
};
use agent_relay_node::relay::process::ProcessInfo;
use agent_relay_node::relay::protocol::{ErrorKind, RefusalReason, RpcError};
use agent_relay_node::relay::spawn::node::NODE_FILE;
use agent_relay_node::relay::workspace::{Secret, Workspace};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const UID: u32 = 501;
/// The connections `Fx::caller_call` binds in a default build, clear of the
/// ids the tests name.
static NEXT_CALLER_CONNECTION: AtomicU64 = AtomicU64::new(1 << 40);
/// The app itself.
const APP: i32 = 100;
/// The command line.
const PROBE: i32 = 101;
/// A caller's runtime, and a second connection of it.
const RUNTIME: i32 = 102;
const RUNTIME2: i32 = 103;
/// Another user's process.
const STRANGER: i32 = 104;
/// A process inside an agent session.
const SESSION: i32 = 105;
/// A same-user process that copied the app's executable name and path.
const FORGED: i32 = 106;

const T0: i64 = 1_800_000_000_000;
const SITE: &str = "https://agentrelay.test";
const BASE_URL: &str = "https://api.relaycast.test";
const WORKSPACE_KEY: &str = "rk_live_WORKSPACEKEY_0123456789";
const NODE_ID: &str = "node_7f3a9c";
const NODE_NAME: &str = "mac-1-desktop";

/// Relaycast as the plugin reaches it, shared across core restarts.
#[derive(Default)]
struct Relaycast {
    /// Every request: register, delete, deliveries, ack and the core's spawn.
    requests: u64,
    spawns: u64,
    next: u64,
    /// Agent id to its token.
    tokens: HashMap<String, String>,
    deleted: Vec<String>,
    /// Every exact worker release with the workspace key: name, agent id
    /// and idempotency key, failed ones included.
    release_requests: Vec<(String, String, String)>,
    /// Workers released, by name and agent id.
    released: Vec<(String, String)>,
    /// How many of the next worker releases fail.
    failing_releases: u32,
    /// Token to the deliveries waiting for it, kept until acknowledged.
    queue: HashMap<String, Vec<Incoming>>,
}

/// What outlives one core process: the disk, the clock, Relaycast and the
/// core's spawn registrations.
struct World {
    _dir: tempfile::TempDir,
    root: PathBuf,
    clock: AtomicI64,
    signed_in: AtomicBool,
    logs: Mutex<Vec<String>>,
    relaycast: Mutex<Relaycast>,
    spawned: Mutex<Vec<Spawned>>,
    keys: AtomicU64,
    /// How often the core's generation moved (an account change or
    /// sign-out), across restarts.
    moves: AtomicU64,
    /// Whether the host can attest the app peer (the real host cannot yet).
    attests: AtomicBool,
    /// When set, the next publisher registration waits for it.
    register_hold: Mutex<Option<Arc<tokio::sync::Notify>>>,
    /// Whether the host attests as the real core does (`host::attestation`),
    /// over this stand-in's process table.
    core_attestation: AtomicBool,
}

fn workspace() -> Workspace {
    Workspace {
        key: Secret::new(WORKSPACE_KEY),
        base_url: BASE_URL.into(),
        workspace_id: Some("rw_test".into()),
        source: Default::default(),
        control_url: None,
    }
}

fn account() -> CloudAccount {
    CloudAccount {
        site_url: SITE.into(),
        workspace_id: "ws_test".into(),
        user_id: Some("user_test".into()),
    }
}

struct FakeHost {
    world: Arc<World>,
    /// Process-lifetime, like the core's.
    generation: AtomicU64,
}

impl Host for FakeHost {
    fn app_data_dir(&self) -> Option<PathBuf> {
        Some(self.world.root.join("app"))
    }

    fn home(&self) -> Option<PathBuf> {
        Some(self.world.root.join("home"))
    }

    fn account(&self) -> Option<CloudAccount> {
        self.world.signed_in.load(Ordering::SeqCst).then(account)
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst) + self.world.moves.load(Ordering::SeqCst)
    }

    fn workspace(&self) -> Option<Workspace> {
        self.world.signed_in.load(Ordering::SeqCst).then(workspace)
    }

    fn machine_prefix(&self) -> Option<String> {
        Some("mac-1".into())
    }

    fn version(&self) -> String {
        "0.0.0-test".into()
    }

    fn socket(&self) -> String {
        self.world.root.join("app/core.sock").display().to_string()
    }

    fn process(&self, pid: i32) -> Option<ProcessInfo> {
        let uid = match pid {
            APP | PROBE | RUNTIME | RUNTIME2 | SESSION | FORGED => UID,
            STRANGER => UID + 1,
            _ => return None,
        };
        Some(ProcessInfo {
            pid,
            parent: 1,
            uid,
            start: pid as u64,
        })
    }

    fn attest_app_peer(&self, pid: i32) -> Attestation {
        if self.world.core_attestation.load(Ordering::SeqCst) {
            return super::host::attestation(pid, self.current_uid(), |pid| {
                self.process(pid).map(|process| process.uid)
            });
        }
        if pid == APP && self.world.attests.load(Ordering::SeqCst) {
            Attestation::App
        } else if pid == APP {
            Attestation::Unavailable
        } else {
            // FORGED carries the app's name and path; names authorize nothing.
            Attestation::NotApp
        }
    }

    fn current_uid(&self) -> u32 {
        UID
    }

    fn in_session(&self, pid: i32) -> bool {
        pid == SESSION
    }

    fn spawned(&self, _workspace: &Workspace) -> Vec<Spawned> {
        self.world.spawned.lock().unwrap().clone()
    }

    fn registration_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .world
            .spawned
            .lock()
            .unwrap()
            .iter()
            .map(|spawned| spawned.name.clone())
            .collect();
        names.push("taken-by-a-session".into());
        names
    }

    fn now_ms(&self) -> i64 {
        self.world.clock.load(Ordering::SeqCst)
    }

    fn log(&self, line: &str) {
        self.world.logs.lock().unwrap().push(line.to_string());
    }

    fn register_agent<'a>(
        &'a self,
        _workspace: &'a Workspace,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Registered, RpcError>> {
        Box::pin(async move {
            let hold = self.world.register_hold.lock().unwrap().take();
            if let Some(hold) = hold {
                hold.notified().await;
            }
            let mut relaycast = self.world.relaycast.lock().unwrap();
            relaycast.requests += 1;
            relaycast.next += 1;
            let id = format!("agt_pub_{}", relaycast.next);
            let token = format!("at_live_TOKENBYTES_{}_q9", relaycast.next);
            relaycast.tokens.insert(id.clone(), token.clone());
            Ok(Registered {
                id,
                name: name.to_string(),
                token: Secret::new(token),
            })
        })
    }

    fn delete_agent<'a>(
        &'a self,
        _workspace: &'a Workspace,
        name: &'a str,
    ) -> BoxFuture<'a, Result<(), RpcError>> {
        Box::pin(async move {
            let mut relaycast = self.world.relaycast.lock().unwrap();
            relaycast.requests += 1;
            relaycast.deleted.push(name.to_string());
            Ok(())
        })
    }

    fn release_worker<'a>(
        &'a self,
        _workspace: &'a Workspace,
        name: &'a str,
        agent_id: &'a str,
        idempotency_key: &'a str,
    ) -> BoxFuture<'a, Result<(), RpcError>> {
        Box::pin(async move {
            let mut relaycast = self.world.relaycast.lock().unwrap();
            relaycast.requests += 1;
            relaycast.release_requests.push((
                name.to_string(),
                agent_id.to_string(),
                idempotency_key.to_string(),
            ));
            if relaycast.failing_releases > 0 {
                relaycast.failing_releases -= 1;
                return Err(RpcError::new(ErrorKind::Internal, "release failed"));
            }
            relaycast
                .released
                .push((name.to_string(), agent_id.to_string()));
            Ok(())
        })
    }

    fn deliveries<'a>(
        &'a self,
        _base_url: &'a str,
        token: &'a Secret,
    ) -> BoxFuture<'a, Result<Vec<Incoming>, RpcError>> {
        Box::pin(async move {
            let mut relaycast = self.world.relaycast.lock().unwrap();
            relaycast.requests += 1;
            Ok(relaycast
                .queue
                .get(token.expose())
                .cloned()
                .unwrap_or_default())
        })
    }

    fn ack<'a>(
        &'a self,
        _base_url: &'a str,
        token: &'a Secret,
        delivery_id: &'a str,
    ) -> BoxFuture<'a, Result<(), RpcError>> {
        Box::pin(async move {
            let mut relaycast = self.world.relaycast.lock().unwrap();
            relaycast.requests += 1;
            if let Some(queue) = relaycast.queue.get_mut(token.expose()) {
                queue.retain(|delivery| delivery.delivery_id != delivery_id);
            }
            Ok(())
        })
    }
}

/// A provisioned caller, as its runtime knows it from `caller.json`.
#[derive(Clone)]
struct Caller {
    id: String,
    name: String,
    secret: String,
    agent_id: String,
    handoff: PathBuf,
}

impl Caller {
    fn with_secret(&self, secret: &str) -> Caller {
        Caller {
            secret: secret.to_string(),
            ..self.clone()
        }
    }
}

/// One core process over the world, and everything its methods answered.
struct Fx {
    world: Arc<World>,
    callers: ServiceCallers,
    seen: Arc<Mutex<Vec<String>>>,
}

impl Fx {
    fn new() -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        for folder in ["app", "home", "work/project/sub", "work/other", "outside"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        let world = Arc::new(World {
            _dir: dir,
            root,
            clock: AtomicI64::new(T0),
            signed_in: AtomicBool::new(true),
            logs: Mutex::new(Vec::new()),
            relaycast: Mutex::new(Relaycast::default()),
            spawned: Mutex::new(Vec::new()),
            keys: AtomicU64::new(0),
            moves: AtomicU64::new(0),
            attests: AtomicBool::new(true),
            register_hold: Mutex::new(None),
            core_attestation: AtomicBool::new(false),
        });
        let fx = Fx::over(world, Arc::new(Mutex::new(Vec::new())));
        fx.write_node(&enrolment());
        fx
    }

    fn over(world: Arc<World>, seen: Arc<Mutex<Vec<String>>>) -> Fx {
        let host = FakeHost {
            world: world.clone(),
            generation: AtomicU64::new(0),
        };
        Fx {
            world,
            callers: ServiceCallers::new(Arc::new(host)),
            seen,
        }
    }

    /// A cold core: a new process with nothing in memory, over the same disk.
    fn restart(&self) -> Fx {
        Fx::over(self.world.clone(), self.seen.clone())
    }

    /// Where Desktop puts `caller_id`'s caller.json when it picks the place.
    fn handoff_of(&self, caller_id: &str) -> PathBuf {
        self.path("app/ServiceCallers/handoff")
            .join(format!("{caller_id}.json"))
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.world.root.join(relative)
    }

    fn project(&self) -> String {
        self.path("work/project").display().to_string()
    }

    fn write_node(&self, value: &Value) {
        std::fs::write(
            self.path("app").join(NODE_FILE),
            serde_json::to_vec(value).unwrap(),
        )
        .unwrap();
    }

    fn advance(&self, ms: i64) {
        self.world.clock.fetch_add(ms, Ordering::SeqCst);
    }

    fn now(&self) -> i64 {
        self.world.clock.load(Ordering::SeqCst)
    }

    fn requests(&self) -> u64 {
        self.world.relaycast.lock().unwrap().requests
    }

    fn spawns(&self) -> u64 {
        self.world.relaycast.lock().unwrap().spawns
    }

    fn token_of(&self, agent_id: &str) -> String {
        self.world.relaycast.lock().unwrap().tokens[agent_id].clone()
    }

    fn see<T: std::fmt::Debug + serde::Serialize>(&self, outcome: &Result<T, RpcError>) {
        let mut seen = self.seen.lock().unwrap();
        seen.push(format!("{outcome:?}"));
        match outcome {
            Ok(value) => seen.push(serde_json::to_string(value).unwrap()),
            Err(error) => seen.push(serde_json::to_string(error).unwrap()),
        }
    }

    /// One bridge call from `pid`, as the extension receives it, on a
    /// connection the core did not identify.
    async fn call(&self, pid: i32, method: &str, params: Value) -> Result<Value, RpcError> {
        self.call_on(pid, None, method, params).await
    }

    /// One bridge call from `pid` on bridge connection `connection_id`.
    async fn call_on(
        &self,
        pid: i32,
        connection_id: Option<u64>,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        let context = CallContext {
            peer_pid: Some(pid),
            connection_id,
        };
        let outcome = self
            .callers
            .call(&context, method, params)
            .await
            .expect("a service-caller method");
        self.see(&outcome);
        outcome
    }

    fn provision_params(&self, name: &str) -> Value {
        json!({
            "name": name,
            "harnesses": ["claude"],
            "directory_roots": [self.project()],
        })
    }

    /// Provisions `name` from the app and reads its `caller.json`.
    async fn provision(&self, name: &str) -> Caller {
        let provisioned = self
            .call(APP, "spawn_callers.provision", self.provision_params(name))
            .await
            .unwrap();
        let handoff = PathBuf::from(provisioned["handoff_path"].as_str().unwrap());
        let file = read_json(&handoff);
        Caller {
            id: provisioned["caller"]["caller_id"]
                .as_str()
                .unwrap()
                .to_string(),
            name: name.to_string(),
            secret: file["secret"].as_str().unwrap().to_string(),
            agent_id: provisioned["caller"]["publisher"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
            handoff,
        }
    }

    fn auth(caller: &Caller) -> serde_json::Map<String, Value> {
        let mut map = serde_json::Map::new();
        map.insert("caller_name".into(), caller.name.clone().into());
        map.insert("caller_secret".into(), caller.secret.clone().into());
        map
    }

    /// A call acting as `caller`. A dev-attest preview build sends the
    /// phase-1 name and secret on no connection; the default build refuses
    /// those, so the call goes on a fresh connection bound to `caller` by
    /// challenge and authentication.
    async fn caller_call(
        &self,
        pid: i32,
        caller: &Caller,
        method: &str,
        extra: Value,
    ) -> Result<Value, RpcError> {
        if cfg!(feature = "service-callers-dev-attest") {
            let mut params = Fx::auth(caller);
            if let Value::Object(extra) = extra {
                params.extend(extra);
            }
            return self.call(pid, method, Value::Object(params)).await;
        }
        let connection = NEXT_CALLER_CONNECTION.fetch_add(1, Ordering::SeqCst);
        self.bind(pid, connection, caller).await?;
        self.call_on(pid, Some(connection), method, extra).await
    }

    /// Binds `connection` to `caller` unless it already is.
    async fn bound_to(&self, pid: i32, connection: u64, caller: &Caller) -> Result<(), RpcError> {
        match self.bind(pid, connection, caller).await {
            Err(error) if error.message != "already_bound" => Err(error),
            _ => Ok(()),
        }
    }

    async fn identity(&self, pid: i32, caller: &Caller) -> Result<Value, RpcError> {
        self.caller_call(pid, caller, "spawn_callers.identity", json!({}))
            .await
    }

    async fn open(&self, pid: i32, caller: &Caller, invocation: &str) -> Result<String, RpcError> {
        self.caller_call(
            pid,
            caller,
            "spawn_callers.inbox.open",
            json!({"invocation_id": invocation}),
        )
        .await
        .map(|opened| opened["iterator_id"].as_str().unwrap().to_string())
    }

    /// `inbox.open` on bridge connection `connection`.
    async fn open_on(
        &self,
        pid: i32,
        connection: u64,
        caller: &Caller,
        invocation: &str,
    ) -> Result<String, RpcError> {
        let params = if cfg!(feature = "service-callers-dev-attest") {
            let mut params = Fx::auth(caller);
            params.insert("invocation_id".into(), invocation.into());
            params
        } else {
            self.bound_to(pid, connection, caller).await?;
            let mut params = serde_json::Map::new();
            params.insert("invocation_id".into(), invocation.into());
            params
        };
        self.call_on(
            pid,
            Some(connection),
            "spawn_callers.inbox.open",
            Value::Object(params),
        )
        .await
        .map(|opened| opened["iterator_id"].as_str().unwrap().to_string())
    }

    /// The core reports that bridge connection `connection` of `pid` ended.
    fn connection_closed(&self, pid: i32, connection: u64) {
        self.callers.connection_closed(&CallContext {
            peer_pid: Some(pid),
            connection_id: Some(connection),
        });
    }

    async fn next_within(
        &self,
        pid: i32,
        caller: &Caller,
        iterator: &str,
        timeout_ms: u64,
    ) -> Result<Option<Value>, RpcError> {
        self.caller_call(
            pid,
            caller,
            "spawn_callers.inbox.next",
            json!({"iterator_id": iterator, "timeout_ms": timeout_ms}),
        )
        .await
        .map(|next| Some(next["delivery"].clone()).filter(|delivery| !delivery.is_null()))
    }

    async fn next(
        &self,
        pid: i32,
        caller: &Caller,
        iterator: &str,
    ) -> Result<Option<Value>, RpcError> {
        self.next_within(pid, caller, iterator, 0).await
    }

    async fn ack(&self, pid: i32, caller: &Caller, item: &str) -> Result<Value, RpcError> {
        self.caller_call(
            pid,
            caller,
            "spawn_callers.inbox.ack",
            json!({"item_id": item, "metadata": {"relayflows_result": "journaled"}}),
        )
        .await
    }

    async fn fail(&self, pid: i32, caller: &Caller, item: &str) -> Result<Value, RpcError> {
        self.caller_call(
            pid,
            caller,
            "spawn_callers.inbox.fail",
            json!({"item_id": item, "metadata": {"relayflows_result": "rejected"}}),
        )
        .await
    }

    async fn close(&self, pid: i32, caller: &Caller, iterator: &str) -> Result<Value, RpcError> {
        self.caller_call(
            pid,
            caller,
            "spawn_callers.inbox.close",
            json!({"iterator_id": iterator}),
        )
        .await
    }

    fn invoke_params(&self, _caller: &Caller) -> SpawnInvokeParams {
        let key = self.world.keys.fetch_add(1, Ordering::SeqCst);
        SpawnInvokeParams {
            caller_name: None,
            node_id: NODE_ID.into(),
            node_name: NODE_NAME.into(),
            name: format!("worker-{key}"),
            harness: "claude".into(),
            task: "TASKTEXT summarise the repository".into(),
            directory: self.project(),
            idempotency_key: format!("idem-{key}"),
        }
    }

    /// Binds a fresh bridge connection from `pid` to `caller` by challenge
    /// and authentication, as `bind` does over the bridge, without awaiting.
    fn bind_now(&self, pid: i32, caller: &Caller) -> Result<u64, RpcError> {
        let connection = NEXT_CALLER_CONNECTION.fetch_add(1, Ordering::SeqCst);
        let challenge = {
            let _entry = self.callers.enter(Some(connection));
            let params = crate::contract::service_callers::CallerChallengeParams {
                caller_id: caller.id.clone(),
            };
            self.callers.challenge(Some(pid), connection, &params)?
        };
        let challenge = serde_json::to_value(challenge).unwrap();
        let body = json!({
            "challenge_id": challenge["challenge_id"],
            "authenticator": answer(caller, &challenge),
        });
        let _entry = self.callers.enter(Some(connection));
        self.callers
            .authenticate_connection(connection, &body, false)?;
        Ok(connection)
    }

    /// How the core resolves a bridge connection from `pid` bound to
    /// `caller` (`SpawnCallers::connection`), and that connection.
    fn resolve_on(&self, pid: i32, caller: &Caller) -> Result<(ExternalCaller, u64), RpcError> {
        let connection = self.bind_now(pid, caller)?;
        let context = CallContext {
            peer_pid: Some(pid),
            connection_id: Some(connection),
        };
        match self.callers.connection(&context) {
            ConnectionCaller::Bound(resolved) => Ok((resolved, connection)),
            ConnectionCaller::Refused(error) => Err(error),
            ConnectionCaller::NotExternal => panic!("a bound connection resolved as a session"),
        }
    }

    /// The external caller a connection bound to `caller` resolves to.
    fn resolve(&self, caller: &Caller) -> Result<ExternalCaller, RpcError> {
        self.resolve_on(RUNTIME, caller)
            .map(|(resolved, _)| resolved)
    }

    /// `spawn.invoke` as the core runs it: resolve the bound connection,
    /// admit, the Relaycast spawn, record. Returns the invocation id and
    /// the worker's agent id.
    fn invoke_with(
        &self,
        pid: i32,
        caller: &Caller,
        params: &SpawnInvokeParams,
    ) -> Result<(String, String), RpcError> {
        let outcome = (|| {
            let workspace = workspace();
            let (caller, connection) = self.resolve_on(pid, caller)?;
            let permit = self.callers.admit(
                &caller,
                &SpawnCall {
                    peer_pid: Some(pid),
                    connection_id: Some(connection),
                    workspace: &workspace,
                    request: SpawnRequest::Invoke(params),
                },
            )?;
            let n = {
                let mut relaycast = self.world.relaycast.lock().unwrap();
                relaycast.requests += 1;
                relaycast.spawns += 1;
                relaycast.next += 1;
                relaycast.next
            };
            let (invocation, worker) = (format!("inv_{n}"), format!("wk_{n}"));
            self.world.spawned.lock().unwrap().push(Spawned {
                agent_id: worker.clone(),
                name: params.name.clone(),
                invocation_id: invocation.clone(),
                requester_id: Some(caller.agent_id.clone()),
                process: None,
                active: true,
            });
            self.callers
                .record(permit, &Ok(invocation_of(&invocation, &worker)))?;
            Ok((invocation, worker))
        })();
        self.see(&outcome);
        outcome
    }

    fn invoke(&self, caller: &Caller) -> Result<(String, String), RpcError> {
        self.invoke_with(RUNTIME, caller, &self.invoke_params(caller))
    }

    /// `spawn.status` or `spawn.release` as the core runs them.
    fn control(&self, caller: &Caller, request: SpawnRequest<'_>) -> Result<(), RpcError> {
        self.control_with(caller, request, invocation_of("inv_x", "wk_x"))
    }

    /// [`Fx::control`] with the invocation the core answers with.
    fn control_with(
        &self,
        caller: &Caller,
        request: SpawnRequest<'_>,
        answer: SpawnControlInvocation,
    ) -> Result<(), RpcError> {
        let outcome = (|| {
            let workspace = workspace();
            let (resolved, connection) = self.resolve_on(RUNTIME, caller)?;
            let permit = self.callers.admit(
                &resolved,
                &SpawnCall {
                    peer_pid: Some(RUNTIME),
                    connection_id: Some(connection),
                    workspace: &workspace,
                    request,
                },
            )?;
            self.world.relaycast.lock().unwrap().requests += 1;
            self.callers.record(permit, &Ok(answer))?;
            Ok(())
        })();
        self.see(&outcome);
        outcome
    }

    fn status(&self, caller: &Caller, invocation: &str) -> Result<(), RpcError> {
        let params = SpawnInvocationParams {
            caller_name: None,
            action: SpawnControlAction::Spawn,
            invocation_id: invocation.into(),
        };
        self.control(caller, SpawnRequest::Status(&params))
    }

    fn release(&self, caller: &Caller, worker: &str) -> Result<(), RpcError> {
        self.release_as(caller, worker, release_receipt_of("inv_x"))
    }

    /// `spawn.release` of `worker`, the core answering with `answer`.
    fn release_as(
        &self,
        caller: &Caller,
        worker: &str,
        answer: SpawnControlInvocation,
    ) -> Result<(), RpcError> {
        let params = SpawnReleaseParams {
            caller_name: None,
            node_id: NODE_ID.into(),
            node_name: NODE_NAME.into(),
            name: "worker".into(),
            expected_agent_id: worker.into(),
            idempotency_key: "release-1".into(),
        };
        self.control_with(caller, SpawnRequest::Release(&params), answer)
    }

    /// `sender` DMs `to`'s publisher identity at Relaycast.
    fn report(&self, sender: &str, to: &Caller, text: &str) -> String {
        let mut relaycast = self.world.relaycast.lock().unwrap();
        relaycast.next += 1;
        let n = relaycast.next;
        let token = relaycast.tokens[&to.agent_id].clone();
        let message_id = format!("msg_{n}");
        relaycast.queue.entry(token).or_default().push(Incoming {
            delivery_id: format!("dlv_{n}"),
            message_id: message_id.clone(),
            sender_id: Some(sender.to_string()),
            sender_name: Some(sender.to_string()),
            text: text.to_string(),
            created_at_ms: Some(self.now()),
        });
        message_id
    }

    fn waiting_at_relaycast(&self, caller: &Caller) -> usize {
        let relaycast = self.world.relaycast.lock().unwrap();
        let token = &relaycast.tokens[&caller.agent_id];
        relaycast.queue.get(token).map_or(0, Vec::len)
    }

    async fn deliver(&self) {
        self.callers.poll_once().await;
    }

    /// An invocation of `caller` whose worker has reported `text`.
    async fn reported(&self, caller: &Caller, text: &str) -> (String, String) {
        let (invocation, worker) = self.invoke(caller).unwrap();
        self.report(&worker, caller, text);
        self.deliver().await;
        (invocation, worker)
    }

    fn audit(&self) -> String {
        let folder = self.path("app/ServiceCallers");
        let mut text = String::new();
        for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            if entry.file_name().to_string_lossy().contains(".audit.jsonl") {
                text.push_str(&std::fs::read_to_string(entry.path()).unwrap());
            }
        }
        text
    }

    /// Every log line, audit line, answer and error so far.
    fn everything_shown(&self) -> String {
        let mut text = self.world.logs.lock().unwrap().join("\n");
        text.push('\n');
        text.push_str(&self.seen.lock().unwrap().join("\n"));
        text.push('\n');
        text.push_str(&self.audit());
        text
    }

    fn registry_file(&self) -> PathBuf {
        let key = super::registry::key(SITE, "ws_test");
        self.path("app/ServiceCallers").join(format!("{key}.json"))
    }
}

fn enrolment() -> Value {
    json!({
        "nodeID": NODE_ID,
        "nodeName": NODE_NAME,
        "token": "node_tok_ENROLMENT",
        "baseURL": BASE_URL,
        "workspace": super::node::fingerprint(&Secret::new(WORKSPACE_KEY)),
    })
}

fn invocation_of(id: &str, worker: &str) -> SpawnControlInvocation {
    SpawnControlInvocation {
        id: id.into(),
        agent_id: Some(worker.into()),
        status: "ready".into(),
        node_id: Some(NODE_ID.into()),
        name: Some("worker".into()),
        spawned: true,
        ready: true,
        released: false,
        task_unconfirmed: false,
        error_code: None,
    }
}

/// A completed `spawn.release` receipt as the core builds it: it names no
/// agent, only that the worker was released.
fn release_receipt_of(id: &str) -> SpawnControlInvocation {
    SpawnControlInvocation {
        id: id.into(),
        agent_id: None,
        status: "completed".into(),
        node_id: Some(NODE_ID.into()),
        name: Some("worker".into()),
        spawned: false,
        ready: false,
        released: true,
        task_unconfirmed: false,
        error_code: None,
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[track_caller]
fn assert_refused(error: &RpcError, reason: RefusalReason, class: &str) {
    assert_eq!(error.data.kind, ErrorKind::Refused, "{error:?}");
    assert_eq!(error.data.reason, Some(reason), "{error:?}");
    assert_eq!(error.message, class);
    assert!(!error.data.retryable);
}

/// A refusal of a caller acting through [`Fx::caller_call`]. A dev-attest
/// preview build answers the phase-1 credential with its own class; the
/// default build authenticates first, which collapses every caller refusal
/// to `auth_failed`.
#[track_caller]
fn assert_caller_refused(error: &RpcError, reason: RefusalReason, class: &str) {
    if cfg!(feature = "service-callers-dev-attest") {
        assert_refused(error, reason, class);
    } else {
        assert_refused(error, RefusalReason::ServiceUnknown, "auth_failed");
    }
}

#[track_caller]
fn assert_kind(error: &RpcError, kind: ErrorKind, class: &str) {
    assert_eq!(error.data.kind, kind, "{error:?}");
    assert_eq!(error.message, class);
}

fn item_id(delivery: &Value) -> String {
    delivery["item_id"].as_str().unwrap().to_string()
}

/// A repeat's answer is the first receipt, field for field and byte for
/// byte.
#[track_caller]
fn assert_same_receipt(first: &Value, repeat: &Value) {
    assert_eq!(first, repeat);
    assert_eq!(
        serde_json::to_string(first).unwrap(),
        serde_json::to_string(repeat).unwrap()
    );
    let typed: InboxSettled = serde_json::from_value(repeat.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), *first);
}

// --- Section 10 ---------------------------------------------------------

#[tokio::test]
async fn a_cold_core_resolves_a_caller_provisioned_before_restart() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    drop(fx.callers);
    let cold = Fx::over(fx.world.clone(), fx.seen.clone());
    let resolved = cold.resolve(&caller).unwrap();
    assert_eq!(resolved.agent_id, caller.agent_id);
    assert_eq!(resolved.name, "flows");
    assert!(resolved.owns_only);
    assert_eq!(resolved.harnesses, Some(vec!["claude".to_string()]));
    assert_eq!(
        resolved.directory_roots,
        Some(vec![PathBuf::from(cold.project())])
    );
    assert_eq!(resolved.token.expose(), cold.token_of(&caller.agent_id));
    assert!(cold.invoke(&caller).is_ok());
}

#[tokio::test]
async fn a_rotated_caller_accepts_the_old_secret_until_the_overlap_ends() {
    let fx = Fx::new();
    let old = fx.provision("flows").await;
    let rotated = fx
        .call(
            APP,
            "spawn_callers.rotate",
            json!({"caller_id": old.id, "overlap_seconds": 60}),
        )
        .await
        .unwrap();
    assert!(rotated["old_secret_retires_at"].is_string());
    let new = old.with_secret(read_json(&old.handoff)["secret"].as_str().unwrap());
    assert_ne!(new.secret, old.secret);
    fx.advance(59_999);
    assert!(fx.identity(RUNTIME, &old).await.is_ok());
    assert!(fx.identity(RUNTIME, &new).await.is_ok());
    fx.advance(1);
    let error = fx.identity(RUNTIME, &old).await.unwrap_err();
    // Retirement removed the old digest (generation advance 2): the old
    // secret now matches nothing, like any unknown one.
    assert_caller_refused(&error, RefusalReason::ServiceUnknown, "caller_unknown");
    assert!(fx.invoke(&old).is_err());
    assert!(fx.identity(RUNTIME, &new).await.is_ok());
}

#[tokio::test]
async fn sign_out_deletes_the_registry_and_refuses_every_caller() {
    let fx = Fx::new();
    let one = fx.provision("flows").await;
    let two = fx.provision("flows-two").await;
    let (invocation, _) = fx.reported(&one, "done").await;
    assert!(fx.registry_file().exists());
    fx.callers.forget(&account()).await;
    assert!(!fx.path("app/ServiceCallers").exists());
    assert!(!one.handoff.exists() && !two.handoff.exists());
    let mut deleted = fx.world.relaycast.lock().unwrap().deleted.clone();
    deleted.sort();
    assert_eq!(deleted, vec!["flows", "flows-two"]);
    for caller in [&one, &two] {
        let error = fx.identity(RUNTIME, caller).await.unwrap_err();
        assert_caller_refused(&error, RefusalReason::ServiceUnknown, "caller_unknown");
        assert!(fx.invoke(caller).is_err());
    }
    assert!(fx.open(RUNTIME, &one, &invocation).await.is_err());
    // Signed out entirely: nothing resolves at all.
    fx.world.signed_in.store(false, Ordering::SeqCst);
    let error = fx.identity(RUNTIME, &one).await.unwrap_err();
    assert_kind(&error, ErrorKind::NotSignedIn, "not_signed_in");
}

#[tokio::test]
async fn an_owns_only_caller_cannot_status_or_release_another_callers_spawn() {
    let fx = Fx::new();
    let one = fx.provision("flows").await;
    let two = fx.provision("other").await;
    let (invocation, worker) = fx.invoke(&one).unwrap();
    assert!(fx.status(&one, &invocation).is_ok());
    assert!(fx.release(&one, &worker).is_ok());
    let before = fx.requests();
    let error = fx.status(&two, &invocation).unwrap_err();
    assert_refused(&error, RefusalReason::NotOwner, "not_in_ledger");
    let error = fx.release(&two, &worker).unwrap_err();
    assert_refused(&error, RefusalReason::NotOwner, "requester_other");
    // An agent no caller started: not in any ledger.
    let error = fx.release(&two, "wk_someone_elses").unwrap_err();
    assert_refused(&error, RefusalReason::NotOwner, "not_in_ledger");
    assert_eq!(fx.requests(), before, "refused before Relaycast");
}

/// A release answers with an invocation of its own: the ledger names it,
/// with the released worker, so its caller can poll it with
/// `spawn.status`, and no other caller can.
#[tokio::test]
async fn a_release_invocation_is_kept_in_its_callers_ledger_and_no_one_elses() {
    let fx = Fx::new();
    let one = fx.provision("flows").await;
    let two = fx.provision("other").await;
    let (_, worker) = fx.invoke(&one).unwrap();
    let receipt = release_receipt_of("inv_release_1");
    assert_eq!(receipt.agent_id, None, "the core's receipt names no agent");
    fx.release_as(&one, &worker, receipt).unwrap();
    assert!(fx.status(&one, "inv_release_1").is_ok());
    let error = fx.status(&two, "inv_release_1").unwrap_err();
    assert_refused(&error, RefusalReason::NotOwner, "not_in_ledger");
    let entry = record_of(&fx, &one.id).unwrap()["invocations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["invocation_id"] == "inv_release_1")
        .cloned()
        .unwrap();
    assert_eq!(entry["worker_agent_id"], worker.as_str());
}

/// A release is ledgered with the worker it was admitted for, never with
/// another one its receipt might name.
#[tokio::test]
async fn a_release_is_ledgered_with_its_admitted_worker_over_the_receipts() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, worker) = fx.invoke(&caller).unwrap();
    let receipt = SpawnControlInvocation {
        agent_id: Some("wk_someone_else".into()),
        ..release_receipt_of("inv_release_1")
    };
    fx.release_as(&caller, &worker, receipt).unwrap();
    let entry = ledger_entry_of(&fx, &caller.id, "inv_release_1");
    assert_eq!(entry["worker_agent_id"], worker.as_str());
}

/// A release whose ledger entry cannot be written answers a retryable
/// `store_failed` instead of a receipt its caller could not poll. The same
/// release retried after the store recovers lands in the ledger with its
/// admitted worker.
#[tokio::test]
async fn a_release_whose_ledger_entry_fails_to_store_is_gated() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, worker) = fx.invoke(&caller).unwrap();
    super::registry::FAIL_SAVE.with(|fail| fail.set(true));
    let outcome = fx.release_as(&caller, &worker, release_receipt_of("inv_release_1"));
    super::registry::FAIL_SAVE.with(|fail| fail.set(false));
    let error = outcome.unwrap_err();
    assert_kind(&error, ErrorKind::StoreFailed, "store_failed");
    assert!(fx
        .audit()
        .contains(r#""method":"spawn.release","outcome":"store_failed""#));
    let error = fx.status(&caller, "inv_release_1").unwrap_err();
    assert_refused(&error, RefusalReason::NotOwner, "not_in_ledger");
    fx.release_as(&caller, &worker, release_receipt_of("inv_release_1"))
        .unwrap();
    assert!(fx.status(&caller, "inv_release_1").is_ok());
    let entry = ledger_entry_of(&fx, &caller.id, "inv_release_1");
    assert_eq!(entry["worker_agent_id"], worker.as_str());
}

#[tokio::test]
async fn a_harness_outside_the_caller_scope_is_refused_before_relaycast() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let before = fx.requests();
    let mut params = fx.invoke_params(&caller);
    params.harness = "codex".into();
    let error = fx.invoke_with(RUNTIME, &caller, &params).unwrap_err();
    assert_refused(&error, RefusalReason::OutsideScope, "harness_outside");
    assert_eq!(fx.requests(), before);
    assert_eq!(fx.spawns(), 0);
    assert!(fx.world.spawned.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_directory_escaping_its_root_by_symlink_or_dotdot_is_refused() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    std::os::unix::fs::symlink(fx.path("outside"), fx.path("work/project/link")).unwrap();
    let escapes = [
        format!("{}/../other", fx.project()),
        format!("{}/link", fx.project()),
        "relative/path".to_string(),
    ];
    for escape in escapes {
        let mut params = fx.invoke_params(&caller);
        params.directory = escape;
        let error = fx.invoke_with(RUNTIME, &caller, &params).unwrap_err();
        assert_refused(&error, RefusalReason::OutsideScope, "directory_outside");
    }
    assert_eq!(fx.spawns(), 0);
    let mut params = fx.invoke_params(&caller);
    params.directory = format!("{}/sub", fx.project());
    assert!(fx.invoke_with(RUNTIME, &caller, &params).is_ok());
    // A root given with `..`, or one that is a link out, is refused too.
    let mut provision = fx.provision_params("dotdot");
    provision["directory_roots"] = json!([format!("{}/../other", fx.project())]);
    let error = fx
        .call(APP, "spawn_callers.provision", provision)
        .await
        .unwrap_err();
    assert_refused(&error, RefusalReason::OutsideScope, "directory_outside");
}

#[tokio::test]
async fn no_log_audit_or_error_contains_a_caller_secret_or_token() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let token = fx.token_of(&caller.agent_id);
    let wrong = caller.with_secret("rsc_WRONGSECRETBYTES");
    // Refusals of every kind, on every path.
    let _ = fx.identity(RUNTIME, &wrong).await;
    let _ = fx.identity(STRANGER, &caller).await;
    let _ = fx.open(RUNTIME, &caller, "inv_not_mine").await;
    let _ = fx.invoke(&wrong);
    let mut params = fx.invoke_params(&caller);
    params.harness = "codex".into();
    let _ = fx.invoke_with(RUNTIME, &caller, &params);
    let _ = fx
        .call(
            APP,
            "spawn_callers.list",
            json!({"caller_secret": caller.secret}),
        )
        .await;
    // And the working path, end to end.
    let (invocation, _) = fx.reported(&caller, "RESULTTEXT all good").await;
    let iterator = fx.open(RUNTIME, &caller, &invocation).await.unwrap();
    let delivery = fx.next(RUNTIME, &caller, &iterator).await.unwrap().unwrap();
    fx.ack(RUNTIME, &caller, &item_id(&delivery)).await.unwrap();
    let _ = fx.fail(RUNTIME, &caller, &item_id(&delivery)).await;
    let _ = fx
        .call(
            APP,
            "spawn_callers.rotate",
            json!({"caller_id": caller.id, "overlap_seconds": 0}),
        )
        .await;
    let _ = fx.identity(RUNTIME, &caller).await;
    // The audit was written, and the scan below reads it.
    assert!(fx.audit().contains("secret_mismatch"));
    assert!(fx.audit().contains("peer_uid_mismatch"));
    let shown = fx.everything_shown();
    let new_secret = read_json(&caller.handoff)["secret"]
        .as_str()
        .unwrap()
        .to_string();
    let digest = super::registry::digest(&caller.secret);
    for needle in [
        caller.secret.as_str(),
        new_secret.as_str(),
        "rsc_WRONGSECRETBYTES",
        token.as_str(),
        "TOKENBYTES",
        digest.as_str(),
        WORKSPACE_KEY,
        "TASKTEXT",
    ] {
        assert!(!shown.contains(needle), "{needle} was shown");
    }
    // The delivery itself carries the result text, and only it does.
    assert!(!fx.audit().contains("RESULTTEXT"));
    assert!(!fx
        .world
        .logs
        .lock()
        .unwrap()
        .join("\n")
        .contains("RESULTTEXT"));
}

#[tokio::test]
async fn a_peer_whose_uid_is_not_this_user_is_refused() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let error = fx.identity(STRANGER, &caller).await.unwrap_err();
    assert_refused(&error, RefusalReason::ManagementPeer, "peer_uid_mismatch");
    let error = fx
        .call(STRANGER, "spawn_callers.list", json!({}))
        .await
        .unwrap_err();
    assert_refused(&error, RefusalReason::ManagementPeer, "peer_uid_mismatch");
    let before = fx.requests();
    let error = fx
        .invoke_with(STRANGER, &caller, &fx.invoke_params(&caller))
        .unwrap_err();
    assert_refused(&error, RefusalReason::ManagementPeer, "peer_uid_mismatch");
    assert_eq!(fx.requests(), before);
    // No peer at all, or one the process table cannot read.
    let params = crate::contract::service_callers::CallerAuthParams {
        caller_name: caller.name.clone(),
        caller_secret: Some(caller.secret.clone()),
    };
    let error = fx.callers.identity(None, &params).unwrap_err();
    assert_refused(&error, RefusalReason::ManagementPeer, "peer_unreadable");
    let error = fx.identity(9999, &caller).await.unwrap_err();
    assert_refused(&error, RefusalReason::ManagementPeer, "peer_unreadable");
}

#[tokio::test]
async fn the_result_inbox_delivers_only_the_callers_own_results() {
    let fx = Fx::new();
    let one = fx.provision("flows").await;
    let two = fx.provision("other").await;
    let (invocation, worker) = fx.invoke(&one).unwrap();
    let (_, others_worker) = fx.invoke(&two).unwrap();
    fx.report("agt_stranger", &one, "not from a worker");
    fx.report(&others_worker, &one, "from another caller's worker");
    fx.report(&worker, &one, "mine");
    fx.deliver().await;
    assert_eq!(
        fx.waiting_at_relaycast(&one),
        0,
        "kept or dropped, all acknowledged"
    );
    assert!(fx.audit().contains(r#""outcome":"dropped","count":2"#));
    let iterator = fx.open(RUNTIME, &one, &invocation).await.unwrap();
    let delivery = fx.next(RUNTIME, &one, &iterator).await.unwrap().unwrap();
    assert_eq!(delivery["text"], "mine");
    assert_eq!(delivery["sender"]["id"], worker.as_str());
    // The other caller can neither open the invocation nor settle the item.
    let error = fx.open(RUNTIME, &two, &invocation).await.unwrap_err();
    assert_refused(&error, RefusalReason::NotOwner, "not_in_ledger");
    let error = fx
        .ack(RUNTIME, &two, &item_id(&delivery))
        .await
        .unwrap_err();
    assert_refused(&error, RefusalReason::NotOwner, "item_not_owned");
}

#[tokio::test]
async fn an_unsettled_delivery_is_redelivered_until_ack_or_fail() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (invocation, _) = fx.reported(&caller, "result").await;
    let first = fx.open(RUNTIME, &caller, &invocation).await.unwrap();
    let delivered = fx.next(RUNTIME, &caller, &first).await.unwrap().unwrap();
    fx.close(RUNTIME, &caller, &first).await.unwrap();
    let second = fx.open(RUNTIME, &caller, &invocation).await.unwrap();
    let again = fx.next(RUNTIME, &caller, &second).await.unwrap().unwrap();
    assert_eq!(again, delivered);
    // A cold core redelivers it too.
    let cold = fx.restart();
    let third = cold.open(RUNTIME, &caller, &invocation).await.unwrap();
    assert_eq!(
        cold.next(RUNTIME, &caller, &third).await.unwrap(),
        Some(delivered.clone())
    );
    cold.ack(RUNTIME, &caller, &item_id(&delivered))
        .await
        .unwrap();
    let fourth = cold.open(RUNTIME, &caller, &invocation).await.unwrap();
    assert_eq!(cold.next(RUNTIME, &caller, &fourth).await.unwrap(), None);
    // And `fail` ends redelivery the same way.
    let (invocation, _) = cold.reported(&caller, "malformed").await;
    let iterator = cold.open(RUNTIME, &caller, &invocation).await.unwrap();
    let delivery = cold
        .next(RUNTIME, &caller, &iterator)
        .await
        .unwrap()
        .unwrap();
    cold.fail(RUNTIME, &caller, &item_id(&delivery))
        .await
        .unwrap();
    cold.close(RUNTIME, &caller, &iterator).await.unwrap();
    let iterator = cold.open(RUNTIME, &caller, &invocation).await.unwrap();
    assert_eq!(cold.next(RUNTIME, &caller, &iterator).await.unwrap(), None);
}

/// Desktop's half: every method of the contract is in `hello.methods`
/// (the core's names and the account extension's), and the extension
/// answers each of its own. Flows' selection itself is Flows' test.
#[tokio::test]
async fn flows_selects_the_local_runtime_only_when_every_contract_method_is_present() {
    let mut hello = agent_relay_node::relay::contract::method_names(true);
    hello.extend(
        crate::contract::account_specs()
            .iter()
            .map(|spec| spec.name),
    );
    let contract = [
        "spawn.targets",
        "spawn.invoke",
        "spawn.status",
        "spawn.release",
        "spawn_callers.identity",
        "spawn_callers.inbox.open",
        "spawn_callers.inbox.next",
        "spawn_callers.inbox.ack",
        "spawn_callers.inbox.fail",
        "spawn_callers.inbox.close",
        "spawn_callers.disconnect",
    ];
    for method in contract {
        assert_eq!(
            hello.iter().filter(|name| **name == method).count(),
            1,
            "{method} listed once"
        );
    }
    let fx = Fx::new();
    for method in contract.iter().filter(|m| m.starts_with("spawn_callers.")) {
        let answer = fx
            .callers
            .call(&CallContext::default(), method, json!({}))
            .await;
        assert!(answer.is_some(), "{method} answered");
    }
}

#[tokio::test]
async fn ack_and_fail_metadata_are_exactly_the_two_fixed_values() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (invocation, _) = fx.reported(&caller, "result").await;
    let iterator = fx.open(RUNTIME, &caller, &invocation).await.unwrap();
    let item = item_id(&fx.next(RUNTIME, &caller, &iterator).await.unwrap().unwrap());
    let refused = [
        (
            "spawn_callers.inbox.ack",
            json!({"relayflows_result": "rejected"}),
        ),
        (
            "spawn_callers.inbox.fail",
            json!({"relayflows_result": "journaled"}),
        ),
        (
            "spawn_callers.inbox.ack",
            json!({"relayflows_result": "JOURNALED"}),
        ),
        (
            "spawn_callers.inbox.ack",
            json!({"relayflows_result": "journaled", "note": "TASKTEXT"}),
        ),
        ("spawn_callers.inbox.fail", json!({})),
    ];
    for (method, metadata) in refused {
        let error = fx
            .caller_call(
                RUNTIME,
                &caller,
                method,
                json!({"item_id": item, "metadata": metadata}),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.data.kind,
            ErrorKind::InvalidParams,
            "{method} {metadata}"
        );
    }
    // Still unsettled: redelivered after a close, then settled exactly.
    fx.close(RUNTIME, &caller, &iterator).await.unwrap();
    let iterator = fx.open(RUNTIME, &caller, &invocation).await.unwrap();
    assert!(fx
        .next(RUNTIME, &caller, &iterator)
        .await
        .unwrap()
        .is_some());
    fx.ack(RUNTIME, &caller, &item).await.unwrap();
    let stored = std::fs::read_to_string(
        fx.path("app/ServiceCallers")
            .join(format!("{}.results", super::registry::key(SITE, "ws_test")))
            .join(format!("{}.json", caller.id)),
    )
    .unwrap();
    assert!(stored.contains(r#""metadata": "journaled""#));
    assert!(!stored.contains("result\""), "settled items keep no text");
}

#[tokio::test]
async fn a_spawn_is_refused_before_relaycast_when_the_512_kib_result_reservation_cannot_be_made() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    for _ in 0..super::inbox::ACTIVE {
        fx.invoke(&caller).unwrap();
        // Released workers: only the reservations are held.
        for spawned in fx.world.spawned.lock().unwrap().iter_mut() {
            spawned.active = false;
        }
    }
    assert_eq!(
        super::inbox::ACTIVE as u64 * super::inbox::SLOT_BYTES,
        16 * 1024 * 1024
    );
    let (requests, spawns) = (fx.requests(), fx.spawns());
    let error = fx.invoke(&caller).unwrap_err();
    assert_kind(&error, ErrorKind::Busy, "reservation_full");
    assert_eq!((fx.requests(), fx.spawns()), (requests, spawns));
    // A settled item releases its reservation.
    let first = fx.world.spawned.lock().unwrap()[0].clone();
    fx.report(&first.agent_id, &caller, "done");
    fx.deliver().await;
    let iterator = fx
        .open(RUNTIME, &caller, &first.invocation_id)
        .await
        .unwrap();
    let item = item_id(&fx.next(RUNTIME, &caller, &iterator).await.unwrap().unwrap());
    fx.ack(RUNTIME, &caller, &item).await.unwrap();
    assert!(fx.invoke(&caller).is_ok());
}

#[tokio::test]
async fn an_invocation_holds_exactly_one_terminal_result_and_later_reports_are_audited() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (invocation, worker) = fx.invoke(&caller).unwrap();
    fx.report(&worker, &caller, "first");
    fx.report(&worker, &caller, "LATERTEXT second");
    fx.deliver().await;
    fx.report(&worker, &caller, "LATERTEXT third");
    fx.deliver().await;
    assert_eq!(fx.waiting_at_relaycast(&caller), 0);
    assert!(fx.audit().contains(r#""outcome":"later_report","count":1"#));
    assert!(!fx.audit().contains("LATERTEXT"));
    let iterator = fx.open(RUNTIME, &caller, &invocation).await.unwrap();
    let delivery = fx.next(RUNTIME, &caller, &iterator).await.unwrap().unwrap();
    assert_eq!(delivery["text"], "first");
    fx.ack(RUNTIME, &caller, &item_id(&delivery)).await.unwrap();
    let iterator = fx.open(RUNTIME, &caller, &invocation).await.unwrap();
    assert_eq!(fx.next(RUNTIME, &caller, &iterator).await.unwrap(), None);
}

#[tokio::test]
async fn an_oversize_report_becomes_a_typed_result_too_large_terminal_failure_that_flows_must_fail()
{
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let oversize = "x".repeat(super::inbox::SLOT_BYTES as usize + 1);
    let (invocation, _) = fx.reported(&caller, &oversize).await;
    assert_eq!(fx.waiting_at_relaycast(&caller), 0, "never left behind");
    let iterator = fx.open(RUNTIME, &caller, &invocation).await.unwrap();
    let delivery = fx.next(RUNTIME, &caller, &iterator).await.unwrap().unwrap();
    assert_eq!(delivery["failure"]["code"], "result_too_large");
    assert!(delivery.get("text").is_none());
    let error = fx
        .ack(RUNTIME, &caller, &item_id(&delivery))
        .await
        .unwrap_err();
    assert_kind(&error, ErrorKind::InvalidParams, "settlement_invalid");
    fx.fail(RUNTIME, &caller, &item_id(&delivery))
        .await
        .unwrap();
    // A report of exactly the budget is text.
    let exact = "y".repeat(super::inbox::SLOT_BYTES as usize);
    let (invocation, _) = fx.reported(&caller, &exact).await;
    let iterator = fx.open(RUNTIME, &caller, &invocation).await.unwrap();
    let delivery = fx.next(RUNTIME, &caller, &iterator).await.unwrap().unwrap();
    assert_eq!(delivery["text"].as_str().map(str::len), Some(exact.len()));
    assert!(delivery.get("failure").is_none());
}

#[tokio::test]
async fn an_iterator_delivers_only_its_own_invocations_results() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (one, worker_one) = fx.invoke(&caller).unwrap();
    let (two, worker_two) = fx.invoke(&caller).unwrap();
    let first = fx.open(RUNTIME, &caller, &one).await.unwrap();
    let second = fx.open(RUNTIME, &caller, &two).await.unwrap();
    fx.report(&worker_one, &caller, "one");
    fx.deliver().await;
    assert_eq!(fx.next(RUNTIME, &caller, &second).await.unwrap(), None);
    let delivery = fx.next(RUNTIME, &caller, &first).await.unwrap().unwrap();
    assert_eq!(delivery["invocation_id"], one.as_str());
    assert_eq!(delivery["text"], "one");
    fx.report(&worker_two, &caller, "two");
    fx.deliver().await;
    let delivery = fx.next(RUNTIME, &caller, &second).await.unwrap().unwrap();
    assert_eq!(delivery["invocation_id"], two.as_str());
    assert_eq!(
        fx.next(RUNTIME, &caller, &first).await.unwrap(),
        None,
        "leased"
    );
}

#[tokio::test]
async fn a_probe_provision_stays_pending_until_the_person_confirms_in_settings() {
    let fx = Fx::new();
    let pending = fx
        .call(
            PROBE,
            "spawn_callers.provision",
            fx.provision_params("flows"),
        )
        .await
        .unwrap();
    assert_eq!(pending["caller"]["state"], "pending_provision");
    let caller_id = pending["caller"]["caller_id"].as_str().unwrap().to_string();
    assert_eq!(pending.get("handoff_path"), None);
    let handoff = fx.handoff_of(&caller_id);
    assert!(!handoff.exists());
    assert_eq!(fx.requests(), 0);
    // Pending callers resolve nothing.
    let guess = Caller {
        id: caller_id.clone(),
        name: "flows".into(),
        secret: "rsc_guess".into(),
        agent_id: String::new(),
        handoff: handoff.clone(),
    };
    assert!(fx.identity(RUNTIME, &guess).await.is_err());
    // The probe cannot confirm it; the person, in the app, can.
    let error = fx
        .call(
            PROBE,
            "spawn_callers.decide",
            json!({"caller_id": caller_id, "approved": true}),
        )
        .await
        .unwrap_err();
    assert_refused(
        &error,
        RefusalReason::ManagementPeer,
        "app_attestation_unavailable",
    );
    assert!(!handoff.exists());
    let decided = fx
        .call(
            APP,
            "spawn_callers.decide",
            json!({"caller_id": caller_id, "approved": true}),
        )
        .await
        .unwrap();
    assert_eq!(decided["caller"]["state"], "enabled");
    assert_eq!(decided["handoff_path"], handoff.display().to_string());
    let caller = guess.with_secret(read_json(&handoff)["secret"].as_str().unwrap());
    assert!(fx.identity(RUNTIME, &caller).await.is_ok());
}

#[tokio::test]
async fn no_method_ever_returns_the_callers_relay_token() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let token = fx.token_of(&caller.agent_id);
    let (invocation, _) = fx.reported(&caller, "result").await;
    let iterator = fx.open(RUNTIME, &caller, &invocation).await.unwrap();
    let item = item_id(&fx.next(RUNTIME, &caller, &iterator).await.unwrap().unwrap());
    for (method, extra) in [
        ("spawn_callers.identity", json!({})),
        (
            "spawn_callers.inbox.next",
            json!({"iterator_id": iterator, "timeout_ms": 0}),
        ),
        (
            "spawn_callers.inbox.ack",
            json!({"item_id": item, "metadata": {"relayflows_result": "journaled"}}),
        ),
        (
            "spawn_callers.inbox.fail",
            json!({"item_id": item, "metadata": {"relayflows_result": "rejected"}}),
        ),
        (
            "spawn_callers.inbox.close",
            json!({"iterator_id": iterator}),
        ),
        ("spawn_callers.disconnect", json!({})),
    ] {
        let _ = fx.caller_call(RUNTIME, &caller, method, extra).await;
    }
    for (method, params) in [
        ("spawn_callers.list", json!({})),
        (
            "spawn_callers.set_enabled",
            json!({"caller_id": caller.id, "enabled": true}),
        ),
        ("spawn_callers.rotate", json!({"caller_id": caller.id})),
        (
            "spawn_callers.decide",
            json!({"caller_id": caller.id, "approved": true}),
        ),
        ("spawn_callers.revoke", json!({"caller_id": caller.id})),
    ] {
        let _ = fx.call(APP, method, params).await;
    }
    let seen = fx.seen.lock().unwrap().join("\n");
    assert!(seen.contains("spawn_callers") || seen.contains("caller_id"));
    assert!(!seen.contains(&token));
    assert!(!seen.contains("TOKENBYTES"));
    // Nor does caller.json.
    let fx2 = fx.provision("again").await;
    assert!(!std::fs::read_to_string(&fx2.handoff)
        .unwrap()
        .contains("TOKENBYTES"));
}

#[tokio::test]
async fn identity_names_the_enrolled_node_and_any_other_node_in_spawn_invoke_is_refused() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let identity = fx.identity(RUNTIME, &caller).await.unwrap();
    assert_eq!(identity["node"], json!({"id": NODE_ID, "name": NODE_NAME}));
    assert_eq!(
        identity["publisher"],
        json!({"id": caller.agent_id, "name": "flows"})
    );
    assert_eq!(identity["base_url"], BASE_URL);
    assert_eq!(identity["workspace_id"], "ws_test");
    assert_eq!(identity["relaycast_workspace_id"], "rw_test");
    let before = fx.requests();
    let mut other_id = fx.invoke_params(&caller);
    other_id.node_id = "node_someone_else".into();
    let error = fx.invoke_with(RUNTIME, &caller, &other_id).unwrap_err();
    assert_kind(&error, ErrorKind::Refused, "node_other");
    let mut other_name = fx.invoke_params(&caller);
    other_name.node_name = "mac-2-desktop".into();
    let error = fx.invoke_with(RUNTIME, &caller, &other_name).unwrap_err();
    assert_kind(&error, ErrorKind::Refused, "node_other");
    assert_eq!(fx.requests(), before);
    // Read on every call: a re-enrolment under an alternate shows at once.
    let mut alternate = enrolment();
    alternate["nodeID"] = json!("node_new");
    alternate["nodeName"] = json!("mac-1-desktop-0a1b");
    fx.write_node(&alternate);
    let identity = fx.identity(RUNTIME, &caller).await.unwrap();
    assert_eq!(
        identity["node"],
        json!({"id": "node_new", "name": "mac-1-desktop-0a1b"})
    );
    assert!(
        fx.invoke(&caller).is_err(),
        "the old node is now another node"
    );
    assert!(!std::fs::read_to_string(&caller.handoff)
        .unwrap()
        .contains("node_"));
}

#[tokio::test]
async fn an_identity_reply_without_a_node_or_with_a_malformed_one_admits_nothing() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let fingerprint = super::node::fingerprint(&Secret::new(WORKSPACE_KEY));
    let mut cases: Vec<Option<Value>> = vec![None];
    for (field, value) in [
        ("baseURL", json!("https://elsewhere.test")),
        ("workspace", json!("0000000000000000")),
        ("nodeName", json!("mac-2-desktop")),
        ("nodeName", json!("mac-1-desktop-XYZW")),
        ("nodeID", json!("")),
        ("nodeID", json!("node with spaces")),
        ("token", json!("")),
    ] {
        let mut enrolment = enrolment();
        enrolment[field] = value;
        cases.push(Some(enrolment));
    }
    cases.push(Some(json!({"nodeID": NODE_ID, "workspace": fingerprint})));
    for case in cases {
        let file = fx.path("app").join(NODE_FILE);
        match &case {
            None => {
                let _ = std::fs::remove_file(&file);
            }
            Some(value) => fx.write_node(value),
        }
        let error = fx.identity(RUNTIME, &caller).await.unwrap_err();
        assert_kind(&error, ErrorKind::Refused, "node_unavailable");
        let before = fx.requests();
        let error = fx.invoke(&caller).unwrap_err();
        assert_kind(&error, ErrorKind::Refused, "node_unavailable");
        assert_eq!(fx.requests(), before, "{case:?}");
    }
    std::fs::write(fx.path("app").join(NODE_FILE), b"{not json").unwrap();
    assert!(fx.identity(RUNTIME, &caller).await.is_err());
    assert_eq!(fx.spawns(), 0);
}

/// Desktop's half: `identity.node` is the enrolment, and `spawn.invoke`
/// refuses a target matching it on one field only. Readiness against
/// `spawn.targets` is Flows' test.
#[tokio::test]
async fn run_admission_needs_exactly_one_ready_target_matching_node_id_and_name() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let node = fx.identity(RUNTIME, &caller).await.unwrap()["node"].clone();
    let (id, name) = (node["id"].as_str().unwrap(), node["name"].as_str().unwrap());
    for (node_id, node_name) in [(id, "mac-1-desktop-0000"), ("node_other", name)] {
        let mut params = fx.invoke_params(&caller);
        params.node_id = node_id.into();
        params.node_name = node_name.into();
        let error = fx.invoke_with(RUNTIME, &caller, &params).unwrap_err();
        assert_kind(&error, ErrorKind::Refused, "node_other");
    }
    assert_eq!(fx.spawns(), 0);
    let mut params = fx.invoke_params(&caller);
    params.node_id = id.into();
    params.node_name = name.into();
    assert!(fx.invoke_with(RUNTIME, &caller, &params).is_ok());
}

#[tokio::test]
async fn one_caller_identity_serves_concurrent_bridge_connections_with_per_invocation_iterators() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (one, worker_one) = fx.invoke(&caller).unwrap();
    let (two, worker_two) = fx.invoke(&caller).unwrap();
    let first = fx.open(RUNTIME, &caller, &one).await.unwrap();
    let second = fx.open(RUNTIME2, &caller, &two).await.unwrap();
    fx.report(&worker_one, &caller, "one");
    fx.report(&worker_two, &caller, "two");
    // Both connections wait in `next` at once; one delivery pass wakes both.
    let (a, b, ()) = tokio::join!(
        fx.next_within(RUNTIME, &caller, &first, 5_000),
        fx.next_within(RUNTIME2, &caller, &second, 5_000),
        async {
            tokio::task::yield_now().await;
            fx.deliver().await;
        }
    );
    let (a, b) = (a.unwrap().unwrap(), b.unwrap().unwrap());
    assert_eq!(
        (a["text"].as_str(), b["text"].as_str()),
        (Some("one"), Some("two"))
    );
    fx.ack(RUNTIME2, &caller, &item_id(&a)).await.unwrap();
    fx.ack(RUNTIME, &caller, &item_id(&b)).await.unwrap();
    // Closing one leaves the identity and the other iterator alone.
    fx.close(RUNTIME, &caller, &first).await.unwrap();
    assert!(fx.next(RUNTIME2, &caller, &second).await.is_ok());
    assert!(fx.identity(RUNTIME, &caller).await.is_ok());
    let error = fx
        .caller_call(RUNTIME, &caller, "spawn_callers.disconnect", json!({}))
        .await
        .unwrap_err();
    assert_kind(&error, ErrorKind::Conflict, "iterator_open");
    fx.close(RUNTIME2, &caller, &second).await.unwrap();
    fx.caller_call(RUNTIME, &caller, "spawn_callers.disconnect", json!({}))
        .await
        .unwrap();
}

/// The owner's decision: an iterator close releases its leases at once; a
/// connection the core did not identify (`connection_id` `None`) has only
/// the 60 s lease expiry as its crash fallback, since no close is reported.
#[tokio::test]
async fn an_iterator_close_releases_its_delivered_unsettled_lease_at_once_and_an_unidentified_connection_falls_back_to_the_lease(
) {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    // Close: released at once, same instant.
    let (invocation, _) = fx.reported(&caller, "closed").await;
    let first = fx.open(RUNTIME, &caller, &invocation).await.unwrap();
    let delivered = fx.next(RUNTIME, &caller, &first).await.unwrap().unwrap();
    let second = fx.open(RUNTIME, &caller, &invocation).await.unwrap();
    assert_eq!(
        fx.next(RUNTIME, &caller, &second).await.unwrap(),
        None,
        "leased"
    );
    fx.close(RUNTIME, &caller, &first).await.unwrap();
    assert_eq!(
        fx.next(RUNTIME, &caller, &second).await.unwrap(),
        Some(delivered)
    );
    // No connection id: the connection simply goes away, nothing reports
    // its close, and the lease holds for 60 s.
    let (invocation, _) = fx.reported(&caller, "dropped").await;
    let dropped = fx.open(RUNTIME, &caller, &invocation).await.unwrap();
    let delivered = fx.next(RUNTIME, &caller, &dropped).await.unwrap().unwrap();
    let later = fx.open(RUNTIME2, &caller, &invocation).await.unwrap();
    fx.advance(super::inbox::LEASE_MS - 1);
    assert_eq!(fx.next(RUNTIME2, &caller, &later).await.unwrap(), None);
    fx.advance(1);
    assert_eq!(
        fx.next(RUNTIME2, &caller, &later).await.unwrap(),
        Some(delivered)
    );
}

/// The drop of the bridge connection that opened an iterator closes it and
/// releases its delivered, unsettled lease at once, not after 60 s.
#[tokio::test]
async fn a_connection_close_releases_its_iterators_leases_at_once() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (invocation, _) = fx.reported(&caller, "result").await;
    let held = fx.open_on(RUNTIME, 7, &caller, &invocation).await.unwrap();
    let delivered = fx.next(RUNTIME, &caller, &held).await.unwrap().unwrap();
    let later = fx.open_on(RUNTIME2, 8, &caller, &invocation).await.unwrap();
    assert_eq!(
        fx.next(RUNTIME2, &caller, &later).await.unwrap(),
        None,
        "leased"
    );
    fx.connection_closed(RUNTIME, 7);
    // The same instant, no clock advance: deliverable to the other connection.
    assert_eq!(
        fx.next(RUNTIME2, &caller, &later).await.unwrap(),
        Some(delivered.clone())
    );
    // The closed connection's iterator is gone, nothing pending on it.
    let error = fx.next(RUNTIME, &caller, &held).await.unwrap_err();
    assert_kind(&error, ErrorKind::NotFound, "not_found");
    // And a fresh open redelivers it once that holder lets go too.
    fx.connection_closed(RUNTIME2, 8);
    let fresh = fx.open(RUNTIME, &caller, &invocation).await.unwrap();
    assert_eq!(
        fx.next(RUNTIME, &caller, &fresh).await.unwrap(),
        Some(delivered.clone())
    );
    fx.ack(RUNTIME, &caller, &item_id(&delivered))
        .await
        .unwrap();
    assert!(fx.audit().contains("\"outcome\":\"connection_closed\""));
}

/// A connection close wakes a `next` waiting on one of its iterators, which
/// ends with no item consumed.
#[tokio::test]
async fn a_connection_close_ends_a_pending_next_on_its_iterator() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (invocation, _) = fx.invoke(&caller).unwrap();
    let iterator = fx.open_on(RUNTIME, 3, &caller, &invocation).await.unwrap();
    let started = std::time::Instant::now();
    let (waited, ()) = tokio::join!(fx.next_within(RUNTIME, &caller, &iterator, 5_000), async {
        tokio::task::yield_now().await;
        fx.connection_closed(RUNTIME, 3);
    });
    assert_eq!(waited.unwrap(), None);
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
    fx.caller_call(RUNTIME, &caller, "spawn_callers.disconnect", json!({}))
        .await
        .unwrap();
}

/// The close of another connection leaves this connection's iterator and
/// its lease alone.
#[tokio::test]
async fn another_connections_close_does_not_release_this_connections_lease() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (invocation, _) = fx.reported(&caller, "result").await;
    let held = fx.open_on(RUNTIME, 1, &caller, &invocation).await.unwrap();
    let delivered = fx.next(RUNTIME, &caller, &held).await.unwrap().unwrap();
    let other = fx.open_on(RUNTIME2, 2, &caller, &invocation).await.unwrap();
    fx.connection_closed(RUNTIME2, 2);
    // Connection 2's own iterator is closed.
    let error = fx.next(RUNTIME2, &caller, &other).await.unwrap_err();
    assert_kind(&error, ErrorKind::NotFound, "not_found");
    // Connection 1's lease still holds against a later connection, and its
    // iterator is still open.
    let later = fx.open_on(RUNTIME2, 3, &caller, &invocation).await.unwrap();
    assert_eq!(fx.next(RUNTIME2, &caller, &later).await.unwrap(), None);
    assert_eq!(fx.next(RUNTIME, &caller, &held).await.unwrap(), None);
    fx.close(RUNTIME, &caller, &held).await.unwrap();
    assert_eq!(
        fx.next(RUNTIME2, &caller, &later).await.unwrap(),
        Some(delivered)
    );
}

/// An identified connection's idle iterator is not swept after 60 s: only
/// a close, or the close of its connection, ends it.
#[tokio::test]
async fn an_identified_connections_idle_iterator_stays_open_until_its_connection_closes() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (invocation, _) = fx.invoke(&caller).unwrap();
    fx.open_on(RUNTIME, 5, &caller, &invocation).await.unwrap();
    fx.advance(super::inbox::LEASE_MS * 2);
    let error = fx
        .caller_call(RUNTIME, &caller, "spawn_callers.disconnect", json!({}))
        .await
        .unwrap_err();
    assert_kind(&error, ErrorKind::Conflict, "iterator_open");
    fx.connection_closed(RUNTIME, 5);
    fx.caller_call(RUNTIME, &caller, "spawn_callers.disconnect", json!({}))
        .await
        .unwrap();
}

/// A connection that opened no iterator, or one the core did not identify,
/// changes nothing when it closes.
#[tokio::test]
async fn a_connection_close_without_iterators_changes_nothing() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (invocation, _) = fx.reported(&caller, "result").await;
    let held = fx.open_on(RUNTIME, 1, &caller, &invocation).await.unwrap();
    let delivered = fx.next(RUNTIME, &caller, &held).await.unwrap().unwrap();
    let unidentified = fx.open(RUNTIME, &caller, &invocation).await.unwrap();
    let audit = fx.audit();
    fx.connection_closed(RUNTIME2, 99);
    fx.callers.connection_closed(&CallContext {
        peer_pid: Some(RUNTIME2),
        connection_id: None,
    });
    assert_eq!(fx.audit(), audit, "nothing audited");
    // Both iterators are open and the lease still holds.
    assert_eq!(
        fx.next(RUNTIME, &caller, &unidentified).await.unwrap(),
        None
    );
    assert_eq!(fx.next(RUNTIME, &caller, &held).await.unwrap(), None);
    fx.close(RUNTIME, &caller, &held).await.unwrap();
    assert_eq!(
        fx.next(RUNTIME, &caller, &unidentified).await.unwrap(),
        Some(delivered)
    );
}

/// Desktop's half: identity and every authority check complete on their
/// own call, with no Relaycast request. When Flows admits is Flows' test.
#[tokio::test]
async fn identity_and_authority_validation_complete_before_flows_admits_a_run() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let disabled = fx.provision("disabled").await;
    fx.call(
        APP,
        "spawn_callers.set_enabled",
        json!({"caller_id": disabled.id, "enabled": false}),
    )
    .await
    .unwrap();
    let before = fx.requests();
    assert!(fx.identity(RUNTIME, &caller).await.is_ok());
    let error = fx
        .identity(RUNTIME, &caller.with_secret("rsc_wrong"))
        .await
        .unwrap_err();
    assert_caller_refused(&error, RefusalReason::ServiceUnknown, "caller_unknown");
    let unknown = Caller {
        id: "sc_00000000000000ff".into(),
        name: "nobody".into(),
        ..caller.clone()
    };
    let unknown_error = fx.identity(RUNTIME, &unknown).await.unwrap_err();
    assert_eq!(
        unknown_error, error,
        "an unknown caller and a wrong secret answer alike"
    );
    let error = fx.identity(RUNTIME, &disabled).await.unwrap_err();
    assert_refused(&error, RefusalReason::ServiceDisabled, "caller_disabled");
    fx.advance(91 * super::registry::DAY_MS);
    let error = fx.identity(RUNTIME, &caller).await.unwrap_err();
    assert_refused(&error, RefusalReason::ServiceExpired, "caller_expired");
    assert_eq!(fx.requests(), before);
    assert_eq!(fx.spawns(), 0);
}

// --- Amendment A: settlement idempotency -------------------------------

/// An invocation's delivered item, with its iterator.
async fn delivered(fx: &Fx, caller: &Caller, text: &str) -> (String, String, String) {
    let (invocation, _) = fx.reported(caller, text).await;
    let iterator = fx.open(RUNTIME, caller, &invocation).await.unwrap();
    let delivery = fx.next(RUNTIME, caller, &iterator).await.unwrap().unwrap();
    (invocation, iterator, item_id(&delivery))
}

#[tokio::test]
async fn a_lost_response_duplicate_ack_returns_the_identical_receipt() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, _, item) = delivered(&fx, &caller, "result").await;
    let first = fx.ack(RUNTIME, &caller, &item).await.unwrap();
    fx.advance(60_000);
    let repeat = fx.ack(RUNTIME, &caller, &item).await.unwrap();
    assert_same_receipt(&first, &repeat);
    assert_eq!(repeat["settled_at_ms"], json!(T0));
    assert!(fx
        .audit()
        .contains(r#""method":"spawn_callers.inbox.ack","outcome":"repeat""#));
}

#[tokio::test]
async fn a_lost_response_duplicate_fail_returns_the_identical_receipt() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, _, item) = delivered(&fx, &caller, "result").await;
    let first = fx.fail(RUNTIME, &caller, &item).await.unwrap();
    fx.advance(60_000);
    let repeat = fx.fail(RUNTIME, &caller, &item).await.unwrap();
    assert_same_receipt(&first, &repeat);
    assert_eq!(repeat["state"], "failed");
}

#[tokio::test]
async fn a_conflicting_settlement_is_refused_nonretryable() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, _, acked) = delivered(&fx, &caller, "one").await;
    fx.ack(RUNTIME, &caller, &acked).await.unwrap();
    let error = fx.fail(RUNTIME, &caller, &acked).await.unwrap_err();
    assert_kind(&error, ErrorKind::Conflict, "settlement_conflict");
    assert!(!error.data.retryable);
    let (_, _, failed) = delivered(&fx, &caller, "two").await;
    fx.fail(RUNTIME, &caller, &failed).await.unwrap();
    let error = fx.ack(RUNTIME, &caller, &failed).await.unwrap_err();
    assert_kind(&error, ErrorKind::Conflict, "settlement_conflict");
    // The first settlement stands.
    assert_eq!(
        fx.ack(RUNTIME, &caller, &acked).await.unwrap()["state"],
        "acked"
    );
    assert_eq!(
        fx.fail(RUNTIME, &caller, &failed).await.unwrap()["state"],
        "failed"
    );
}

#[tokio::test]
async fn a_later_connection_settles_an_exact_item_id_without_an_iterator() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let other = fx.provision("other").await;
    let (_, iterator, item) = delivered(&fx, &caller, "result").await;
    // The first connection drops; a disconnect would release leases, never items.
    drop(iterator);
    let error = fx.ack(RUNTIME2, &other, &item).await.unwrap_err();
    assert_refused(&error, RefusalReason::NotOwner, "item_not_owned");
    fx.ack(RUNTIME2, &caller, &item).await.unwrap();
    let error = fx.ack(RUNTIME2, &caller, "item_unknown").await.unwrap_err();
    assert_kind(&error, ErrorKind::NotFound, "not_found");
}

#[tokio::test]
async fn a_restart_replays_settlement_tombstones_and_unsettled_items() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (settled_invocation, _, settled) = delivered(&fx, &caller, "settled").await;
    let first = fx.ack(RUNTIME, &caller, &settled).await.unwrap();
    let (unsettled_invocation, _, unsettled) = delivered(&fx, &caller, "unsettled").await;
    let cold = fx.restart();
    drop(fx);
    // Only the unsettled item is redelivered.
    let iterator = cold
        .open(RUNTIME, &caller, &settled_invocation)
        .await
        .unwrap();
    assert_eq!(cold.next(RUNTIME, &caller, &iterator).await.unwrap(), None);
    cold.advance(60_000);
    assert_same_receipt(&first, &cold.ack(RUNTIME, &caller, &settled).await.unwrap());
    let error = cold.fail(RUNTIME, &caller, &settled).await.unwrap_err();
    assert_kind(&error, ErrorKind::Conflict, "settlement_conflict");
    let iterator = cold
        .open(RUNTIME, &caller, &unsettled_invocation)
        .await
        .unwrap();
    let delivery = cold
        .next(RUNTIME, &caller, &iterator)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item_id(&delivery), unsettled);
    assert_eq!(delivery["text"], "unsettled");
}

#[tokio::test]
async fn a_settled_tombstone_is_not_found_after_seven_days() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, _, item) = delivered(&fx, &caller, "result").await;
    let (unsettled_invocation, _, unsettled) = delivered(&fx, &caller, "kept").await;
    let first = fx.ack(RUNTIME, &caller, &item).await.unwrap();
    fx.advance(super::inbox::TOMBSTONE_MS - 1);
    assert_same_receipt(&first, &fx.ack(RUNTIME, &caller, &item).await.unwrap());
    fx.advance(1);
    let error = fx.ack(RUNTIME, &caller, &item).await.unwrap_err();
    assert_kind(&error, ErrorKind::NotFound, "not_found");
    let error = fx.fail(RUNTIME, &caller, &item).await.unwrap_err();
    assert_kind(&error, ErrorKind::NotFound, "not_found");
    // Unsettled items never expire.
    let refreshed = fx
        .call(
            APP,
            "spawn_callers.rotate",
            json!({"caller_id": caller.id, "overlap_seconds": 0}),
        )
        .await;
    assert!(refreshed.is_ok());
    let caller = caller.with_secret(read_json(&caller.handoff)["secret"].as_str().unwrap());
    let iterator = fx
        .open(RUNTIME, &caller, &unsettled_invocation)
        .await
        .unwrap();
    let delivery = fx.next(RUNTIME, &caller, &iterator).await.unwrap().unwrap();
    assert_eq!(item_id(&delivery), unsettled);
}

/// Capacity refuses new admission; it never evicts a live tombstone, and
/// an unsettled item stays.
#[tokio::test]
async fn a_tombstone_is_retained_under_capacity_pressure() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, _, settled) = delivered(&fx, &caller, "settled").await;
    let first = fx.ack(RUNTIME, &caller, &settled).await.unwrap();
    let (unsettled_invocation, _, unsettled) = delivered(&fx, &caller, "unsettled").await;
    // Fill the caller's records to the limit with fresh tombstones.
    let path = fx
        .path("app/ServiceCallers")
        .join(format!("{}.results", super::registry::key(SITE, "ws_test")))
        .join(format!("{}.json", caller.id));
    let mut file: Value = read_json(&path);
    let slots = file["slots"].as_array_mut().unwrap();
    let template = slots
        .iter()
        .find(|slot| slot["settled"].is_object())
        .unwrap()
        .clone();
    let mut n = 0;
    while slots.len() < super::inbox::RECORDS {
        let mut slot = template.clone();
        slot["key"] = json!(format!("filler-{n}"));
        slot["invocation_id"] = json!(format!("inv_filler_{n}"));
        slot["item"]["item_id"] = json!(format!("item_filler_{n}"));
        slots.push(slot);
        n += 1;
    }
    std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
    let (requests, spawns) = (fx.requests(), fx.spawns());
    let error = fx.invoke(&caller).unwrap_err();
    assert_kind(&error, ErrorKind::Busy, "reservation_full");
    assert_eq!((fx.requests(), fx.spawns()), (requests, spawns));
    let stored = read_json(&path);
    assert_eq!(
        stored["slots"].as_array().unwrap().len(),
        super::inbox::RECORDS
    );
    // The tombstone still answers a repeat, and still refuses a conflict.
    assert_same_receipt(&first, &fx.ack(RUNTIME, &caller, &settled).await.unwrap());
    let error = fx.fail(RUNTIME, &caller, &settled).await.unwrap_err();
    assert_kind(&error, ErrorKind::Conflict, "settlement_conflict");
    // The unsettled item is still delivered, at full capacity.
    fx.advance(super::inbox::LEASE_MS);
    let iterator = fx
        .open(RUNTIME, &caller, &unsettled_invocation)
        .await
        .unwrap();
    let delivery = fx.next(RUNTIME, &caller, &iterator).await.unwrap().unwrap();
    assert_eq!(item_id(&delivery), unsettled);
}

// --- Amendment B: the lease bound --------------------------------------

#[tokio::test]
async fn an_expired_lease_is_redelivered_to_a_later_same_caller_connection() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (invocation, _, item) = delivered(&fx, &caller, "result").await;
    fx.advance(super::inbox::LEASE_MS);
    // A later connection, opened after the first went away.
    let later = fx.open(RUNTIME2, &caller, &invocation).await.unwrap();
    let delivery = fx.next(RUNTIME2, &caller, &later).await.unwrap().unwrap();
    assert_eq!(item_id(&delivery), item);
}

// --- Amendment C: no secret or token in any answer ----------------------

#[tokio::test]
async fn no_response_serializes_a_caller_secret_or_token() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let pending = fx
        .call(
            PROBE,
            "spawn_callers.provision",
            fx.provision_params("probe-made"),
        )
        .await
        .unwrap();
    let pending_id = pending["caller"]["caller_id"].as_str().unwrap().to_string();
    fx.call(
        APP,
        "spawn_callers.decide",
        json!({"caller_id": pending_id, "approved": true}),
    )
    .await
    .unwrap();
    fx.call(APP, "spawn_callers.rotate", json!({"caller_id": caller.id}))
        .await
        .unwrap();
    fx.call(
        PROBE,
        "spawn_callers.rotate",
        json!({"caller_id": caller.id}),
    )
    .await
    .unwrap();
    fx.call(
        APP,
        "spawn_callers.decide",
        json!({"caller_id": caller.id, "approved": true}),
    )
    .await
    .unwrap();
    fx.call(APP, "spawn_callers.list", json!({})).await.unwrap();
    let current = caller.with_secret(read_json(&caller.handoff)["secret"].as_str().unwrap());
    let (_, _, item) = delivered(&fx, &current, "result").await;
    fx.identity(RUNTIME, &current).await.unwrap();
    fx.ack(RUNTIME, &current, &item).await.unwrap();
    let probe_made = read_json(
        &fx.path("app/ServiceCallers/handoff")
            .join(format!("{pending_id}.json")),
    );
    let seen = fx.seen.lock().unwrap().join("\n");
    let tokens: Vec<String> = fx
        .world
        .relaycast
        .lock()
        .unwrap()
        .tokens
        .values()
        .cloned()
        .collect();
    for secret in [
        caller.secret.as_str(),
        current.secret.as_str(),
        probe_made["secret"].as_str().unwrap(),
    ] {
        assert!(!seen.contains(secret));
        assert!(!seen.contains(&super::registry::digest(secret)));
    }
    for token in tokens {
        assert!(!seen.contains(&token));
    }
    assert!(!seen.contains("rsc_"));
}

// --- Amendment D: the error wire ----------------------------------------

#[tokio::test]
async fn a_settlement_conflict_is_kind_conflict_and_not_retryable() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, _, item) = delivered(&fx, &caller, "result").await;
    fx.fail(RUNTIME, &caller, &item).await.unwrap();
    let error = fx.ack(RUNTIME, &caller, &item).await.unwrap_err();
    let wire = serde_json::to_value(&error).unwrap();
    assert_eq!(
        wire,
        json!({"code": -32000, "message": "settlement_conflict",
               "data": {"kind": "conflict", "retryable": false}})
    );
}

#[tokio::test]
async fn an_expired_tombstone_replay_is_kind_not_found() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, _, item) = delivered(&fx, &caller, "result").await;
    fx.fail(RUNTIME, &caller, &item).await.unwrap();
    fx.advance(super::inbox::TOMBSTONE_MS);
    let error = fx.fail(RUNTIME, &caller, &item).await.unwrap_err();
    let wire = serde_json::to_value(&error).unwrap();
    assert_eq!(wire["code"], -32000);
    assert_eq!(wire["message"], "not_found");
    assert_eq!(wire["data"]["kind"], "not_found");
}

#[tokio::test]
async fn another_callers_item_is_refused_not_owner() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let other = fx.provision("other").await;
    let (_, _, item) = delivered(&fx, &caller, "result").await;
    let error = fx.fail(RUNTIME, &other, &item).await.unwrap_err();
    let wire = serde_json::to_value(&error).unwrap();
    assert_eq!(wire["code"], -32000);
    assert_eq!(wire["message"], "item_not_owned");
    assert_eq!(wire["data"]["kind"], "refused");
    assert_eq!(wire["data"]["reason"], "not_owner");
    assert_eq!(wire["data"]["retryable"], false);
    // Still the owner's to settle.
    fx.ack(RUNTIME, &caller, &item).await.unwrap();
}

#[tokio::test]
async fn a_terminal_failure_delivery_carries_code_bytes_limit_and_no_text() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let size = super::inbox::SLOT_BYTES as usize + 7;
    let (invocation, _) = fx.reported(&caller, &"z".repeat(size)).await;
    let iterator = fx.open(RUNTIME, &caller, &invocation).await.unwrap();
    let delivery = fx.next(RUNTIME, &caller, &iterator).await.unwrap().unwrap();
    assert_eq!(
        delivery["failure"],
        json!({"code": "result_too_large", "bytes": size, "limit": 524288})
    );
    let keys: Vec<&str> = delivery
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert!(!keys.contains(&"text"), "{keys:?}");
    for key in [
        "item_id",
        "message_id",
        "sender",
        "invocation_id",
        "sent_at",
    ] {
        assert!(keys.contains(&key), "{key}");
    }
}

// --- Amendment E: the pending surface -----------------------------------

#[tokio::test]
async fn a_probe_provision_is_pending_and_writes_nothing_until_approved() {
    let fx = Fx::new();
    let pending = fx
        .call(
            PROBE,
            "spawn_callers.provision",
            fx.provision_params("flows"),
        )
        .await
        .unwrap();
    let caller = &pending["caller"];
    assert_eq!(caller["state"], "pending_provision");
    assert_eq!(caller["requested_by"], "probe");
    assert_eq!(caller["enabled"], false);
    assert_eq!(caller["secrets_live"], 0);
    assert_eq!(caller["publisher"]["id"], "");
    assert!(caller["requested_at"].is_string());
    assert_eq!(pending.get("handoff_path"), None);
    assert!(!fx
        .handoff_of(caller["caller_id"].as_str().unwrap())
        .exists());
    assert_eq!(fx.requests(), 0, "no Relaycast registration");
    // Durable: the record is on disk, with no secret and no token.
    let registry = std::fs::read_to_string(fx.registry_file()).unwrap();
    assert!(registry.contains(r#""kind": "provision""#));
    assert!(registry.contains(r#""secrets": []"#));
    assert!(!registry.contains(r#""token""#));
}

#[tokio::test]
async fn approving_a_pending_provision_writes_caller_json_and_enables_it() {
    let fx = Fx::new();
    let pending = fx
        .call(
            PROBE,
            "spawn_callers.provision",
            fx.provision_params("flows"),
        )
        .await
        .unwrap();
    let caller_id = pending["caller"]["caller_id"].as_str().unwrap().to_string();
    let decided = fx
        .call(
            APP,
            "spawn_callers.decide",
            json!({"caller_id": caller_id, "approved": true}),
        )
        .await
        .unwrap();
    assert_eq!(decided["caller"]["state"], "enabled");
    assert_eq!(decided["caller"]["secrets_live"], 1);
    assert_eq!(fx.requests(), 1, "one registration");
    let handoff = PathBuf::from(decided["handoff_path"].as_str().unwrap());
    assert_eq!(handoff, fx.handoff_of(&caller_id));
    let file = read_json(&handoff);
    assert_eq!(file["caller_id"], caller_id.as_str());
    assert_eq!(
        file["publisher"]["id"],
        decided["caller"]["publisher"]["id"]
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&handoff).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let caller = Caller {
        id: caller_id,
        name: "flows".into(),
        secret: file["secret"].as_str().unwrap().into(),
        agent_id: file["publisher"]["id"].as_str().unwrap().into(),
        handoff,
    };
    assert!(fx.identity(RUNTIME, &caller).await.is_ok());
    assert!(fx.invoke(&caller).is_ok());
}

#[tokio::test]
async fn rejecting_a_pending_provision_deletes_it() {
    let fx = Fx::new();
    let pending = fx
        .call(
            PROBE,
            "spawn_callers.provision",
            fx.provision_params("flows"),
        )
        .await
        .unwrap();
    let caller_id = pending["caller"]["caller_id"].as_str().unwrap().to_string();
    fx.call(
        APP,
        "spawn_callers.decide",
        json!({"caller_id": caller_id, "approved": false}),
    )
    .await
    .unwrap();
    let list = fx.call(APP, "spawn_callers.list", json!({})).await.unwrap();
    assert_eq!(list["callers"], json!([]));
    assert_eq!(fx.requests(), 0);
    assert!(!fx.handoff_of(&caller_id).exists());
    let error = fx
        .call(
            APP,
            "spawn_callers.decide",
            json!({"caller_id": caller_id, "approved": true}),
        )
        .await
        .unwrap_err();
    assert_kind(&error, ErrorKind::NotFound, "not_found");
}

#[tokio::test]
async fn a_pending_request_survives_restart() {
    let fx = Fx::new();
    let pending = fx
        .call(
            PROBE,
            "spawn_callers.provision",
            fx.provision_params("flows"),
        )
        .await
        .unwrap();
    let caller_id = pending["caller"]["caller_id"].as_str().unwrap().to_string();
    let rotating = fx.provision("rotating").await;
    fx.call(
        PROBE,
        "spawn_callers.rotate",
        json!({"caller_id": rotating.id}),
    )
    .await
    .unwrap();
    let cold = fx.restart();
    drop(fx);
    let list = cold
        .call(APP, "spawn_callers.list", json!({}))
        .await
        .unwrap();
    let states: HashMap<String, String> = list["callers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap().into(),
                c["state"].as_str().unwrap().into(),
            )
        })
        .collect();
    assert_eq!(states["flows"], "pending_provision");
    assert_eq!(states["rotating"], "pending_rotation");
    let decided = cold
        .call(
            APP,
            "spawn_callers.decide",
            json!({"caller_id": caller_id, "approved": true}),
        )
        .await
        .unwrap();
    assert_eq!(decided["caller"]["state"], "enabled");
    let decided = cold
        .call(
            APP,
            "spawn_callers.decide",
            json!({"caller_id": rotating.id, "approved": true}),
        )
        .await
        .unwrap();
    assert_eq!(decided["caller"]["state"], "enabled");
    assert_ne!(
        read_json(&rotating.handoff)["secret"],
        rotating.secret.as_str()
    );
}

#[tokio::test]
async fn a_probe_rotate_keeps_the_old_secret_live_until_approved() {
    let fx = Fx::new();
    let old = fx.provision("flows").await;
    let rotated = fx
        .call(
            PROBE,
            "spawn_callers.rotate",
            json!({"caller_id": old.id, "overlap_seconds": 30}),
        )
        .await
        .unwrap();
    assert_eq!(rotated["old_secret_retires_at"], Value::Null);
    assert_eq!(rotated["caller"]["state"], "pending_rotation");
    assert_eq!(rotated["caller"]["requested_by"], "probe");
    assert_eq!(
        read_json(&old.handoff)["secret"],
        old.secret.as_str(),
        "caller.json untouched"
    );
    fx.advance(3_600_000);
    assert!(
        fx.identity(RUNTIME, &old).await.is_ok(),
        "the old secret keeps working"
    );
    fx.call(
        APP,
        "spawn_callers.decide",
        json!({"caller_id": old.id, "approved": true}),
    )
    .await
    .unwrap();
    let new = old.with_secret(read_json(&old.handoff)["secret"].as_str().unwrap());
    assert_ne!(new.secret, old.secret);
    assert!(fx.identity(RUNTIME, &new).await.is_ok());
    assert!(
        fx.identity(RUNTIME, &old).await.is_ok(),
        "within the requested overlap"
    );
    fx.advance(30_000);
    assert!(fx.identity(RUNTIME, &old).await.is_err());
    // A rejected rotation leaves the current secret alone.
    fx.call(PROBE, "spawn_callers.rotate", json!({"caller_id": old.id}))
        .await
        .unwrap();
    fx.call(
        APP,
        "spawn_callers.decide",
        json!({"caller_id": old.id, "approved": false}),
    )
    .await
    .unwrap();
    assert_eq!(read_json(&old.handoff)["secret"], new.secret.as_str());
    assert!(fx.identity(RUNTIME, &new).await.is_ok());
}

#[tokio::test]
async fn decide_on_a_non_pending_caller_is_kind_conflict_reason_not_pending() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    for approved in [true, false] {
        let error = fx
            .call(
                APP,
                "spawn_callers.decide",
                json!({"caller_id": caller.id, "approved": approved}),
            )
            .await
            .unwrap_err();
        assert_kind(&error, ErrorKind::Conflict, "not_pending");
        assert!(!error.data.retryable);
        // The core's closed RefusalReason has no not_pending; the class is
        // the contract's typed CallerErrorClass, carried in the message.
        assert_eq!(error.data.reason, None);
        let class: CallerErrorClass = serde_json::from_value(json!(error.message)).unwrap();
        assert_eq!(class, CallerErrorClass::NotPending);
        let wire = serde_json::to_value(&error).unwrap();
        assert_eq!(wire["code"], -32000);
        assert_eq!(wire["data"]["kind"], "conflict");
    }
    assert!(fx.identity(RUNTIME, &caller).await.is_ok());
}

// --- Amendment F: probe disclosure, attestation, decide -----------------

/// The registry's record of `caller_id`, as stored.
fn record_of(fx: &Fx, caller_id: &str) -> Option<Value> {
    let registry = read_json(&fx.registry_file());
    registry["callers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|caller| caller["caller_id"] == caller_id)
        .cloned()
}

/// The ledger entry of `invocation_id` in `caller_id`'s stored record.
fn ledger_entry_of(fx: &Fx, caller_id: &str, invocation_id: &str) -> Value {
    record_of(fx, caller_id).unwrap()["invocations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["invocation_id"] == invocation_id)
        .cloned()
        .unwrap()
}

#[tokio::test]
async fn a_probe_provision_discloses_no_path_secret_or_fd() {
    let fx = Fx::new();
    let pending = fx
        .call(
            PROBE,
            "spawn_callers.provision",
            fx.provision_params("flows"),
        )
        .await
        .unwrap();
    let keys: Vec<&String> = pending.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["caller"], "only the pending caller");
    let caller_id = pending["caller"]["caller_id"].as_str().unwrap().to_string();
    assert_eq!(pending["caller"]["state"], "pending_provision");
    let text = pending.to_string();
    for word in [
        "handoff",
        "rsc_",
        "\"secret\"",
        "token",
        "\"fd\"",
        "ServiceCallers",
    ] {
        assert!(!text.contains(word), "{word} in {text}");
    }
    // Only the pending record is written: no place, no secret, no token.
    let record = record_of(&fx, &caller_id).unwrap();
    assert_eq!(record.get("handoff_path"), None);
    assert_eq!(record["secrets"], json!([]));
    assert_eq!(record.get("token"), None);
    assert!(!fx.handoff_of(&caller_id).exists());
    assert!(
        !fx.path("app/ServiceCallers/handoff").exists()
            || std::fs::read_dir(fx.path("app/ServiceCallers/handoff"))
                .unwrap()
                .next()
                .is_none()
    );
    assert_eq!(fx.requests(), 0);
    // Nor will it take a place for the secret.
    let mut params = fx.provision_params("flows-2");
    params["handoff_path"] = json!(fx.path("work/project/caller.json"));
    let error = fx
        .call(PROBE, "spawn_callers.provision", params)
        .await
        .unwrap_err();
    assert_refused(
        &error,
        RefusalReason::ManagementPeer,
        "app_attestation_unavailable",
    );
    assert!(!fx.path("work/project/caller.json").exists());
    // A probe rotation discloses nothing either.
    let enabled = fx.provision("enabled").await;
    let rotated = fx
        .call(
            PROBE,
            "spawn_callers.rotate",
            json!({"caller_id": enabled.id}),
        )
        .await
        .unwrap();
    assert_eq!(rotated.get("handoff_path"), None);
    assert!(!rotated.to_string().contains("rsc_"));
    let error = fx
        .call(
            PROBE,
            "spawn_callers.rotate",
            json!({"caller_id": enabled.id, "handoff_path": fx.path("work/project/caller.json")}),
        )
        .await
        .unwrap_err();
    assert_refused(
        &error,
        RefusalReason::ManagementPeer,
        "app_attestation_unavailable",
    );
}

#[tokio::test]
async fn a_forged_app_peer_cannot_decide() {
    let fx = Fx::new();
    let pending = fx
        .call(
            PROBE,
            "spawn_callers.provision",
            fx.provision_params("flows"),
        )
        .await
        .unwrap();
    let caller_id = pending["caller"]["caller_id"].as_str().unwrap().to_string();
    let enabled = fx.provision("enabled").await;
    // Same user, the app's executable name and path, no attestation.
    for (method, params) in [
        (
            "spawn_callers.decide",
            json!({"caller_id": caller_id, "approved": true}),
        ),
        (
            "spawn_callers.decide",
            json!({"caller_id": caller_id, "approved": false}),
        ),
        (
            "spawn_callers.set_enabled",
            json!({"caller_id": enabled.id, "enabled": false}),
        ),
        ("spawn_callers.revoke", json!({"caller_id": enabled.id})),
    ] {
        let error = fx.call(FORGED, method, params).await.unwrap_err();
        assert_refused(
            &error,
            RefusalReason::ManagementPeer,
            "app_attestation_unavailable",
        );
    }
    // What it can do is what the probe can: ask.
    let asked = fx
        .call(
            FORGED,
            "spawn_callers.provision",
            fx.provision_params("forged"),
        )
        .await
        .unwrap();
    assert_eq!(asked["caller"]["state"], "pending_provision");
    assert_eq!(asked["caller"]["requested_by"], "probe");
    assert_eq!(asked.get("handoff_path"), None);
    assert_eq!(
        record_of(&fx, &caller_id).unwrap()["pending"]["kind"],
        "provision"
    );
    assert!(!fx.handoff_of(&caller_id).exists());
    assert!(fx.identity(RUNTIME, &enabled).await.is_ok());
    assert!(fx.audit().contains("app_attestation_unavailable"));
}

#[tokio::test]
async fn app_only_mutations_report_unavailable_without_attestation() {
    let fx = Fx::new();
    let enabled = fx.provision("enabled").await;
    let pending = fx
        .call(
            PROBE,
            "spawn_callers.provision",
            fx.provision_params("flows"),
        )
        .await
        .unwrap();
    let pending_id = pending["caller"]["caller_id"].as_str().unwrap().to_string();
    // The real host: the app cannot be told apart from any same-user process.
    fx.world.attests.store(false, Ordering::SeqCst);
    let before = fx.requests();
    let mut immediate = fx.provision_params("immediate");
    immediate["handoff_path"] = json!(fx.path("work/project/caller.json"));
    for (method, params) in [
        (
            "spawn_callers.decide",
            json!({"caller_id": pending_id, "approved": true}),
        ),
        ("spawn_callers.provision", immediate),
        (
            "spawn_callers.rotate",
            json!({"caller_id": enabled.id, "handoff_path": fx.path("work/project/caller.json")}),
        ),
        (
            "spawn_callers.set_enabled",
            json!({"caller_id": enabled.id, "enabled": false}),
        ),
        ("spawn_callers.revoke", json!({"caller_id": enabled.id})),
    ] {
        let error = fx.call(APP, method, params).await.unwrap_err();
        assert_refused(
            &error,
            RefusalReason::ManagementPeer,
            "app_attestation_unavailable",
        );
        let class: CallerErrorClass = serde_json::from_value(json!(error.message)).unwrap();
        assert_eq!(class, CallerErrorClass::AppAttestationUnavailable);
    }
    assert_eq!(fx.requests(), before, "nothing reached Relaycast");
    // list stays readable, and asking still works.
    let list = fx.call(APP, "spawn_callers.list", json!({})).await.unwrap();
    assert_eq!(list["callers"].as_array().unwrap().len(), 2);
    let asked = fx
        .call(APP, "spawn_callers.provision", fx.provision_params("asked"))
        .await
        .unwrap();
    assert_eq!(asked["caller"]["state"], "pending_provision");
    let rotated = fx
        .call(
            APP,
            "spawn_callers.rotate",
            json!({"caller_id": enabled.id}),
        )
        .await
        .unwrap();
    assert_eq!(rotated["caller"]["state"], "pending_rotation");
    assert_eq!(
        read_json(&enabled.handoff)["secret"],
        enabled.secret.as_str()
    );
    assert!(fx.identity(RUNTIME, &enabled).await.is_ok());
    assert!(!fx.handoff_of(&pending_id).exists());
}

#[tokio::test]
async fn approving_a_rotation_commits_atomically_with_the_old_secret_live_until_commit() {
    let fx = Fx::new();
    let old = fx.provision("flows").await;
    fx.call(PROBE, "spawn_callers.rotate", json!({"caller_id": old.id}))
        .await
        .unwrap();
    let before = record_of(&fx, &old.id).unwrap();
    assert_eq!(before["secrets"].as_array().unwrap().len(), 1);
    // caller.json cannot be replaced: a non-empty folder stands in its place.
    std::fs::remove_file(&old.handoff).unwrap();
    std::fs::create_dir_all(old.handoff.join("blocker")).unwrap();
    let error = fx
        .call(
            APP,
            "spawn_callers.decide",
            json!({"caller_id": old.id, "approved": true}),
        )
        .await
        .unwrap_err();
    assert_kind(&error, ErrorKind::StoreFailed, "store_failed");
    // Nothing committed: the old secret is the live one, still pending.
    assert_eq!(record_of(&fx, &old.id).unwrap(), before);
    assert!(fx.identity(RUNTIME, &old).await.is_ok());
    let list = fx.call(APP, "spawn_callers.list", json!({})).await.unwrap();
    assert_eq!(list["callers"][0]["state"], "pending_rotation");
    // A cold core agrees.
    let cold = fx.restart();
    assert!(cold.identity(RUNTIME, &old).await.is_ok());
    // Once the place is writable, the approval commits as one.
    std::fs::remove_dir_all(&old.handoff).unwrap();
    let decided = cold
        .call(
            APP,
            "spawn_callers.decide",
            json!({"caller_id": old.id, "approved": true}),
        )
        .await
        .unwrap();
    assert_eq!(decided["caller"]["state"], "enabled");
    assert_eq!(decided["handoff_path"], old.handoff.display().to_string());
    let new = old.with_secret(read_json(&old.handoff)["secret"].as_str().unwrap());
    assert_ne!(new.secret, old.secret);
    assert!(cold.identity(RUNTIME, &new).await.is_ok());
    let after = record_of(&cold, &old.id).unwrap();
    assert_eq!(after.get("pending"), None);
    assert_eq!(
        after["secrets"].as_array().unwrap().len(),
        2,
        "old one within its overlap"
    );
}

#[tokio::test]
async fn rejecting_leaves_no_credential_or_registration_artifacts() {
    let fx = Fx::new();
    let pending = fx
        .call(
            PROBE,
            "spawn_callers.provision",
            fx.provision_params("flows"),
        )
        .await
        .unwrap();
    let caller_id = pending["caller"]["caller_id"].as_str().unwrap().to_string();
    let rejected = fx
        .call(
            APP,
            "spawn_callers.decide",
            json!({"caller_id": caller_id, "approved": false}),
        )
        .await
        .unwrap();
    assert_eq!(rejected.get("handoff_path"), None);
    assert_eq!(record_of(&fx, &caller_id), None);
    assert!(!std::fs::read_to_string(fx.registry_file())
        .unwrap()
        .contains(&caller_id));
    assert!(!fx.handoff_of(&caller_id).exists());
    let results = fx
        .path("app/ServiceCallers")
        .join(format!("{}.results", super::registry::key(SITE, "ws_test")))
        .join(format!("{caller_id}.json"));
    assert!(!results.exists());
    {
        let relaycast = fx.world.relaycast.lock().unwrap();
        assert_eq!(relaycast.requests, 0, "no registration and no delete");
        assert!(relaycast.tokens.is_empty());
        assert!(relaycast.deleted.is_empty());
    }
    assert!(fx.world.spawned.lock().unwrap().is_empty());
    // A rejected rotation mints nothing: the one secret stays the only one.
    let enabled = fx.provision("enabled").await;
    let requests = fx.requests();
    fx.call(
        PROBE,
        "spawn_callers.rotate",
        json!({"caller_id": enabled.id}),
    )
    .await
    .unwrap();
    fx.call(
        APP,
        "spawn_callers.decide",
        json!({"caller_id": enabled.id, "approved": false}),
    )
    .await
    .unwrap();
    let record = record_of(&fx, &enabled.id).unwrap();
    assert_eq!(record["secrets"].as_array().unwrap().len(), 1);
    assert_eq!(record.get("pending"), None);
    assert_eq!(
        read_json(&enabled.handoff)["secret"],
        enabled.secret.as_str()
    );
    assert_eq!(fx.requests(), requests);
    let cold = fx.restart();
    let list = cold
        .call(APP, "spawn_callers.list", json!({}))
        .await
        .unwrap();
    let names: Vec<&str> = list["callers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|caller| caller["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["enabled"]);
}

#[test]
fn every_error_class_is_a_typed_contract_class() {
    use Class::*;
    let every = [
        CallerUnknown,
        SecretMismatch,
        CallerDisabled,
        CallerExpired,
        SecretRetired,
        AccountMismatch,
        WorkspaceMismatch,
        GenerationMoved,
        NotInLedger,
        RequesterOther,
        ItemNotOwned,
        HarnessOutside,
        DirectoryOutside,
        PeerUnreadable,
        PeerUidMismatch,
        PeerInSessionTree,
        SecretOnManagement,
        PeerCheckUnavailable,
        AppAttestationUnavailable,
        NotSignedIn,
        RegistryUnreadable,
        StoreFailed,
        NodeUnavailable,
        NodeOther,
        RateLimited,
        LiveAgentCap,
        ReservationFull,
        SettlementConflict,
        SettlementInvalid,
        NotFound,
        IteratorOpen,
        NameTaken,
        NotPending,
        InvalidInput,
        AuthFailed,
        CallerUnbound,
        BindingStale,
        ConnectionClosed,
        AlreadyBound,
        ChallengeOutstanding,
        ChallengeCapacity,
        AuthLocked,
        BindingUnavailable,
        LegacyCredentialRefused,
        ExternalConnection,
        RotationRollbackPending,
        ChallengeExpired,
        ChallengeConsumed,
    ];
    for class in every {
        let error = class.error();
        let typed: CallerErrorClass = serde_json::from_value(json!(error.message)).unwrap();
        assert_ne!(typed, CallerErrorClass::Other, "{}", error.message);
        assert!(matches!(error.code, -32000 | -32602), "{error:?}");
    }
}

// --- Management peers ----------------------------------------------------

#[tokio::test]
async fn management_refuses_a_secret_and_a_session_peer() {
    let fx = Fx::new();
    let error = fx
        .call(
            SESSION,
            "spawn_callers.provision",
            fx.provision_params("flows"),
        )
        .await
        .unwrap_err();
    assert_refused(
        &error,
        RefusalReason::ManagementPeer,
        "peer_in_session_tree",
    );
    let mut params = fx.provision_params("flows");
    params["caller_secret"] = json!("rsc_anything");
    let error = fx
        .call(APP, "spawn_callers.provision", params)
        .await
        .unwrap_err();
    if cfg!(feature = "service-callers-dev-attest") {
        assert_refused(
            &error,
            RefusalReason::ManagementPeer,
            "secret_on_management",
        );
    } else {
        assert_kind(&error, ErrorKind::Refused, "legacy_credential_refused");
        assert!(!error.data.retryable);
    }
    let error = fx
        .call(
            APP,
            "spawn_callers.provision",
            fx.provision_params("taken-by-a-session"),
        )
        .await
        .unwrap_err();
    assert_kind(&error, ErrorKind::Conflict, "name_taken");
    assert_eq!(fx.requests(), 0);
}

/// Without the preview feature the real core attests no peer, so a probe
/// cannot choose a place for a caller's secret.
#[cfg(not(feature = "service-callers-dev-attest"))]
#[tokio::test]
async fn without_dev_attest_the_core_attests_nothing_and_refuses_a_handoff_provision() {
    use super::host::CoreHost;
    let host = CoreHost::default();
    assert_eq!(
        host.attest_app_peer(std::process::id() as i32),
        Attestation::Unavailable
    );
    assert_eq!(host.attest_app_peer(1), Attestation::Unavailable);
    let fx = Fx::new();
    fx.world.core_attestation.store(true, Ordering::SeqCst);
    let mut params = fx.provision_params("flows");
    params["handoff_path"] = json!(fx.path("work/project/caller.json"));
    for pid in [APP, PROBE] {
        let error = fx
            .call(pid, "spawn_callers.provision", params.clone())
            .await
            .unwrap_err();
        assert_refused(
            &error,
            RefusalReason::ManagementPeer,
            "app_attestation_unavailable",
        );
    }
    assert!(!fx.path("work/project/caller.json").exists());
    assert_eq!(fx.requests(), 0);
}

/// The preview feature's rule, without real processes.
#[cfg(feature = "service-callers-dev-attest")]
#[test]
fn dev_attest_takes_only_the_same_uid_for_the_app() {
    use super::host::same_uid;
    assert_eq!(same_uid(Some(UID), UID), Attestation::App);
    assert_eq!(same_uid(Some(UID + 1), UID), Attestation::NotApp);
    assert_eq!(same_uid(Some(0), UID), Attestation::NotApp);
    assert_eq!(same_uid(None, UID), Attestation::NotApp);
}

#[cfg(all(feature = "service-callers-dev-attest", target_os = "linux"))]
#[test]
fn dev_attest_reads_the_uid_line_of_proc_status() {
    use super::host::status_uid;
    let status = "Name:\tx\nUid:\t501\t501\t501\t501\nGid:\t20\t20\t20\t20\n";
    assert_eq!(status_uid(status), Some(501));
    // A setuid process is not this user's.
    assert_eq!(status_uid("Uid:\t501\t0\t0\t0\n"), None);
    assert_eq!(status_uid("Uid:\t0\t501\t501\t501\n"), None);
    assert_eq!(status_uid("Name:\tx\n"), None);
    assert_eq!(status_uid("Uid:\tnope\n"), None);
}

/// The real core host over real processes: this test process is this uid;
/// a missing process, or another user's, is not the app.
#[cfg(all(
    feature = "service-callers-dev-attest",
    any(target_os = "linux", target_os = "macos")
))]
#[test]
fn dev_attest_the_core_host_attests_a_same_uid_process() {
    use super::host::CoreHost;
    let host = CoreHost::default();
    assert_eq!(
        host.attest_app_peer(std::process::id() as i32),
        Attestation::App
    );
    assert_eq!(host.attest_app_peer(i32::MAX), Attestation::NotApp);
    assert_eq!(host.attest_app_peer(-1), Attestation::NotApp);
    if host.current_uid() != 0 {
        // launchd or init runs as root.
        assert_eq!(host.attest_app_peer(1), Attestation::NotApp);
    }
}

#[cfg(feature = "service-callers-dev-attest")]
#[tokio::test]
async fn dev_attest_a_same_uid_probe_provisions_into_a_0600_handoff_file() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fx::new();
    fx.world.core_attestation.store(true, Ordering::SeqCst);
    let folder = fx.path("work/project");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
    let handoff = folder.join("caller.json");
    let mut params = fx.provision_params("flows");
    params["handoff_path"] = json!(handoff);
    let provisioned = fx
        .call(PROBE, "spawn_callers.provision", params.clone())
        .await
        .unwrap();
    assert_eq!(provisioned["caller"]["state"], "enabled");
    assert_eq!(provisioned["caller"]["requested_by"], "app");
    assert_eq!(
        PathBuf::from(provisioned["handoff_path"].as_str().unwrap()),
        handoff
    );
    assert_eq!(
        std::fs::metadata(&handoff).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let file = read_json(&handoff);
    let caller = Caller {
        id: provisioned["caller"]["caller_id"]
            .as_str()
            .unwrap()
            .to_string(),
        name: "flows".into(),
        secret: file["secret"].as_str().unwrap().into(),
        agent_id: file["publisher"]["id"].as_str().unwrap().into(),
        handoff: handoff.clone(),
    };
    assert_eq!(fx.requests(), 1, "one registration");
    assert!(fx.identity(RUNTIME, &caller).await.is_ok());
    // Another user's process is refused before attestation is asked.
    params["name"] = json!("stranger");
    let error = fx
        .call(STRANGER, "spawn_callers.provision", params)
        .await
        .unwrap_err();
    assert_refused(&error, RefusalReason::ManagementPeer, "peer_uid_mismatch");
}

#[cfg(feature = "service-callers-dev-attest")]
#[tokio::test]
async fn dev_attest_a_same_uid_peer_approves_a_pending_provision() {
    let fx = Fx::new();
    // Asked for while the peer was not attested, as Settings would see it.
    let pending = fx
        .call(
            PROBE,
            "spawn_callers.provision",
            fx.provision_params("flows"),
        )
        .await
        .unwrap();
    assert_eq!(pending["caller"]["state"], "pending_provision");
    let caller_id = pending["caller"]["caller_id"].as_str().unwrap().to_string();
    fx.world.core_attestation.store(true, Ordering::SeqCst);
    let decided = fx
        .call(
            PROBE,
            "spawn_callers.decide",
            json!({"caller_id": caller_id, "approved": true}),
        )
        .await
        .unwrap();
    assert_eq!(decided["caller"]["state"], "enabled");
    let handoff = PathBuf::from(decided["handoff_path"].as_str().unwrap());
    assert_eq!(handoff, fx.handoff_of(&caller_id));
    assert!(handoff.exists());
}

// --- Settlement receipts -------------------------------------------------

/// An invocation's delivered item, as the delivery itself.
async fn delivered_full(fx: &Fx, caller: &Caller, text: &str) -> Value {
    let (invocation, _) = fx.reported(caller, text).await;
    let iterator = fx.open(RUNTIME, caller, &invocation).await.unwrap();
    fx.next(RUNTIME, caller, &iterator).await.unwrap().unwrap()
}

#[track_caller]
fn assert_receipt(receipt: &Value, delivery: &Value, action: &str, state: &str, at_ms: i64) {
    let keys: Vec<&str> = receipt
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["action", "item_id", "message_id", "settled_at_ms", "state"]
    );
    assert_eq!(receipt["item_id"], delivery["item_id"]);
    assert_eq!(receipt["message_id"], delivery["message_id"]);
    assert!(receipt["message_id"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));
    assert_eq!(receipt["action"], action);
    assert_eq!(receipt["state"], state);
    assert_eq!(receipt["settled_at_ms"], json!(at_ms));
}

/// The settled slot of `item` in `caller`'s inbox file on disk.
fn settled_on_disk(fx: &Fx, caller: &Caller, item: &str) -> Value {
    let path = fx
        .path("app/ServiceCallers")
        .join(format!("{}.results", super::registry::key(SITE, "ws_test")))
        .join(format!("{}.json", caller.id));
    let file = read_json(&path);
    file["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["item"]["item_id"] == item)
        .map(|slot| slot["settled"].clone())
        .unwrap_or(Value::Null)
}

#[tokio::test]
async fn first_ack_returns_the_acked_receipt() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let delivery = delivered_full(&fx, &caller, "result").await;
    fx.advance(1_234);
    let receipt = fx.ack(RUNTIME, &caller, &item_id(&delivery)).await.unwrap();
    assert_receipt(&receipt, &delivery, "ack", "acked", T0 + 1_234);
}

#[tokio::test]
async fn first_fail_returns_the_failed_receipt() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let delivery = delivered_full(&fx, &caller, "result").await;
    fx.advance(4_321);
    let receipt = fx
        .fail(RUNTIME, &caller, &item_id(&delivery))
        .await
        .unwrap();
    assert_receipt(&receipt, &delivery, "fail", "failed", T0 + 4_321);
}

#[tokio::test]
async fn a_later_bound_connection_of_the_same_caller_replays_the_receipt() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (invocation, _) = fx.reported(&caller, "result").await;
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
    let iterator = fx.bound_open(RUNTIME, 1, &invocation).await.unwrap();
    let delivery = fx
        .bound_next(RUNTIME, 1, &iterator, 0)
        .await
        .unwrap()
        .unwrap();
    let item = item_id(&delivery);
    let first = fx
        .bound_settle(RUNTIME, 1, "ack", &item, "journaled")
        .await
        .unwrap();
    assert_receipt(&first, &delivery, "ack", "acked", T0);
    // The response is lost with the connection; the runtime reconnects.
    fx.connection_closed(RUNTIME, 1);
    fx.advance(5_000);
    fx.bind(RUNTIME2, 2, &caller).await.unwrap();
    let repeat = fx
        .bound_settle(RUNTIME2, 2, "ack", &item, "journaled")
        .await
        .unwrap();
    assert_same_receipt(&first, &repeat);
    // The per-request path of the same caller replays it too.
    assert_same_receipt(&first, &fx.ack(RUNTIME, &caller, &item).await.unwrap());
}

#[tokio::test]
async fn a_metadata_mismatch_on_a_settled_item_is_a_settlement_conflict() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, _, item) = delivered(&fx, &caller, "result").await;
    let first = fx.ack(RUNTIME, &caller, &item).await.unwrap();
    let before = settled_on_disk(&fx, &caller, &item);
    let error = fx
        .caller_call(
            RUNTIME,
            &caller,
            "spawn_callers.inbox.ack",
            json!({"item_id": item, "metadata": {"relayflows_result": "other"}}),
        )
        .await
        .unwrap_err();
    assert_kind(&error, ErrorKind::Conflict, "settlement_conflict");
    assert!(!error.data.retryable);
    assert!(error.data.reason.is_none());
    assert_eq!(settled_on_disk(&fx, &caller, &item), before);
    assert_same_receipt(&first, &fx.ack(RUNTIME, &caller, &item).await.unwrap());
}

#[tokio::test]
async fn another_callers_settlement_is_item_not_owned_and_changes_nothing() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let other = fx.provision("other").await;
    let (_, _, item) = delivered(&fx, &caller, "result").await;
    for error in [
        fx.ack(RUNTIME2, &other, &item).await.unwrap_err(),
        fx.fail(RUNTIME2, &other, &item).await.unwrap_err(),
    ] {
        assert_refused(&error, RefusalReason::NotOwner, "item_not_owned");
    }
    assert_eq!(settled_on_disk(&fx, &caller, &item), Value::Null);
    let first = fx.fail(RUNTIME, &caller, &item).await.unwrap();
    assert_eq!(first["state"], "failed");
    let error = fx.fail(RUNTIME2, &other, &item).await.unwrap_err();
    assert_refused(&error, RefusalReason::NotOwner, "item_not_owned");
    assert_same_receipt(&first, &fx.fail(RUNTIME, &caller, &item).await.unwrap());
}

#[tokio::test]
async fn an_unknown_item_is_not_found() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    for error in [
        fx.ack(RUNTIME, &caller, "item_unknown").await.unwrap_err(),
        fx.fail(RUNTIME, &caller, "item_unknown").await.unwrap_err(),
    ] {
        assert_kind(&error, ErrorKind::NotFound, "not_found");
        assert!(error.data.reason.is_none());
    }
}

/// Crash after commit: the tombstone is on disk before the receipt is
/// returned, so a cold core answers the lost response's retry with it.
#[tokio::test]
async fn a_receipt_is_durable_before_it_is_returned() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, _, item) = delivered(&fx, &caller, "result").await;
    let first = fx.ack(RUNTIME, &caller, &item).await.unwrap();
    let stored = settled_on_disk(&fx, &caller, &item);
    assert_eq!(stored["at_ms"], first["settled_at_ms"]);
    assert_eq!(stored["message_id"], first["message_id"]);
    // The response never reaches the runtime; the core goes down.
    let cold = fx.restart();
    drop(fx);
    cold.advance(3_600_000);
    assert_same_receipt(&first, &cold.ack(RUNTIME, &caller, &item).await.unwrap());
    let error = cold.fail(RUNTIME, &caller, &item).await.unwrap_err();
    assert_kind(&error, ErrorKind::Conflict, "settlement_conflict");
}

/// Crash before commit: no receipt, nothing settled, and the item is
/// delivered again.
#[tokio::test]
async fn a_failed_commit_returns_no_receipt_and_leaves_the_item_redeliverable() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (invocation, _, item) = delivered(&fx, &caller, "result").await;
    super::inbox::FAIL_SAVE.with(|fail| fail.set(true));
    let outcome = fx.ack(RUNTIME, &caller, &item).await;
    super::inbox::FAIL_SAVE.with(|fail| fail.set(false));
    let error = outcome.unwrap_err();
    assert_kind(&error, ErrorKind::StoreFailed, "store_failed");
    assert_eq!(settled_on_disk(&fx, &caller, &item), Value::Null);
    let cold = fx.restart();
    drop(fx);
    let iterator = cold.open(RUNTIME, &caller, &invocation).await.unwrap();
    let delivery = cold
        .next(RUNTIME, &caller, &iterator)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item_id(&delivery), item);
    let receipt = cold.fail(RUNTIME, &caller, &item).await.unwrap();
    assert_receipt(&receipt, &delivery, "fail", "failed", T0);
}

/// An empty `next`, a lease expiry and a connection loss settle nothing.
#[tokio::test]
async fn no_timeout_lease_expiry_or_transport_loss_synthesizes_a_receipt() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (invocation, _) = fx.reported(&caller, "result").await;
    let iterator = fx.open_on(RUNTIME, 6, &caller, &invocation).await.unwrap();
    let delivery = fx.next(RUNTIME, &caller, &iterator).await.unwrap().unwrap();
    let item = item_id(&delivery);
    assert_eq!(
        fx.next_within(RUNTIME, &caller, &iterator, 10)
            .await
            .unwrap(),
        None
    );
    fx.advance(super::inbox::LEASE_MS);
    fx.connection_closed(RUNTIME, 6);
    assert_eq!(settled_on_disk(&fx, &caller, &item), Value::Null);
    assert!(!fx.audit().contains(r#""outcome":"settled""#));
    let later = fx.open(RUNTIME2, &caller, &invocation).await.unwrap();
    let again = fx.next(RUNTIME2, &caller, &later).await.unwrap().unwrap();
    assert_eq!(item_id(&again), item);
}

/// The generated wire shape: exactly the five receipt fields, all
/// required, with the two closed enums.
#[test]
fn inbox_settled_has_exactly_the_receipt_fields() {
    let schema = serde_json::to_value(schemars::schema_for!(InboxSettled)).unwrap();
    let mut properties: Vec<&str> = schema["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    properties.sort_unstable();
    assert_eq!(
        properties,
        ["action", "item_id", "message_id", "settled_at_ms", "state"]
    );
    let mut required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| key.as_str().unwrap())
        .collect();
    required.sort_unstable();
    assert_eq!(required, properties);
    assert_eq!(schema["additionalProperties"], json!(false));
    let text = schema.to_string();
    for value in ["\"ack\"", "\"fail\"", "\"acked\"", "\"failed\""] {
        assert!(text.contains(value), "{value}");
    }
    for gone in ["already_settled", "tombstoned", "delivery_id"] {
        assert!(!text.contains(gone), "{gone}");
    }
    let typed: InboxSettled = serde_json::from_value(json!({
        "item_id": "item_1",
        "message_id": "msg_1",
        "action": "fail",
        "state": "failed",
        "settled_at_ms": 7,
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_string(&typed).unwrap(),
        r#"{"item_id":"item_1","message_id":"msg_1","action":"fail","state":"failed","settled_at_ms":7}"#
    );
}

// --- Connection challenge auth (docs/service-callers-auth.md section 8) ---

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn bytes_of(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&hex[at..at + 2], 16).unwrap())
        .collect()
}

/// The authenticator a runtime holding `secret` computes for `challenge`,
/// issued to `caller_id` in this host's scope.
fn answer_with(secret: &str, caller_id: &str, challenge: &Value) -> String {
    use base64::Engine;
    use sha2::Digest;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let key: [u8; 32] = sha2::Sha256::digest(secret.as_bytes()).into();
    let nonce = b64.decode(challenge["nonce"].as_str().unwrap()).unwrap();
    let transcript = super::auth::Transcript {
        version: 1,
        domain: super::auth::DOMAIN,
        challenge_id: challenge["challenge_id"].as_str().unwrap().as_bytes(),
        nonce: &nonce,
        caller_id: caller_id.as_bytes(),
        site_url: SITE.as_bytes(),
        workspace_id: b"ws_test",
        user_id: b"user_test",
        issued_at_ms: challenge["issued_at_ms"].as_u64().unwrap(),
        expires_at_ms: challenge["expires_at_ms"].as_u64().unwrap(),
    }
    .bytes();
    b64.encode(super::auth::hmac(&key, &transcript))
}

fn answer(caller: &Caller, challenge: &Value) -> String {
    answer_with(&caller.secret, &caller.id, challenge)
}

impl Fx {
    async fn challenge_on(&self, pid: i32, conn: u64, caller_id: &str) -> Result<Value, RpcError> {
        self.call_on(
            pid,
            Some(conn),
            "spawn_callers.challenge",
            json!({"caller_id": caller_id}),
        )
        .await
    }

    async fn authenticate_on(
        &self,
        pid: i32,
        conn: u64,
        challenge: &Value,
        authenticator: &str,
    ) -> Result<Value, RpcError> {
        self.call_on(
            pid,
            Some(conn),
            "spawn_callers.authenticate",
            json!({
                "challenge_id": challenge["challenge_id"],
                "authenticator": authenticator,
            }),
        )
        .await
    }

    /// Challenge and authenticate `conn` as `caller`.
    async fn bind(&self, pid: i32, conn: u64, caller: &Caller) -> Result<Value, RpcError> {
        let challenge = self.challenge_on(pid, conn, &caller.id).await?;
        self.authenticate_on(pid, conn, &challenge, &answer(caller, &challenge))
            .await
    }

    /// A binding-required call on `conn` with no credential.
    async fn bound(
        &self,
        pid: i32,
        conn: u64,
        method: &str,
        params: Value,
    ) -> Result<Value, RpcError> {
        self.call_on(pid, Some(conn), method, params).await
    }

    async fn bound_identity(&self, pid: i32, conn: u64) -> Result<Value, RpcError> {
        self.bound(pid, conn, "spawn_callers.identity", json!({}))
            .await
    }

    async fn bound_open(&self, pid: i32, conn: u64, invocation: &str) -> Result<String, RpcError> {
        self.bound(
            pid,
            conn,
            "spawn_callers.inbox.open",
            json!({"invocation_id": invocation}),
        )
        .await
        .map(|opened| opened["iterator_id"].as_str().unwrap().to_string())
    }

    async fn bound_next(
        &self,
        pid: i32,
        conn: u64,
        iterator: &str,
        timeout_ms: u64,
    ) -> Result<Option<Value>, RpcError> {
        self.bound(
            pid,
            conn,
            "spawn_callers.inbox.next",
            json!({"iterator_id": iterator, "timeout_ms": timeout_ms}),
        )
        .await
        .map(|next| Some(next["delivery"].clone()).filter(|delivery| !delivery.is_null()))
    }

    async fn bound_settle(
        &self,
        pid: i32,
        conn: u64,
        action: &str,
        item: &str,
        result: &str,
    ) -> Result<Value, RpcError> {
        self.bound(
            pid,
            conn,
            &format!("spawn_callers.inbox.{action}"),
            json!({"item_id": item, "metadata": {"relayflows_result": result}}),
        )
        .await
    }

    fn rotate(
        &self,
        caller: &Caller,
        overlap_seconds: u64,
    ) -> impl std::future::Future<Output = Result<Value, RpcError>> + '_ {
        let params = json!({"caller_id": caller.id, "overlap_seconds": overlap_seconds});
        async move { self.call(APP, "spawn_callers.rotate", params).await }
    }
}

#[track_caller]
fn assert_auth_failed(error: &RpcError) {
    assert_refused(error, RefusalReason::ServiceUnknown, "auth_failed");
}

#[test]
fn transcript_matches_the_v1_fixture() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/service-caller-auth-v1.json");
    let fixture = read_json(&path);
    assert!(!fixture.to_string().contains("connection"));
    let key: [u8; 32] = bytes_of(fixture["key_hex"].as_str().unwrap())
        .try_into()
        .unwrap();
    let inputs = &fixture["inputs"];
    let text = |name: &str| inputs[name].as_str().unwrap().as_bytes().to_vec();
    let authenticator = |overrides: &[(&str, &Value)]| {
        let pick = |name: &str| {
            overrides
                .iter()
                .find(|(field, _)| *field == name)
                .map_or(&inputs[name], |(_, value)| *value)
        };
        let string = |name: &str| pick(name).as_str().unwrap().as_bytes().to_vec();
        let nonce = bytes_of(pick("nonce").as_str().unwrap());
        let (domain, challenge_id, caller_id) = (
            string("domain"),
            string("challenge_id"),
            string("caller_id"),
        );
        let (site_url, workspace_id, user_id) = (
            string("site_url"),
            string("workspace_id"),
            string("user_id"),
        );
        let transcript = super::auth::Transcript {
            version: pick("version").as_u64().unwrap() as u8,
            domain: &domain,
            challenge_id: &challenge_id,
            nonce: &nonce,
            caller_id: &caller_id,
            site_url: &site_url,
            workspace_id: &workspace_id,
            user_id: &user_id,
            issued_at_ms: pick("issued_at_ms").as_u64().unwrap(),
            expires_at_ms: pick("expires_at_ms").as_u64().unwrap(),
        }
        .bytes();
        let mac = super::auth::hmac(&key, &transcript);
        (transcript, mac)
    };
    assert_eq!(text("domain"), super::auth::DOMAIN);
    let (transcript, mac) = authenticator(&[]);
    assert_eq!(
        transcript.len() as u64,
        fixture["transcript_len"].as_u64().unwrap()
    );
    assert_eq!(
        hex_of(&transcript),
        fixture["transcript_hex"].as_str().unwrap()
    );
    assert_eq!(hex_of(&mac), fixture["authenticator_hex"].as_str().unwrap());
    use base64::Engine;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    assert_eq!(
        b64.encode(mac),
        fixture["authenticator_b64url"].as_str().unwrap()
    );
    assert_eq!(
        b64.encode(bytes_of(inputs["nonce"].as_str().unwrap())),
        fixture["nonce_b64url"].as_str().unwrap()
    );
    let rows = fixture["substitutions"].as_array().unwrap();
    assert_eq!(rows.len(), 10);
    for row in rows {
        let field = row["field"].as_str().unwrap();
        let (_, mac) = authenticator(&[(field, &row["value"])]);
        assert_eq!(
            b64.encode(mac),
            row["authenticator_b64url"].as_str().unwrap(),
            "{field}"
        );
    }
}

#[test]
fn key_is_the_hex_decoded_secret_digest() {
    use sha2::Digest;
    let fixture = read_json(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/service-caller-auth-v1.json"),
    );
    let secret = fixture["secret"].as_str().unwrap();
    let digest = super::registry::digest(secret);
    assert_eq!(digest, fixture["key_hex"].as_str().unwrap());
    let key = super::auth::key_of(&digest).unwrap();
    assert_eq!(
        key.to_vec(),
        sha2::Sha256::digest(secret.as_bytes()).to_vec()
    );
    assert_eq!(super::auth::key_of(&digest.to_uppercase()), None);
    assert_eq!(super::auth::key_of(&digest[..62]), None);
}

#[tokio::test]
async fn authenticate_binds_the_connection_and_returns_the_generation() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let challenge = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap();
    assert_eq!(challenge["version"], 1);
    assert!(super::auth::is_challenge_id(
        challenge["challenge_id"].as_str().unwrap()
    ));
    assert_eq!(challenge["nonce"].as_str().unwrap().len(), 43);
    assert_eq!(challenge["issued_at_ms"], json!(T0));
    assert_eq!(challenge["expires_at_ms"], json!(T0 + super::auth::TTL_MS));
    assert_eq!(fx.callers.connection_state(1), Some("challenged"));
    let bound = fx
        .authenticate_on(RUNTIME, 1, &challenge, &answer(&caller, &challenge))
        .await
        .unwrap();
    assert_eq!(
        bound,
        json!({"caller_id": caller.id, "caller_name": "flows", "authority_generation": 1})
    );
    assert_eq!(fx.callers.connection_state(1), Some("bound"));
    let identity = fx.bound_identity(RUNTIME, 1).await.unwrap();
    assert_eq!(identity["publisher"]["id"], caller.agent_id.as_str());
    assert!(fx.audit().contains(r#""outcome":"bound""#));
    // A binding-required call on an unbound connection is refused.
    fx.challenge_on(RUNTIME, 2, &caller.id).await.unwrap();
    let error = fx.bound_identity(RUNTIME, 2).await.unwrap_err();
    assert_refused(&error, RefusalReason::ServiceUnknown, "caller_unbound");
}

#[tokio::test]
async fn replay_of_a_consumed_challenge_is_auth_failed() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let challenge = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap();
    let right = answer(&caller, &challenge);
    let wrong = answer_with("rsc_wrong", &caller.id, &challenge);
    assert_auth_failed(
        &fx.authenticate_on(RUNTIME, 1, &challenge, &wrong)
            .await
            .unwrap_err(),
    );
    assert_auth_failed(
        &fx.authenticate_on(RUNTIME, 1, &challenge, &right)
            .await
            .unwrap_err(),
    );
    assert!(fx.audit().contains("challenge_consumed"));
    // After a binding, the same answer again is already_bound.
    let challenge = fx.challenge_on(RUNTIME, 2, &caller.id).await.unwrap();
    let right = answer(&caller, &challenge);
    fx.authenticate_on(RUNTIME, 2, &challenge, &right)
        .await
        .unwrap();
    let error = fx
        .authenticate_on(RUNTIME, 2, &challenge, &right)
        .await
        .unwrap_err();
    assert_kind(&error, ErrorKind::Conflict, "already_bound");
}

#[tokio::test]
async fn authenticator_from_another_connection_is_auth_failed() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let first = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap();
    let second = fx.challenge_on(RUNTIME, 2, &caller.id).await.unwrap();
    // Connection 2 answers connection 1's challenge.
    let error = fx
        .authenticate_on(RUNTIME, 2, &first, &answer(&caller, &first))
        .await
        .unwrap_err();
    assert_auth_failed(&error);
    assert_eq!(fx.callers.connection_state(2), Some("challenged"));
    // Connection 2's own challenge was consumed by that attempt.
    let error = fx
        .authenticate_on(RUNTIME, 2, &second, &answer(&caller, &second))
        .await
        .unwrap_err();
    assert_auth_failed(&error);
    // Connection 1's challenge is untouched.
    fx.authenticate_on(RUNTIME, 1, &first, &answer(&caller, &first))
        .await
        .unwrap();
}

#[tokio::test]
async fn expiry_boundary_passes_at_ttl_minus_one_and_fails_at_ttl() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let challenge = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap();
    fx.advance(super::auth::TTL_MS - 1);
    fx.authenticate_on(RUNTIME, 1, &challenge, &answer(&caller, &challenge))
        .await
        .unwrap();
    let challenge = fx.challenge_on(RUNTIME, 2, &caller.id).await.unwrap();
    fx.advance(super::auth::TTL_MS);
    let error = fx
        .authenticate_on(RUNTIME, 2, &challenge, &answer(&caller, &challenge))
        .await
        .unwrap_err();
    assert_auth_failed(&error);
    assert!(fx.audit().contains("challenge_expired"));
}

#[tokio::test]
async fn concurrent_double_submit_binds_exactly_once() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let challenge = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap();
    let right = answer(&caller, &challenge);
    let (a, b) = tokio::join!(
        fx.authenticate_on(RUNTIME, 1, &challenge, &right),
        fx.authenticate_on(RUNTIME, 1, &challenge, &right),
    );
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let error = a.err().or(b.err()).unwrap();
    assert!(
        matches!(error.message.as_str(), "already_bound" | "auth_failed"),
        "{error:?}"
    );
    assert_eq!(fx.audit().matches(r#""outcome":"bound""#).count(), 1);
}

/// An unknown caller id and a wrong secret answer alike, after the same
/// MAC work.
#[tokio::test]
async fn unknown_caller_and_wrong_authenticator_are_indistinguishable() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let unknown = fx
        .challenge_on(RUNTIME, 1, "sc_0123456789abcdef")
        .await
        .unwrap();
    let known = fx.challenge_on(RUNTIME, 2, &caller.id).await.unwrap();
    let unknown_answer = answer_with(&caller.secret, "sc_0123456789abcdef", &unknown);
    let wrong_answer = answer_with("rsc_wrong", &caller.id, &known);
    let calls = || super::auth::HMAC_CALLS.with(std::cell::Cell::get);
    let start = calls();
    let first = fx
        .authenticate_on(RUNTIME, 1, &unknown, &unknown_answer)
        .await
        .unwrap_err();
    let middle = calls();
    let second = fx
        .authenticate_on(RUNTIME, 2, &known, &wrong_answer)
        .await
        .unwrap_err();
    let end = calls();
    assert_eq!(middle - start, 2);
    assert_eq!(end - middle, 2);
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap()
    );
    assert_auth_failed(&first);
    let audit = fx.audit();
    assert!(audit.contains("caller_unknown"));
    assert!(audit.contains("secret_mismatch"));
}

/// Every way `authenticate` can fail reads the registry once, as a wrong
/// authenticator for a known caller does: an unknown caller, a consumed,
/// absent or expired challenge never skip the load.
#[tokio::test]
async fn every_authenticate_failure_reads_the_registry_alike() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let loads = || super::registry::LOADS.with(std::cell::Cell::get);
    let mut counted = Vec::new();
    // A wrong authenticator for a known caller.
    let known = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap();
    let wrong = answer_with("rsc_wrong", &caller.id, &known);
    let start = loads();
    assert_auth_failed(
        &fx.authenticate_on(RUNTIME, 1, &known, &wrong)
            .await
            .unwrap_err(),
    );
    counted.push(("secret_mismatch", loads() - start));
    // An unknown caller.
    let unknown = fx
        .challenge_on(RUNTIME, 2, "sc_0123456789abcdef")
        .await
        .unwrap();
    let theirs = answer_with(&caller.secret, "sc_0123456789abcdef", &unknown);
    let start = loads();
    assert_auth_failed(
        &fx.authenticate_on(RUNTIME, 2, &unknown, &theirs)
            .await
            .unwrap_err(),
    );
    counted.push(("caller_unknown", loads() - start));
    // A consumed challenge: the same body again.
    let start = loads();
    assert_auth_failed(
        &fx.authenticate_on(RUNTIME, 1, &known, &wrong)
            .await
            .unwrap_err(),
    );
    counted.push(("challenge_consumed", loads() - start));
    // No challenge on the connection at all.
    let right = answer(&caller, &known);
    let start = loads();
    assert_auth_failed(
        &fx.authenticate_on(RUNTIME, 3, &known, &right)
            .await
            .unwrap_err(),
    );
    counted.push(("absent", loads() - start));
    // An expired challenge with the right answer.
    let late = fx.challenge_on(RUNTIME, 4, &caller.id).await.unwrap();
    let right = answer(&caller, &late);
    fx.advance(10 * 60_000);
    let start = loads();
    assert_auth_failed(
        &fx.authenticate_on(RUNTIME, 4, &late, &right)
            .await
            .unwrap_err(),
    );
    counted.push(("challenge_expired", loads() - start));
    for (case, count) in &counted {
        assert_eq!(*count, counted[0].1, "{case}: {counted:?}");
    }
    assert!(counted[0].1 >= 1, "{counted:?}");
}

#[tokio::test]
async fn challenge_answers_unknown_caller_ids() {
    let fx = Fx::new();
    let challenge = fx
        .challenge_on(RUNTIME, 1, "sc_fedcba9876543210")
        .await
        .unwrap();
    assert_eq!(challenge["version"], 1);
    for bad in ["sc_FEDCBA9876543210", "sc_123", "flows", ""] {
        let error = fx.challenge_on(RUNTIME, 2, bad).await.unwrap_err();
        assert_eq!(error.code, -32602, "{bad}: {error:?}");
        assert_eq!(error.message, "invalid_input");
    }
}

#[tokio::test]
async fn third_challenge_after_two_failed_attempts_still_binds() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    for _ in 0..2 {
        let challenge = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap();
        let wrong = answer_with("rsc_wrong", &caller.id, &challenge);
        assert_auth_failed(
            &fx.authenticate_on(RUNTIME, 1, &challenge, &wrong)
                .await
                .unwrap_err(),
        );
    }
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
}

#[tokio::test]
async fn fourth_challenge_after_three_ended_attempts_is_auth_locked() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    // A wrong answer, an expiry and a malformed answer each end an attempt.
    let challenge = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap();
    let wrong = answer_with("rsc_wrong", &caller.id, &challenge);
    assert_auth_failed(
        &fx.authenticate_on(RUNTIME, 1, &challenge, &wrong)
            .await
            .unwrap_err(),
    );
    fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap();
    fx.advance(super::auth::TTL_MS);
    let challenge = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap();
    let error = fx
        .authenticate_on(RUNTIME, 1, &challenge, "short")
        .await
        .unwrap_err();
    assert_eq!(error.code, -32602);
    assert_eq!(fx.callers.connection_state(1), Some("locked"));
    let error = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap_err();
    assert_kind(&error, ErrorKind::RateLimited, "auth_locked");
    assert!(!error.data.retryable);
    let error = fx.bound_identity(RUNTIME, 1).await.unwrap_err();
    assert_kind(&error, ErrorKind::RateLimited, "auth_locked");
    // A new connection starts afresh.
    fx.bind(RUNTIME, 2, &caller).await.unwrap();
}

/// A phase-1 key on `authenticate` is refused only after the take: the
/// outstanding challenge is consumed and the attempt counted first, so the
/// exact clean body retried is `auth_failed`, and three such refusals lock
/// the connection.
#[tokio::test]
async fn a_legacy_key_authenticate_consumes_and_counts_the_challenge() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let keys = [
        &["caller_secret"][..],
        &["caller_name"],
        &["caller_name", "caller_secret"],
    ];
    for (attempt, keys) in keys.into_iter().enumerate() {
        let challenge = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap();
        let clean = json!({
            "challenge_id": challenge["challenge_id"],
            "authenticator": answer(&caller, &challenge),
        });
        let mut keyed = clean.clone();
        for key in keys {
            keyed[*key] = Value::from(Fx::auth(&caller)[*key].clone());
        }
        let error = fx
            .call_on(RUNTIME, Some(1), "spawn_callers.authenticate", keyed)
            .await
            .unwrap_err();
        assert_kind(&error, ErrorKind::Refused, "legacy_credential_refused");
        assert!(!error.data.retryable);
        assert_eq!(fx.callers.outstanding_challenges(), 0);
        let replay = fx
            .call_on(RUNTIME, Some(1), "spawn_callers.authenticate", clean)
            .await
            .unwrap_err();
        if attempt < 2 {
            assert_auth_failed(&replay);
            assert_eq!(fx.callers.connection_state(1), Some("challenged"));
        } else {
            assert_kind(&replay, ErrorKind::RateLimited, "auth_locked");
            assert_eq!(fx.callers.connection_state(1), Some("locked"));
        }
    }
    let error = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap_err();
    assert_kind(&error, ErrorKind::RateLimited, "auth_locked");
    assert!(!fx.everything_shown().contains(&caller.secret));
    fx.bind(RUNTIME, 2, &caller).await.unwrap();
}

#[tokio::test]
async fn second_outstanding_challenge_is_refused() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let challenge = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap();
    let error = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap_err();
    assert_kind(&error, ErrorKind::Busy, "challenge_outstanding");
    // The first is still answerable.
    fx.authenticate_on(RUNTIME, 1, &challenge, &answer(&caller, &challenge))
        .await
        .unwrap();
    let error = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap_err();
    assert_kind(&error, ErrorKind::Conflict, "already_bound");
    // Per caller id, across connections.
    for conn in 0..super::auth::PER_CALLER as u64 {
        fx.challenge_on(RUNTIME, 100 + conn, &caller.id)
            .await
            .unwrap();
    }
    let error = fx.challenge_on(RUNTIME, 200, &caller.id).await.unwrap_err();
    assert_kind(&error, ErrorKind::Busy, "challenge_capacity");
}

#[tokio::test]
async fn oversized_or_malformed_auth_params_are_invalid_input_and_consume() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let max = super::auth::AUTH_PARAMS_MAX_BYTES;
    let bodies = |challenge: &Value| {
        let id = challenge["challenge_id"].clone();
        let right = answer(&caller, challenge);
        vec![
            // Oversized, though otherwise well formed.
            json!({"challenge_id": id, "authenticator": right, "pad": "x".repeat(max)}),
            json!({"challenge_id": id, "authenticator": "A".repeat(max)}),
            json!({"challenge_id": id}),
            json!({"challenge_id": id, "authenticator": right, "extra": 1}),
            json!({"challenge_id": id, "authenticator": 7}),
            json!({"challenge_id": "ch_short", "authenticator": right}),
            json!({"challenge_id": id, "authenticator": format!("{}=", &right[..42])}),
            json!([id, right]),
            json!(null),
        ]
    };
    let probe = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap();
    let count = bodies(&probe).len();
    for index in 0..count {
        let conn = 10 + index as u64;
        let challenge = fx.challenge_on(RUNTIME, conn, &caller.id).await.unwrap();
        let body = bodies(&challenge).swap_remove(index);
        let error = fx
            .call_on(
                RUNTIME,
                Some(conn),
                "spawn_callers.authenticate",
                body.clone(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, -32602, "{body}: {error:?}");
        assert_kind(&error, ErrorKind::InvalidParams, "invalid_input");
        // The challenge was consumed: the right answer no longer binds.
        let error = fx
            .authenticate_on(RUNTIME, conn, &challenge, &answer(&caller, &challenge))
            .await
            .unwrap_err();
        assert_auth_failed(&error);
        assert_eq!(fx.callers.connection_state(conn), Some("challenged"));
    }
    // Each consumed one attempt: three lock the connection.
    for _ in 0..super::auth::MAX_ATTEMPTS {
        let challenge = fx.challenge_on(RUNTIME, 2, &caller.id).await.unwrap();
        let error = fx
            .call_on(
                RUNTIME,
                Some(2),
                "spawn_callers.authenticate",
                json!({"x": challenge["challenge_id"]}),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, -32602);
    }
    assert_eq!(fx.callers.connection_state(2), Some("locked"));
    assert!(!fx.everything_shown().contains(&"x".repeat(max)));
}

#[tokio::test]
async fn reconnect_gets_a_new_connection_and_no_binding() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
    fx.connection_closed(RUNTIME, 1);
    assert_eq!(fx.callers.connection_state(1), None);
    let error = fx.bound_identity(RUNTIME, 2).await.unwrap_err();
    assert_refused(&error, RefusalReason::ServiceUnknown, "caller_unbound");
    fx.bind(RUNTIME, 2, &caller).await.unwrap();
    fx.bound_identity(RUNTIME, 2).await.unwrap();
}

#[tokio::test]
async fn pid_is_not_part_of_the_binding() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
    let identity = fx.bound_identity(RUNTIME2, 1).await.unwrap();
    assert_eq!(identity["publisher"]["id"], caller.agent_id.as_str());
}

#[tokio::test]
async fn unix_peer_check_stays_defence_in_depth() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let error = fx.challenge_on(STRANGER, 1, &caller.id).await.unwrap_err();
    assert_refused(&error, RefusalReason::ManagementPeer, "peer_uid_mismatch");
}

#[tokio::test]
async fn rotation_invalidates_a_live_binding() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
    fx.rotate(&caller, 0).await.unwrap();
    let error = fx.bound_identity(RUNTIME, 1).await.unwrap_err();
    assert_refused(&error, RefusalReason::ServiceExpired, "binding_stale");
    assert_eq!(fx.callers.connection_state(1), Some("stale"));
    let error = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap_err();
    assert_kind(&error, ErrorKind::Conflict, "already_bound");
    // The old secret no longer binds; the new one does, at the next
    // generation.
    let challenge = fx.challenge_on(RUNTIME, 2, &caller.id).await.unwrap();
    assert_auth_failed(
        &fx.authenticate_on(RUNTIME, 2, &challenge, &answer(&caller, &challenge))
            .await
            .unwrap_err(),
    );
    let rotated = caller.with_secret(read_json(&caller.handoff)["secret"].as_str().unwrap());
    let bound = fx.bind(RUNTIME, 3, &rotated).await.unwrap();
    assert_eq!(bound["authority_generation"], 2);
}

#[tokio::test]
async fn revocation_and_disable_invalidate_bindings() {
    let fx = Fx::new();
    let disabled = fx.provision("disabled").await;
    let revoked = fx.provision("revoked").await;
    fx.bind(RUNTIME, 1, &disabled).await.unwrap();
    fx.bind(RUNTIME, 2, &revoked).await.unwrap();
    fx.call(
        APP,
        "spawn_callers.set_enabled",
        json!({"caller_id": disabled.id, "enabled": false}),
    )
    .await
    .unwrap();
    fx.call(
        APP,
        "spawn_callers.revoke",
        json!({"caller_id": revoked.id}),
    )
    .await
    .unwrap();
    for conn in [1, 2] {
        let error = fx.bound_identity(RUNTIME, conn).await.unwrap_err();
        assert_refused(&error, RefusalReason::ServiceExpired, "binding_stale");
    }
    // Enabling again does not revive the old binding.
    fx.call(
        APP,
        "spawn_callers.set_enabled",
        json!({"caller_id": disabled.id, "enabled": true}),
    )
    .await
    .unwrap();
    let error = fx.bound_identity(RUNTIME, 1).await.unwrap_err();
    assert_refused(&error, RefusalReason::ServiceExpired, "binding_stale");
}

#[tokio::test]
async fn overlap_retirement_ends_a_binding_made_with_the_old_secret() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    fx.rotate(&caller, 60).await.unwrap();
    // Bound with the old secret inside the overlap.
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
    let rotated = caller.with_secret(read_json(&caller.handoff)["secret"].as_str().unwrap());
    fx.bind(RUNTIME, 2, &rotated).await.unwrap();
    fx.bound_identity(RUNTIME, 1).await.unwrap();
    let record = record_of(&fx, &caller.id).unwrap();
    assert_eq!(record["secrets"].as_array().unwrap().len(), 2);
    let generation = record["authority_generation"].as_u64().unwrap();
    fx.advance(60_000);
    // The first check after `retires_at_ms` removes the old digest and
    // advances the generation once: every binding of the caller is stale,
    // the one made with the new secret too.
    let error = fx.bound_identity(RUNTIME, 1).await.unwrap_err();
    assert_refused(&error, RefusalReason::ServiceExpired, "binding_stale");
    let error = fx.bound_identity(RUNTIME, 2).await.unwrap_err();
    assert_refused(&error, RefusalReason::ServiceExpired, "binding_stale");
    let record = record_of(&fx, &caller.id).unwrap();
    assert_eq!(record["secrets"].as_array().unwrap().len(), 1);
    assert_eq!(record["authority_generation"], generation + 1);
    // Later checks advance nothing more, and the new secret binds again.
    fx.bind(RUNTIME, 3, &rotated).await.unwrap();
    fx.bound_identity(RUNTIME, 3).await.unwrap();
    assert_eq!(
        record_of(&fx, &caller.id).unwrap()["authority_generation"],
        generation + 1
    );
    let error = fx.bind(RUNTIME, 4, &caller).await.unwrap_err();
    assert_auth_failed(&error);
}

/// Expiry advances the generation once, at the first check after
/// `expires_at`, tearing down every binding; renewal by rotation lets it
/// advance again at the next expiry.
#[tokio::test]
async fn caller_expiry_advances_the_generation_once() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
    fx.bind(RUNTIME, 2, &caller).await.unwrap();
    let generation = record_of(&fx, &caller.id).unwrap()["authority_generation"]
        .as_u64()
        .unwrap();
    fx.advance(91 * super::registry::DAY_MS);
    // The periodic sweep is a check too: it finds the expiry first.
    fx.callers.sweep_lifecycle();
    let record = record_of(&fx, &caller.id).unwrap();
    assert_eq!(record["authority_generation"], generation + 1);
    assert_eq!(record["expiry_advanced"], true);
    assert_eq!(fx.callers.connection_state(1), Some("stale"));
    assert_eq!(fx.callers.connection_state(2), Some("stale"));
    for conn in [1, 2] {
        let error = fx.bound_identity(RUNTIME, conn).await.unwrap_err();
        assert_refused(&error, RefusalReason::ServiceExpired, "binding_stale");
    }
    let error = fx.bind(RUNTIME, 3, &caller).await.unwrap_err();
    assert_refused(&error, RefusalReason::ServiceExpired, "caller_expired");
    fx.callers.sweep_lifecycle();
    assert_eq!(
        record_of(&fx, &caller.id).unwrap()["authority_generation"],
        generation + 1,
        "expiry advances once"
    );
    // A rotation renews `expires_at` and clears the mark.
    fx.rotate(&caller, 0).await.unwrap();
    let record = record_of(&fx, &caller.id).unwrap();
    assert_eq!(record["authority_generation"], generation + 2);
    assert!(record.get("expiry_advanced").is_none());
    let renewed = caller.with_secret(read_json(&caller.handoff)["secret"].as_str().unwrap());
    fx.bind(RUNTIME, 4, &renewed).await.unwrap();
    fx.advance(91 * super::registry::DAY_MS);
    let error = fx.bound_identity(RUNTIME, 4).await.unwrap_err();
    assert_refused(&error, RefusalReason::ServiceExpired, "binding_stale");
    assert_eq!(
        record_of(&fx, &caller.id).unwrap()["authority_generation"],
        generation + 3
    );
}

/// A rotation whose `caller.json` write fails and whose rollback write
/// fails too reports it, and no check reads authority from the registry
/// until the old record is written back; the next check that can write it
/// does.
#[tokio::test]
async fn failed_rotation_rollback_reconciles_before_any_check() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
    let before = record_of(&fx, &caller.id).unwrap();
    super::handoff::FAIL_WRITE.with(|fail| fail.set(true));
    super::handoff::FAIL_REGISTRY_AFTER.with(|after| after.set(true));
    let error = fx.rotate(&caller, 60).await.unwrap_err();
    super::handoff::FAIL_WRITE.with(|fail| fail.set(false));
    super::handoff::FAIL_REGISTRY_AFTER.with(|after| after.set(false));
    assert_kind(&error, ErrorKind::StoreFailed, "rotation_rollback_pending");
    // The registry on disk still names the undelivered secret.
    assert_ne!(record_of(&fx, &caller.id).unwrap(), before);
    assert_eq!(
        read_json(&caller.handoff)["secret"].as_str(),
        Some(caller.secret.as_str())
    );
    // While the record cannot be put back nothing authenticates, not even
    // the old secret the overlap would have kept.
    assert!(fx.bind(RUNTIME, 2, &caller).await.is_err());
    assert!(fx.bound_identity(RUNTIME, 1).await.is_err());
    assert!(fx.call(APP, "spawn_callers.list", json!({})).await.is_err());
    super::registry::FAIL_SAVE.with(|fail| fail.set(false));
    // The next check writes the old record back, and the old secret binds
    // at the old generation.
    fx.bind(RUNTIME, 3, &caller).await.unwrap();
    assert_eq!(record_of(&fx, &caller.id).unwrap(), before);
    let list = fx.call(APP, "spawn_callers.list", json!({})).await.unwrap();
    assert_eq!(list["callers"][0]["state"], "enabled");
}

/// Regression by flows-fixer: after a rotation whose `caller.json` write and
/// rollback write both fail, a cold core over the same state finds the
/// durable rollback marker and refuses the old caller's bind until the old
/// record can be written back.
#[tokio::test]
async fn failed_rotation_rollback_remains_closed_after_restart() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    super::handoff::FAIL_WRITE.with(|fail| fail.set(true));
    super::handoff::FAIL_REGISTRY_AFTER.with(|after| after.set(true));
    let error = fx.rotate(&caller, 60).await.unwrap_err();
    super::handoff::FAIL_WRITE.with(|fail| fail.set(false));
    super::handoff::FAIL_REGISTRY_AFTER.with(|after| after.set(false));
    assert_kind(&error, ErrorKind::StoreFailed, "rotation_rollback_pending");
    let cold = fx.restart();
    let outcome = cold.bind(RUNTIME, 1, &caller).await;
    assert!(
        outcome.is_err(),
        "a cold core must not authenticate from the unrolled-back registry: {outcome:?}"
    );
    super::registry::FAIL_SAVE.with(|fail| fail.set(false));
}

/// A rotation whose `caller.json` write and rollback write both fail
/// leaves its rollback marker in the registry: a cold core reconciles it
/// before any check, back to the old record and generation when
/// `caller.json` still holds the old secret, and keeping the new record
/// when `caller.json` already holds the new one.
#[tokio::test]
async fn a_rotation_rollback_pending_across_restart_is_reconciled_from_disk() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
    let before = record_of(&fx, &caller.id).unwrap();
    super::handoff::FAIL_WRITE.with(|fail| fail.set(true));
    super::handoff::FAIL_REGISTRY_AFTER.with(|after| after.set(true));
    let error = fx.rotate(&caller, 60).await.unwrap_err();
    super::handoff::FAIL_WRITE.with(|fail| fail.set(false));
    super::handoff::FAIL_REGISTRY_AFTER.with(|after| after.set(false));
    super::registry::FAIL_SAVE.with(|fail| fail.set(false));
    assert_kind(&error, ErrorKind::StoreFailed, "rotation_rollback_pending");
    // On disk: the new record, and the marker naming the old one.
    let pending = record_of(&fx, &caller.id).unwrap();
    assert_ne!(
        pending["authority_generation"],
        before["authority_generation"]
    );
    let marker = &pending["rollback"];
    assert_eq!(
        marker["authority_generation"],
        before["authority_generation"]
    );
    // The core goes down before anything reconciled it.
    let cold = fx.restart();
    drop(fx);
    cold.bind(RUNTIME, 2, &caller).await.unwrap();
    assert_eq!(record_of(&cold, &caller.id).unwrap(), before);
    assert_eq!(
        read_json(&caller.handoff)["secret"].as_str(),
        Some(caller.secret.as_str())
    );

    // The other way: `caller.json` reached the disk and only the marker's
    // clearing did not. A cold core keeps the new record.
    cold.rotate(&caller, 0).await.unwrap();
    let rotated = record_of(&cold, &caller.id).unwrap();
    let renewed = caller.with_secret(read_json(&caller.handoff)["secret"].as_str().unwrap());
    let mut file = read_json(&cold.registry_file());
    let callers = file["callers"].as_array_mut().unwrap();
    let index = callers
        .iter()
        .position(|record| record["caller_id"] == caller.id.as_str())
        .unwrap();
    callers[index]["rollback"] = before.clone();
    std::fs::write(
        cold.registry_file(),
        serde_json::to_vec_pretty(&file).unwrap(),
    )
    .unwrap();
    let again = cold.restart();
    drop(cold);
    again.bind(RUNTIME, 3, &renewed).await.unwrap();
    assert_eq!(record_of(&again, &caller.id).unwrap(), rotated);
    assert!(again.bind(RUNTIME, 4, &caller).await.is_err());
}

#[tokio::test]
async fn connection_drop_during_next_releases_the_lease_at_once() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (invocation, _) = fx.reported(&caller, "result").await;
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
    let iterator = fx.bound_open(RUNTIME, 1, &invocation).await.unwrap();
    let delivery = fx
        .bound_next(RUNTIME, 1, &iterator, 0)
        .await
        .unwrap()
        .unwrap();
    let started = std::time::Instant::now();
    let (waited, ()) = tokio::join!(fx.bound_next(RUNTIME, 1, &iterator, 5_000), async {
        tokio::task::yield_now().await;
        fx.connection_closed(RUNTIME, 1);
    });
    let error = waited.unwrap_err();
    assert_kind(&error, ErrorKind::Refused, "connection_closed");
    assert!(started.elapsed() < std::time::Duration::from_secs(4));
    // The lease went with the connection: a new one is delivered the item
    // at once.
    fx.bind(RUNTIME, 2, &caller).await.unwrap();
    let later = fx.bound_open(RUNTIME, 2, &invocation).await.unwrap();
    let again = fx.bound_next(RUNTIME, 2, &later, 0).await.unwrap().unwrap();
    assert_eq!(item_id(&again), item_id(&delivery));
}

#[tokio::test]
async fn closed_marker_is_kept_until_in_flight_calls_drain() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
    let in_flight = fx.callers.enter(Some(1));
    fx.connection_closed(RUNTIME, 1);
    assert_eq!(fx.callers.connection_state(1), Some("closed"));
    let error = fx.bound_identity(RUNTIME, 1).await.unwrap_err();
    assert_kind(&error, ErrorKind::Refused, "connection_closed");
    let error = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap_err();
    assert_kind(&error, ErrorKind::Refused, "connection_closed");
    drop(in_flight);
    assert_eq!(fx.callers.connection_state(1), None);
}

/// A call counted in before the close keeps the untouched entry alive; a
/// challenge on it afterwards is refused and leaves nothing behind.
#[tokio::test]
async fn a_challenge_racing_a_close_is_connection_closed() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let before = fx.callers.outstanding_challenges();
    let in_flight = fx.callers.enter(Some(77));
    fx.connection_closed(RUNTIME, 77);
    assert_eq!(fx.callers.connection_state(77), Some("untouched"));
    let params = crate::contract::service_callers::CallerChallengeParams {
        caller_id: caller.id.clone(),
    };
    let error = fx
        .callers
        .challenge(Some(RUNTIME), 77, &params)
        .unwrap_err();
    assert_kind(&error, ErrorKind::Refused, "connection_closed");
    assert!(!error.data.retryable);
    assert_eq!(fx.callers.outstanding_challenges(), before);
    drop(in_flight);
    assert_eq!(fx.callers.connection_state(77), None);
    assert_eq!(fx.callers.outstanding_challenges(), before);
}

/// The same for `authenticate`, challenged or untouched at the close.
#[tokio::test]
async fn an_authentication_racing_a_close_is_connection_closed() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let challenge = fx.challenge_on(RUNTIME, 78, &caller.id).await.unwrap();
    let body = json!({
        "challenge_id": challenge["challenge_id"],
        "authenticator": answer(&caller, &challenge),
    });
    for conn in [78, 79] {
        let in_flight = fx.callers.enter(Some(conn));
        fx.connection_closed(RUNTIME, conn);
        let error = fx
            .callers
            .authenticate_connection(conn, &body, false)
            .unwrap_err();
        assert_kind(&error, ErrorKind::Refused, "connection_closed");
        assert!(!error.data.retryable);
        drop(in_flight);
        assert_eq!(fx.callers.connection_state(conn), None);
    }
    assert_eq!(fx.callers.outstanding_challenges(), 0);
}

/// A phase-1 `inbox.open` counted in before the close opens no iterator
/// that the close would have ended.
#[tokio::test]
async fn a_legacy_open_racing_a_close_is_connection_closed() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (invocation, _) = fx.reported(&caller, "result").await;
    let in_flight = fx.callers.enter(Some(80));
    fx.connection_closed(RUNTIME, 80);
    let who = super::auth::Who::Legacy {
        peer_pid: Some(RUNTIME),
        connection_id: Some(80),
    };
    let params = crate::contract::service_callers::InboxOpenParams {
        caller_name: caller.name.clone(),
        caller_secret: Some(caller.secret.clone()),
        invocation_id: invocation,
    };
    let error = fx.callers.open(&who, &params).unwrap_err();
    assert_kind(&error, ErrorKind::Refused, "connection_closed");
    assert!(!error.data.retryable);
    drop(in_flight);
    assert_eq!(fx.callers.connection_state(80), None);
    assert!(fx.callers.live().iterators.is_empty());
}

/// After teardown no call on the old id acts as its binding.
#[tokio::test]
async fn a_late_call_after_teardown_never_acts_as_the_binding() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
    fx.connection_closed(RUNTIME, 1);
    let error = fx.bound_identity(RUNTIME, 1).await.unwrap_err();
    assert_kind(&error, ErrorKind::Refused, "connection_closed");
    assert_eq!(fx.callers.connection_state(1), None);
}

#[tokio::test]
async fn legacy_caller_secret_is_refused_on_an_external_connection() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap();
    fx.bind(RUNTIME, 2, &caller).await.unwrap();
    for conn in [1, 2] {
        let mut params = Fx::auth(&caller);
        params.insert("invocation_id".into(), "inv_any".into());
        let error = fx
            .call_on(
                RUNTIME,
                Some(conn),
                "spawn_callers.inbox.open",
                Value::Object(params),
            )
            .await
            .unwrap_err();
        assert_kind(&error, ErrorKind::Refused, "legacy_credential_refused");
    }
    // Never on challenge or authenticate, even before any challenge.
    let mut params = Fx::auth(&caller);
    params.insert("caller_id".into(), caller.id.clone().into());
    let error = fx
        .call_on(
            RUNTIME,
            Some(3),
            "spawn_callers.challenge",
            Value::Object(params),
        )
        .await
        .unwrap_err();
    assert_kind(&error, ErrorKind::Refused, "legacy_credential_refused");
    assert!(!fx.everything_shown().contains(&caller.secret));
}

/// Decision: a connection that never challenged keeps the phase-1
/// per-request credential.
#[cfg(feature = "service-callers-dev-attest")]
#[tokio::test]
async fn legacy_caller_secret_still_works_on_a_connection_that_never_challenged() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let mut params = Fx::auth(&caller);
    params.insert("caller_name".into(), caller.name.clone().into());
    fx.call_on(
        RUNTIME,
        Some(4),
        "spawn_callers.identity",
        Value::Object(params),
    )
    .await
    .unwrap();
    fx.identity(RUNTIME, &caller).await.unwrap();
    assert_eq!(fx.callers.connection_state(4), None);
}

/// Outside a dev-attest preview build a phase-1 name or secret is refused
/// before any lookup, on any connection, and never shown; the challenge
/// path still works on the same connection.
#[cfg(not(feature = "service-callers-dev-attest"))]
#[tokio::test]
async fn the_default_build_refuses_a_legacy_credential_before_any_lookup() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (invocation, _) = fx.reported(&caller, "result").await;
    let (requests, spawns) = (fx.requests(), fx.spawns());
    let inbox_before = std::fs::read_to_string(inbox_file_of(&fx, &caller)).unwrap();
    let calls = [
        (None, "spawn_callers.identity", json!({})),
        (Some(5), "spawn_callers.identity", json!({})),
        (
            None,
            "spawn_callers.inbox.open",
            json!({"invocation_id": invocation}),
        ),
        (
            Some(5),
            "spawn_callers.inbox.open",
            json!({"invocation_id": invocation}),
        ),
        (
            Some(5),
            "spawn_callers.challenge",
            json!({"caller_id": caller.id}),
        ),
        (None, "spawn_callers.list", json!({})),
    ];
    for (conn, method, extra) in calls {
        for keys in [
            &["caller_name", "caller_secret"][..],
            &["caller_secret"],
            &["caller_name"],
        ] {
            let mut params = Fx::auth(&caller);
            params.retain(|key, _| keys.contains(&key.as_str()));
            if let Value::Object(extra) = &extra {
                params.extend(extra.clone());
            }
            let error = fx
                .call_on(RUNTIME, conn, method, Value::Object(params))
                .await
                .unwrap_err();
            assert_kind(&error, ErrorKind::Refused, "legacy_credential_refused");
            assert!(!error.data.retryable, "{method}");
            assert_eq!(fx.callers.connection_state(5), None, "{method}");
        }
    }
    assert!(!fx.everything_shown().contains(&caller.secret));
    assert_eq!(fx.requests(), requests);
    assert_eq!(fx.spawns(), spawns);
    assert_eq!(
        std::fs::read_to_string(inbox_file_of(&fx, &caller)).unwrap(),
        inbox_before
    );
    fx.bind(RUNTIME, 5, &caller).await.unwrap();
    assert_eq!(
        fx.bound_identity(RUNTIME, 5).await.unwrap()["publisher"]["id"],
        caller.agent_id.as_str()
    );
}

#[tokio::test]
async fn management_is_refused_on_an_external_connection() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
    let error = fx
        .call_on(APP, Some(1), "spawn_callers.list", json!({}))
        .await
        .unwrap_err();
    assert_refused(&error, RefusalReason::ManagementPeer, "external_connection");
    fx.call_on(APP, Some(2), "spawn_callers.list", json!({}))
        .await
        .unwrap();
}

/// A management call already in flight when its connection is challenged
/// or closed makes no change: the recheck under the live lock at its
/// side-effect step refuses it, and the publisher it registered is deleted.
#[tokio::test]
async fn management_rechecks_its_connection_at_the_side_effect() {
    for closes in [false, true] {
        let fx = Fx::new();
        let other = fx.provision("other").await;
        let hold = Arc::new(tokio::sync::Notify::new());
        *fx.world.register_hold.lock().unwrap() = Some(hold.clone());
        let (outcome, ()) = tokio::join!(
            fx.call_on(
                APP,
                Some(7),
                "spawn_callers.provision",
                fx.provision_params("flows"),
            ),
            async {
                if closes {
                    fx.callers.connection_closed(&CallContext {
                        peer_pid: Some(APP),
                        connection_id: Some(7),
                    });
                } else {
                    fx.challenge_on(APP, 7, &other.id).await.unwrap();
                }
                hold.notify_one();
            }
        );
        let error = outcome.unwrap_err();
        if closes {
            assert_kind(&error, ErrorKind::Refused, "connection_closed");
        } else {
            assert_refused(&error, RefusalReason::ManagementPeer, "external_connection");
        }
        let registry = read_json(&fx.registry_file());
        let flows: Vec<&Value> = registry["callers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|caller| caller["name"] == "flows")
            .collect();
        assert_eq!(flows.len(), 1, "closes={closes}");
        assert_eq!(flows[0]["enabled"], false, "closes={closes}");
        assert!(flows[0]["token"].is_null(), "closes={closes}");
        let id = flows[0]["caller_id"].as_str().unwrap();
        assert!(!fx.handoff_of(id).exists(), "closes={closes}");
        assert_eq!(
            fx.world.relaycast.lock().unwrap().deleted,
            vec!["flows".to_string()],
            "closes={closes}"
        );
    }
}

#[tokio::test]
async fn internal_calls_cannot_challenge() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let error = fx
        .call(
            RUNTIME,
            "spawn_callers.challenge",
            json!({"caller_id": caller.id}),
        )
        .await
        .unwrap_err();
    assert_refused(&error, RefusalReason::ManagementPeer, "binding_unavailable");
    let error = fx
        .call(
            RUNTIME,
            "spawn_callers.authenticate",
            json!({"challenge_id": format!("ch_{}", "0".repeat(32)), "authenticator": "A".repeat(43)}),
        )
        .await
        .unwrap_err();
    assert_refused(&error, RefusalReason::ManagementPeer, "binding_unavailable");
}

#[tokio::test]
async fn disconnect_tears_down_the_binding() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
    fx.bound(RUNTIME, 1, "spawn_callers.disconnect", json!({}))
        .await
        .unwrap();
    let error = fx.bound_identity(RUNTIME, 1).await.unwrap_err();
    assert_refused(&error, RefusalReason::ServiceExpired, "binding_stale");
    let error = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap_err();
    assert_kind(&error, ErrorKind::Conflict, "already_bound");
}

#[tokio::test]
async fn restart_drops_every_challenge_and_binding() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
    let challenge = fx.challenge_on(RUNTIME, 2, &caller.id).await.unwrap();
    let cold = fx.restart();
    drop(fx);
    let error = cold.bound_identity(RUNTIME, 1).await.unwrap_err();
    assert_refused(&error, RefusalReason::ServiceUnknown, "caller_unbound");
    let error = cold
        .authenticate_on(RUNTIME, 2, &challenge, &answer(&caller, &challenge))
        .await
        .unwrap_err();
    assert_auth_failed(&error);
}

/// The secret, its key and every authenticator stay out of logs, audit,
/// errors and results; the challenge's own fields are the only values
/// shown.
#[tokio::test]
async fn no_secret_key_or_authenticator_is_ever_shown() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let challenge = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap();
    let right = answer(&caller, &challenge);
    let wrong = answer_with("rsc_wrong", &caller.id, &challenge);
    fx.authenticate_on(RUNTIME, 1, &challenge, &wrong)
        .await
        .unwrap_err();
    let challenge = fx.challenge_on(RUNTIME, 1, &caller.id).await.unwrap();
    let right_too = answer(&caller, &challenge);
    fx.authenticate_on(RUNTIME, 1, &challenge, &right_too)
        .await
        .unwrap();
    let (invocation, _) = fx.reported(&caller, "result").await;
    let iterator = fx.bound_open(RUNTIME, 1, &invocation).await.unwrap();
    let delivery = fx
        .bound_next(RUNTIME, 1, &iterator, 0)
        .await
        .unwrap()
        .unwrap();
    fx.bound_settle(RUNTIME, 1, "ack", &item_id(&delivery), "journaled")
        .await
        .unwrap();
    fx.connection_closed(RUNTIME, 1);
    let shown = fx.everything_shown();
    let digest = super::registry::digest(&caller.secret);
    for secret in [
        caller.secret.as_str(),
        digest.as_str(),
        &right,
        &wrong,
        &right_too,
    ] {
        assert!(!shown.contains(secret), "{secret}");
    }
    let audit = fx.audit();
    assert!(!audit.contains(challenge["nonce"].as_str().unwrap()));
    assert!(!audit.contains(challenge["challenge_id"].as_str().unwrap()));
}

/// A handler cancelled between `admit` and `record` drops its permit; the
/// reservation goes with it at once, not after `inbox::UNBOUND_MS`.
#[tokio::test]
async fn a_permit_dropped_after_admit_releases_its_reservation() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let params = fx.invoke_params(&caller);
    let workspace = workspace();
    let resolved = fx.resolve(&caller).unwrap();
    let inbox_file = fx
        .path("app/ServiceCallers")
        .join(format!("{}.results", super::registry::key(SITE, "ws_test")))
        .join(format!("{}.json", caller.id));
    let reserved =
        |key: &str| std::fs::read_to_string(&inbox_file).is_ok_and(|text| text.contains(key));
    let permit = fx
        .callers
        .admit(
            &resolved,
            &SpawnCall {
                peer_pid: Some(RUNTIME),
                connection_id: None,
                workspace: &workspace,
                request: SpawnRequest::Invoke(&params),
            },
        )
        .unwrap();
    assert!(reserved(&params.idempotency_key));
    drop(permit);
    assert!(!reserved(&params.idempotency_key));
    // No time has passed; the full budget of reservations is free again.
    for _ in 0..super::inbox::ACTIVE {
        fx.invoke(&caller).unwrap();
        for spawned in fx.world.spawned.lock().unwrap().iter_mut() {
            spawned.active = false;
        }
    }
    // A recorded permit is not released by its drop.
    let file = read_json(&inbox_file);
    assert_eq!(
        file["slots"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|slot| !slot["invocation_id"].is_null())
            .count(),
        super::inbox::ACTIVE
    );
}

// --- Connection close against in-flight work (Relay PR #2115 gate) ------

#[track_caller]
fn assert_typed_closed(error: &RpcError) {
    assert_kind(error, ErrorKind::Refused, "connection_closed");
    assert!(!error.data.retryable);
    let class: CallerErrorClass = serde_json::from_value(json!(error.message)).unwrap();
    assert_eq!(class, CallerErrorClass::ConnectionClosed);
}

fn inbox_file_of(fx: &Fx, caller: &Caller) -> PathBuf {
    fx.path("app/ServiceCallers")
        .join(format!("{}.results", super::registry::key(SITE, "ws_test")))
        .join(format!("{}.json", caller.id))
}

/// The slots of `caller`'s inbox file holding `key`'s reservation, as
/// `(bound to an invocation, unbound)`.
fn reservations_of(fx: &Fx, caller: &Caller, key: &str) -> (usize, usize) {
    let Ok(text) = std::fs::read_to_string(inbox_file_of(fx, caller)) else {
        return (0, 0);
    };
    let file: Value = serde_json::from_str(&text).unwrap();
    let slots: Vec<&Value> = file["slots"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|slot| slot["key"] == key)
        .collect();
    let bound = slots
        .iter()
        .filter(|slot| !slot["invocation_id"].is_null())
        .count();
    (bound, slots.len() - bound)
}

/// `spawn.invoke` admitted as the core admits it, its permit not yet
/// recorded.
fn admitted_invoke(fx: &Fx, caller: &Caller) -> (SpawnInvokeParams, Permit) {
    let params = fx.invoke_params(caller);
    let permit = admitted_with(fx, caller, &params);
    (params, permit)
}

/// `spawn.invoke` of `params` admitted as the core admits it.
fn admitted_with(fx: &Fx, caller: &Caller, params: &SpawnInvokeParams) -> Permit {
    let workspace = workspace();
    let resolved = fx.resolve(&caller).unwrap();
    let permit = fx
        .callers
        .admit(
            &resolved,
            &SpawnCall {
                peer_pid: Some(RUNTIME),
                connection_id: None,
                workspace: &workspace,
                request: SpawnRequest::Invoke(params),
            },
        )
        .unwrap();
    permit
}

fn failed_invoke() -> Result<SpawnControlInvocation, RpcError> {
    Err(RpcError::new(ErrorKind::Internal, "spawn failed"))
}

#[tokio::test]
async fn a_pending_next_woken_by_connection_closed_returns_typed_closed_and_consumes_nothing() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (invocation, _) = fx.reported(&caller, "result").await;
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
    let iterator = fx.bound_open(RUNTIME, 1, &invocation).await.unwrap();
    let delivery = fx
        .bound_next(RUNTIME, 1, &iterator, 0)
        .await
        .unwrap()
        .unwrap();
    let (waited, ()) = tokio::join!(fx.bound_next(RUNTIME, 1, &iterator, 5_000), async {
        tokio::task::yield_now().await;
        fx.connection_closed(RUNTIME, 1);
    });
    assert_typed_closed(&waited.unwrap_err());
    assert_eq!(
        settled_on_disk(&fx, &caller, &item_id(&delivery)),
        Value::Null
    );
    fx.bind(RUNTIME2, 2, &caller).await.unwrap();
    let later = fx.bound_open(RUNTIME2, 2, &invocation).await.unwrap();
    let again = fx
        .bound_next(RUNTIME2, 2, &later, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again, delivery);
    let receipt = fx
        .bound_settle(RUNTIME2, 2, "ack", &item_id(&again), "journaled")
        .await
        .unwrap();
    assert_receipt(&receipt, &delivery, "ack", "acked", T0);
}

/// `admit` is not tied to a bridge connection in this pin (`SpawnCall`
/// carries no connection id), so closing the caller's connection leaves an
/// admitted invoke to finish; `record` then stores it once, and the permit
/// it consumed releases nothing more.
#[tokio::test]
async fn an_admitted_invoke_quiesces_and_records_exactly_once_after_its_connection_closes() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    // Succeeds: the reservation is bound to the invocation exactly once.
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
    let (params, permit) = admitted_invoke(&fx, &caller);
    assert_eq!(
        reservations_of(&fx, &caller, &params.idempotency_key),
        (0, 1)
    );
    fx.connection_closed(RUNTIME, 1);
    let _ = fx
        .callers
        .record(permit, &Ok(invocation_of("inv_quiesce", "wk_quiesce")));
    assert_eq!(
        reservations_of(&fx, &caller, &params.idempotency_key),
        (1, 0)
    );
    let recorded = |fx: &Fx| {
        fx.audit()
            .lines()
            .filter(|line| line.contains("\"spawn.invoke\"") && line.contains(r#""outcome":"ok""#))
            .count()
    };
    assert_eq!(recorded(&fx), 1);
    // Fails: the reservation is released exactly once.
    fx.bind(RUNTIME, 2, &caller).await.unwrap();
    let (failed, permit) = admitted_invoke(&fx, &caller);
    fx.connection_closed(RUNTIME, 2);
    let _ = fx.callers.record(permit, &failed_invoke());
    assert_eq!(
        reservations_of(&fx, &caller, &failed.idempotency_key),
        (0, 0)
    );
    // `record` consumed both permits; their drops changed nothing, and the
    // first invoke's slot is still bound.
    assert_eq!(
        reservations_of(&fx, &caller, &params.idempotency_key),
        (1, 0)
    );
    assert_eq!(recorded(&fx), 1);
    // A late call on either closed connection admits nothing new.
    let error = fx.bound_identity(RUNTIME, 1).await.unwrap_err();
    assert_typed_closed(&error);
}

/// Two admitted invokes of one key share its slot: the first ending
/// unbound, by a failure or a drop, leaves it for the second to bind.
#[tokio::test]
async fn a_duplicate_invoke_failing_or_dropped_first_leaves_the_slot_to_the_other() {
    for drop_first in [false, true] {
        let fx = Fx::new();
        let caller = fx.provision("flows").await;
        let params = fx.invoke_params(&caller);
        let key = params.idempotency_key.clone();
        let first = admitted_with(&fx, &caller, &params);
        let second = admitted_with(&fx, &caller, &params);
        assert_eq!(reservations_of(&fx, &caller, &key), (0, 1));
        if drop_first {
            drop(first);
        } else {
            let _ = fx.callers.record(first, &failed_invoke());
        }
        assert_eq!(reservations_of(&fx, &caller, &key), (0, 1), "{drop_first}");
        let _ = fx
            .callers
            .record(second, &Ok(invocation_of("inv_duplicate", "wk_duplicate")));
        assert_eq!(reservations_of(&fx, &caller, &key), (1, 0), "{drop_first}");
        let file = read_json(&inbox_file_of(&fx, &caller));
        assert!(file["slots"]
            .as_array()
            .unwrap()
            .iter()
            .any(|slot| slot["key"] == key.as_str() && slot["invocation_id"] == "inv_duplicate"));
    }
}

/// The slot goes once every admitted invoke of its key ends unbound.
#[tokio::test]
async fn duplicate_invokes_that_all_fail_or_drop_release_the_slot() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let params = fx.invoke_params(&caller);
    let key = params.idempotency_key.clone();
    let first = admitted_with(&fx, &caller, &params);
    let second = admitted_with(&fx, &caller, &params);
    let third = admitted_with(&fx, &caller, &params);
    let _ = fx.callers.record(first, &failed_invoke());
    drop(second);
    assert_eq!(reservations_of(&fx, &caller, &key), (0, 1));
    let _ = fx.callers.record(third, &failed_invoke());
    assert_eq!(reservations_of(&fx, &caller, &key), (0, 0));
}

/// Every file under the app's service-caller folder.
fn caller_artifacts(fx: &Fx) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&fx.path("app/ServiceCallers"), &mut out);
    out.sort();
    out
}

/// A dropped permit whose release write fails leaves its reservation on
/// disk once; the caller's next reservation writes the release first, so
/// the full budget is free again without waiting for `inbox::UNBOUND_MS`.
#[tokio::test]
async fn a_failed_release_is_written_at_the_next_reservation() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (params, permit) = admitted_invoke(&fx, &caller);
    assert_eq!(
        reservations_of(&fx, &caller, &params.idempotency_key),
        (0, 1)
    );
    super::inbox::FAIL_SAVE.with(|fail| fail.set(true));
    drop(permit);
    super::inbox::FAIL_SAVE.with(|fail| fail.set(false));
    // The write failed: the slot is still reserved on disk.
    assert_eq!(
        reservations_of(&fx, &caller, &params.idempotency_key),
        (0, 1)
    );
    fx.invoke(&caller).unwrap();
    assert_eq!(
        reservations_of(&fx, &caller, &params.idempotency_key),
        (0, 0)
    );
    // No time has passed and the whole budget is usable.
    for _ in 1..super::inbox::ACTIVE {
        for spawned in fx.world.spawned.lock().unwrap().iter_mut() {
            spawned.active = false;
        }
        fx.invoke(&caller).unwrap();
    }
}

/// A permit outliving its caller's revoke writes nothing when it ends,
/// dropped or recorded: the revoked inbox file stays gone.
#[tokio::test]
async fn a_late_drop_or_record_after_revoke_recreates_no_caller_file() {
    for record in [false, true] {
        let fx = Fx::new();
        let caller = fx.provision("flows").await;
        let (_, permit) = admitted_invoke(&fx, &caller);
        let inbox = inbox_file_of(&fx, &caller);
        assert!(inbox.exists());
        fx.call(APP, "spawn_callers.revoke", json!({"caller_id": caller.id}))
            .await
            .unwrap();
        assert!(!inbox.exists());
        let after_revoke = caller_artifacts(&fx);
        if record {
            let _ = fx
                .callers
                .record(permit, &Ok(invocation_of("inv_late", "wk_late")));
        } else {
            drop(permit);
        }
        assert!(!inbox.exists(), "record={record}");
        assert_eq!(caller_artifacts(&fx), after_revoke, "record={record}");
    }
}

/// How an admitted invoke's caller ends while the invoke is in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CallerEnd {
    Revoke,
    SignOut,
}

/// How the in-flight invoke's permit ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PermitEnd {
    /// The handler started the worker and the permit is recorded.
    RecordSuccess,
    /// The handler failed and the permit is recorded.
    RecordError,
    /// The handler was cancelled: the permit is dropped unrecorded.
    Drop,
}

/// Ends `caller` as `end` does in the app.
async fn end_caller(fx: &Fx, caller: &Caller, end: CallerEnd) {
    match end {
        CallerEnd::Revoke => {
            fx.call(APP, "spawn_callers.revoke", json!({"caller_id": caller.id}))
                .await
                .unwrap();
        }
        CallerEnd::SignOut => {
            fx.callers.forget(&account()).await;
            fx.world.signed_in.store(false, Ordering::SeqCst);
            fx.world.moves.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// Audit and log lines that name an invoke's terminal outcome for
/// `caller_id`.
fn invoke_lines(fx: &Fx, caller_id: &str) -> (usize, usize) {
    let audit = fx
        .audit()
        .lines()
        .filter(|line| line.contains(caller_id) && line.contains("\"spawn.invoke\""))
        .count();
    let logs = fx
        .world
        .logs
        .lock()
        .unwrap()
        .iter()
        .filter(|line| line.starts_with("spawn.invoke:"))
        .count();
    (audit, logs)
}

/// An invoke admitted before its caller was revoked or signed out may still
/// start a live worker. Whatever the handler did, and whether its permit is
/// recorded or dropped, the caller's files and tree stay gone, the worker
/// it started is released exactly once with the exact name and agent id,
/// and the call ends with exactly one terminal line: in the audit after a
/// revoke (the registry stays), in the host log after a sign-out (the tree
/// is gone and stays gone).
#[tokio::test]
async fn an_invoke_in_flight_across_revoke_or_sign_out_is_cleaned_up_exactly_once() {
    for end in [CallerEnd::Revoke, CallerEnd::SignOut] {
        for finish in [
            PermitEnd::RecordSuccess,
            PermitEnd::RecordError,
            PermitEnd::Drop,
        ] {
            let case = format!("{end:?}/{finish:?}");
            let fx = Fx::new();
            let caller = fx.provision("flows").await;
            let (params, permit) = admitted_invoke(&fx, &caller);
            // A duplicate of the same idempotency key, admitted alongside.
            let twin = admitted_with(&fx, &caller, &params);
            let inbox = inbox_file_of(&fx, &caller);
            assert!(inbox.exists(), "{case}");
            if finish == PermitEnd::RecordSuccess {
                // The core registered the worker the handler started.
                fx.world.spawned.lock().unwrap().push(Spawned {
                    agent_id: "wk_late".into(),
                    name: "worker".into(),
                    invocation_id: "inv_late".into(),
                    requester_id: Some(caller.agent_id.clone()),
                    process: None,
                    active: true,
                });
            }
            end_caller(&fx, &caller, end).await;
            assert!(!inbox.exists(), "{case}");
            let tree = fx.path("app/ServiceCallers");
            assert_eq!(tree.exists(), end == CallerEnd::Revoke, "{case}");
            let after_end = caller_artifacts(&fx);
            let lines_before = invoke_lines(&fx, &caller.id);
            match finish {
                PermitEnd::RecordSuccess => {
                    let late = invocation_of("inv_late", "wk_late");
                    let _ = fx.callers.record(permit, &Ok(late.clone()));
                    let _ = fx.callers.record(twin, &Ok(late));
                }
                PermitEnd::RecordError => {
                    let failed = || Err(RpcError::new(ErrorKind::Refused, "node_offline"));
                    let _ = fx.callers.record(permit, &failed());
                    let _ = fx.callers.record(twin, &failed());
                }
                PermitEnd::Drop => {
                    drop(permit);
                    drop(twin);
                }
            }
            // Every later tick, past every lookup and retry window.
            for _ in 0..3 {
                fx.callers.release_late().await;
                fx.advance(super::inbox::UNBOUND_MS);
            }
            fx.callers.release_late().await;
            // No file and no tree recreated.
            assert!(!inbox.exists(), "{case}");
            assert_eq!(tree.exists(), end == CallerEnd::Revoke, "{case}");
            assert_eq!(caller_artifacts(&fx), after_end, "{case}");
            // The worker released exactly once, exactly; nothing else.
            let relaycast = fx.world.relaycast.lock().unwrap();
            let expected: Vec<(String, String)> = match finish {
                PermitEnd::RecordSuccess => vec![("worker".into(), "wk_late".into())],
                _ => Vec::new(),
            };
            assert_eq!(relaycast.released, expected, "{case}");
            assert_eq!(relaycast.release_requests.len(), expected.len(), "{case}");
            drop(relaycast);
            // One terminal line per permit that has its own outcome; the
            // duplicate permit of the one invocation adds no second
            // cleanup.
            let (audit, logs) = invoke_lines(&fx, &caller.id);
            let new = (audit - lines_before.0, logs - lines_before.1);
            let terminal = match finish {
                PermitEnd::RecordSuccess => 1,
                PermitEnd::RecordError | PermitEnd::Drop => 2,
            };
            let expected = match end {
                CallerEnd::Revoke => (terminal, 0),
                CallerEnd::SignOut => (0, terminal),
            };
            assert_eq!(new, expected, "{case}");
            let shown = fx.everything_shown();
            let outcome = match finish {
                PermitEnd::RecordSuccess => "late_worker_released",
                PermitEnd::RecordError => "failed",
                PermitEnd::Drop => "dropped",
            };
            assert!(shown.contains(outcome), "{case}: {shown}");
            // Each permit's reserved claim ended.
            assert!(fx.callers.files().is_empty(), "{case}");
        }
    }
}

/// A late worker whose release fails is retried on the next tick and
/// released once; one the invoke reported without an agent id is found by
/// its exact invocation id among the core's spawns. Each ends with one
/// terminal line.
#[tokio::test]
async fn a_late_worker_release_is_retried_and_found_by_its_invocation_id() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, permit) = admitted_invoke(&fx, &caller);
    fx.call(APP, "spawn_callers.revoke", json!({"caller_id": caller.id}))
        .await
        .unwrap();
    fx.world.relaycast.lock().unwrap().failing_releases = 1;
    let mut pending = invocation_of("inv_late", "wk_late");
    pending.agent_id = None;
    pending.name = None;
    pending.status = "pending".into();
    pending.spawned = false;
    pending.ready = false;
    let _ = fx.callers.record(permit, &Ok(pending));
    // Not yet registered: nothing to release, nothing ended.
    fx.callers.release_late().await;
    assert!(fx
        .world
        .relaycast
        .lock()
        .unwrap()
        .release_requests
        .is_empty());
    assert_eq!(invoke_lines(&fx, &caller.id).0, 0);
    fx.world.spawned.lock().unwrap().push(Spawned {
        agent_id: "wk_late".into(),
        name: "worker-late".into(),
        invocation_id: "inv_late".into(),
        requester_id: Some(caller.agent_id.clone()),
        process: None,
        active: true,
    });
    // Found, and the first release fails: retried, not ended.
    fx.callers.release_late().await;
    assert_eq!(invoke_lines(&fx, &caller.id).0, 0);
    fx.callers.release_late().await;
    fx.callers.release_late().await;
    let relaycast = fx.world.relaycast.lock().unwrap();
    assert_eq!(relaycast.release_requests.len(), 2);
    assert!(relaycast
        .release_requests
        .iter()
        .all(|(name, agent, key)| name == "worker-late"
            && agent == "wk_late"
            && key == "service-caller-late-inv_late"));
    assert_eq!(
        relaycast.released,
        vec![("worker-late".to_string(), "wk_late".to_string())]
    );
    drop(relaycast);
    assert_eq!(invoke_lines(&fx, &caller.id).0, 1);
    assert!(fx.audit().contains("late_worker_released"));
}

/// A late worker still pending when the core goes down after its caller's
/// revoke is kept in the registry the revoke left: a cold core releases it
/// once, writes its one terminal line and forgets it.
#[tokio::test]
async fn a_late_worker_pending_at_revoke_is_released_after_restart() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, permit) = admitted_invoke(&fx, &caller);
    fx.call(APP, "spawn_callers.revoke", json!({"caller_id": caller.id}))
        .await
        .unwrap();
    let _ = fx
        .callers
        .record(permit, &Ok(invocation_of("inv_late", "wk_late")));
    let kept = read_json(&fx.registry_file())["late_workers"].clone();
    assert_eq!(kept.as_array().map(Vec::len), Some(1), "{kept}");
    assert_eq!(kept[0]["invocation_id"], "inv_late");
    assert_eq!(kept[0]["agent_id"], "wk_late");
    // The core goes down before any tick released it.
    let cold = fx.restart();
    drop(fx);
    assert!(cold
        .world
        .relaycast
        .lock()
        .unwrap()
        .release_requests
        .is_empty());
    cold.callers.release_late().await;
    let relaycast = cold.world.relaycast.lock().unwrap();
    assert_eq!(
        relaycast.released,
        vec![("worker".to_string(), "wk_late".to_string())]
    );
    assert_eq!(
        relaycast.release_requests,
        vec![(
            "worker".to_string(),
            "wk_late".to_string(),
            "service-caller-late-inv_late".to_string()
        )]
    );
    drop(relaycast);
    assert_eq!(invoke_lines(&cold, &caller.id).0, 1);
    assert!(cold.audit().contains("late_worker_released"));
    assert!(read_json(&cold.registry_file())
        .get("late_workers")
        .is_none());
    // A second cold core finds nothing left to release.
    let again = cold.restart();
    drop(cold);
    again.callers.release_late().await;
    assert_eq!(
        again.world.relaycast.lock().unwrap().release_requests.len(),
        1
    );
    assert_eq!(invoke_lines(&again, &caller.id).0, 1);
}

/// Regression by flows-fixer: a successful `spawn.invoke` whose
/// reservation bind cannot be written has the bind written at the caller's
/// next reservation, and the worker's report is kept and delivered once.
#[tokio::test]
async fn review_a_failed_successful_bind_is_written_at_the_next_reservation() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (params, permit) = admitted_invoke(&fx, &caller);
    fx.world.spawned.lock().unwrap().push(Spawned {
        agent_id: "wk_bind".into(),
        name: params.name.clone(),
        invocation_id: "inv_bind".into(),
        requester_id: Some(caller.agent_id.clone()),
        process: None,
        active: true,
    });
    super::inbox::FAIL_SAVE.with(|fail| fail.set(true));
    let _ = fx
        .callers
        .record(permit, &Ok(invocation_of("inv_bind", "wk_bind")));
    super::inbox::FAIL_SAVE.with(|fail| fail.set(false));
    assert_eq!(
        reservations_of(&fx, &caller, &params.idempotency_key),
        (0, 1)
    );
    // The next reservation writes the bind first.
    fx.invoke(&caller).unwrap();
    assert_eq!(
        reservations_of(&fx, &caller, &params.idempotency_key),
        (1, 0)
    );
    fx.report("wk_bind", &caller, "result");
    fx.deliver().await;
    fx.deliver().await;
    let iterator = fx.open(RUNTIME, &caller, "inv_bind").await.unwrap();
    fx.next(RUNTIME, &caller, &iterator).await.unwrap().unwrap();
    assert_eq!(
        fx.next_within(RUNTIME, &caller, &iterator, 10)
            .await
            .unwrap(),
        None
    );
}

/// The crash and restart variant of the previous test: a successful
/// `spawn.invoke` whose reservation bind cannot be written
/// still has its invocation and pending bind in the registry before its
/// claim ends: after a restart the bind is written, the worker's report is
/// kept and delivered exactly once.
#[tokio::test]
async fn a_failed_invoke_bind_is_written_after_restart_and_delivered_once() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (params, permit) = admitted_invoke(&fx, &caller);
    fx.world.spawned.lock().unwrap().push(Spawned {
        agent_id: "wk_bind".into(),
        name: params.name.clone(),
        invocation_id: "inv_bind".into(),
        requester_id: Some(caller.agent_id.clone()),
        process: None,
        active: true,
    });
    super::inbox::FAIL_SAVE.with(|fail| fail.set(true));
    let _ = fx
        .callers
        .record(permit, &Ok(invocation_of("inv_bind", "wk_bind")));
    super::inbox::FAIL_SAVE.with(|fail| fail.set(false));
    // The bind did not reach the disk; the registry names it.
    assert_eq!(
        reservations_of(&fx, &caller, &params.idempotency_key),
        (0, 1)
    );
    let entry = record_of(&fx, &caller.id).unwrap()["invocations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["invocation_id"] == "inv_bind")
        .cloned()
        .unwrap();
    assert_eq!(entry["slot_key"], params.idempotency_key.as_str());
    let cold = fx.restart();
    drop(fx);
    cold.report("wk_bind", &caller, "result");
    cold.deliver().await;
    assert_eq!(
        reservations_of(&cold, &caller, &params.idempotency_key),
        (1, 0)
    );
    let entry = record_of(&cold, &caller.id).unwrap()["invocations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["invocation_id"] == "inv_bind")
        .cloned()
        .unwrap();
    assert!(entry.get("slot_key").is_none(), "{entry}");
    let iterator = cold.open(RUNTIME, &caller, "inv_bind").await.unwrap();
    let delivery = cold
        .next(RUNTIME, &caller, &iterator)
        .await
        .unwrap()
        .unwrap();
    // Further polls and a further cold core add no second copy.
    cold.deliver().await;
    let again = cold.restart();
    drop(cold);
    again.deliver().await;
    assert_eq!(
        reservations_of(&again, &caller, &params.idempotency_key),
        (1, 0)
    );
    let iterator = again.open(RUNTIME2, &caller, "inv_bind").await.unwrap();
    let first = again
        .next(RUNTIME2, &caller, &iterator)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(item_id(&first), item_id(&delivery));
    assert_eq!(
        again
            .next_within(RUNTIME2, &caller, &iterator, 10)
            .await
            .unwrap(),
        None
    );
}

/// After `connection_closed(X)`, every service-caller call on X fails
/// closed as `connection_closed` before its params are read. `spawn.invoke`
/// is admitted per request in this pin and carries no connection id, so it
/// cannot name X; see the previous test.
#[tokio::test]
async fn a_call_after_connection_closed_fails_closed_without_side_effects() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (invocation, _) = fx.reported(&caller, "result").await;
    // Learn the item's id on connection 3, whose close releases its lease.
    fx.bind(RUNTIME, 3, &caller).await.unwrap();
    let peek = fx.bound_open(RUNTIME, 3, &invocation).await.unwrap();
    let delivery = fx.bound_next(RUNTIME, 3, &peek, 0).await.unwrap().unwrap();
    let item = item_id(&delivery);
    fx.connection_closed(RUNTIME, 3);
    // Connection 1: bound, with an open iterator, then closed.
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
    let iterator = fx.bound_open(RUNTIME, 1, &invocation).await.unwrap();
    fx.connection_closed(RUNTIME, 1);
    // Connection 4: never challenged, then closed. Only a dev-attest
    // preview build answers a phase-1 credential on it.
    let never_challenged = fx
        .call_on(RUNTIME, Some(4), "spawn_callers.identity", {
            let mut params = Fx::auth(&caller);
            params.insert("caller_name".into(), caller.name.clone().into());
            Value::Object(params)
        })
        .await;
    assert_eq!(
        never_challenged.is_ok(),
        cfg!(feature = "service-callers-dev-attest")
    );
    fx.connection_closed(RUNTIME, 4);
    let inbox_before = std::fs::read_to_string(inbox_file_of(&fx, &caller)).unwrap();
    let ledger_before = std::fs::read_to_string(
        fx.path("app/ServiceCallers")
            .join(format!("{}.json", super::registry::key(SITE, "ws_test"))),
    )
    .ok();
    let settle = |action: &str| {
        (
            format!("spawn_callers.inbox.{action}"),
            json!({"item_id": item, "metadata": {"relayflows_result": "journaled"}}),
        )
    };
    let mut legacy = Fx::auth(&caller);
    legacy.insert("iterator_id".into(), iterator.clone().into());
    legacy.insert("timeout_ms".into(), 0.into());
    let calls: Vec<(u64, String, Value)> = vec![
        (
            1,
            "spawn_callers.inbox.next".into(),
            json!({"iterator_id": iterator, "timeout_ms": 0}),
        ),
        (
            1,
            "spawn_callers.inbox.open".into(),
            json!({"invocation_id": invocation}),
        ),
        (1, settle("ack").0, settle("ack").1),
        (1, settle("fail").0, settle("fail").1),
        (
            1,
            "spawn_callers.inbox.close".into(),
            json!({"iterator_id": iterator}),
        ),
        (1, "spawn_callers.identity".into(), json!({})),
        (1, "spawn_callers.disconnect".into(), json!({})),
        (
            1,
            "spawn_callers.challenge".into(),
            json!({"caller_id": caller.id}),
        ),
        (
            1,
            "spawn_callers.authenticate".into(),
            json!({"challenge_id": format!("ch_{}", "0".repeat(32)), "authenticator": "A".repeat(43)}),
        ),
        (
            1,
            "spawn_callers.inbox.next".into(),
            Value::Object(legacy.clone()),
        ),
        (1, "spawn_callers.list".into(), json!({})),
        (4, "spawn_callers.inbox.next".into(), Value::Object(legacy)),
        (4, settle("ack").0, {
            let mut params = Fx::auth(&caller);
            params.insert("item_id".into(), item.clone().into());
            params.insert("metadata".into(), json!({"relayflows_result": "journaled"}));
            Value::Object(params)
        }),
        (4, "spawn_callers.list".into(), json!({})),
    ];
    for (conn, method, params) in calls {
        let legacy_refused =
            super::auth::has_legacy_keys(&params) && !cfg!(feature = "service-callers-dev-attest");
        let error = fx
            .call_on(RUNTIME, Some(conn), &method, params)
            .await
            .unwrap_err();
        if legacy_refused {
            assert_kind(&error, ErrorKind::Refused, "legacy_credential_refused");
            assert!(!error.data.retryable);
        } else {
            assert_typed_closed(&error);
        }
        assert_eq!(fx.callers.connection_state(conn), None, "{method}");
    }
    // Nothing moved: no lease, reservation or tombstone.
    assert_eq!(
        std::fs::read_to_string(inbox_file_of(&fx, &caller)).unwrap(),
        inbox_before
    );
    assert_eq!(
        std::fs::read_to_string(
            fx.path("app/ServiceCallers")
                .join(format!("{}.json", super::registry::key(SITE, "ws_test"))),
        )
        .ok(),
        ledger_before
    );
    assert_eq!(settled_on_disk(&fx, &caller, &item), Value::Null);
    fx.bind(RUNTIME, 2, &caller).await.unwrap();
    let fresh = fx.bound_open(RUNTIME, 2, &invocation).await.unwrap();
    let again = fx.bound_next(RUNTIME, 2, &fresh, 0).await.unwrap().unwrap();
    assert_eq!(item_id(&again), item);
}

/// `connection_closed` and `record` race from two threads; both finish in
/// bounded time, and each reservation is bound or released exactly once.
/// A params value that is not an object is answered `invalid_params` before
/// the binding decision; a closed connection still answers first.
#[tokio::test]
async fn a_non_object_params_value_beats_the_binding_decision() {
    let fx = Fx::new();
    let not_object = || json!("not an object");
    // (method, peer, connection, the refusal a well-formed `{}` still gets)
    let cases = [
        ("spawn_callers.identity", APP, Some(9), "caller_unbound"),
        ("spawn_callers.identity", APP, None, "caller_unbound"),
        ("spawn_callers.inbox.open", APP, Some(9), "caller_unbound"),
        ("spawn_callers.inbox.open", APP, None, "caller_unbound"),
        (
            "spawn_callers.challenge",
            RUNTIME,
            None,
            "binding_unavailable",
        ),
        ("spawn_callers.list", STRANGER, Some(9), "peer_uid_mismatch"),
        ("spawn_callers.list", STRANGER, None, "peer_uid_mismatch"),
    ];
    for (method, pid, connection, refusal) in cases {
        let error = fx
            .call_on(pid, connection, method, not_object())
            .await
            .unwrap_err();
        assert_eq!(
            error.data.kind,
            ErrorKind::InvalidParams,
            "{method} {connection:?}: {error:?}"
        );
        let error = fx
            .call_on(pid, connection, method, json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.message, refusal, "{method} {connection:?}: {error:?}");
    }
    // A challenge on a connection that exists answers the shape too.
    let error = fx
        .call_on(RUNTIME, Some(9), "spawn_callers.challenge", not_object())
        .await
        .unwrap_err();
    assert_eq!(error.data.kind, ErrorKind::InvalidParams, "{error:?}");
    // A closed connection still answers first.
    fx.connection_closed(RUNTIME, 11);
    for method in ["spawn_callers.identity", "spawn_callers.challenge"] {
        let error = fx
            .call_on(RUNTIME, Some(11), method, not_object())
            .await
            .unwrap_err();
        assert_typed_closed(&error);
    }
}

#[tokio::test]
async fn connection_closed_and_record_do_not_deadlock() {
    const ROUNDS: u64 = 40;
    let fx = Arc::new(Fx::new());
    let caller = fx.provision("flows").await;
    let (invocation, _) = fx.reported(&caller, "result").await;
    let mut keys = Vec::new();
    for round in 0..ROUNDS {
        let conn = 100 + round;
        fx.bind(RUNTIME, conn, &caller).await.unwrap();
        // An open iterator gives the close work under the live lock.
        fx.bound_open(RUNTIME, conn, &invocation).await.unwrap();
        let (params, permit) = admitted_invoke(&fx, &caller);
        let succeeds = round % 2 == 0;
        keys.push((params.idempotency_key.clone(), succeeds));
        let start = Arc::new(std::sync::Barrier::new(2));
        let (done, finished) = std::sync::mpsc::channel();
        {
            let (fx, start, done) = (fx.clone(), start.clone(), done.clone());
            std::thread::spawn(move || {
                start.wait();
                let outcome = if succeeds {
                    Ok(invocation_of(&format!("inv_race_{round}"), "wk_race"))
                } else {
                    failed_invoke()
                };
                let _ = fx.callers.record(permit, &outcome);
                done.send("record").unwrap();
            });
        }
        {
            let fx = fx.clone();
            std::thread::spawn(move || {
                start.wait();
                fx.connection_closed(RUNTIME, conn);
                done.send("close").unwrap();
            });
        }
        for _ in 0..2 {
            finished
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap_or_else(|_| panic!("round {round}: record or close did not finish"));
        }
        fx.advance(1_000);
    }
    for (key, succeeds) in keys {
        let expected = if succeeds { (1, 0) } else { (0, 0) };
        assert_eq!(reservations_of(&fx, &caller, &key), expected, "{key}");
    }
}

/// Where the releases a sign-out left pending are kept, beside the tree.
fn releases_file(fx: &Fx) -> PathBuf {
    fx.path("app").join(super::releases::FILE)
}

/// A worker an invoke started across a sign-out is kept in the releases
/// file beside the deleted tree: a cold start, still signed out, releases
/// it once with the idempotent key, and a second cold start does nothing.
#[tokio::test]
async fn a_late_worker_pending_at_sign_out_is_released_once_after_restart() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, permit) = admitted_invoke(&fx, &caller);
    end_caller(&fx, &caller, CallerEnd::SignOut).await;
    let _ = fx
        .callers
        .record(permit, &Ok(invocation_of("inv_out", "wk_out")));
    assert!(!fx.path("app/ServiceCallers").exists());
    let kept = read_json(&releases_file(&fx));
    assert_eq!(kept["releases"].as_array().map(Vec::len), Some(1), "{kept}");
    assert_eq!(kept["releases"][0]["invocation_id"], "inv_out");
    // The core goes down before any tick released it.
    let cold = fx.restart();
    drop(fx);
    assert!(!cold.world.signed_in.load(Ordering::SeqCst));
    cold.callers.release_late().await;
    let requests = cold
        .world
        .relaycast
        .lock()
        .unwrap()
        .release_requests
        .clone();
    assert_eq!(
        requests,
        vec![(
            "worker".to_string(),
            "wk_out".to_string(),
            "service-caller-late-inv_out".to_string()
        )]
    );
    assert_eq!(invoke_lines(&cold, &caller.id), (0, 1));
    assert!(!releases_file(&cold).exists());
    assert!(!cold.path("app/ServiceCallers").exists());
    // A later tick and a second cold start find nothing left.
    cold.callers.release_late().await;
    let again = cold.restart();
    drop(cold);
    again.callers.release_late().await;
    assert_eq!(
        again.world.relaycast.lock().unwrap().release_requests.len(),
        1
    );
    assert_eq!(invoke_lines(&again, &caller.id), (0, 1));
    assert!(!again.everything_shown().contains(WORKSPACE_KEY));
}

/// A late worker a revoke kept in the registry, still pending when the
/// person signs out, moves to the releases file before the tree goes; a
/// cold start releases it exactly once.
#[tokio::test]
async fn a_late_worker_kept_at_revoke_survives_sign_out_and_restart() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, permit) = admitted_invoke(&fx, &caller);
    fx.call(APP, "spawn_callers.revoke", json!({"caller_id": caller.id}))
        .await
        .unwrap();
    let _ = fx
        .callers
        .record(permit, &Ok(invocation_of("inv_late", "wk_late")));
    assert!(read_json(&fx.registry_file())["late_workers"].is_array());
    // The release fails while signed in, so it is still pending.
    fx.world.relaycast.lock().unwrap().failing_releases = 1;
    fx.callers.release_late().await;
    assert!(fx.world.relaycast.lock().unwrap().released.is_empty());
    end_caller(&fx, &caller, CallerEnd::SignOut).await;
    assert!(!fx.path("app/ServiceCallers").exists());
    let kept = read_json(&releases_file(&fx));
    assert_eq!(kept["releases"].as_array().map(Vec::len), Some(1), "{kept}");
    let cold = fx.restart();
    drop(fx);
    cold.callers.release_late().await;
    cold.callers.release_late().await;
    let (released, requests) = {
        let relaycast = cold.world.relaycast.lock().unwrap();
        (
            relaycast.released.clone(),
            relaycast.release_requests.clone(),
        )
    };
    assert_eq!(
        released,
        vec![("worker".to_string(), "wk_late".to_string())]
    );
    assert!(requests
        .iter()
        .all(|(_, _, key)| key == "service-caller-late-inv_late"));
    assert!(!releases_file(&cold).exists());
    let again = cold.restart();
    drop(cold);
    let before = again.world.relaycast.lock().unwrap().release_requests.len();
    again.callers.release_late().await;
    assert_eq!(
        again.world.relaycast.lock().unwrap().release_requests.len(),
        before
    );
}

/// The releases file is 0600 and holds the workspace key only while a
/// release is pending.
#[cfg(unix)]
#[tokio::test]
async fn the_releases_file_is_private_and_deleted_once_empty() {
    use std::os::unix::fs::PermissionsExt;
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, permit) = admitted_invoke(&fx, &caller);
    end_caller(&fx, &caller, CallerEnd::SignOut).await;
    let _ = fx
        .callers
        .record(permit, &Ok(invocation_of("inv_p", "wk_p")));
    let mode = std::fs::metadata(releases_file(&fx))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    fx.callers.release_late().await;
    assert!(!releases_file(&fx).exists());
}

/// A provisioned caller's file is schema `/2`, with the auth route, the
/// scope and binding fields and the result-inbox route, and reads back.
#[tokio::test]
async fn caller_file_v2_round_trips() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let raw = read_json(&caller.handoff);
    assert_eq!(raw["schema"], "agent-relay-service-caller/2");
    assert_eq!(
        raw["auth"],
        json!({
            "scheme": "challenge-hmac-sha256/v1",
            "challenge": "spawn_callers.challenge",
            "authenticate": "spawn_callers.authenticate",
        })
    );
    assert_eq!(
        raw["inbox"],
        json!({
            "transport": "core-bridge",
            "open": "spawn_callers.inbox.open",
            "next": "spawn_callers.inbox.next",
            "ack": "spawn_callers.inbox.ack",
            "fail": "spawn_callers.inbox.fail",
            "close": "spawn_callers.inbox.close",
            "disconnect": "spawn_callers.disconnect",
        })
    );
    let mut keys: Vec<&str> = raw
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "account",
            "auth",
            "caller_id",
            "caller_name",
            "created_at",
            "desktop_version",
            "directory_scope",
            "expires_at",
            "harnesses",
            "inbox",
            "owns_only",
            "publisher",
            "schema",
            "secret",
            "socket",
            "workspace",
        ]
    );
    let file = super::handoff::read(&caller.handoff).unwrap();
    assert_eq!(file.caller_id, caller.id);
    assert_eq!(file.caller_name, caller.name);
    assert_eq!(file.secret, caller.secret);
    assert_eq!(file.publisher.id, caller.agent_id);
    assert_eq!(file.workspace.site_url, SITE);
    assert_eq!(file.workspace.base_url, BASE_URL);
    assert_eq!(file.workspace.workspace_id, "ws_test");
    assert_eq!(file.workspace.relaycast_workspace_id, "rw_test");
    assert_eq!(file.account.user_id, "user_test");
    assert_eq!(file.harnesses, vec!["claude".to_string()]);
    assert_eq!(raw["owns_only"], file.owns_only);
    assert_eq!(file.socket, raw["socket"].as_str().unwrap());
    // Serialized again, it is the same file.
    let again: Value = serde_json::to_value(&file).unwrap();
    assert_eq!(again, raw);
    assert_eq!(
        super::handoff::parse(&serde_json::to_vec(&again).unwrap()).unwrap(),
        file
    );
}

/// A `/1` file is refused with a clear error, never upgraded, and holds no
/// usable secret: the challenge path ignores it.
#[tokio::test]
async fn caller_file_v1_is_refused() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let mut raw = read_json(&caller.handoff);
    raw["schema"] = "agent-relay-service-caller/1".into();
    raw.as_object_mut().unwrap().remove("auth");
    raw.as_object_mut().unwrap().remove("inbox");
    raw.as_object_mut().unwrap().remove("owns_only");
    let bytes = serde_json::to_vec_pretty(&raw).unwrap();
    let error = super::handoff::parse(&bytes).unwrap_err();
    assert_eq!(error, super::handoff::CallerFileError::RetiredSchema);
    assert!(error
        .to_string()
        .contains("agent-relay-service-caller/1 is no longer accepted"));
    // The same bytes on disk: refused the same way, and left as they were.
    agent_relay_node::platform::fs::write_private(&caller.handoff, &bytes).unwrap();
    assert_eq!(
        super::handoff::read(&caller.handoff).unwrap_err(),
        super::handoff::CallerFileError::RetiredSchema
    );
    assert_eq!(std::fs::read(&caller.handoff).unwrap(), bytes);
    // Unknown schema, unknown auth, a missing field and an extra one.
    let mut other = read_json(&fx.provision("other").await.handoff);
    let cases: Vec<(Value, super::handoff::CallerFileError)> = vec![
        (
            {
                let mut v = other.clone();
                v["schema"] = "agent-relay-service-caller/3".into();
                v
            },
            super::handoff::CallerFileError::UnknownSchema,
        ),
        (
            {
                let mut v = other.clone();
                v["auth"]["scheme"] = "plain-secret".into();
                v
            },
            super::handoff::CallerFileError::UnknownAuth,
        ),
        (
            {
                let mut v = other.clone();
                v.as_object_mut().unwrap().remove("auth");
                v
            },
            super::handoff::CallerFileError::Malformed,
        ),
        (
            {
                let mut v = other.clone();
                v["caller_secret"] = v["secret"].clone();
                v
            },
            super::handoff::CallerFileError::Malformed,
        ),
    ];
    for (value, expected) in cases {
        assert_eq!(
            super::handoff::parse(&serde_json::to_vec(&value).unwrap()).unwrap_err(),
            expected,
            "{value}"
        );
    }
    other["schema"] = Value::Null;
    assert_eq!(
        super::handoff::parse(&serde_json::to_vec(&other).unwrap()).unwrap_err(),
        super::handoff::CallerFileError::Malformed
    );
}

/// The secret is in no error, no `Debug` and no log or audit line, for a
/// good file and for every refused one.
#[tokio::test]
async fn caller_file_secret_is_never_formatted() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let file = super::handoff::read(&caller.handoff).unwrap();
    assert!(!format!("{file:?}").contains(&caller.secret));
    assert!(!format!("{file:#?}").contains(&caller.secret));
    assert!(format!("{file:?}").contains("<redacted>"));
    let raw = read_json(&caller.handoff);
    let mut variants = Vec::new();
    for schema in [
        "agent-relay-service-caller/1",
        "agent-relay-service-caller/9",
    ] {
        let mut v = raw.clone();
        v["schema"] = schema.into();
        variants.push(serde_json::to_vec(&v).unwrap());
    }
    // A wrong type right next to the secret, and the secret where a
    // number belongs: serde would name the value; the reader does not.
    let mut v = raw.clone();
    v["owns_only"] = v["secret"].clone();
    variants.push(serde_json::to_vec(&v).unwrap());
    let mut v = raw.clone();
    v["auth"]["scheme"] = v["secret"].clone();
    variants.push(serde_json::to_vec(&v).unwrap());
    variants.push(format!("{{\"schema\": {}", raw["secret"]).into_bytes());
    for bytes in variants {
        let error = super::handoff::parse(&bytes).unwrap_err();
        for shown in [
            error.to_string(),
            format!("{error:?}"),
            format!("{error:#?}"),
        ] {
            assert!(!shown.contains(&caller.secret), "{shown}");
        }
    }
    // A rotation reads the file through the same reader.
    let rotated = fx
        .call(APP, "spawn_callers.rotate", json!({"caller_id": caller.id}))
        .await
        .unwrap();
    assert!(!rotated.to_string().contains(&caller.secret));
    assert!(!fx.everything_shown().contains(&caller.secret));
}

/// `SpawnCallers::connection`: a connection never challenged takes the
/// session path, a bound one resolves to its caller, and one only
/// challenged, or closed after binding, is refused with its class.
#[tokio::test]
async fn the_core_resolves_a_connection_by_its_binding_only() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let resolve = |connection_id| {
        fx.callers.connection(&CallContext {
            peer_pid: Some(RUNTIME),
            connection_id,
        })
    };
    assert!(matches!(resolve(None), ConnectionCaller::NotExternal));
    assert!(matches!(resolve(Some(7)), ConnectionCaller::NotExternal));
    fx.challenge_on(RUNTIME, 8, &caller.id).await.unwrap();
    match resolve(Some(8)) {
        ConnectionCaller::Refused(error) => {
            assert_refused(&error, RefusalReason::ServiceUnknown, "caller_unbound")
        }
        other => panic!("a challenged connection resolved as {other:?}"),
    }
    fx.bind(RUNTIME, 9, &caller).await.unwrap();
    match resolve(Some(9)) {
        ConnectionCaller::Bound(resolved) => assert_eq!(resolved.agent_id, caller.agent_id),
        other => panic!("a bound connection resolved as {other:?}"),
    }
    fx.connection_closed(RUNTIME, 9);
    match resolve(Some(9)) {
        ConnectionCaller::Refused(error) => {
            assert_kind(&error, ErrorKind::Refused, "connection_closed")
        }
        other => panic!("a closed connection resolved as {other:?}"),
    }
}

// --- Review fixes: Windows policy, in-memory connection, ledger ---------

/// How the core resolves the bridge connection `connection` from `pid`.
fn connection_of(fx: &Fx, pid: i32, connection: u64) -> Result<ExternalCaller, RpcError> {
    let context = CallContext {
        peer_pid: Some(pid),
        connection_id: Some(connection),
    };
    match fx.callers.connection(&context) {
        ConnectionCaller::Bound(resolved) => Ok(resolved),
        ConnectionCaller::Refused(error) => Err(error),
        ConnectionCaller::NotExternal => panic!("a bound connection resolved as a session"),
    }
}

/// The binding the fixture's core is under.
fn binding_of(fx: &Fx) -> super::registry::Binding {
    let host = FakeHost {
        world: fx.world.clone(),
        generation: AtomicU64::new(0),
    };
    super::registry::Binding::current(&host).unwrap()
}

/// Where the peer check is unavailable (Windows), every service-caller
/// method refuses `peer_check_unavailable` before anything else, the
/// challenge and authentication included, and a connection bound before
/// resolves to nothing.
#[tokio::test]
async fn without_a_peer_check_every_method_and_connection_is_peer_check_unavailable() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    fx.bind(RUNTIME, 1, &caller).await.unwrap();
    let (_, connection) = fx.resolve_on(RUNTIME, &caller).unwrap();
    let challenge = fx.challenge_on(RUNTIME, 2, &caller.id).await.unwrap();
    let before = fx.requests();
    super::peer::UNAVAILABLE.with(|unavailable| unavailable.set(true));
    let errors = vec![
        fx.challenge_on(RUNTIME, 3, &caller.id).await.unwrap_err(),
        fx.authenticate_on(RUNTIME, 2, &challenge, &answer(&caller, &challenge))
            .await
            .unwrap_err(),
        fx.bound_identity(RUNTIME, 1).await.unwrap_err(),
        fx.identity(RUNTIME, &caller).await.unwrap_err(),
        fx.call(APP, "spawn_callers.list", json!({}))
            .await
            .unwrap_err(),
        fx.call(PROBE, "spawn_callers.list", json!({}))
            .await
            .unwrap_err(),
        connection_of(&fx, RUNTIME, connection).unwrap_err(),
    ];
    super::peer::UNAVAILABLE.with(|unavailable| unavailable.set(false));
    for error in &errors {
        assert_refused(
            error,
            RefusalReason::ManagementPeer,
            "peer_check_unavailable",
        );
    }
    assert_eq!(fx.requests(), before);
    // The policy is the platform's, not a state: once a peer check exists
    // the same connection resolves again.
    assert!(connection_of(&fx, RUNTIME, connection).is_ok());
}

/// A successful invoke whose identity generation moved while it ran, the
/// account and workspace unchanged, keeps its worker: the ledger owns it,
/// the receipt stands, and a fresh binding can read and release it.
#[tokio::test]
async fn an_invoke_that_outlives_a_generation_move_keeps_its_worker_owned() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, permit) = admitted_invoke(&fx, &caller);
    fx.world.moves.fetch_add(1, Ordering::SeqCst);
    fx.callers
        .record(permit, &Ok(invocation_of("inv_moved", "wk_moved")))
        .unwrap();
    let ledger = record_of(&fx, &caller.id).unwrap()["invocations"].clone();
    assert!(
        ledger
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["invocation_id"] == "inv_moved"
                && entry["worker_agent_id"] == "wk_moved"),
        "{ledger}"
    );
    assert!(fx
        .audit()
        .contains(r#""method":"spawn.invoke","outcome":"generation_moved""#));
    fx.status(&caller, "inv_moved").unwrap();
    fx.release(&caller, "wk_moved").unwrap();
    assert!(fx.open(RUNTIME, &caller, "inv_moved").await.is_ok());
}

/// `connection` resolves from memory: it reads no registry, so a registry
/// that has become unreadable since the bind does not block or refuse it.
#[tokio::test]
async fn connection_resolves_without_reading_the_registry() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, connection) = fx.resolve_on(RUNTIME, &caller).unwrap();
    let loads = || super::registry::LOADS.with(std::cell::Cell::get);
    let start = loads();
    for _ in 0..3 {
        assert!(connection_of(&fx, RUNTIME, connection).is_ok());
    }
    assert_eq!(loads(), start);
    let bytes = std::fs::read(fx.registry_file()).unwrap();
    std::fs::write(fx.registry_file(), b"not json").unwrap();
    let resolved = connection_of(&fx, RUNTIME, connection).unwrap();
    assert_eq!(resolved.agent_id, caller.agent_id);
    assert_eq!(loads(), start);
    std::fs::write(fx.registry_file(), bytes).unwrap();
}

/// What the app writes reaches the next `connection` from memory: a
/// revoked or disabled caller is refused on its very next bound call.
#[tokio::test]
async fn a_revoke_or_disable_is_honoured_on_the_next_bound_call() {
    let fx = Fx::new();
    let revoked = fx.provision("revoked").await;
    let disabled = fx.provision("disabled").await;
    let (_, revoked_on) = fx.resolve_on(RUNTIME, &revoked).unwrap();
    let (_, disabled_on) = fx.resolve_on(RUNTIME2, &disabled).unwrap();
    fx.call(
        APP,
        "spawn_callers.revoke",
        json!({"caller_id": revoked.id}),
    )
    .await
    .unwrap();
    fx.call(
        APP,
        "spawn_callers.set_enabled",
        json!({"caller_id": disabled.id, "enabled": false}),
    )
    .await
    .unwrap();
    let loads = || super::registry::LOADS.with(std::cell::Cell::get);
    let start = loads();
    let error = connection_of(&fx, RUNTIME, revoked_on).unwrap_err();
    assert_refused(&error, RefusalReason::ServiceExpired, "binding_stale");
    let error = connection_of(&fx, RUNTIME2, disabled_on).unwrap_err();
    assert_refused(&error, RefusalReason::ServiceExpired, "binding_stale");
    assert_eq!(loads(), start);
}

/// An expiry or a retirement the clock makes due, with no write yet to
/// record it, holds on the next bound call all the same.
#[tokio::test]
async fn an_expiry_or_retirement_due_by_the_clock_is_honoured_without_a_write() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (_, connection) = fx.resolve_on(RUNTIME, &caller).unwrap();
    let expires = record_of(&fx, &caller.id).unwrap()["expires_at_ms"]
        .as_i64()
        .unwrap();
    fx.advance(expires - fx.now());
    let error = connection_of(&fx, RUNTIME, connection).unwrap_err();
    assert_refused(&error, RefusalReason::ServiceExpired, "binding_stale");

    // A rotation stales every binding at once; one bound again with the
    // old secret during the overlap holds until the old secret retires.
    let fx = Fx::new();
    let old = fx.provision("rotated").await;
    fx.call(
        APP,
        "spawn_callers.rotate",
        json!({"caller_id": old.id, "overlap_seconds": 60}),
    )
    .await
    .unwrap();
    let (_, connection) = fx.resolve_on(RUNTIME, &old).unwrap();
    assert!(connection_of(&fx, RUNTIME, connection).is_ok());
    fx.advance(60_000);
    let error = connection_of(&fx, RUNTIME, connection).unwrap_err();
    assert_refused(&error, RefusalReason::ServiceExpired, "binding_stale");
}

/// A spawn whose record cannot be written answers a retryable
/// `store_failed`, audited, and the same request again replays the
/// invocation Relaycast already made and recovers its receipt.
#[tokio::test]
async fn a_spawn_whose_record_fails_to_store_is_gated_and_replayed_by_the_same_request() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let params = fx.invoke_params(&caller);
    let permit = admitted_with(&fx, &caller, &params);
    super::registry::FAIL_SAVE.with(|fail| fail.set(true));
    let outcome = fx
        .callers
        .record(permit, &Ok(invocation_of("inv_gated", "wk_gated")));
    super::registry::FAIL_SAVE.with(|fail| fail.set(false));
    let error = outcome.unwrap_err();
    assert_kind(&error, ErrorKind::StoreFailed, "store_failed");
    assert!(fx
        .audit()
        .contains(r#""method":"spawn.invoke","outcome":"store_failed""#));
    let owned = |fx: &Fx| {
        record_of(fx, &caller.id).unwrap()["invocations"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["invocation_id"] == "inv_gated")
            .count()
    };
    assert_eq!(owned(&fx), 0);
    // The same request again: Relaycast replays the same invocation for
    // the same idempotency key, and this time the record is kept.
    let permit = admitted_with(&fx, &caller, &params);
    fx.callers
        .record(permit, &Ok(invocation_of("inv_gated", "wk_gated")))
        .unwrap();
    assert_eq!(owned(&fx), 1);
    fx.status(&caller, "inv_gated").unwrap();
}

/// A root may not span the home folder or Desktop's own files, and a spawn
/// directory inside the app data folder is refused even under a root.
#[test]
fn a_root_spanning_home_or_app_data_and_a_directory_in_app_data_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let base = std::fs::canonicalize(dir.path()).unwrap();
    let app_data = base.join("data/app");
    let home = base.join("users/me");
    let work = base.join("users/me/work");
    for folder in [&app_data, &work] {
        std::fs::create_dir_all(folder).unwrap();
    }
    let roots =
        |given: &Path| super::scope::roots(&[given.display().to_string()], Some(&home), &app_data);
    for refused in [
        &base,
        &base.join("users"),
        &home,
        &base.join("data"),
        &app_data,
    ] {
        assert_eq!(
            roots(refused).err(),
            Some(Class::DirectoryOutside),
            "{}",
            refused.display()
        );
    }
    assert!(roots(&work).is_ok());
    let wide = vec![super::registry::Root {
        given: base.join("data").display().to_string(),
        resolved: base.join("data"),
    }];
    let inside = app_data.join("ServiceCallers");
    std::fs::create_dir_all(&inside).unwrap();
    for refused in [&app_data, &inside] {
        assert_eq!(
            super::scope::directory(&refused.display().to_string(), &wide, &app_data).err(),
            Some(Class::DirectoryOutside)
        );
    }
    std::fs::create_dir_all(base.join("data/other")).unwrap();
    assert!(super::scope::directory(
        &base.join("data/other").display().to_string(),
        &wide,
        &app_data
    )
    .is_ok());
}

/// A caller record of `fx`'s with a full ledger of `ids`, oldest first.
fn full_ledger(fx: &Fx, caller: &Caller, ids: &[String]) -> super::registry::CallerRecord {
    let mut record: super::registry::CallerRecord =
        serde_json::from_value(record_of(fx, &caller.id).unwrap()).unwrap();
    record.invocations = ids
        .iter()
        .map(|id| super::registry::LedgerEntry {
            invocation_id: id.clone(),
            worker_agent_id: None,
            slot_key: None,
        })
        .collect();
    record
}

/// Past the ledger's capacity the oldest settled entry goes first; an
/// unsettled one is evicted only when every entry is unsettled, and then
/// its id is handed back so its slot is dropped.
#[tokio::test]
async fn the_ledger_evicts_settled_entries_before_unsettled_ones() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let ids: Vec<String> = (0..super::registry::LEDGER)
        .map(|n| format!("inv_{n}"))
        .collect();
    let mut record = full_ledger(&fx, &caller, &ids);
    let dropped = record.remember("inv_new", None, |id| id == "inv_0" || id == "inv_1");
    assert!(dropped.is_empty());
    let kept: Vec<&str> = record
        .invocations
        .iter()
        .map(|entry| entry.invocation_id.as_str())
        .collect();
    assert_eq!(kept.len(), super::registry::LEDGER);
    assert_eq!(&kept[..3], ["inv_0", "inv_1", "inv_3"]);
    assert_eq!(kept.last(), Some(&"inv_new"));

    let mut record = full_ledger(&fx, &caller, &ids);
    let dropped = record.remember("inv_new", None, |_| true);
    assert_eq!(dropped, ["inv_0"]);
    assert_eq!(record.invocations.len(), super::registry::LEDGER);
    assert_eq!(record.invocations[0].invocation_id, "inv_1");
}

/// An invocation the ledger evicts takes its inbox slot with it, so an
/// unsettled slot nothing can open or settle any more does not hold one
/// of the caller's active slots for good.
#[tokio::test]
async fn an_evicted_invocations_inbox_slot_is_dropped() {
    let fx = Fx::new();
    let caller = fx.provision("flows").await;
    let (evicted, _) = fx.reported(&caller, "first").await;
    let (kept, _) = fx.reported(&caller, "second").await;
    let binding = binding_of(&fx);
    let unsettled = super::inbox::unsettled(&binding, &caller.id);
    assert!(unsettled.contains(&evicted) && unsettled.contains(&kept));
    super::inbox::drop_invocations(&binding, &caller.id, &[evicted.clone()]);
    let unsettled = super::inbox::unsettled(&binding, &caller.id);
    assert!(!unsettled.contains(&evicted) && unsettled.contains(&kept));
    // Through the ledger: a full ledger of unsettled entries evicts the
    // oldest and drops its slot as the invoke is recorded.
    let mut file: Value = read_json(&fx.registry_file());
    let index = file["callers"]
        .as_array()
        .unwrap()
        .iter()
        .position(|record| record["caller_id"] == caller.id.as_str())
        .unwrap();
    let mut ledger = vec![json!({"invocation_id": kept})];
    for n in 1..super::registry::LEDGER {
        ledger.push(json!({"invocation_id": format!("inv_old_{n}")}));
    }
    file["callers"][index]["invocations"] = Value::Array(ledger);
    std::fs::write(fx.registry_file(), serde_json::to_vec(&file).unwrap()).unwrap();
    let cold = fx.restart();
    // Every older entry settled: `inv_old_1` goes, the unsettled one stays.
    cold.invoke(&caller).unwrap();
    let ledger = record_of(&cold, &caller.id).unwrap()["invocations"].clone();
    let ids: Vec<&str> = ledger
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["invocation_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), super::registry::LEDGER);
    assert_eq!(&ids[..2], [kept.as_str(), "inv_old_2"]);
    assert!(super::inbox::unsettled(&binding, &caller.id).contains(&kept));
}
