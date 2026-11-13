//! Where the installation identity and the session grants live: one private
//! file under the RelayHistory config directory, beside the stage auth files
//! and written the same way (0600, atomic replace). Not in `history.db`: a
//! local session reset deletes that database and mints a new origin, and the
//! installation and its grants must outlive it.
use crate::history::cloud;
use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::PathBuf;

/// A secret string: the installation proof or a grant G. Never printed, and
/// overwritten when dropped.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Secret(String);
impl Secret {
    pub fn new(value: String) -> Self {
        Self(value)
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}
impl Drop for Secret {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.0).into_bytes();
        bytes.iter_mut().for_each(|byte| *byte = 0);
        std::hint::black_box(&bytes);
    }
}
impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}
impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self)
    }
}

/// The exact account an installation and its grants belong to: the trusted
/// site origin (normalized, as the install's config holds it), the Cloud
/// user, and the org and workspace exactly as the stored session names them
/// (`""` for no workspace). Two users of one workspace, or one workspace
/// id on two sites, never share an installation, a grant or an operation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Account {
    /// Absent from a state file written before the account was exact: such
    /// entries are dropped when the file is read ([`Store::load`]).
    #[serde(default)]
    pub site: String,
    #[serde(default)]
    pub user: String,
    pub org: String,
    pub workspace: String,
}
impl Account {
    /// Whether every part that tells two accounts apart is present.
    pub fn exact(&self) -> bool {
        !self.site.is_empty() && !self.user.is_empty() && !self.org.is_empty()
    }
}

/// This installation as registered with one account. The proof is generated
/// and saved before the first registration request, so a lost response is
/// replayed with the same `request_uuid` and proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Installation {
    pub account: Account,
    pub request_uuid: String,
    pub proof: Secret,
    #[serde(default)]
    pub installation_id: Option<String>,
    /// The origin the installation was registered from.
    pub origin_id: String,
    #[serde(default)]
    pub issuer_key_id: Option<String>,
}

/// A session's grant. Keyed by account, source and session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub account: Account,
    pub source: String,
    pub session_id: String,
    pub grant_id: String,
    pub owner_epoch: u64,
    pub grant: Secret,
    /// RFC 3339 UTC; null in v1.
    #[serde(default)]
    pub expires_at: Option<String>,
    /// The origin the grant is bound to. After a reset this differs from the
    /// current origin until the rebind succeeds.
    pub origin_id: String,
}

/// A native issuer operation: the whole request, saved before it is first
/// sent and replayed from this record byte for byte (same uuid, same
/// installation, same origin, same body) until it reaches a final outcome.
/// A busy or transport failure keeps it as it is; only a terminal refusal
/// ends it, and that refusal is kept to pace the next operation, which gets
/// a new uuid. At most one operation per (account, source, session).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Operation {
    pub account: Account,
    /// [`CLAIM`] or [`REBIND`].
    pub kind: String,
    pub source: String,
    pub session_id: String,
    pub operation_uuid: String,
    #[serde(default)]
    pub installation_id: String,
    /// The origin the request names: the claim's origin, or the rebind's new
    /// origin. The current origin when the operation began, which a later
    /// reset does not change.
    #[serde(default)]
    pub origin_id: String,
    /// A rebind's target grant, its epoch (kept unchanged by a rebind) and
    /// the origin it was bound to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<Secret>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_origin_id: Option<String>,
    /// The terminal refusal code; the operation is never sent again.
    #[serde(default)]
    pub refused: Option<String>,
    /// A refused operation paces the next one until then.
    #[serde(default)]
    pub not_before_ms: i64,
}
pub const CLAIM: &str = "claim";
pub const REBIND: &str = "rebind";
impl Operation {
    /// Not refused: replayed exactly before anything else is tried.
    pub fn pending(&self) -> bool {
        self.refused.is_none()
    }
    fn complete(&self) -> bool {
        let rebind = self.grant_id.is_some()
            && self.grant.is_some()
            && self.owner_epoch.is_some()
            && self.previous_origin_id.is_some();
        !self.operation_uuid.is_empty()
            && !self.installation_id.is_empty()
            && !self.origin_id.is_empty()
            && match self.kind.as_str() {
                CLAIM => true,
                REBIND => rebind,
                _ => false,
            }
    }
}

/// A session waiting on its owner's status, polled on the hold cadence with
/// no new operation: an owner wait (`owner_*`, `not_owner`,
/// `handover_not_ready`), a grant whose status is not its own active grant,
/// or an expired grant (renewal is not supported). Only a change of the
/// status from `observed` releases the session or plans a new operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Watch {
    pub account: Account,
    pub source: String,
    pub session_id: String,
    pub reason: String,
    /// The status when the wait began; `None` until one was read.
    #[serde(default)]
    pub observed: Option<Observed>,
    #[serde(default)]
    pub not_before_ms: i64,
}
/// What a status says about one session's owner, minus the revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Observed {
    /// `unknown`, `active`, `draining` or `released`; `unknown` when the
    /// status leaves the session out.
    pub state: String,
    #[serde(default)]
    pub owner_epoch: Option<u64>,
    #[serde(default)]
    pub grant_id: Option<String>,
}

/// The enrolled sources the service last published for one exact account.
/// Used while the service does not say, or cannot be asked: an unknown list
/// is never read as "not enrolled".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Enrolled {
    pub account: Account,
    pub sources: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub installations: Vec<Installation>,
    #[serde(default)]
    pub grants: Vec<Grant>,
    #[serde(default)]
    pub operations: Vec<Operation>,
    /// Accounts whose issuer answered `not_authorized`, with the sign-in
    /// that was refused. Absent from the file while empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reauthorization: Vec<Reauthorization>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watches: Vec<Watch>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enrolled: Vec<Enrolled>,
}

/// Acquisition for `account` waits for a sign-in with session ownership:
/// the issuer refused the session `sign_in` names
/// ([`super::sign_in_fingerprint`]). A different sign-in is tried again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reauthorization {
    pub account: Account,
    pub sign_in: String,
}
impl State {
    pub fn reauthorization(&self, account: &Account) -> Option<&Reauthorization> {
        self.reauthorization
            .iter()
            .find(|marker| marker.account == *account)
    }
    pub fn installation(&self, account: &Account) -> Option<&Installation> {
        self.installations
            .iter()
            .find(|installation| installation.account == *account)
    }
    pub fn grant(&self, account: &Account, source: &str, session_id: &str) -> Option<&Grant> {
        self.grants.iter().find(|grant| {
            grant.account == *account && grant.source == source && grant.session_id == session_id
        })
    }
    pub fn put_grant(&mut self, grant: Grant) {
        self.grants.retain(|kept| {
            !(kept.account == grant.account
                && kept.source == grant.source
                && kept.session_id == grant.session_id)
        });
        self.grants.push(grant);
    }
    /// The session's operation, pending or refused.
    pub fn operation(
        &self,
        account: &Account,
        source: &str,
        session_id: &str,
    ) -> Option<&Operation> {
        self.operations.iter().find(|operation| {
            operation.account == *account
                && operation.source == source
                && operation.session_id == session_id
        })
    }
    pub fn end_operation(&mut self, account: &Account, source: &str, session: &str) {
        self.operations.retain(|operation| {
            !(operation.account == *account
                && operation.source == source
                && operation.session_id == session)
        });
    }
    pub fn watch(&self, account: &Account, source: &str, session_id: &str) -> Option<&Watch> {
        self.watches.iter().find(|watch| {
            watch.account == *account && watch.source == source && watch.session_id == session_id
        })
    }
    pub fn put_watch(&mut self, watch: Watch) {
        self.end_watch(
            &watch.account.clone(),
            &watch.source.clone(),
            &watch.session_id.clone(),
        );
        self.watches.push(watch);
    }
    pub fn end_watch(&mut self, account: &Account, source: &str, session_id: &str) {
        self.watches.retain(|watch| {
            !(watch.account == *account && watch.source == source && watch.session_id == session_id)
        });
    }
    /// The enrolled sources last published for `account`, if ever.
    pub fn enrolled(&self, account: &Account) -> Option<std::collections::BTreeSet<String>> {
        self.enrolled
            .iter()
            .find(|kept| kept.account == *account)
            .map(|kept| kept.sources.iter().cloned().collect())
    }
    pub fn put_enrolled(
        &mut self,
        account: &Account,
        sources: &std::collections::BTreeSet<String>,
    ) {
        let sources: Vec<String> = sources.iter().cloned().collect();
        if let Some(kept) = self
            .enrolled
            .iter_mut()
            .find(|kept| kept.account == *account)
        {
            kept.sources = sources;
        } else {
            self.enrolled.push(Enrolled {
                account: account.clone(),
                sources,
            });
        }
    }
    /// Drop what a state file written before the exact account kept: any
    /// entry whose account lacks the site or the user, and any operation
    /// that is not a whole request. Nothing of it can be told apart from
    /// another user's or another site's.
    fn exact_only(&mut self) {
        self.installations.retain(|kept| kept.account.exact());
        self.grants.retain(|kept| kept.account.exact());
        self.operations
            .retain(|kept| kept.account.exact() && kept.complete());
        self.reauthorization.retain(|kept| kept.account.exact());
        self.watches.retain(|kept| kept.account.exact());
        self.enrolled.retain(|kept| kept.account.exact());
    }
}

/// The acquisition state file.
#[derive(Debug, Clone)]
pub struct Store {
    path: PathBuf,
}
impl Store {
    /// `<config dir>/acquisition/state.json`.
    pub fn default_location() -> Self {
        Self::at(cloud::config_dir().join("acquisition"))
    }
    pub fn at(directory: PathBuf) -> Self {
        Self {
            path: directory.join("state.json"),
        }
    }
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
    pub fn load(&self) -> Result<State> {
        match std::fs::read(&self.path) {
            Ok(bytes) => {
                let mut state: State =
                    serde_json::from_slice(&bytes).context("acquisition state is unreadable")?;
                state.exact_only();
                Ok(state)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(error) => Err(error).context("acquisition state could not be read"),
        }
    }
    /// Read, change and save the state under an exclusive lock, so the app
    /// and a collector never lose each other's writes.
    pub fn update<T>(&self, change: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        use fs2::FileExt;
        let directory = self
            .path
            .parent()
            .context("acquisition state has no directory")?;
        std::fs::create_dir_all(directory)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(directory.join("state.lock"))?;
        lock.lock_exclusive()?;
        let stored = std::fs::read(&self.path).ok();
        let mut state = self.load()?;
        let before = state.clone();
        let result = change(&mut state)?;
        // Also rewritten when reading dropped entries of an older shape.
        let migrated = stored.is_some_and(|stored| {
            serde_json::from_slice::<State>(&stored).is_ok_and(|raw| raw != before)
        });
        if state != before || migrated {
            cloud::write_private(&self.path, &serde_json::to_string(&state)?)?;
        }
        let _ = FileExt::unlock(&lock);
        Ok(result)
    }
}
