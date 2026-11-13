//! Registered agent names, recorded in local history so "@agent" search
//! finds a session by the name it is messaged by on the relay.
//!
//! The daemon knows which sessions are on the relay and by which name; the
//! detached collector owns the history store. Whenever the agents change the
//! daemon publishes the map, upload key to display name, into the install's
//! directory ([`publish`], [`FILE`]). Every delivery pass the collector reads
//! it and, when it changed since the pass that last applied it, applies the
//! names to the store ([`before_delivery`]) before the drain prepares a
//! batch. The store's change feed then carries the new revision through the
//! normal cursor, filling and scrub, like any other change.
//!
//! [`apply`] is the one seam to the store: it records each name with
//! `SessionStore::set_registered_agent_name(source, session_id, name)`
//! ([`record_in_store`] is the one call). Names are checked here against
//! the setter's limits ([`valid_name`]) so it never refuses one as an
//! invalid argument, and they are only ever applied to the collector's
//! writable store, never a read-only one.

use crate::history::session_model::agent_display_name;
use agent_relay_node::relay::contract::agents::LocalSession;
use agent_relay_node::relay::store::{SessionKind, Target as RelayTarget};
use ai_hist::{SessionStore, Source};
use anyhow::Result;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The published names, in the install's directory: a JSON object from
/// upload key (`source:id`) to the session's registered display name.
pub const FILE: &str = "registered-agent-names.json";

/// The history session a name is recorded on: the setter's `source` and
/// `session_id`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionKey {
    pub source: Source,
    pub session_id: String,
}
impl SessionKey {
    pub fn new(source: Source, session_id: impl Into<String>) -> Self {
        Self {
            source,
            session_id: session_id.into(),
        }
    }
}

/// The longest name the history store accepts.
pub const NAME_LIMIT: usize = 128;

/// Whether the history store accepts `name` as a registered agent name: not
/// empty, at most [`NAME_LIMIT`] long (counted in bytes, which is never
/// fewer than characters, so a name kept here is never refused as too
/// long), and free of control characters. A name it refuses is left out.
pub fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= NAME_LIMIT && !name.chars().any(char::is_control)
}

/// Registered names by upload key (`source:id`), as published.
pub type Names = BTreeMap<String, String>;

/// The history key a registered session's transcript is captured under. A
/// Codex thread is named by its own id, `codex:<native id>` (the core's
/// `Target::upload_key`); every other kind by the discovered session's
/// `upload_key`, which also names a Claude Code on the web session by its
/// remote transcript. `None` for a kind history does not capture.
fn history_key(session: &LocalSession) -> Option<String> {
    match session.target.kind {
        SessionKind::Codex => session.target.upload_key(),
        _ => session.upload_key(),
    }
}

/// The registered display name of every session in `sessions` that is on
/// the relay, by its history key: the alias once renamed, as the sessions
/// list shows it (`LocalSessionRow::agent_name`). Only registrations
/// `in_workspace` accepts count. A stopped session and a live one sharing a
/// key keep the live one's name, as the merged list does.
pub fn names_for(sessions: &[LocalSession], in_workspace: impl Fn(&RelayTarget) -> bool) -> Names {
    let mut kept: HashMap<String, (bool, String)> = HashMap::new();
    for session in sessions {
        let Some(registration) = session.registration.as_ref() else {
            continue;
        };
        if !in_workspace(&session.target) {
            continue;
        }
        let Some(key) = history_key(session) else {
            continue;
        };
        let name = agent_display_name(&registration.name, registration.alias.as_deref());
        if !valid_name(name) {
            continue;
        }
        match kept.get_mut(&key) {
            Some(existing) if !existing.0 && session.live => {
                *existing = (true, name.to_owned());
            }
            Some(_) => {}
            None => {
                kept.insert(key, (session.live, name.to_owned()));
            }
        }
    }
    kept.into_iter()
        .map(|(key, (_, name))| (key, name))
        .collect()
}

/// Each published name with the history session it belongs to. A key whose
/// source history does not know, or that names no session, is left out, and
/// so is a name the store would refuse.
pub fn resolve(names: &Names) -> Vec<(SessionKey, String)> {
    names
        .iter()
        .filter_map(|(key, name)| {
            let (source, session_id) = key.split_once(':')?;
            if session_id.is_empty() || !valid_name(name) {
                return None;
            }
            Some((
                SessionKey::new(Source::parse(source)?, session_id),
                name.clone(),
            ))
        })
        .collect()
}

/// What recording one name did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placed {
    /// The store recorded the name (`Ok(true)`), or already held it
    /// (`Ok(false)`).
    Recorded,
    /// The store has no such session yet (`SessionNotFound`): the name is
    /// offered again on a later pass.
    NotInHistoryYet,
}

/// Where names are recorded: the history store, or a test's observer.
/// The shape of the store's setter: `None` clears a registered name
/// (never a provider-native one).
pub trait NameSink {
    fn set_registered_agent_name(
        &mut self,
        source: Source,
        session_id: &str,
        name: Option<&str>,
    ) -> Result<Placed>;
}

impl NameSink for SessionStore {
    fn set_registered_agent_name(
        &mut self,
        source: Source,
        session_id: &str,
        name: Option<&str>,
    ) -> Result<Placed> {
        record_in_store(self, source, session_id, name)
    }
}

/// The one call into the store's setter. The store stamps a changed name
/// with a new revision and exports it in the change feed as `agent_name`. A
/// session the store does not hold yet is not an error: the name is offered
/// again on a later pass.
fn record_in_store(
    store: &mut SessionStore,
    source: Source,
    session_id: &str,
    name: Option<&str>,
) -> Result<Placed> {
    // The inherent setter, by path: `store.set_registered_agent_name` would
    // pick this module's `NameSink` impl and recurse.
    match SessionStore::set_registered_agent_name(store, source, session_id, name) {
        Ok(_) => Ok(Placed::Recorded),
        Err(ai_hist::Error::SessionNotFound(_)) => Ok(Placed::NotInHistoryYet),
        Err(error) => Err(error.into()),
    }
}

/// What one [`apply`] did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Applied {
    /// Names offered to the store.
    pub offered: usize,
    /// Names the store recorded.
    pub recorded: usize,
    /// Names whose session the store does not hold yet.
    pub not_in_history_yet: usize,
}

/// Records each registered name on its history session. The one seam to the
/// store: returns how many names were offered, recorded, and left for a later
/// pass because history has not captured their session yet.
pub fn apply(store: &mut SessionStore, names: &[(SessionKey, String)]) -> Result<Applied> {
    apply_to(store, names)
}

/// [`apply`] through any [`NameSink`].
pub fn apply_to(sink: &mut dyn NameSink, names: &[(SessionKey, String)]) -> Result<Applied> {
    let mut applied = Applied::default();
    for (session, name) in names {
        applied.offered += 1;
        match sink.set_registered_agent_name(session.source, &session.session_id, Some(name))? {
            Placed::Recorded => applied.recorded += 1,
            Placed::NotInHistoryYet => applied.not_in_history_yet += 1,
        }
    }
    Ok(applied)
}

/// A content-free digest of resolved names, for the unchanged check. It is
/// compared only within this process.
pub fn digest(names: &[(SessionKey, String)]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut sorted: Vec<_> = names.iter().collect();
    sorted.sort();
    sorted.hash(&mut hasher);
    hasher.finish()
}

/// The digest each install directory's names were last applied under, so an
/// idle pass neither opens the store nor offers a name again.
#[derive(Default)]
pub struct Memo(HashMap<PathBuf, u64>);
impl Memo {
    pub fn unchanged(&self, directory: &Path, digest: u64) -> bool {
        self.0.get(directory) == Some(&digest)
    }
    pub fn record(&mut self, directory: &Path, digest: u64) {
        self.0.insert(directory.to_owned(), digest);
    }
    pub fn forget(&mut self, directory: &Path) {
        self.0.remove(directory);
    }
    pub fn applied(&self, directory: &Path) -> Option<u64> {
        self.0.get(directory).copied()
    }
}

static MEMO: Mutex<Option<Memo>> = Mutex::new(None);

/// The digest this process last applied `directory`'s names under.
pub fn applied(directory: &Path) -> Option<u64> {
    MEMO.lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|memo| memo.applied(directory))
}

/// The names published in `directory`. `None` when none were, or the file
/// cannot be read.
pub fn published(directory: &Path) -> Option<Names> {
    let bytes = std::fs::read(directory.join(FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Run by the collector at the start of each delivery pass, before the drain
/// fills a batch: applies `directory`'s published names to its history
/// store, opened writable with `open` (never a read-only store), when they
/// changed since this process last applied them. Best effort: a failure is logged by class only and retried
/// on the next pass, and never fails delivery.
pub fn before_delivery(
    directory: &Path,
    open: impl FnOnce(&Path) -> Result<SessionStore>,
) -> Option<Applied> {
    let mut memo = MEMO.lock().unwrap_or_else(|e| e.into_inner());
    before_delivery_with(memo.get_or_insert_with(Memo::default), directory, |db| {
        open(db).map(|store| Box::new(store) as Box<dyn NameSink>)
    })
}

/// [`before_delivery`] with its memo and sink given.
pub fn before_delivery_with(
    memo: &mut Memo,
    directory: &Path,
    open: impl FnOnce(&Path) -> Result<Box<dyn NameSink>>,
) -> Option<Applied> {
    let names = resolve(&published(directory)?);
    let digest = digest(&names);
    if memo.unchanged(directory, digest) {
        return None;
    }
    // Names that changed may be only partly applied below, so the digest
    // last applied no longer describes the store: a return to it must be
    // applied again.
    memo.forget(directory);
    let applied =
        open(&directory.join("history.db")).and_then(|mut sink| apply_to(sink.as_mut(), &names));
    match applied {
        Ok(applied) => {
            // A name whose session history has not captured yet is offered
            // again on the next pass.
            if applied.not_in_history_yet == 0 {
                memo.record(directory, digest);
            }
            Some(applied)
        }
        Err(_) => {
            eprintln!("registered agent names deferred: local history unavailable");
            None
        }
    }
}

/// Writes `names` to `directory` when they differ from what is published
/// there. Nothing is written for no names where none were published.
pub fn write_published(directory: &Path, names: &Names) -> Result<bool> {
    let current = published(directory);
    if current.as_ref() == Some(names) || (current.is_none() && names.is_empty()) {
        return Ok(false);
    }
    crate::history::probe::save_json(&directory.join(FILE), names)?;
    Ok(true)
}

/// Publishes the registered names of the upload workspace's sessions into
/// the attached install's directory, for its collector. Run whenever the
/// agents change; does nothing while no install is attached.
pub(crate) async fn publish(
    core: &std::sync::Arc<agent_relay_node::relay::daemon::Core>,
    uploader: &crate::history::uploader::Uploader,
) {
    // One publish at a time, so an older map never replaces a newer one.
    static ONE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _one = ONE.lock().await;
    let Some(install) = uploader.install() else {
        return;
    };
    let Some(directory) = install.directory.map(PathBuf::from) else {
        return;
    };
    let registered: std::collections::HashSet<RelayTarget> = core.store.read(|store| {
        crate::history::relay_uploads::upload_workspace_registrations(store, &install.target)
            .into_iter()
            .map(|registration| registration.target)
            .collect()
    });
    let sessions = agent_relay_node::relay::local_methods::sessions_list(core)
        .await
        .sessions;
    let names = names_for(&sessions, |target| registered.contains(target));
    let written = tokio::task::spawn_blocking(move || write_published(&directory, &names)).await;
    if !matches!(written, Ok(Ok(_))) {
        eprintln!("registered agent names not published: install directory unavailable");
    }
}

#[cfg(test)]
mod tests;
