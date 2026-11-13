//! What the service callers read from the core they serve, behind a trait
//! so the tests can stand in for the core, its processes, its clock and
//! Relaycast.

use agent_relay_node::relay::cloud_session::CloudAccount;
use agent_relay_node::relay::daemon::Core;
use agent_relay_node::relay::process::ProcessInfo;
use agent_relay_node::relay::protocol::{ErrorKind, RpcError};
use agent_relay_node::relay::workspace::{Secret, Workspace};
use futures_util::future::BoxFuture;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, Weak};

// The preview bypass below must never reach a person: an optimized build
// with it on does not compile.
#[cfg(all(feature = "service-callers-dev-attest", not(debug_assertions)))]
compile_error!(
    "service-callers-dev-attest is a preview-only bypass of the service-caller app \
     attestation (every same-uid local peer counts as the app). It builds only in the \
     debug profile and must never be in a release build or a release diff."
);

/// The loud startup line of a core built with `service-callers-dev-attest`.
#[cfg(feature = "service-callers-dev-attest")]
pub(crate) const DEV_ATTEST_WARNING: &str = "DANGEROUS PREVIEW: service-callers-dev-attest is \
     enabled; every same-uid local peer is treated as the app. Never ship this build.";

/// A spawned agent this computer keeps a registration for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spawned {
    pub agent_id: String,
    pub name: String,
    pub invocation_id: String,
    /// The requester Relaycast recorded for the spawn, by immutable id.
    pub requester_id: Option<String>,
    /// The spawned agent's own process, when it has one.
    pub process: Option<(i32, u64)>,
    /// Neither archived nor missing.
    pub active: bool,
}

/// One message delivered to a caller's publisher identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Incoming {
    pub delivery_id: String,
    pub message_id: String,
    pub sender_id: Option<String>,
    pub sender_name: Option<String>,
    pub text: String,
    pub created_at_ms: Option<i64>,
}

/// A Relaycast agent registered for a caller.
pub struct Registered {
    pub id: String,
    pub name: String,
    pub token: Secret,
}

/// What the operating system says about a management peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Attestation {
    /// This app's own signed code, attested by the operating system.
    App,
    /// Attested, and not this app.
    NotApp,
    /// This platform cannot attest a peer.
    Unavailable,
}

/// The core as the service callers see it. Every Relaycast request goes
/// through here, so a test can tell that none was made.
pub trait Host: Send + Sync {
    fn app_data_dir(&self) -> Option<PathBuf>;
    /// The person's home folder.
    fn home(&self) -> Option<PathBuf>;
    fn account(&self) -> Option<CloudAccount>;
    /// The account's identity generation (`CloudAccess::identity_sync_generation`).
    fn generation(&self) -> u64;
    fn workspace(&self) -> Option<Workspace>;
    fn machine_prefix(&self) -> Option<String>;
    fn version(&self) -> String;
    /// The bridge endpoint a caller connects to.
    fn socket(&self) -> String;
    fn process(&self, pid: i32) -> Option<ProcessInfo>;
    fn current_uid(&self) -> u32;
    /// Whether `pid` runs inside a session, or its session cannot be read.
    fn in_session(&self, pid: i32) -> bool;
    /// Whether the operating system attests `pid` as this app's own signed
    /// code. A file or process name is never evidence: only an attestation
    /// the peer cannot forge answers `App`.
    fn attest_app_peer(&self, pid: i32) -> Attestation;
    /// Spawned agents registered in `workspace`.
    fn spawned(&self, workspace: &Workspace) -> Vec<Spawned>;
    /// Every registration name this computer keeps.
    fn registration_names(&self) -> Vec<String>;
    /// Milliseconds since the epoch.
    fn now_ms(&self) -> i64;
    /// A diagnostic line. Never given a secret, hash, token or text.
    fn log(&self, line: &str);
    fn register_agent<'a>(
        &'a self,
        workspace: &'a Workspace,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Registered, RpcError>>;
    fn delete_agent<'a>(
        &'a self,
        workspace: &'a Workspace,
        name: &'a str,
    ) -> BoxFuture<'a, Result<(), RpcError>>;
    /// Releases exactly the worker `name` whose agent id is `agent_id`,
    /// with the workspace key: the late cleanup of a worker started for a
    /// caller that a revoke or sign-out ended while its invoke was in
    /// flight. `idempotency_key` makes a retry the same request.
    fn release_worker<'a>(
        &'a self,
        workspace: &'a Workspace,
        name: &'a str,
        agent_id: &'a str,
        idempotency_key: &'a str,
    ) -> BoxFuture<'a, Result<(), RpcError>>;
    fn deliveries<'a>(
        &'a self,
        base_url: &'a str,
        token: &'a Secret,
    ) -> BoxFuture<'a, Result<Vec<Incoming>, RpcError>>;
    fn ack<'a>(
        &'a self,
        base_url: &'a str,
        token: &'a Secret,
        delivery_id: &'a str,
    ) -> BoxFuture<'a, Result<(), RpcError>>;
}

/// The real core, once the account is attached to it.
pub struct CoreHost {
    core: OnceLock<Weak<Core>>,
}

impl Default for CoreHost {
    fn default() -> Self {
        let host = CoreHost {
            core: OnceLock::new(),
        };
        #[cfg(feature = "service-callers-dev-attest")]
        host.log(DEV_ATTEST_WARNING);
        host
    }
}

impl CoreHost {
    pub(crate) fn attach(&self, core: &Arc<Core>) {
        let _ = self.core.set(Arc::downgrade(core));
    }

    fn core(&self) -> Option<Arc<Core>> {
        self.core.get()?.upgrade()
    }
}

impl Host for CoreHost {
    fn app_data_dir(&self) -> Option<PathBuf> {
        Some(self.core()?.config().app_data_dir.clone())
    }

    fn home(&self) -> Option<PathBuf> {
        Some(self.core()?.config().home.clone())
    }

    fn account(&self) -> Option<CloudAccount> {
        self.core()?.plugins.cloud.account()
    }

    fn generation(&self) -> u64 {
        self.core()
            .map_or(0, |core| core.plugins.cloud.identity_sync_generation())
    }

    fn workspace(&self) -> Option<Workspace> {
        self.core()?.workspace()
    }

    fn machine_prefix(&self) -> Option<String> {
        Some(self.core()?.machine_prefix.clone())
    }

    fn version(&self) -> String {
        self.core()
            .map(|core| core.config().version.clone())
            .unwrap_or_default()
    }

    fn socket(&self) -> String {
        self.core()
            .map(|core| core.config().paths.socket.display().to_string())
            .unwrap_or_default()
    }

    fn process(&self, pid: i32) -> Option<ProcessInfo> {
        self.core()?.plugins.processes.process(pid)
    }

    fn current_uid(&self) -> u32 {
        agent_relay_node::relay::peer::current_uid()
    }

    fn in_session(&self, pid: i32) -> bool {
        use agent_relay_node::relay::sessions::CallerMiss;
        let Some(core) = self.core() else {
            return true;
        };
        !matches!(
            core.plugins.sessions.caller(pid),
            Err(CallerMiss::NotASession)
        )
    }

    /// No platform attests a same-user peer in this phase: macOS code
    /// signing attestation from the audit token comes later, and Linux and
    /// Windows have no peer identity a same-user process cannot forge. A
    /// debug build with the `service-callers-dev-attest` preview feature
    /// takes any same-uid local peer for the app ([`attestation`]).
    fn attest_app_peer(&self, pid: i32) -> Attestation {
        attestation(pid, self.current_uid(), system_peer_uid)
    }

    fn spawned(&self, workspace: &Workspace) -> Vec<Spawned> {
        let Some(core) = self.core() else {
            return Vec::new();
        };
        core.store.read(|store| {
            store
                .registrations
                .iter()
                .filter(|r| {
                    r.base_url == workspace.base_url
                        && r.workspace_key.as_ref() == Some(&workspace.key)
                })
                .filter_map(|r| {
                    let spawn = r.spawn.as_ref()?;
                    Some(Spawned {
                        agent_id: r.agent_id.clone(),
                        name: r.name.clone(),
                        invocation_id: spawn.invocation_id.clone(),
                        requester_id: spawn.requester.as_ref().map(|r| r.agent_id.clone()),
                        process: spawn.process.as_ref().map(|p| (p.pid, p.start)),
                        active: r.archived_at.is_none() && r.missing_since.is_none(),
                    })
                })
                .collect()
        })
    }

    fn registration_names(&self) -> Vec<String> {
        let Some(core) = self.core() else {
            return Vec::new();
        };
        core.store
            .read(|store| store.registrations.iter().map(|r| r.name.clone()).collect())
    }

    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    fn log(&self, line: &str) {
        eprintln!("service callers: {line}");
    }

    fn register_agent<'a>(
        &'a self,
        workspace: &'a Workspace,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Registered, RpcError>> {
        Box::pin(async move {
            use agent_relay_node::relay::{client::Joining, sdk_gaps};
            let prefix = self.machine_prefix().ok_or_else(no_core)?;
            let http = sdk_gaps::http(&workspace.base_url, &workspace.key)
                .map_err(|error| RpcError::from_relaycast(&error))?;
            let joining = Joining {
                machine_prefix: &prefix,
                general: false,
            };
            let mut metadata = serde_json::Map::new();
            metadata.insert("service_caller".into(), true.into());
            let agent = sdk_gaps::register_agent(&http, name, joining, metadata)
                .await
                .map_err(|error| RpcError::from_relaycast(&error))?;
            Ok(Registered {
                id: agent.id,
                name: agent.name,
                token: agent.token,
            })
        })
    }

    fn delete_agent<'a>(
        &'a self,
        workspace: &'a Workspace,
        name: &'a str,
    ) -> BoxFuture<'a, Result<(), RpcError>> {
        Box::pin(async move {
            agent_relay_node::relay::client::delete_agent(&workspace.base_url, &workspace.key, name)
                .await
                .map_err(|error| RpcError::from_relaycast(&error))
        })
    }

    fn release_worker<'a>(
        &'a self,
        workspace: &'a Workspace,
        name: &'a str,
        agent_id: &'a str,
        idempotency_key: &'a str,
    ) -> BoxFuture<'a, Result<(), RpcError>> {
        Box::pin(async move {
            use agent_relay_node::relay::{diagnostic_http::RequestOptions, sdk_gaps};
            let http = sdk_gaps::http(&workspace.base_url, &workspace.key)
                .map_err(|error| RpcError::from_relaycast(&error))?;
            // The exact release the core's own `spawn.release` sends.
            let body = serde_json::json!({
                "name": name,
                "expected_agent_id": agent_id,
                "delete_agent": true,
                "reason": "Released after its service caller ended",
            });
            http.post::<serde_json::Value>(
                "/v1/agents/release-exact",
                Some(body),
                Some(RequestOptions::with_idempotency_key(
                    idempotency_key.to_string(),
                )),
            )
            .await
            .map(|_| ())
            .map_err(|error| RpcError::from_relaycast(&error))
        })
    }

    fn deliveries<'a>(
        &'a self,
        base_url: &'a str,
        token: &'a Secret,
    ) -> BoxFuture<'a, Result<Vec<Incoming>, RpcError>> {
        Box::pin(async move {
            let deliveries = agent_relay_node::relay::client::deliveries(base_url, token)
                .await
                .map_err(|error| RpcError::from_relaycast(&error))?;
            Ok(deliveries
                .into_iter()
                .map(|delivery| Incoming {
                    delivery_id: delivery.id,
                    message_id: delivery.message_id,
                    sender_id: delivery.sender_id,
                    sender_name: delivery.sender,
                    text: delivery.text.unwrap_or_default(),
                    created_at_ms: delivery.created_at_ms,
                })
                .collect())
        })
    }

    fn ack<'a>(
        &'a self,
        base_url: &'a str,
        token: &'a Secret,
        delivery_id: &'a str,
    ) -> BoxFuture<'a, Result<(), RpcError>> {
        Box::pin(async move {
            agent_relay_node::relay::client::ack(base_url, token, delivery_id)
                .await
                .map_err(|error| RpcError::from_relaycast(&error))
        })
    }
}

/// What the core answers for `pid`, whose uid `uid_of` reads: unavailable,
/// in every build without the preview feature.
#[cfg(not(all(feature = "service-callers-dev-attest", not(windows))))]
pub(crate) fn attestation(
    _pid: i32,
    _own_uid: u32,
    _uid_of: impl Fn(i32) -> Option<u32>,
) -> Attestation {
    Attestation::Unavailable
}

/// PREVIEW ONLY (`service-callers-dev-attest`, debug builds): the app is any
/// local process of this core's uid. Windows stays unavailable.
#[cfg(all(feature = "service-callers-dev-attest", not(windows)))]
pub(crate) fn attestation(
    pid: i32,
    own_uid: u32,
    uid_of: impl Fn(i32) -> Option<u32>,
) -> Attestation {
    same_uid(uid_of(pid), own_uid)
}

/// The preview feature's whole rule: the same uid is the app, a different
/// or unreadable one is not.
#[cfg(all(feature = "service-callers-dev-attest", not(windows)))]
pub(crate) fn same_uid(peer_uid: Option<u32>, own_uid: u32) -> Attestation {
    match peer_uid {
        Some(uid) if uid == own_uid => Attestation::App,
        _ => Attestation::NotApp,
    }
}

/// The uid `pid` runs as, from `/proc/<pid>/status`.
#[cfg(target_os = "linux")]
fn system_peer_uid(pid: i32) -> Option<u32> {
    if pid <= 0 {
        return None;
    }
    status_uid(&std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?)
}

/// The `Uid:` line of a `/proc/<pid>/status`: the effective uid, and only
/// when the real uid is the same.
#[cfg(target_os = "linux")]
pub(crate) fn status_uid(status: &str) -> Option<u32> {
    let line = status.lines().find_map(|line| line.strip_prefix("Uid:"))?;
    let mut uids = line.split_whitespace().map(str::parse::<u32>);
    let real = uids.next()?.ok()?;
    let effective = uids.next()?.ok()?;
    (real == effective).then_some(effective)
}

/// The uid `pid` runs as, from the kernel's BSD process record
/// (`proc_pidinfo`, through the core's process table).
#[cfg(target_os = "macos")]
fn system_peer_uid(pid: i32) -> Option<u32> {
    use agent_relay_node::relay::process::{ProcessTable, SystemProcesses};
    SystemProcesses::default()
        .process(pid)
        .map(|process| process.uid)
}

/// No other platform reads a peer's uid here.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn system_peer_uid(_pid: i32) -> Option<u32> {
    None
}

fn no_core() -> RpcError {
    RpcError::new(ErrorKind::Internal, "The core is not running.")
}
