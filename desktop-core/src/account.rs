//! The person's Agent Relay account on this computer: sign-in, the kept
//! Cloud session, the account's workspace, and the uploader's connection.
//!
//! `account.sign_in` gets a Cloud session (reusing a kept one, or through
//! the browser or a device approval), learns who signed in, keeps the
//! session, resolves the account workspace's Relaycast key into the agent
//! store, and runs the probe's `cloud install` with that session so the
//! uploader connects without a second approval. Progress is the
//! `status.account` snapshot and the `sign_in` event.
//!
//! `Account` is also the daemon's `CloudAccess`: every Cloud call made as
//! the person goes through `request`, which refreshes the kept session and
//! retries once when Cloud refuses it.

use crate::browser_login::{BrowserLogin, DeviceLogin, LoginError};
use agent_relay_node::relay::cloud_session::{self, access_is_fresh, can_refresh, AgentOwner, CloudAccess, CloudAccount, CloudError, Identity};
use crate::cloud_api::SessionFile;
use agent_relay_node::relay::contract::account::{SignInLink, SignInReport};
use agent_relay_node::relay::daemon::Core;
use agent_relay_node::relay::links::built_in_site_url;
use agent_relay_node::relay::protocol::{ErrorKind, RpcError};
use agent_relay_node::relay::state::{
    AccountAction, AccountPerson, AccountStatus, Level, Listing, SignInState,
    WorkspaceStatus,
};
use agent_relay_node::relay::store::CloudSession;
use crate::telemetry::{
    self, Telemetry, TelemetryEvent, TelemetryFailure, TelemetryOperation, TelemetryOutcome,
};
use crate::history::uploader::{Install, Uploader, UploaderError};
use crate::contract::account::Target;
use agent_relay_node::relay::workspace::{validated_base_url, Secret};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

pub mod plugin;
mod usage_tracking;
pub(crate) mod cloud_binding;
mod credential;
mod health;
mod mcp_auth;
mod operator_policy;
mod profile;
mod progress;
mod provider_credentials;
mod provider_connections;
mod service_callers;
mod session;
mod sign_out;
mod site_choices;
#[cfg(any(test, feature = "test-site"))]
mod test_site;
mod upload_failure;
mod watchers;
mod validation;
mod workspace_recovery;
pub(crate) use cloud_binding::report as cloud_identity_sync;
use progress::capture_progress;
pub use upload_failure::{upload_health, upload_legacy_schedule_jobs};

/// How long a browser sign-in waits for its return.
pub const BROWSER_WAIT: Duration = Duration::from_secs(600);

/// An access token with less than this left is refreshed before use.
const FRESH_MARGIN: Duration = Duration::from_secs(300);

/// How a sign-in gets its session.
pub use crate::contract::account::Flow;

pub use crate::contract::account::SignInRequest;

/// The link a browser sign-in waits on: Cloud's login page.
fn browser_link(url: String) -> SignInLink {
    SignInLink {
        verification_url: Some(url.clone()),
        verification_url_complete: Some(url),
        user_code: None,
        expires_at: Some(cloud_session::iso(
            agent_relay_node::relay::state::now_ms() + BROWSER_WAIT.as_millis() as i64,
        )),
        interval: None,
    }
}

/// How the attempt starts.
enum Start {
    /// Select and refresh an existing login in the background, else open the browser.
    Continue,
    /// The kept session.
    Reused(CloudSession),
    Device(DeviceLogin),
    Browser(BrowserLogin),
}

mod login_failure;
mod login_start;
mod session_refresh;

/// The kept session for a site.
enum Kept {
    /// This app keeps no session there.
    Absent,
    Usable(CloudSession),
    /// Kept, but expired and not renewable now.
    Unrenewable,
    /// Kept and renewable, but Cloud could not renew it just now.
    Unavailable(CloudError),
}

/// The attempt in flight.
#[derive(Default)]
struct Attempt {
    id: u64,
    task: Option<tokio::task::JoinHandle<()>>,
    /// The sign-in link, while one is waiting.
    link: Option<SignInLink>,
    workspace_selection: Option<tokio::sync::oneshot::Sender<String>>,
    /// The connection this attempt's `cloud install` made and nothing has
    /// attached yet: a cancel or sign-out disconnects it.
    made: Option<Target>,
    /// What this attempt replaced, until it finishes: a cancel restores it.
    undo: Undo,
}

/// The values a sign-in attempt replaced; `None` where it changed nothing.
#[derive(Default)]
struct Undo {
    session: Option<Option<CloudSession>>,
    workspace: Option<Option<agent_relay_node::relay::store::CloudWorkspace>>,
    desktop_identity_clears: Option<Option<Vec<agent_relay_node::relay::store::DesktopIdentityClear>>>,
    identity: Option<Option<Identity>>,
}

/// The account, the session and the uploader for one daemon.
pub struct Account {
    pub(crate) copilot_dispatch: Arc<crate::copilot_dispatch::Registry>,
    core: OnceLock<Weak<Core>>,
    /// The site this build signs in to: the one it was made for
    /// (`links::built_in_site_url`, `AGENT_RELAY_SITE_URL` at build time).
    /// Nothing at run time changes it; only a test build (the `test-site`
    /// feature) points an account at a fake Cloud (`with_site`).
    site_url: String,
    /// `CLOUD_API_ACCESS_TOKEN`, a bearer the environment supplies.
    env_token: Option<Secret>,
    /// Linux's `cloud-session.json`, imported into the store once.
    legacy_session: SessionFile,
    pub uploader: Arc<Uploader>,
    /// Production MCP setup refreshed within the core lifecycle.
    pub mcp_config: OnceLock<Arc<crate::mcp_settings::Installer>>,
    /// This installation's Claude chat attestation key.
    pub attestation_key: crate::claude_attestation::KeySlot,
    attempt: Mutex<Attempt>,
    /// Bumped on every sign-out: work begun before one keeps nothing.
    epoch: AtomicU64,
    /// One refresh at a time: Cloud rotates the refresh token on use.
    refreshing: tokio::sync::Mutex<()>,
    /// The kept session's refresh token Cloud last refused, if any.
    refused_refresh: Mutex<Option<Secret>>,
    /// One attempt per refused account key and sign-out epoch, including failures.
    workspace_recovery: tokio::sync::Mutex<Option<(u64, agent_relay_node::relay::workspace::Workspace)>>,
    identity: Mutex<Option<Identity>>,
    auth_check: tokio::sync::Mutex<()>,
    mcp_capability: tokio::sync::Mutex<Option<mcp_auth::Capability>>,
    provider_credentials: tokio::sync::Mutex<()>,
    auth_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    auth_generation: AtomicU64,
    connection_checks: provider_connections::Checks,
    /// An install whose disconnect did not finish; the next sign-in retries it.
    pending_disconnect: Mutex<Option<Target>>,
    /// One disconnect at a time, and no sign-in connects until it is done:
    /// a disconnect still running would tear the new connection down.
    disconnecting: tokio::sync::Mutex<()>,
    /// The desktop identity sync's state.
    pub identity_sync: Arc<crate::desktop_identity::DesktopIdentitySync>,
    /// Contractor access, made on first use.
    contractors: OnceLock<Arc<crate::contractor_access::ContractorAccess>>,
    /// Turn-end coordination advisories, where this build hands them out.
    pub(crate) coordination_advisories:
        OnceLock<Arc<crate::coordination_advisories::Advisories>>,
    /// The workspace operator's upload policy, which the uploader consults.
    pub(crate) operator_policy: Arc<crate::history::operator_policy::OperatorPolicy>,
    /// Operation telemetry, when this daemon reports it.
    pub telemetry: Option<Arc<Telemetry>>,
    pub(crate) upload_observations: Mutex<telemetry::UploadObservations>,
    pub(crate) upload_sample: AtomicU64,
    /// This binary, its command-line copy's path, and whether it replaces
    /// another build of its release there (`site_choices::command_line_copy`).
    pub command_line_copy: Option<(PathBuf, PathBuf, bool)>,
    /// Turn Linux launch at login on after the first production sign-in.
    /// The headless service leaves it off: its systemd unit starts Agent
    /// Relay, and the app's entry would start a second copy.
    pub launch_at_login_after_sign_in: bool,
    /// Registered sessions asked to upload (`relay_uploads`).
    pub(crate) relay_uploads: Arc<crate::history::relay_uploads::RelayUploads>,
    /// Where sessions run, observed for the daemon's life once it has them.
    session_locations: Option<Arc<crate::session_locations::SessionLocations>>,
    /// Service callers and the core they read (`docs/service-callers.md`).
    pub(crate) service_callers: service_callers::Callers,
    /// How often the uploader and the mailbox are supervised.
    pub supervise_every: Duration,
    /// Wakes supervision early: a session the person just approved may
    /// activate the mailbox.
    wake: tokio::sync::Notify,
    /// When supervision may next refresh the session for blocked uploads.
    upload_auth_retry: Mutex<Retry>,
    workspace_name_retry: Mutex<Retry>,
}

/// Backoff for a Cloud call supervision repeats.
#[derive(Default)]
struct Retry {
    failures: u32,
    not_before: Option<tokio::time::Instant>,
}

fn failed(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorKind::CloudFailed, message)
}

impl Account {
    pub fn new(
        env: &std::collections::BTreeMap<String, String>,
        app_data_dir: &std::path::Path,
        uploader: Arc<Uploader>,
    ) -> Account {
        Account {
            copilot_dispatch: Default::default(),
            core: OnceLock::new(),
            site_url: built_in_site_url().to_string(),
            env_token: env
                .get("CLOUD_API_ACCESS_TOKEN")
                .map(|token| token.trim())
                .filter(|token| !token.is_empty())
                .map(Secret::new),
            legacy_session: SessionFile::new(app_data_dir),
            operator_policy: operator_policy::for_uploader(env, app_data_dir, &uploader),
            uploader,
            attempt: Mutex::default(),
            epoch: AtomicU64::new(0),
            refreshing: tokio::sync::Mutex::new(()),
            refused_refresh: Mutex::default(),
            workspace_recovery: tokio::sync::Mutex::new(None),
            identity: Mutex::new(None),
            auth_check: tokio::sync::Mutex::new(()),
            mcp_capability: tokio::sync::Mutex::new(None),
            provider_credentials: tokio::sync::Mutex::new(()),
            auth_task: Mutex::new(None),
            auth_generation: AtomicU64::new(0),
            connection_checks: Default::default(),
            pending_disconnect: Mutex::new(None),
            disconnecting: tokio::sync::Mutex::new(()),
            identity_sync: Default::default(),
            contractors: OnceLock::new(),
            coordination_advisories: OnceLock::new(),
            telemetry: None,
            upload_observations: Mutex::default(),
            upload_sample: AtomicU64::new(0),
            command_line_copy: None,
            launch_at_login_after_sign_in: true,
            relay_uploads: Arc::default(),
            session_locations: None,
            mcp_config: OnceLock::new(),
            attestation_key: Default::default(),
            service_callers: service_callers::Callers::new(),
            supervise_every: crate::history::uploader::SUPERVISE_EVERY,
            wake: tokio::sync::Notify::new(),
            upload_auth_retry: Mutex::default(),
            workspace_name_retry: Mutex::default(),
        }
    }

    /// Ties the account to the core it serves; called once, by the core.
    pub fn attach(&self, core: &Arc<Core>) {
        let _ = self.core.set(Arc::downgrade(core));
        self.service_callers.attach(core);
        self.import_legacy_session(core);
    }

    /// The Linux app kept its session in `cloud-session.json`; the store's
    /// `cloudSession` is the one place now. Imported once, then removed.
    fn import_legacy_session(&self, core: &Core) {
        let Some(legacy) = self.legacy_session.load() else {
            return;
        };
        let kept = core.store.update(|store| {
            if store.cloud_session.is_none() {
                store.cloud_session = Some(legacy);
            }
        });
        if kept.is_ok() {
            self.legacy_session.remove();
        }
    }

    // The kept session.

    /// A session for `site` with no person involved: the kept one, with at
    /// least `margin` left; one kept but not renewable gives `None`.
    async fn session_for(&self, site: &str, margin: Duration) -> Option<CloudSession> {
        match self.kept_session(site, margin).await {
            Kept::Usable(session) => Some(session),
            Kept::Unrenewable | Kept::Unavailable(_) | Kept::Absent => None,
        }
    }

    /// A bearer for `site` without asking the person: the environment's,
    /// else `session_for`'s; `true` when it is the kept session.
    async fn bearer(&self, site: &str) -> Option<(Secret, bool)> {
        if let Some(token) = &self.env_token {
            return Some((token.clone(), false));
        }
        self.session_for(site, FRESH_MARGIN)
            .await
            .map(|session| (session.access_token, true))
    }

    /// A session to start setup with: fresh for at least a minute.
    async fn reusable_session(&self, site: &str) -> Option<CloudSession> {
        self.session_for(site, Duration::from_secs(60)).await
    }

    // State.

    /// `status.account`.
    pub fn status(&self) -> AccountStatus {
        self.core()
            .map(|core| core.status().account)
            .unwrap_or_default()
    }

    fn update(&self, change: impl FnOnce(&mut AccountStatus)) -> AccountStatus {
        let Some(core) = self.core() else {
            return AccountStatus::default();
        };
        core.hub
            .update(|status| change(&mut status.account))
            .account
    }

    /// Publishes the sign-in event: the account state plus the link to open.
    fn announce(&self, status: &AccountStatus) {
        let link = self.attempt.lock().unwrap().link.clone();
        self.publish(status, link);
    }

    fn publish(&self, status: &AccountStatus, link: Option<SignInLink>) {
        let Some(core) = self.core() else { return };
        core.hub.sign_in(&SignInReport {
            account: status.clone(),
            link: link.unwrap_or_default(),
        });
    }

    /// Sets the waiting link, for `attempt` only: a stale attempt never
    /// replaces the link the current one shows.
    fn set_link(&self, attempt: u64, link: Option<SignInLink>) {
        let mut current = self.attempt.lock().unwrap();
        if current.id == attempt {
            current.link = link;
        }
    }

    /// Changes `status.account` for `attempt` only. The check, the change
    /// and its announcement hold the attempt lock, so a cancel or sign-out
    /// that moves the attempt on is never followed by a stale state.
    fn set(&self, attempt: u64, change: impl FnOnce(&mut AccountStatus)) -> bool {
        let current = self.attempt.lock().unwrap();
        if current.id != attempt {
            return false;
        }
        let status = self.update(change);
        self.publish(&status, current.link.clone());
        true
    }

    /// The account state as `account.sign_in` and `account.status` report
    /// it: `status.account` plus the link while one is waiting.
    pub fn report(&self) -> SignInReport {
        SignInReport {
            account: self.status(),
            link: self
                .attempt
                .lock()
                .unwrap()
                .link
                .clone()
                .unwrap_or_default(),
        }
    }

    /// The attached install, reflected into `status.account`, analytics and
    /// the owner of session agents. Analytics follow an attached install
    /// only: a sign-in that is cancelled or never attaches stays anonymous.
    /// Analytics attach once the status says signed in, which a site's
    /// gateway needs for the session it sends with.
    fn show_install(&self, install: Option<&Install>) {
        if let (Some(core), Ok(runtime)) = (self.core(), tokio::runtime::Handle::try_current()) {
            runtime.spawn(async move { core.credit_agents().await });
        }
        let workspace_name = self.core().and_then(|core| {
            core.store
                .read(|s| s.cloud.as_ref().and_then(|c| c.name.clone()))
        });
        self.update(|account| match install {
            Some(install) => {
                account.state = SignInState::SignedIn;
                account.phase = None;
                account.error = None;
                account.action = None;
                account.progress = None;
                let same_account = account.workspace_id == install.target.workspace_id
                    && account.site_url.as_deref() == Some(install.target.site_url.as_str())
                    && account.person.as_ref().and_then(|p| p.id.as_ref())
                        == install.target.account_id.as_ref();
                if !same_account {
                    account.auth_health = None;
                    account.auth_error = None;
                    account.auth_checked_at = None;
                    account.workspace_count = None;
                    account.workspace_avatar_url = None;
                }
                let previous_name = (account.workspace_id == install.target.workspace_id
                    && account.site_url.as_deref() == Some(install.target.site_url.as_str())
                    && account
                        .person
                        .as_ref()
                        .and_then(|person| person.id.as_ref())
                        == install.target.account_id.as_ref())
                .then(|| account.workspace_name.clone())
                .flatten();
                account.site_url = Some(install.target.site_url.clone());
                account.workspace_id = install.target.workspace_id.clone();
                account.workspace_name =
                    install
                        .workspace_name
                        .clone()
                        .or(previous_name)
                        .or_else(|| {
                            self.core().and_then(|core| {
                                core.store.read(|store| {
                                    store
                                        .cloud
                                        .as_ref()
                                        .filter(|cloud| {
                                            cloud.account_workspace_id
                                                == install.target.workspace_id
                                        })
                                        .and_then(|_| workspace_name.clone())
                                })
                            })
                        });
                account.person = Some(AccountPerson {
                    id: install.target.account_id.clone(),
                    name: install.account_name.clone(),
                    email: install.account_email.clone(),
                    avatar_url: install.account_avatar_url.clone(),
                });
            }
            None => *account = AccountStatus::default(),
        });
        if let (Some(telemetry), Some(install)) = (&self.telemetry, install) {
            telemetry.attach(usage_tracking::telemetry_account(install));
        }
        if let Some(core) = self.core() {
            core.hub.invalidated(Listing::Uploader);
        }
    }

    // Lifecycle.

    /// Finds this computer's connection, then supervises the uploader for
    /// the daemon's life. Discovery is bounded; nothing waits on it.
    pub async fn run(self: Arc<Self>) {
        if let Some(copy) = self.command_line_copy.clone() {
            site_choices::keep_command_line_copy(copy);
        }
        self.discover().await;
        self.uploader.reclaim();
        if let Some(core) = self.core() {
            self.spawn_watchers(core);
        }
        let mut last: Option<Value> = None;
        let mut failing: Option<TelemetryFailure> = None;
        let mut ticks = tokio::time::interval(self.supervise_every);
        loop {
            tokio::select! {
                _ = ticks.tick() => {}
                _ = self.wake.notified() => {}
            }
            if self.uploader.install().is_none() {
                self.discover().await;
            }
            if let Some((_, status)) = self.uploader.supervise().await {
                let summary = |s: &Value| {
                    json!([
                        s["stage"],
                        s["needs_start"],
                        s["running"],
                        s["paused"],
                        s["delivery"]["pending_records"],
                        s["delivery"]["failure"],
                        s["delivery"]["renewal_failure"],
                        s["sessions"]
                    ])
                };
                if last.as_ref().map(summary) != Some(summary(&status)) {
                    if let Some(core) = self.core() {
                        core.hub.invalidated(Listing::Uploader);
                    }
                }
                failing = self.track_upload_health(&status, failing);
                self.refresh_for_uploads(&status).await;
                last = Some(status);
                // A sharing change has no deadline, so it never holds up
                // the collector's supervision.
                if let Some(core) = self.core() {
                    let (uploads, uploader) = (self.relay_uploads.clone(), self.uploader.clone());
                    tokio::spawn(async move { uploads.sync(&core, &uploader).await });
                }
            }
            self.operator_policy_tick().await;
            self.schedule_auth_health();
            self.sync_cloud_identities().await;
        }
    }

    /// Reports when a running uploader starts failing, fails for another
    /// reason, or recovers; returns the failure it reports now.
    fn track_upload_health(
        &self,
        status: &Value,
        reported: Option<TelemetryFailure>,
    ) -> Option<TelemetryFailure> {
        match upload_health(status) {
            None => reported,
            Some(Some(failure)) => {
                if reported != Some(failure) {
                    self.track_upload_failure(failure, upload_legacy_schedule_jobs(status));
                }
                Some(failure)
            }
            Some(None) => {
                if reported.is_some() {
                    self.track(
                        TelemetryOperation::UploadHealth,
                        TelemetryOutcome::Succeeded,
                        None,
                    );
                }
                None
            }
        }
    }

    /// While uploads are blocked on `authentication_required`, keeps this
    /// app's Cloud session for the connection's site fresh, so the collector
    /// renews its upload session from it (`cloud::renewal_bearer`) on its next
    /// pass. Only a failed refresh counts against the backoff, which
    /// runs from one supervision period, doubling to 32, and clears once
    /// uploads are no longer blocked on sign-in. A refresh here is not the
    /// person's approval, so the mailbox backoff is left alone.
    async fn refresh_for_uploads(&self, status: &Value) {
        if status["delivery"]["failure_code"] != "authentication_required" {
            *self.upload_auth_retry.lock().unwrap() = Retry::default();
            return;
        }
        let Some(target) = self.uploader.target() else {
            return;
        };
        let now = tokio::time::Instant::now();
        if self
            .upload_auth_retry
            .lock()
            .unwrap()
            .not_before
            .is_some_and(|at| now < at)
        {
            return;
        }
        let kept = self.kept_session(&target.site_url, FRESH_MARGIN).await;
        let mut retry = self.upload_auth_retry.lock().unwrap();
        if matches!(kept, Kept::Unrenewable | Kept::Unavailable(_)) {
            retry.failures = retry.failures.saturating_add(1);
            retry.not_before = Some(now + self.supervise_every * (1 << retry.failures.min(5)));
        } else {
            *retry = Retry::default();
        }
    }

    /// Attaches this computer's connection for the site, when nothing is
    /// attached, no sign-in is running and no sign-out is still
    /// disconnecting one: at start, and again on every supervision tick, so
    /// a slow first listing or a connection made in a terminal is found.
    async fn discover(&self) {
        let quiet = || {
            self.uploader.install().is_none()
                && !self.busy()
                && self.pending_disconnect.lock().unwrap().is_none()
        };
        if !quiet() {
            return;
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        match self.uploader.installs(&self.site_url, None).await {
            Ok(Some(found)) if quiet() && self.epoch.load(Ordering::SeqCst) == epoch => {
                self.track(
                    TelemetryOperation::DiscoverInstall,
                    TelemetryOutcome::Succeeded,
                    None,
                );
                self.uploader.attach(Some(found.clone()));
                self.show_install(Some(&found));
            }
            Ok(None) if quiet() && self.epoch.load(Ordering::SeqCst) == epoch => {
                // Nothing is attached: analytics leave the last user's id, and
                // nothing uploads here (`relay_uploads::admitted`).
                self.telemetry.iter().for_each(|telemetry| telemetry.attach(None));
                self.relay_uploads.looked_for_install();
            }
            Ok(_) => {}
            Err(error) => {
                self.track(
                    TelemetryOperation::DiscoverInstall,
                    TelemetryOutcome::Failed,
                    Some(TelemetryFailure::HelperFailed),
                );
                if let Some(core) = self.core() {
                    core.hub.activity(Level::Warning, error.message);
                }
            }
        }
    }

    // Sign-in.

    pub(super) fn busy(&self) -> bool {
        matches!(
            self.status().state,
            SignInState::PendingApproval | SignInState::Preparing
        )
    }

    /// Starts a sign-in and answers at once with where it stands; it
    /// continues in the background.
    pub async fn sign_in(
        self: &Arc<Self>,
        request: SignInRequest,
    ) -> Result<SignInReport, RpcError> {
        // The site is the build's own; nothing in the request chooses it.
        let site = self.site_url.clone();
        if let Some(workspace) = request.workspace.as_deref() {
            if !validation::valid_id(workspace) {
                return Err(RpcError::invalid_params(
                    "That is not an Agent Relay workspace.",
                ));
            }
        }
        if let Some(install) = self.uploader.install() {
            if !self.busy() && self.needs_auth_renewal(&install).await {
                let flow = request.flow.unwrap_or_else(Flow::platform);
                return self.renew_sign_in(install, flow, request.provider).await;
            }
            return Ok(self.report());
        }
        // Checked and claimed at once: a second caller sees this attempt.
        let attempt = {
            let mut current = self.attempt.lock().unwrap();
            if self.busy() || self.uploader.install().is_some() {
                drop(current);
                return Ok(self.report());
            }
            if let Some(task) = current.task.take() {
                task.abort();
            }
            current.id += 1;
            current.link = None;
            let id = current.id;
            self.update(|account| {
                *account = AccountStatus {
                    state: SignInState::Preparing,
                    phase: Some("checking_existing_login".into()),
                    site_url: Some(site.clone()),
                    ..AccountStatus::default()
                }
            });
            id
        };
        if let Err(error) = self.finish_pending_disconnect().await {
            self.fail(
                attempt,
                &LoginError {
                    state: "error",
                    message: error.message.clone(),
                },
            );
            return Err(error);
        }
        let flow = request.flow.unwrap_or_else(Flow::platform);
        if let Some(telemetry) = &self.telemetry {
            telemetry.record(
                TelemetryEvent::new(TelemetryOperation::SignIn, TelemetryOutcome::Started)
                    .mode(request.sharing),
            );
        }
        self.set(attempt, |account| {
            *account = AccountStatus {
                state: SignInState::Preparing,
                phase: Some("checking_existing_login".into()),
                site_url: Some(site.clone()),
                ..AccountStatus::default()
            }
        });
        // Default setup reuses an eligible login before opening Google. An
        // explicit browser request (Use another account) chooses an account.
        let reusable = if request.flow == Some(Flow::Device) {
            self.reusable_session(&site).await
        } else {
            None
        };
        let start = if request.flow.is_none() {
            Start::Continue
        } else if let Some(session) = reusable {
            Start::Reused(session)
        } else {
            self.begin_login(attempt, &site, flow).await?
        };
        let (phase, link, reused) = match &start {
            Start::Continue => ("checking_existing_login", None, None),
            Start::Reused(..) => ("reusing_existing_login", None, Some(true)),
            Start::Device(login) => (
                "waiting_for_device_approval",
                Some(login.public()),
                Some(false),
            ),
            Start::Browser(login) => {
                let url = login_start::authorization_url(
                    login,
                    &site,
                    request.flow.is_none(),
                    request.workspace.as_deref(),
                    request.provider,
                )
                    .ok_or_else(|| {
                        RpcError::new(
                            ErrorKind::Refused,
                            "The Agent Relay site must be an https origin.",
                        )
                    })?;
                ("waiting_for_browser", Some(browser_link(url)), None)
            }
        };
        let pending = matches!(start, Start::Device(..) | Start::Browser(..));
        self.set_link(attempt, link);
        self.set(attempt, |account| {
            account.state = if pending {
                SignInState::PendingApproval
            } else {
                SignInState::Preparing
            };
            account.phase = Some(phase.into());
            account.reused_login = reused;
        });
        let account = self.clone();
        let task =
            tokio::spawn(
                async move { account.finish_sign_in(attempt, site, request, start).await },
            );
        {
            let mut current = self.attempt.lock().unwrap();
            if current.id == attempt {
                current.task = Some(task);
            } else {
                task.abort();
            }
        }
        Ok(self.report())
    }

    /// `account.sign_in` while this computer stays connected but the core
    /// holds no session to call Cloud as the person: Cloud refused the kept
    /// one. The person
    /// approves again and the session is kept, as a sign-in's; the uploader's
    /// connection is left alone.
    async fn renew_sign_in(
        self: &Arc<Self>,
        install: Install,
        flow: Flow,
        provider: Option<crate::contract::account::SignInProvider>,
    ) -> Result<SignInReport, RpcError> {
        let site = install.target.site_url.clone();
        let attempt = {
            let mut current = self.attempt.lock().unwrap();
            if self.busy() {
                drop(current);
                return Ok(self.report());
            }
            if let Some(task) = current.task.take() {
                task.abort();
            }
            current.id += 1;
            current.link = None;
            current.id
        };
        let (start, phase, link) = match flow {
            Flow::Device => {
                let login = DeviceLogin::begin(&site, Some(&self.desktop_host()?))
                    .await
                    .map_err(|error| failed(error.message))?;
                let link = login.public();
                (Start::Device(login), "waiting_for_device_approval", link)
            }
            Flow::Browser => {
                let login = self.desktop_browser_login().await?;
                let url = login_start::authorization_url(
                    &login,
                    &site,
                    true,
                    install.target.workspace_id.as_deref(),
                    provider,
                )
                    .ok_or_else(|| {
                        RpcError::new(
                            ErrorKind::Refused,
                            "The Agent Relay site must be an https origin.",
                        )
                    })?;
                (
                    Start::Browser(login),
                    "waiting_for_browser",
                    browser_link(url),
                )
            }
        };
        self.set_link(attempt, Some(link));
        self.set(attempt, |account| {
            account.state = SignInState::PendingApproval;
            account.phase = Some(phase.into());
            account.error = None;
        });
        let account = self.clone();
        let task =
            tokio::spawn(
                async move { account.finish_renewal(attempt, site, install, start).await },
            );
        {
            let mut current = self.attempt.lock().unwrap();
            if current.id == attempt {
                current.task = Some(task);
            } else {
                task.abort();
            }
        }
        Ok(self.report())
    }

    /// Waits for the approval, keeps the session when it is the connected
    /// account's, then shows the connection again.
    async fn finish_renewal(
        self: Arc<Self>,
        attempt: u64,
        site: String,
        install: Install,
        start: Start,
    ) {
        let epoch = self.epoch.load(Ordering::SeqCst);
        let session = match start {
            Start::Continue => self
                .default_session(attempt, &site, install.target.workspace_id.as_deref())
                .await,
            Start::Device(login) => login.poll().await.map_err(login_failure::device_ended),
            Start::Browser(login) => self.browser_session(&login, &site).await,
            Start::Reused(session) => Ok(session),
        };
        let kept = match session {
            Ok(session) => {
                self.keep_renewed(attempt, epoch, &site, &install, &session)
                    .await
            }
            Err(error) => Err(error.message),
        };
        {
            let mut current = self.attempt.lock().unwrap();
            if current.id != attempt {
                return;
            }
            current.link = None;
            current.task = None;
        }
        self.show_install(Some(&install));
        self.announce(&self.status());
        if let (Some(telemetry), Ok(())) = (&self.telemetry, &kept) {
            telemetry.sign_in_completed();
        }
        if let Err(message) = kept {
            if let Some(core) = self.core() {
                core.hub.activity(Level::Warning, message);
            }
        }
    }

    async fn keep_renewed(
        &self,
        attempt: u64,
        epoch: u64,
        site: &str,
        install: &Install,
        session: &CloudSession,
    ) -> Result<(), String> {
        let identity = cloud_session::whoami(site, &session.access_token)
            .await
            .map_err(|error| error.to_string())?;
        if install
            .target
            .account_id
            .as_deref()
            .is_some_and(|id| id != identity.user_id)
        {
            return Err(
                "That is a different Agent Relay account. Sign in as the account this computer uploads to."
                    .into(),
            );
        }
        let _refresh = self.refreshing.lock().await;
        let current = self.attempt.lock().unwrap();
        if current.id != attempt || self.epoch.load(Ordering::SeqCst) != epoch {
            return Ok(());
        }
        self.keep_session(session)?;
        *self.identity.lock().unwrap() = Some(identity);
        Ok(())
    }

    /// Default native/setup continuation. Even refreshing a saved login is
    /// background work so the initial RPC can return and cancellation can run.
    async fn default_session(
        self: &Arc<Self>,
        attempt: u64,
        site: &str,
        workspace: Option<&str>,
    ) -> Result<CloudSession, LoginError> {
        // Cloud may already have rotated the token when the person cancels.
        // Let credential maintenance finish; abort only the awaiting attempt.
        let account = self.clone();
        let refresh_site = site.to_string();
        let reusable = tokio::spawn(async move { account.reusable_session(&refresh_site).await })
            .await
            .map_err(|_| LoginError {
                state: "error",
                message: "Could not check the saved sign-in.".into(),
            })?;
        if let Some(session) = reusable {
            self.set(attempt, |account| {
                account.phase = Some("reusing_existing_login".into());
                account.reused_login = Some(true);
            });
            return Ok(session);
        }
        let login = BrowserLogin::bind().await?;
        let url = login
            .authorization_url_for_workspace(site, true, workspace)
            .ok_or_else(|| LoginError {
                state: "error",
                message: "The Agent Relay site must be an https origin.".into(),
            })?;
        self.set_link(attempt, Some(browser_link(url)));
        self.set(attempt, |account| {
            account.state = SignInState::PendingApproval;
            account.phase = Some("waiting_for_browser".into());
            account.reused_login = Some(false);
        });
        self.browser_session(&login, site).await
    }

    /// Completes only the active, authenticated picker; arbitrary workspace IDs
    /// cannot skip the membership list. Cloud checks membership again on bind.
    pub fn select_workspace(&self, workspace: &str) -> Result<(), RpcError> {
        let mut current = self.attempt.lock().unwrap();
        let status = self.status();
        if status.phase.as_deref() != Some("selecting_workspace")
            || !status
                .workspace_choices
                .as_ref()
                .is_some_and(|choices| choices.iter().any(|choice| choice.id == workspace))
        {
            return Err(RpcError::invalid_params(
                "Choose one of your available workspaces.",
            ));
        }
        let sender = current
            .workspace_selection
            .take()
            .ok_or_else(|| RpcError::invalid_params("Workspace selection is no longer waiting."))?;
        sender
            .send(workspace.to_string())
            .map_err(|_| RpcError::invalid_params("Sign-in was cancelled."))
    }

    async fn choose_workspace(
        &self,
        attempt: u64,
        site: &str,
        session: &CloudSession,
        identity: &Identity,
        requested: Option<&str>,
        native: bool,
    ) -> Result<(Option<String>, bool), String> {
        // Remote setup clients can only poll status; they cannot answer a
        // native picker. Preserve their current/explicit workspace behavior.
        if !native {
            return Ok((
                requested
                    .map(str::to_string)
                    .or(identity.current_workspace_id.clone()),
                false,
            ));
        }
        let choices = match crate::cloud_api::desktop_workspaces(site, &session.access_token).await {
            Ok(choices) => choices,
            // Older servers keep the browser-owned picker.
            Err(CloudError::Status { status: 404, .. }) => {
                return Ok((
                    requested
                        .map(str::to_string)
                        .or(identity.current_workspace_id.clone()),
                    false,
                ))
            }
            Err(error) => return Err(error.to_string()),
        };
        if let Some(requested) = requested {
            return choices
                .iter()
                .find(|choice| choice.id == requested)
                .map(|choice| (Some(choice.id.clone()), false))
                .ok_or_else(|| "This workspace is not available to your account.".into());
        }
        if choices.len() < 2 {
            return Ok((choices.first().map(|choice| choice.id.clone()), false));
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        {
            let mut current = self.attempt.lock().unwrap();
            if current.id != attempt {
                return Err("Sign-in was cancelled.".into());
            }
            current.workspace_selection = Some(sender);
        }
        self.set(attempt, |account| {
            account.phase = Some("selecting_workspace".into());
            account.workspace_choices = Some(choices);
            account.person = Some(AccountPerson {
                id: Some(identity.user_id.clone()),
                name: identity.name.clone(),
                email: identity.email.clone(),
                avatar_url: identity.avatar_url.clone(),
            });
        });
        let selected = receiver
            .await
            .map_err(|_| "Sign-in was cancelled.".to_string())?;
        self.set(attempt, |account| {
            account.phase = Some("connecting_workspace".into());
            account.workspace_choices = None;
        });
        Ok((Some(selected), true))
    }

    async fn refresh_setup_session(
        self: &Arc<Self>,
        session: &CloudSession,
        kept: bool,
    ) -> Result<CloudSession, String> {
        if kept {
            // Keep rotating/saving even if the awaiting picker is cancelled.
            let account = self.clone();
            let (at, refused) = (session.site_url.clone(), session.access_token.clone());
            tokio::spawn(async move { account.renew(&at, &refused).await })
                .await
                .unwrap_or(Err(CloudError::Unreachable))
                .map_err(|error| error.to_string())
        } else {
            cloud_session::refresh(session)
                .await
                .map_err(|error| error.to_string())
        }
    }

    /// Gets the session, then connects the workspace and the uploader.
    async fn finish_sign_in(
        self: Arc<Self>,
        attempt: u64,
        site: String,
        request: SignInRequest,
        start: Start,
    ) {
        let epoch = self.epoch.load(Ordering::SeqCst);
        let session = match start {
            Start::Continue => {
                self.default_session(attempt, &site, request.workspace.as_deref())
                    .await
            }
            Start::Reused(session) => Ok(session),
            Start::Device(login) => login.poll().await.map_err(login_failure::device_ended),
            Start::Browser(login) => self.browser_session(&login, &site).await,
        };
        let session = match session {
            Ok(session) => session,
            Err(error) => return self.fail(attempt, &error),
        };
        if self.epoch.load(Ordering::SeqCst) != epoch {
            return;
        }
        self.set_link(attempt, None);
        self.set(attempt, |account| {
            account.state = SignInState::Preparing;
            account.phase = Some("connecting_workspace".into());
        });
        let connected = self
            .connect(attempt, epoch, &site, &session, &request)
            .await;
        {
            // A setup that ended on its own keeps what it did, and leaves
            // its connection to `discover`, as one made in a terminal.
            let mut current = self.attempt.lock().unwrap();
            if current.id == attempt {
                current.made = None;
                current.undo = Undo::default();
            }
        }
        if let Err(message) = connected {
            self.fail(
                attempt,
                &LoginError {
                    state: "error",
                    message,
                },
            );
        }
    }

    /// The browser's return, traded for a renewable session. When the trade
    /// fails, the handoff session is kept so sign-in still completes; it
    /// lasts until its access token expires.
    async fn browser_session(
        &self,
        login: &BrowserLogin,
        site: &str,
    ) -> Result<CloudSession, LoginError> {
        let handoff = tokio::time::timeout(BROWSER_WAIT, login.receive(site))
            .await
            .map_err(|_| LoginError {
                state: "expired",
                message: "Browser sign-in timed out. Try again.".into(),
            })?;
        Ok(crate::cloud_api::exchange_handoff(&handoff, login.verifier())
            .await
            .unwrap_or(handoff))
    }

    async fn connect(
        self: &Arc<Self>,
        attempt: u64,
        epoch: u64,
        site: &str,
        session: &CloudSession,
        request: &SignInRequest,
    ) -> Result<(), String> {
        let mut active_session = session.clone();
        let mut identity = cloud_session::whoami(site, &active_session.access_token).await;
        if matches!(identity, Err(CloudError::Rejected))
            && self
                .load_session()
                .is_some_and(|kept| kept.access_token == active_session.access_token)
        {
            // Rotation must finish even if the awaiting setup is cancelled.
            let account = self.clone();
            let (at, refused) = (site.to_string(), active_session.access_token.clone());
            let refreshed = tokio::spawn(async move { account.renew(&at, &refused).await })
                .await
                .unwrap_or(Err(CloudError::Unreachable));
            match refreshed {
                Ok(session) => {
                    active_session = session;
                    identity = cloud_session::whoami(site, &active_session.access_token).await;
                }
                Err(error) => identity = Err(error),
            }
        }
        if matches!(identity, Err(CloudError::Rejected)) && request.flow.is_none() {
            // Terminally rejected saved credentials continue this same attempt
            // in the browser.
            let login = BrowserLogin::bind().await.map_err(|error| error.message)?;
            let url = login
                .authorization_url_for_workspace(site, true, request.workspace.as_deref())
                .ok_or("The Agent Relay site must be an https origin.")?;
            self.set_link(attempt, Some(browser_link(url)));
            self.set(attempt, |account| {
                account.state = SignInState::PendingApproval;
                account.phase = Some("waiting_for_browser".into());
                account.reused_login = Some(false);
            });
            active_session = match self.browser_session(&login, site).await {
                Ok(session) => session,
                Err(error) => {
                    self.fail(attempt, &error);
                    return Ok(());
                }
            };
            self.set_link(attempt, None);
            self.set(attempt, |account| {
                account.state = SignInState::Preparing;
                account.phase = Some("connecting_workspace".into());
            });
            identity = cloud_session::whoami(site, &active_session.access_token).await;
        }
        let mut identity = identity.map_err(|error| error.to_string())?;
        let was_kept = self
            .load_session()
            .is_some_and(|saved| saved.access_token == active_session.access_token);
        let (workspace, waited) = self
            .choose_workspace(
                attempt,
                site,
                &active_session,
                &identity,
                request.workspace.as_deref(),
                request.flow != Some(Flow::Device),
            )
            .await?;
        let Some(workspace) = workspace else {
            return Ok(self.fail(attempt, &LoginError::no_workspace()));
        };
        if waited {
            if was_kept {
                active_session = self.load_session().ok_or("Sign-in was cancelled.")?;
            }
            // A person may leave this screen open past access-token expiry.
            // Verify again, using the latest saved bearer after background rotation.
            let mut checked = cloud_session::whoami(site, &active_session.access_token).await;
            if matches!(checked, Err(CloudError::Rejected)) {
                active_session = self
                    .refresh_setup_session(&active_session, was_kept)
                    .await?;
                checked = cloud_session::whoami(site, &active_session.access_token).await;
            }
            identity = checked.map_err(|error| error.to_string())?;
        }
        let rebind = identity.current_workspace_id.as_deref() != Some(workspace.as_str());
        if rebind {
            let mut selected = crate::cloud_api::select_desktop_workspace(
                site,
                &active_session.access_token,
                &workspace,
            )
            .await;
            if matches!(selected, Err(CloudError::Rejected)) {
                active_session = self
                    .refresh_setup_session(&active_session, was_kept)
                    .await?;
                selected = crate::cloud_api::select_desktop_workspace(
                    site,
                    &active_session.access_token,
                    &workspace,
                )
                .await;
            }
            active_session = selected.map_err(|error| error.to_string())?;
            identity = cloud_session::whoami(site, &active_session.access_token)
                .await
                .map_err(|error| error.to_string())?;
            if identity.current_workspace_id.as_deref() != Some(workspace.as_str()) {
                return Err("Agent Relay did not connect the selected workspace.".into());
            }
        }
        let session = &active_session;
        if !validation::valid_id(&identity.user_id) || !validation::valid_id(&workspace) {
            return Err("Agent Relay returned an account this app cannot use.".into());
        }
        if self.epoch.load(Ordering::SeqCst) != epoch {
            return Ok(());
        }
        {
            // Recorded with the change, under the attempt's lock: a cancel
            // restores exactly what this attempt replaced.
            // Wait for any earlier token rotation before capturing the undo
            // session, so a later cancellation restores usable credentials.
            let _refresh = self.refreshing.lock().await;
            let mut current = self.attempt.lock().unwrap();
            if current.id != attempt || self.epoch.load(Ordering::SeqCst) != epoch {
                return Ok(());
            }
            let before = self.load_session();
            if !(was_kept && !rebind) && before.as_ref() != Some(session) {
                self.keep_session(session)?;
                current.undo.session.get_or_insert(before);
            }
            let before = self.identity.lock().unwrap().replace(identity.clone());
            current.undo.identity.get_or_insert(before);
        }
        self.adopt_workspace(attempt, site, &session.access_token, &workspace)
            .await;
        self.publish_workspace_plan(identity.current_workspace_plan.clone());
        self.set(attempt, |account| {
            account.workspace_id = Some(workspace.clone());
            account.person = Some(AccountPerson {
                id: Some(identity.user_id.clone()),
                name: identity.name.clone(),
                email: identity.email.clone(),
                avatar_url: identity.avatar_url.clone(),
            });
        });
        let target = Target {
            site_url: site.to_string(),
            account_id: None,
            workspace_id: Some(workspace.clone()),
        };
        // Read before the credential lease below: a read may renew it.
        let (install, enforced) = self.operator_policy_install(&target, request).await;
        // Background Cloud reads may rotate the token while adopting the
        // workspace. Read the latest credential under the refresh lock, then
        // keep it stable until the child has exchanged it for a History session.
        let mut credential_lease = Some(self.refreshing.lock().await);
        if self.attempt.lock().unwrap().id != attempt || self.epoch.load(Ordering::SeqCst) != epoch
        {
            return Ok(());
        }
        let setup_session = self.load_session().unwrap_or_else(|| session.clone());
        let mut run = self
            .uploader
            .runner()
            .install(install, Some(setup_session.access_token.clone()))
            .map_err(|error| error.message)?;
        let mut expected = target.clone();
        while let Some(event) = run.events.recv().await {
            self.operator_policy_event(&workspace, &event);
            self.install_event(attempt, &event, &mut expected);
            if event["event"] == "connected" {
                credential_lease.take();
            }
        }
        credential_lease.take();
        let outcome = run.finished.await.unwrap_or_else(|_| {
            Err(UploaderError::new(
                "Agent Relay could not finish signing in. Please try again.",
            ))
        });
        drop(run.cancel);
        if self.attempt.lock().unwrap().id != attempt || self.epoch.load(Ordering::SeqCst) != epoch
        {
            return Ok(());
        }
        if let Err(error) = outcome {
            if error.cancelled {
                return Err("Sign-in was cancelled.".into());
            }
            let action = error.needs_schedule_acknowledgement();
            if action {
                self.track(
                    TelemetryOperation::SignIn,
                    TelemetryOutcome::RequiresAction,
                    Some(TelemetryFailure::ScheduleInspection),
                );
            } else {
                self.track(
                    TelemetryOperation::HistorySetup,
                    TelemetryOutcome::Failed,
                    Some(TelemetryFailure::HelperFailed),
                );
            }
            self.set(attempt, |account| {
                account.state = SignInState::Error;
                account.phase = None;
                account.progress = None;
                account.error = Some(error.message.clone());
                account.action = action.then_some(AccountAction::AcknowledgeUninspectedSchedules);
            });
            return Ok(());
        }
        let found = self
            .uploader
            .installs(site, Some(&expected))
            .await
            .map_err(|error| error.message)?
            .ok_or("Setup finished, but Agent Relay could not find the connection it created. Please try again.")?;
        {
            // Attached under the attempt's lock: a cancel either takes the
            // connection to disconnect first, or finds it attached.
            let mut current = self.attempt.lock().unwrap();
            if current.id != attempt || self.epoch.load(Ordering::SeqCst) != epoch {
                return Ok(());
            }
            current.made = None;
            current.undo = Undo::default();
            self.uploader.attach(Some(found.clone()));
        }
        self.show_install(Some(&found));
        self.track_sign_in_completed();
        self.enforce_at_sign_in(&found, request, enforced).await;
        let status = self.status();
        self.announce(&status);
        self.track(
            TelemetryOperation::HistorySetup,
            TelemetryOutcome::Succeeded,
            None,
        );
        self.signed_in().await;
        Ok(())
    }

    /// This account's contractor access.
    pub(crate) fn contractors(
        &self,
        core: &Arc<Core>,
    ) -> Arc<crate::contractor_access::ContractorAccess> {
        self.contractors
            .get_or_init(|| crate::contractor_access::for_core(core))
            .clone()
    }

    /// After the uploader connects: the person's contractor access, and on
    /// Linux launch at login, which the app has always turned on at its first
    /// sign-in to a release profile's site.
    async fn signed_in(&self) {
        let Some(core) = self.core() else { return };
        // Messaging's push channel follows the account.
        core.sync_agents().await;
        let cloud = core.plugins.cloud.as_ref();
        if let Err(error) = self.contractors(&core).adopt(cloud).await {
            if error.data.kind != ErrorKind::NotSignedIn {
                core.hub.activity(Level::Warning, error.message);
            }
        }
        let config = core.config();
        // The account's site: the build's in a shipped core, a fake Cloud's
        // (a loopback http origin) in a test, whose sign-in must not touch
        // login items; nor does a development run's (`site_choices`).
        if cfg!(not(target_os = "macos"))
            && self.launch_at_login_after_sign_in
            && !site_choices::development_site(&self.site_url)
            && !config.env.contains_key("RELAY_DESKTOP_PROBE")
        {
            launch_at_login_once(&config.app_data_dir, || {
                crate::login_item::enabled(&config.home, &config.env)
                    || crate::login_item::set(&config.home, &config.env, true).is_ok()
            })
            .unwrap_or_else(|| {
                self.track(
                    TelemetryOperation::LaunchAtLogin,
                    TelemetryOutcome::Failed,
                    Some(TelemetryFailure::SystemSetting),
                )
            });
        }
    }

    fn install_event(&self, attempt: u64, event: &Value, expected: &mut Target) {
        match event["event"].as_str() {
            Some("approval") => {
                self.set_link(attempt, Some(progress::approval_link(event)));
                self.set(attempt, |account| {
                    account.state = SignInState::PendingApproval;
                    account.phase = Some("waiting_for_device_approval".into());
                });
            }
            Some("authenticated" | "connected") => {
                if self.status().phase.as_deref() != Some("preparing_local_sessions") {
                    self.track(
                        TelemetryOperation::SignIn,
                        TelemetryOutcome::Succeeded,
                        None,
                    );
                    self.track(
                        TelemetryOperation::HistorySetup,
                        TelemetryOutcome::Started,
                        None,
                    );
                }
                if let (Some(account_id), Some(workspace)) =
                    (event["account_id"].as_str(), event["workspace_id"].as_str())
                {
                    expected.account_id = Some(account_id.into());
                    expected.workspace_id = Some(workspace.into());
                }
                // From `connected` on, this computer's connection exists.
                if event["event"] == "connected" {
                    let mut current = self.attempt.lock().unwrap();
                    if current.id == attempt {
                        current.made = Some(expected.clone());
                    }
                }
                self.set_link(attempt, None);
                self.set(attempt, |account| {
                    account.state = SignInState::Preparing;
                    account.phase = Some("preparing_local_sessions".into());
                    if let Some(person) = account.person.as_mut() {
                        person.id = event["account_id"]
                            .as_str()
                            .map(str::to_string)
                            .or(person.id.take());
                        if let Some(email) = event["account_email"].as_str() {
                            person.email = Some(email.into());
                        }
                        if let Some(name) = event["account_name"].as_str() {
                            person.name = Some(name.into());
                        }
                        if let Some(avatar) = event["account_avatar_url"].as_str() {
                            person.avatar_url = Some(avatar.into());
                        }
                    }
                });
            }
            Some("progress") if event["progress"].is_object() => {
                let progress = capture_progress(&event["progress"]);
                self.set(attempt, |account| account.progress = Some(progress));
            }
            Some("ready") => {
                self.set(attempt, |account| account.progress = None);
            }
            _ => {}
        }
    }

    /// Resolves the account workspace's Relaycast key into the agent store,
    /// so agents work without a pasted key. A workspace that shares none is
    /// reported, not fatal: the uploader still connects.
    async fn adopt_workspace(&self, attempt: u64, site: &str, bearer: &Secret, workspace: &str) {
        let Some(core) = self.core() else { return };
        if !crate::cloud_api::is_workspace_uuid(workspace) {
            return;
        }
        let epoch = self.epoch.load(Ordering::SeqCst);
        match crate::cloud_api::resolve_workspace(site, bearer, workspace).await {
            Ok(resolved) => {
                if self.epoch.load(Ordering::SeqCst) != epoch {
                    return;
                }
                let name = resolved.name.clone();
                {
                    let mut current = self.attempt.lock().unwrap();
                    if current.id != attempt {
                        return;
                    }
                    let before = core.store.read(|s| s.cloud.clone());
                    if before.as_ref() != Some(&resolved) {
                        let before_clears = core
                            .store
                            .read(|s| s.desktop_identity_clears_pending.clone());
                        if let Err(error) = core.store.update(|s| {
                            let changed_workspace = s
                                .cloud
                                .as_ref()
                                .and_then(|cloud| cloud.account_workspace_id.as_deref())
                                != resolved.account_workspace_id.as_deref();
                            if changed_workspace {
                                s.queue_desktop_identity_clear(false);
                            }
                            s.cloud = Some(resolved);
                        }) {
                            core.hub.activity(Level::Warning, error.to_string());
                            return;
                        }
                        current.undo.workspace.get_or_insert(before);
                        current
                            .undo
                            .desktop_identity_clears
                            .get_or_insert(before_clears);
                    }
                }
                self.update(|account| account.workspace_name = name.clone());
                core.refresh_workspace().await;
            }
            Err(error) => core.hub.activity(Level::Warning, error.to_string()),
        }
    }

    /// Stops a sign-in in flight. One already finished stays signed in; one
    /// that had not leaves nothing behind: the session and workspace key it
    /// kept are restored to what they were, and a connection it made is
    /// disconnected.
    pub fn cancel(self: &Arc<Self>) {
        let (queued, workspace) = self.stop_attempt(true);
        if !queued && !workspace {
            return;
        }
        let account = self.clone();
        tokio::spawn(async move {
            if workspace {
                if let Some(core) = account.core() {
                    core.refresh_workspace().await;
                }
            }
            if let Err(error) = account.finish_pending_disconnect().await {
                if let Some(core) = account.core() {
                    core.hub.activity(Level::Warning, error.message);
                }
            }
        });
    }

    /// Stops the attempt in flight, restores what it replaced when `undo`,
    /// and queues the connection it made for `finish_pending_disconnect`.
    /// Until that disconnect finishes, `discover` leaves the connection
    /// alone. Answers whether a connection was queued and whether the
    /// workspace key was restored.
    fn stop_attempt(&self, undo: bool) -> (bool, bool) {
        let (task, made, replaced) = {
            let mut current = self.attempt.lock().unwrap();
            current.id += 1;
            current.link = None;
            current.workspace_selection = None;
            let replaced = std::mem::take(&mut current.undo);
            (current.task.take(), current.made.take(), replaced)
        };
        if let Some(task) = task {
            task.abort();
        }
        let mut workspace = false;
        if undo {
            if let Some(session) = replaced.session {
                if let Err(error) = self.save_session(session.as_ref()) {
                    if let Some(core) = self.core() {
                        core.hub.activity(Level::Warning, error);
                    }
                }
            }
            if let Some(identity) = replaced.identity {
                *self.identity.lock().unwrap() = identity;
            }
            if let Some(core) = self.core() {
                let restores_workspace = replaced.workspace.is_some();
                let restores_clears = replaced.desktop_identity_clears.is_some();
                match core.store.update(|s| {
                    if let Some(cloud) = replaced.workspace {
                        s.cloud = cloud;
                    }
                    if let Some(clears) = replaced.desktop_identity_clears {
                        s.desktop_identity_clears_pending = clears;
                    }
                }) {
                    Ok(_) => workspace = true,
                    Err(error) => core.hub.activity(Level::Warning, error.to_string()),
                }
                workspace = workspace && (restores_workspace || restores_clears);
            }
        }
        let queued = made.is_some();
        if let Some(target) = made {
            *self.pending_disconnect.lock().unwrap() = Some(target);
        }
        let install = self.uploader.install();
        if install.is_some() {
            self.show_install(install.as_ref());
        } else {
            let status = self.update(|account| *account = AccountStatus::default());
            self.announce(&status);
        }
        (queued, workspace)
    }

    /// Disconnects an install a sign-out left connected.
    async fn finish_pending_disconnect(&self) -> Result<(), RpcError> {
        let _one = self.disconnecting.lock().await;
        let Some(target) = self.pending_disconnect.lock().unwrap().clone() else {
            return Ok(());
        };
        self.track(
            TelemetryOperation::Disconnect,
            TelemetryOutcome::Started,
            None,
        );
        let result = async {
            self.uploader.runner().cancel_all().await?;
            self.uploader
                .disconnect_target(&target)
                .await
                .map_err(|e| UploaderError::new(e.message))
        }
        .await;
        match result {
            Ok(_) => {
                self.track(
                    TelemetryOperation::Disconnect,
                    TelemetryOutcome::Succeeded,
                    None,
                );
                *self.pending_disconnect.lock().unwrap() = None;
                Ok(())
            }
            Err(error) => {
                self.track(
                    TelemetryOperation::Disconnect,
                    TelemetryOutcome::Failed,
                    Some(TelemetryFailure::HelperFailed),
                );
                Err(RpcError::new(
                ErrorKind::Refused,
                format!(
                    "Could not finish disconnecting this computer. Uploads may still be running. Sign in again to retry. {}",
                    error.message
                ),
            ))
            }
        }
    }

    // Workspace.

    /// Connects the account's workspace: resolves its Relaycast key with a
    /// kept session, or, when there is none, the browser sign-in (instant
    /// in a browser already signed in).
    pub async fn connect_workspace(
        self: &Arc<Self>,
        workspace_id: &str,
    ) -> Result<WorkspaceStatus, RpcError> {
        if !crate::cloud_api::is_workspace_uuid(workspace_id) {
            return Err(RpcError::invalid_params(
                "That is not an Agent Relay workspace.",
            ));
        }
        let site = self
            .account()
            .map(|account| account.site_url)
            .unwrap_or_else(|| self.site_url.clone());
        let epoch = self.epoch.load(Ordering::SeqCst);
        let bearer = match self.bearer(&site).await {
            Some((bearer, _)) => bearer,
            None => self.browser_bearer(&site).await?,
        };
        if self.epoch.load(Ordering::SeqCst) != epoch {
            return Err(RpcError::new(
                ErrorKind::NotSignedIn,
                "Signed out of Agent Relay.",
            ));
        }
        let core = self.core().ok_or_else(|| {
            RpcError::new(ErrorKind::ShuttingDown, "Agent Relay core is stopping.")
        })?;
        let resolved = crate::cloud_api::resolve_workspace(&site, &bearer, workspace_id)
            .await
            .map_err(|error| match error {
                crate::cloud_api::ResolveError::Cloud(error) => RpcError::cloud(&error),
                other => failed(other.to_string()),
            })?;
        let name = resolved.name.clone();
        core.store
            .update(|s| {
                let changed_workspace = s
                    .cloud
                    .as_ref()
                    .and_then(|cloud| cloud.account_workspace_id.as_deref())
                    != resolved.account_workspace_id.as_deref();
                if changed_workspace {
                    s.queue_desktop_identity_clear(false);
                }
                s.cloud = Some(resolved);
            })
            .map_err(|error| RpcError::store(&error))?;
        self.update(|account| {
            if account.workspace_id.as_deref() == Some(workspace_id) {
                account.workspace_name = name;
            }
        });
        let status = core.refresh_workspace().await;
        core.agents_changed();
        self.refresh_attached_operator_policy().await;
        Ok(status)
    }

    /// A browser sign-in used only for its session: the person approves in
    /// the browser, and the session is kept.
    async fn browser_bearer(&self, site: &str) -> Result<Secret, RpcError> {
        let login = BrowserLogin::bind().await.map_err(|e| failed(e.message))?;
        let url = login.authorization_url(site, true).ok_or_else(|| {
            RpcError::new(
                ErrorKind::Refused,
                "The Agent Relay site must be an https origin.",
            )
        })?;
        let attempt = {
            let mut current = self.attempt.lock().unwrap();
            current.id += 1;
            current.link = Some(browser_link(url));
            current.id
        };
        let before = self.status();
        self.set(attempt, |account| {
            account.state = SignInState::PendingApproval;
            account.phase = Some("waiting_for_browser".into());
        });
        let epoch = self.epoch.load(Ordering::SeqCst);
        // Cancelled by `account.sign_in.cancel`, which moves the attempt on.
        let watch = async {
            loop {
                tokio::time::sleep(Duration::from_millis(200)).await;
                if self.attempt.lock().unwrap().id != attempt {
                    return;
                }
            }
        };
        let session = tokio::select! {
            session = self.browser_session(&login, site) => Some(session),
            _ = watch => None,
        };
        let finished = self.attempt.lock().unwrap().id == attempt;
        if finished {
            self.attempt.lock().unwrap().link = None;
            self.set(attempt, |account| *account = before);
        }
        let session = session
            .ok_or_else(|| RpcError::new(ErrorKind::NotSignedIn, "Sign-in was cancelled."))?
            .map_err(|error| RpcError::new(ErrorKind::NotSignedIn, error.message))?;
        if self.epoch.load(Ordering::SeqCst) != epoch {
            return Err(RpcError::new(
                ErrorKind::NotSignedIn,
                "Signed out of Agent Relay.",
            ));
        }
        self.keep_session(&session)
            .map_err(|message| RpcError::new(ErrorKind::StoreFailed, message))?;
        Ok(session.access_token)
    }
}

/// Turns launch at login on the first time only (as the macOS app's
/// `loginItemConfigured` did): a person who turns it off keeps it off
/// across later sign-ins. `None` when turning it on failed.
pub fn launch_at_login_once(
    app_data_dir: &std::path::Path,
    enable: impl FnOnce() -> bool,
) -> Option<()> {
    let marker = app_data_dir.join("launch-at-login-configured");
    if marker.exists() {
        return Some(());
    }
    // Recorded only once it took: a failed attempt is tried again at the
    // next sign-in.
    enable().then_some(())?;
    let _ = agent_relay_node::platform::fs::write_private(&marker, b"");
    Some(())
}


impl CloudAccess for Account {
    fn may_sync_identities(&self) -> bool {
        self.identity_sync_allowed()
    }

    fn identity_sync_generation(&self) -> u64 {
        self.attempt
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .id
    }

    fn agent_owner(&self) -> Option<AgentOwner> {
        let install = self.uploader.install()?;
        Some(AgentOwner {
            user_id: install.target.account_id?,
            workspace_id: install.target.workspace_id,
        })
    }

    fn account(&self) -> Option<CloudAccount> {
        let identity = self.identity.lock().unwrap().clone();
        if let Some(install) = self.uploader.install() {
            return Some(CloudAccount {
                site_url: install.target.site_url,
                workspace_id: install.target.workspace_id?,
                user_id: install.target.account_id.or(identity.map(|i| i.user_id)),
            });
        }
        let session = self.load_session()?;
        let workspace = self.core()?.store.read(|s| {
            s.cloud
                .as_ref()
                .and_then(|c| c.account_workspace_id.clone())
        })?;
        Some(CloudAccount {
            site_url: session.site_url,
            workspace_id: workspace,
            user_id: identity.map(|i| i.user_id),
        })
    }

    fn credential(&self, refused: Option<Secret>) -> BoxFuture<'_, Option<(String, Secret)>> {
        Box::pin(self.pinned_credential(refused))
    }

    fn request(
        &self,
        method: &'static str,
        path: String,
        body: Option<Value>,
    ) -> BoxFuture<'_, Result<Value, CloudError>> {
        Box::pin(async move {
            let site = self.account().ok_or(CloudError::SignedOut)?.site_url;
            let answer = async {
                let (bearer, kept) = self.request_bearer(&site).await?;
                match cloud_session::api(&site, &bearer, method, &path, body.clone()).await {
                    Err(CloudError::Rejected) if kept => {
                        // Revoked or expired server-side despite looking fresh:
                        // renew once; a session Cloud refuses is forgotten, one
                        // it could not answer for is kept.
                        let renewed = self.renew(&site, &bearer).await?;
                        let retried =
                            cloud_session::api(&site, &renewed.access_token, method, &path, body)
                                .await;
                        if matches!(retried, Err(CloudError::Rejected)) {
                            self.note_refused(&renewed);
                        }
                        retried
                    }
                    other => other,
                }
            }
            .await;
            self.note_cloud_answer(&answer);
            answer
        })
    }
}

/// `plugins` with this account as the Cloud access: the uploader over this
/// very binary, telemetry (`telemetry::posthog`; a site build's own
/// collector takes this account's session), and the settings
/// telemetry, contractor access and session history retention own. The
/// account signs in to the site this build was made for
/// (`links::built_in_site_url`).
pub fn with_account(
    mut plugins: agent_relay_node::relay::daemon::Plugins,
    config: &agent_relay_node::relay::daemon::Config,
    headless: bool,
) -> agent_relay_node::relay::daemon::Plugins {
    let installer = plugin::mcp_installer(config, &mut plugins);
    let executable = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("agent-relay-probe"));
    let uploader = Arc::new(Uploader::new(Arc::new(
        // The probe's own session renewal reads the Cloud session from this
        // build's store (Agent Relay Dev keeps its own). Only a default core
        // replaces a collector another build of its release launched.
        crate::history::uploader::ProcessRunner::new(executable.clone())
            .with_env(
                crate::history::cloud::APP_DATA_DIR_VARIABLE,
                &config.app_data_dir,
            )
            .replacing_same_release(site_choices::default_core(config)),
    )));
    let mut account = Account::new(&config.env, &config.app_data_dir, uploader.clone());
    let mut telemetry_config = telemetry::TelemetryConfig::new(
        config.app_data_dir.clone(),
        config.home.clone(),
        config.env.clone(),
        config.version.clone(),
    );
    // Keyed on the site this core is built for and whether a telemetry key
    // was built in (`site_choices`, tested with production, a customer's
    // site and a development one).
    let project = telemetry_config.project.take();
    let version = config.version.as_str();
    let choices =
        site_choices::for_build(built_in_site_url(), project.is_some(), &config.env, version);
    telemetry_config.project = project.filter(|_| choices.telemetry_project);
    account.command_line_copy = choices
        .command_line_copy
        .then(|| site_choices::command_line_copy(executable, config));
    account.launch_at_login_after_sign_in = !headless;
    plugin::session_locations(config, &mut plugins, &mut account);
    plugins.spawn_callers = Some(account.service_callers.plugin());
    let telemetry = Telemetry::production(telemetry_config);
    account.telemetry = Some(telemetry.clone());
    let account = account.shared();
    plugins.cloud = account.clone();
    plugin::coordination_advisories(config, &mut plugins, &account);
    plugins.settings.push(telemetry);
    let app_data = &config.app_data_dir;
    plugins.settings.push(Arc::new(crate::contractor_access::Setting::new(app_data)));
    plugins.settings.push(crate::history::history_retention::setting(app_data, &uploader));
    plugins.session_policy = Some(Arc::new(crate::history::relay_uploads::UploaderPolicy {
        uploader: uploader.clone(),
        uploads: account.relay_uploads.clone(),
    }));
    plugins.history_version = Some(crate::history::HISTORY_VERSION);
    let _ = account.mcp_config.set(installer);
    plugins.extensions.push(Arc::new(plugin::AccountMethods(account.clone())));
    plugins.account = Some(account.into_plugin());
    plugins
}

/// A development build's private client profiles; `None` in production.
pub fn client_profile(
    config: &agent_relay_node::relay::daemon::Config,
) -> Option<agent_relay_node::relay::local::ClientProfile> {
    let clients = agent_relay_node::relay::local::is_development(config)
        .then(|| crate::mcp_settings::development::Clients::new(config))?;
    Some(agent_relay_node::relay::local::ClientProfile {
        home: clients.home.clone(),
        env: clients.env.clone(),
        agent_environment: clients.agent_environment(None),
        safe: clients.safe(),
    })
}

#[cfg(test)]
mod tests {
    mod command_line_copy;
    use super::*;

    #[test]
    fn desktop_startup_keeps_the_account_and_its_command_line_copy() {
        use agent_relay_node::relay::daemon::{Config, Plugins};
        use agent_relay_node::relay::paths::CorePaths;

        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            paths: CorePaths::for_socket(dir.path().join("core.sock")).unwrap(),
            home: dir.path().to_path_buf(),
            app_data_dir: dir.path().join("app-data"),
            env: Default::default(),
            version: "0.0.0-test".into(),
            host_name: "Test Desk.local".into(),
            rediscover_every: Duration::from_secs(3600),
            push_timing: Default::default(),
            delivery_timing: Default::default(),
        };
        // Both native apps use this factory, for the site the build is for.
        // CI runs this without AGENT_RELAY_SITE_URL, where the built-in site
        // is production; that a non-default site flows through the factory
        // is what the Linux end-to-end lane proves (linux_app_core_e2e).
        // The build-keyed choices themselves (no command-line copy for a
        // development site, telemetry only with a key built in) are
        // `site_choices`' tests; that they reach the account is asserted
        // below for whichever site this suite is built for.
        let plugins = with_account(Plugins::default(), &config, false);
        let account = plugins
            .account
            .expect("Cloud sign-in remains available")
            .as_any()
            .downcast::<Account>()
            .unwrap();
        assert_eq!(account.site_url(), built_in_site_url());
        // A release profile's site copies the command line unless a probe
        // is named in the environment; a development site never does.
        assert_eq!(
            account.command_line_copy.is_some(),
            !site_choices::development_site(built_in_site_url())
        );
        let named = Config {
            env: std::collections::BTreeMap::from([(
                "RELAY_DESKTOP_PROBE".to_string(),
                "/x".to_string(),
            )]),
            ..config
        };
        let plugins = with_account(Plugins::default(), &named, false);
        let account = plugins
            .account
            .unwrap()
            .as_any()
            .downcast::<Account>()
            .unwrap();
        assert!(account.command_line_copy.is_none());
    }

    #[test]
    fn with_site_takes_an_origin_and_refuses_anything_else() {
        let dir = tempfile::tempdir().unwrap();
        let account = || {
            Account::new(
                &Default::default(),
                &dir.path().join("app-data"),
                Arc::new(Uploader::new(Arc::new(
                    crate::history::uploader::ProcessRunner::new(
                        dir.path().join("no-probe"),
                    ),
                ))),
            )
        };
        assert_eq!(account().site_url(), built_in_site_url());
        assert_eq!(
            account()
                .with_site("http://127.0.0.1:1")
                .unwrap()
                .site_url(),
            "http://127.0.0.1:1"
        );
        for site in ["not a url", "ftp://x", "https://x/path", ""] {
            assert!(account().with_site(site).is_err(), "{site}");
        }
    }

    #[test]
    fn agent_runner_preserves_the_generic_environment_hook() {
        use agent_relay_node::relay::claude::{CommandRunner, RunOutcome, SystemRunner};

        let dir = tempfile::tempdir().unwrap();
        // npm's `claude.cmd` on Windows; its environment hook still sees
        // the command as `claude`.
        #[cfg(windows)]
        let program = {
            let program = dir.path().join("claude.cmd");
            std::fs::write(&program, "@echo off\r\necho %RELAY_TEST_MARK%\r\n").unwrap();
            program
        };
        #[cfg(unix)]
        let program = {
            use std::os::unix::fs::PermissionsExt;
            let program = dir.path().join("claude");
            std::fs::write(&program, "#!/bin/sh\necho \"$RELAY_TEST_MARK\"\n").unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            program
        };
        let runner = SystemRunner {
            environment: Some(Arc::new(
                |cli: &str, mut base: std::collections::BTreeMap<String, String>| {
                    base.insert("RELAY_TEST_MARK".into(), format!("launched {cli}"));
                    base
                },
            )),
        };
        match runner.run(&program, &[], b"", Duration::from_secs(5)) {
            RunOutcome::Exited { stdout, .. } => {
                assert_eq!(String::from_utf8_lossy(&stdout).trim(), "launched claude")
            }
            _ => panic!("the stand-in ran"),
        }
    }

    #[test]
    fn launch_at_login_is_turned_on_at_the_first_sign_in_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut calls = 0;
        assert_eq!(
            launch_at_login_once(dir.path(), || {
                calls += 1;
                true
            }),
            Some(())
        );
        assert_eq!(
            launch_at_login_once(dir.path(), || {
                calls += 1;
                true
            }),
            Some(())
        );
        assert_eq!(calls, 1);
        let failing = tempfile::tempdir().unwrap();
        assert_eq!(launch_at_login_once(failing.path(), || false), None);
        assert_eq!(launch_at_login_once(failing.path(), || true), Some(()));
    }

    #[test]
    fn upload_health_reads_delivery_then_the_cycle() {
        let running = |fields: Value| {
            let mut status = json!({"running": true, "paused": false, "delivery": {"failure": null}, "last_cycle": {"ok": true}});
            if let (Value::Object(base), Value::Object(fields)) = (&mut status, fields) {
                base.extend(fields);
            }
            status
        };
        assert_eq!(upload_health(&running(json!({}))), Some(None));
        assert_eq!(
            upload_health(&running(json!({"delivery": {"failure": "offline"}}))),
            Some(Some(TelemetryFailure::UploadDelivery))
        );
        assert_eq!(
            upload_health(&running(json!({"last_cycle": {"ok": false}}))),
            Some(Some(TelemetryFailure::UploadCycle))
        );
        assert_eq!(
            upload_health(&running(
                json!({"last_cycle": {"ok": false}, "upgrading": true})
            )),
            Some(None)
        );
        assert_eq!(upload_health(&running(json!({"paused": true}))), None);
        assert_eq!(upload_health(&running(json!({"running": false}))), None);
        // The allowlisted code names the delivery failure.
        assert_eq!(
            upload_health(&running(json!({"delivery": {"failure": "Sign in again.",
                "failure_code": "authentication_required"}}))),
            Some(Some(TelemetryFailure::UploadAuthenticationRequired))
        );
        assert_eq!(
            upload_health(&running(json!({"delivery": {"failure": "Slow down.",
                "failure_code": "rate_limited"}}))),
            Some(Some(TelemetryFailure::UploadRateLimited))
        );
        // A queue waiting with no acknowledgement and no failure is unhealthy.
        assert_eq!(
            upload_health(&running(json!({"stalled": true}))),
            Some(Some(TelemetryFailure::UploadStalled))
        );
    }
}
