//! The registry (`docs/service-callers.md` section 3): one private file per
//! account and workspace, holding each caller's record, its secret digests,
//! its scope and its ownership ledger, and the probe's pending requests.

use super::{Class, Host};
use crate::contract::service_callers::{
    CallerPublisher, CallerRequester, CallerState, CallerSummary,
};
use agent_relay_node::platform::fs as private_fs;
use agent_relay_node::relay::daemon::ExternalCaller;
use agent_relay_node::relay::workspace::{Secret, Workspace};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub(crate) const SCHEMA: u32 = 1;
pub(crate) const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// The ownership ledger keeps the newest this many invocations.
pub(crate) const LEDGER: usize = 1000;
pub(crate) const FOLDER: &str = "ServiceCallers";

/// The account, workspace and files a call is bound to.
#[derive(Clone)]
pub(crate) struct Binding {
    pub(crate) site_url: String,
    pub(crate) workspace_id: String,
    pub(crate) user_id: String,
    pub(crate) workspace: Workspace,
    pub(crate) relaycast_workspace_id: String,
    pub(crate) folder: PathBuf,
    pub(crate) key: String,
}

impl Binding {
    /// The signed-in account and the core's workspace, or `not_signed_in`.
    pub(crate) fn current(host: &dyn Host) -> Result<Binding, Class> {
        let app_data = host.app_data_dir().ok_or(Class::NotSignedIn)?;
        let account = host.account().ok_or(Class::NotSignedIn)?;
        let user_id = account
            .user_id
            .filter(|id| !id.is_empty())
            .ok_or(Class::NotSignedIn)?;
        let workspace = host.workspace().ok_or(Class::NotSignedIn)?;
        let relaycast_workspace_id = workspace
            .workspace_id
            .clone()
            .filter(|id| !id.is_empty())
            .ok_or(Class::NotSignedIn)?;
        if account.site_url.is_empty() || account.workspace_id.is_empty() {
            return Err(Class::NotSignedIn);
        }
        Ok(Binding {
            key: key(&account.site_url, &account.workspace_id),
            site_url: account.site_url,
            workspace_id: account.workspace_id,
            user_id,
            workspace,
            relaycast_workspace_id,
            folder: app_data.join(FOLDER),
        })
    }

    pub(crate) fn registry(&self) -> PathBuf {
        self.folder.join(format!("{}.json", self.key))
    }

    pub(crate) fn results(&self) -> PathBuf {
        self.folder.join(format!("{}.results", self.key))
    }

    pub(crate) fn audit(&self) -> PathBuf {
        self.folder.join(format!("{}.audit.jsonl", self.key))
    }

    pub(crate) fn default_handoff(&self, caller_id: &str) -> PathBuf {
        self.folder
            .join("handoff")
            .join(format!("{caller_id}.json"))
    }

    /// Whether `workspace` is the one this binding was made for.
    pub(crate) fn is(&self, workspace: &Workspace) -> bool {
        workspace.base_url == self.workspace.base_url
            && workspace.workspace_id.as_deref() == Some(self.relaycast_workspace_id.as_str())
    }
}

/// The registry key: the one `cloud_chat::conversation_path` uses.
pub(crate) fn key(site_url: &str, workspace_id: &str) -> String {
    hex(&Sha256::digest(
        format!("{site_url}/{workspace_id}").as_bytes(),
    ))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The digest a secret is kept as.
pub(crate) fn digest(secret: &str) -> String {
    hex(&Sha256::digest(secret.as_bytes()))
}

/// Compares two digests in time independent of where they differ.
pub(crate) fn same(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |differ, (a, b)| differ | (a ^ b))
        == 0
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct SecretDigest {
    pub(crate) sha256: String,
    pub(crate) created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) retires_at_ms: Option<i64>,
}

impl SecretDigest {
    pub(crate) fn live(&self, now: i64) -> bool {
        self.retires_at_ms.is_none_or(|retires| retires > now)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Root {
    pub(crate) given: String,
    pub(crate) resolved: PathBuf,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct LedgerEntry {
    pub(crate) invocation_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) worker_agent_id: Option<String>,
    /// The idempotency key of the reserved inbox slot this invocation is
    /// still to be bound to: written with the entry before the invoke's
    /// claim ends, cleared once the bind is on disk. A bind that failed, or
    /// a crash before it, is written at the caller's next inbox load, so
    /// the worker's report still finds its slot after a restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) slot_key: Option<String>,
}

/// A provision or rotation the probe asked for, waiting for the person.
/// Durable, so it survives app and core restarts.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct PendingChange {
    /// `provision` or `rotation`.
    pub(crate) kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expires_in_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) overlap_seconds: Option<u32>,
}

pub(crate) const PENDING_PROVISION: &str = "provision";
pub(crate) const PENDING_ROTATION: &str = "rotation";

/// One caller. Holds its token, so it has no `Debug`.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct CallerRecord {
    pub(crate) caller_id: String,
    pub(crate) name: String,
    /// Empty while a provision is pending.
    pub(crate) agent_id: String,
    /// `None` while a provision is pending.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) token: Option<Secret>,
    pub(crate) secrets: Vec<SecretDigest>,
    pub(crate) harnesses: Vec<String>,
    pub(crate) directory_roots: Vec<Root>,
    pub(crate) owns_only: bool,
    pub(crate) enabled: bool,
    pub(crate) created_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rotated_at_ms: Option<i64>,
    pub(crate) expires_at_ms: i64,
    /// Where `caller.json` is. `None` while a probe-initiated provision is
    /// pending: Desktop picks the place when the person approves it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) handoff_path: Option<PathBuf>,
    /// `app` or `probe`: who asked for the newest provision or rotation.
    pub(crate) requested_by: String,
    pub(crate) requested_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) pending: Option<PendingChange>,
    #[serde(default)]
    pub(crate) invocations: Vec<LedgerEntry>,
    /// Advances on every change of what the caller may do: an applied
    /// rotation and an enable or disable. A connection binding records it
    /// and is stale once it moves (`docs/service-callers-auth.md`
    /// section 4). A record written before it existed reads as 1.
    #[serde(default = "first_generation")]
    pub(crate) authority_generation: u64,
    /// Whether the generation has advanced for the current `expires_at`
    /// (generation advance 5, once per expiry). Cleared whenever
    /// `expires_at` is renewed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) expiry_advanced: bool,
    /// While a rotation is being applied: the record as it was before.
    /// Written with the rotated record, before `caller.json` is replaced,
    /// and cleared once the rotation commits or is rolled back. A record
    /// that still carries it after a failed rollback write or a crash is
    /// settled by the next check against what `caller.json` holds
    /// ([`super::ServiceCallers::load_current`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rollback: Option<Box<CallerRecord>>,
}

fn first_generation() -> u64 {
    1
}

impl CallerRecord {
    /// The secret's digest against every kept digest; an unknown name and a
    /// wrong secret answer alike.
    pub(crate) fn check_secret(&self, secret: &str, now: i64) -> Result<(), Class> {
        let given = digest(secret);
        let mut matched = None;
        for kept in &self.secrets {
            if same(&kept.sha256, &given) {
                matched = Some(kept.live(now));
            }
        }
        match matched {
            Some(true) => Ok(()),
            Some(false) => Err(Class::SecretRetired),
            None => Err(Class::SecretMismatch),
        }
    }

    /// Provisioned, enabled and not expired.
    pub(crate) fn live(&self, now: i64) -> Result<(), Class> {
        if self.token.is_none() || self.agent_id.is_empty() {
            return Err(Class::CallerUnknown);
        }
        if !self.enabled {
            return Err(Class::CallerDisabled);
        }
        if self.expires_at_ms <= now {
            return Err(Class::CallerExpired);
        }
        Ok(())
    }

    pub(crate) fn owns(&self, invocation_id: &str) -> bool {
        self.invocations
            .iter()
            .any(|entry| entry.invocation_id == invocation_id)
    }

    /// Adds `invocation_id` to the ledger, newest last, keeping [`LEDGER`].
    /// Past the limit the oldest settled entry goes first: one whose inbox
    /// slot is still `unsettled`, or is yet to be bound, is kept while a
    /// settled one can go, since only the ledger lets its caller open and
    /// settle that slot. When every older entry is unsettled the oldest
    /// goes anyway, and its id is returned so the caller drops its slot:
    /// an item no one can settle any more must not hold an active slot.
    pub(crate) fn remember(
        &mut self,
        invocation_id: &str,
        worker: Option<&str>,
        unsettled: impl Fn(&str) -> bool,
    ) -> Vec<String> {
        if let Some(entry) = self
            .invocations
            .iter_mut()
            .find(|entry| entry.invocation_id == invocation_id)
        {
            if entry.worker_agent_id.is_none() {
                entry.worker_agent_id = worker.map(str::to_string);
            }
            return Vec::new();
        }
        self.invocations.push(LedgerEntry {
            invocation_id: invocation_id.to_string(),
            worker_agent_id: worker.map(str::to_string),
            slot_key: None,
        });
        let mut dropped = Vec::new();
        while self.invocations.len() > LEDGER {
            let older = self.invocations.len() - 1;
            let settled = self.invocations[..older]
                .iter()
                .position(|entry| entry.slot_key.is_none() && !unsettled(&entry.invocation_id));
            let entry = self.invocations.remove(settled.unwrap_or(0));
            if settled.is_none() {
                dropped.push(entry.invocation_id);
            }
        }
        dropped
    }

    /// Records that `invocation_id`'s inbox slot, reserved under `key`, is
    /// yet to be bound ([`LedgerEntry::slot_key`]).
    pub(crate) fn unbound_slot(&mut self, invocation_id: &str, key: &str) {
        if let Some(entry) = self
            .invocations
            .iter_mut()
            .find(|entry| entry.invocation_id == invocation_id)
        {
            entry.slot_key = Some(key.to_string());
        }
    }

    /// The invocations whose inbox slot is yet to be bound, with its key.
    pub(crate) fn unbound_slots(&self) -> Vec<(String, String)> {
        self.invocations
            .iter()
            .filter_map(|entry| Some((entry.invocation_id.clone(), entry.slot_key.clone()?)))
            .collect()
    }

    /// Forgets the pending binds of `invocation_ids`: they are on disk.
    pub(crate) fn slots_bound<'a>(&mut self, invocation_ids: impl IntoIterator<Item = &'a str>) {
        for invocation_id in invocation_ids {
            if let Some(entry) = self
                .invocations
                .iter_mut()
                .find(|entry| entry.invocation_id == invocation_id)
            {
                entry.slot_key = None;
            }
        }
    }

    pub(crate) fn summary(&self, now: i64) -> CallerSummary {
        CallerSummary {
            caller_id: self.caller_id.clone(),
            name: self.name.clone(),
            publisher: self.publisher(),
            harnesses: self.harnesses.clone(),
            directory_roots: self
                .directory_roots
                .iter()
                .map(|root| root.resolved.display().to_string())
                .collect(),
            owns_only: self.owns_only,
            enabled: self.enabled,
            state: self.state(now),
            requested_by: if self.requested_by == "app" {
                CallerRequester::App
            } else {
                CallerRequester::Probe
            },
            requested_at: super::rfc3339(self.requested_at_ms),
            created_at: super::rfc3339(self.created_at_ms),
            rotated_at: self.rotated_at_ms.map(super::rfc3339),
            expires_at: super::rfc3339(self.expires_at_ms),
            secrets_live: self.secrets.iter().filter(|s| s.live(now)).count() as u32,
        }
    }

    pub(crate) fn state(&self, now: i64) -> CallerState {
        match self.pending.as_ref().map(|pending| pending.kind.as_str()) {
            Some(PENDING_PROVISION) => CallerState::PendingProvision,
            Some(_) => CallerState::PendingRotation,
            None if !self.enabled => CallerState::Disabled,
            None if self.expires_at_ms <= now => CallerState::Expired,
            None => CallerState::Enabled,
        }
    }

    pub(crate) fn pending_provision(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.kind == PENDING_PROVISION)
    }

    pub(crate) fn publisher(&self) -> CallerPublisher {
        CallerPublisher {
            id: self.agent_id.clone(),
            name: self.name.clone(),
        }
    }

    /// What the core acts as for this caller: always scoped, always
    /// owner-only. `None` while a provision is pending.
    pub(crate) fn external(&self) -> Option<ExternalCaller> {
        Some(ExternalCaller {
            agent_id: self.agent_id.clone(),
            name: self.name.clone(),
            token: self.token.clone()?,
            harnesses: Some(self.harnesses.clone()),
            directory_roots: Some(
                self.directory_roots
                    .iter()
                    .map(|root| root.resolved.clone())
                    .collect(),
            ),
            owns_only: true,
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct RegistryFile {
    pub(crate) schema: u32,
    pub(crate) site_url: String,
    pub(crate) workspace_id: String,
    pub(crate) user_id: String,
    pub(crate) base_url: String,
    pub(crate) relaycast_workspace_id: String,
    #[serde(default)]
    pub(crate) callers: Vec<CallerRecord>,
    /// Workers started by invokes whose caller was revoked while they were
    /// in flight, each waiting for its one exact release
    /// ([`super::spawn::Late`]). Kept here so a restart still releases them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) late_workers: Vec<LateEntry>,
}

/// A late worker waiting for its release. Names no secret: the release
/// uses the workspace key of the binding the registry belongs to.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct LateEntry {
    pub(crate) invocation_id: String,
    pub(crate) caller_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent_id: Option<String>,
    pub(crate) since_ms: i64,
}

impl RegistryFile {
    fn new(binding: &Binding) -> RegistryFile {
        RegistryFile {
            schema: SCHEMA,
            site_url: binding.site_url.clone(),
            workspace_id: binding.workspace_id.clone(),
            user_id: binding.user_id.clone(),
            base_url: binding.workspace.base_url.clone(),
            relaycast_workspace_id: binding.relaycast_workspace_id.clone(),
            callers: Vec::new(),
            late_workers: Vec::new(),
        }
    }

    /// The caller `name` and `secret` prove, live.
    pub(crate) fn authenticate(&self, name: &str, secret: &str, now: i64) -> Result<usize, Class> {
        let Some(index) = self.callers.iter().position(|c| c.name == name) else {
            // The same work as a wrong secret.
            let _ = same(&digest(secret), &digest(name));
            return Err(Class::CallerUnknown);
        };
        let caller = &self.callers[index];
        caller.check_secret(secret, now)?;
        caller.live(now)?;
        Ok(index)
    }

    pub(crate) fn by_id(&self, caller_id: &str) -> Option<usize> {
        self.callers.iter().position(|c| c.caller_id == caller_id)
    }
}

/// Reads a private file this user owns, refusing a symbolic link; `None`
/// when there is none.
pub(crate) fn read_private(path: &Path) -> Result<Option<Vec<u8>>, Class> {
    use std::io::Read;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Class::RegistryUnreadable),
    };
    if !metadata.is_file()
        || !private_fs::owned_by_current_user(&metadata)
        || !private_fs::private_to_owner(&metadata, 0o077)
    {
        return Err(Class::RegistryUnreadable);
    }
    let mut file = private_fs::no_follow(std::fs::OpenOptions::new().read(true))
        .open(path)
        .map_err(|_| Class::RegistryUnreadable)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| Class::RegistryUnreadable)?;
    Ok(Some(bytes))
}

pub(crate) fn write(path: &Path, value: &impl Serialize) -> Result<(), Class> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| Class::StoreFailed)?;
    private_fs::write_private(path, &bytes).map_err(|_| Class::StoreFailed)
}

/// The binding's registry. One written for another person is deleted with
/// everything it holds (`account_mismatch`); one for another Relaycast
/// workspace resolves nothing (`workspace_mismatch`). Registries of other
/// workspaces are deleted.
pub(crate) fn load(binding: &Binding) -> Result<RegistryFile, Class> {
    #[cfg(test)]
    LOADS.with(|loads| loads.set(loads.get() + 1));
    let loaded = read_registry(binding);
    remember_read(binding, &loaded);
    loaded
}

/// [`load`] without the cache: what the registry file holds now.
fn read_registry(binding: &Binding) -> Result<RegistryFile, Class> {
    sweep_others(binding);
    let Some(bytes) = read_private(&binding.registry())? else {
        return Ok(RegistryFile::new(binding));
    };
    let file: RegistryFile =
        serde_json::from_slice(&bytes).map_err(|_| Class::RegistryUnreadable)?;
    if file.schema != SCHEMA {
        return Err(Class::RegistryUnreadable);
    }
    if file.user_id != binding.user_id
        || file.site_url != binding.site_url
        || file.workspace_id != binding.workspace_id
    {
        delete_all(binding, Some(&file));
        return Err(Class::AccountMismatch);
    }
    if file.base_url != binding.workspace.base_url
        || file.relaycast_workspace_id != binding.relaycast_workspace_id
    {
        return Err(Class::WorkspaceMismatch);
    }
    Ok(file)
}

pub(crate) fn save(binding: &Binding, file: &RegistryFile) -> Result<(), Class> {
    #[cfg(test)]
    if FAIL_SAVE.with(|fail| fail.get()) {
        return Err(Class::StoreFailed);
    }
    write(&binding.registry(), file)?;
    remember_read(binding, &Ok(file.clone()));
    Ok(())
}

/// The registry each binding's file held when this process last read or
/// wrote it, by registry path. Every read and successful write goes through
/// [`load`] and [`save`], which refresh it, and every delete forgets it, so
/// it is what the file holds; nothing else writes a registry. A bound
/// connection resolves from it alone ([`cached`]), without file I/O.
type Cache = std::collections::HashMap<PathBuf, Result<std::sync::Arc<RegistryFile>, Class>>;

fn cache() -> std::sync::MutexGuard<'static, Cache> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Cache>> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn remember_read(binding: &Binding, loaded: &Result<RegistryFile, Class>) {
    let entry = match loaded {
        Ok(file) => Ok(std::sync::Arc::new(file.clone())),
        Err(class) => Err(*class),
    };
    cache().insert(binding.registry(), entry);
}

/// What [`load`] last gave for `binding`, or the registry [`save`] last
/// wrote, from memory only: no file is read or written. `None` when this
/// process has neither read nor written it since it was deleted.
pub(crate) fn cached(binding: &Binding) -> Option<Result<std::sync::Arc<RegistryFile>, Class>> {
    cache().get(&binding.registry()).cloned()
}

/// Forgets every cached registry under `folder` (sign-out deletes it).
pub(crate) fn forget_cached(folder: &Path) {
    cache().retain(|path, _| !path.starts_with(folder));
}

#[cfg(test)]
thread_local! {
    /// A failpoint: every registry write on this thread fails.
    pub(crate) static FAIL_SAVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Registry loads on this thread, so a test can see that every
    /// `authenticate` failure reads the registry alike.
    pub(crate) static LOADS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Deletes a registry and its inboxes, audit and handoff files.
pub(crate) fn delete_all(binding: &Binding, file: Option<&RegistryFile>) {
    let read;
    let file = match file {
        Some(file) => Some(file),
        None => {
            read = read_private(&binding.registry())
                .ok()
                .flatten()
                .and_then(|bytes| serde_json::from_slice::<RegistryFile>(&bytes).ok());
            read.as_ref()
        }
    };
    if let Some(file) = file {
        for caller in &file.callers {
            super::handoff::remove_kept(caller);
        }
    }
    let _ = std::fs::remove_file(binding.registry());
    cache().remove(&binding.registry());
    let _ = std::fs::remove_dir_all(binding.results());
    let _ = std::fs::remove_file(binding.audit());
    let _ = std::fs::remove_file(binding.audit().with_extension("jsonl.1"));
}

/// Registries of another account or workspace in the same folder: a
/// workspace change ends their callers.
fn sweep_others(binding: &Binding) {
    let Ok(entries) = std::fs::read_dir(&binding.folder) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(key) = name.strip_suffix(".json") else {
            continue;
        };
        if key == binding.key || key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let other = Binding {
            key: key.to_string(),
            ..binding.clone()
        };
        delete_all(&other, None);
    }
}
