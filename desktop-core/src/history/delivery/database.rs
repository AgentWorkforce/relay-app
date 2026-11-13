//! Stable identity across ordinary writes and path replacement on all hosts.
use super::*;

/// Collector startup migrates the base store before arming capture. Historical
/// derived projections wait until watchers are armed; `open_db` initializes
/// those on the ensuing delivery open.
pub fn open_for_capture(path: &std::path::Path) -> Result<Connection> {
    create_parent(path)?;
    let mut options = ai_hist::StoreOptions::default();
    options.db_path = Some(path.to_path_buf());
    ai_hist::SessionStore::open(options)?;
    sidecar::migrate_initialized(path, |conn| {
        schema::init(conn)?;
        policy::init(conn)?;
        schema::rebuild(conn, || feed::store(conn))
    })
}

#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DatabaseIdentity {
    device: u64,
    inode: u64,
}

#[cfg(not(unix))]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DatabaseIdentity(same_file::Handle);

impl DatabaseIdentity {
    #[cfg(unix)]
    pub(crate) fn open_or_create(path: &std::path::Path) -> std::io::Result<Self> {
        match Self::read(path) {
            Ok(identity) => return Ok(identity),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        #[cfg(test)]
        tests::creation_hook("missing");
        let target = creation_target(path)?;
        let parent = target.parent().filter(|p| !p.as_os_str().is_empty());
        // Close the private 0600 file BEFORE publishing its inode at the DB
        // pathname. A raw close after SQLite opens it would release every
        // POSIX lock this process holds on that database, even on other FDs.
        let temporary =
            tempfile::NamedTempFile::new_in(parent.unwrap_or(std::path::Path::new(".")))?
                .into_temp_path();
        match temporary.persist_noclobber(&target) {
            Ok(()) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.error),
        }
        #[cfg(test)]
        tests::creation_hook("published");
        Self::read(path)
    }

    #[cfg(not(unix))]
    pub(crate) fn open_or_create(path: &std::path::Path) -> std::io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        same_file::Handle::from_file(file).map(Self)
    }

    #[cfg(unix)]
    pub(crate) fn read(path: &std::path::Path) -> std::io::Result<Self> {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(path)?;
        // Existing SQLite FDs keep their inode alive. This snapshot does not
        // pin an otherwise unopened inode or promise identity across reuse.
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    #[cfg(not(unix))]
    pub(crate) fn read(path: &std::path::Path) -> std::io::Result<Self> {
        same_file::Handle::from_path(path).map(Self)
    }
}

/// Follow a dangling final symlink as the old create-open did. Parents still
/// resolve through the filesystem; publication never replaces a symlink.
#[cfg(unix)]
fn creation_target(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let mut target = path.to_owned();
    for _ in 0..40 {
        match std::fs::read_link(&target) {
            Ok(link) => {
                target = if link.is_absolute() {
                    link
                } else {
                    target
                        .parent()
                        .unwrap_or(std::path::Path::new("."))
                        .join(link)
                };
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput
                ) =>
            {
                return Ok(target)
            }
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::other("too many database symlinks"))
}

#[cfg(all(test, unix))]
mod tests;

/// Local history predates the schema this build reads, and the migration is
/// waiting for the collector. On a large store it runs for minutes, so only
/// the collector lock's holder, which no caller's timeout can kill, runs it:
/// a killed migration rolls back, and every other process that tried it would
/// hold the writer lock just long enough to starve the one that could finish.
#[derive(Debug)]
pub struct UpgradePending;
impl std::fmt::Display for UpgradePending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("local history is waiting for the collector to upgrade it")
    }
}
impl std::error::Error for UpgradePending {}
/// Whether `error` is local history waiting for its upgrade: [`UpgradePending`]
/// or the store refusing a read the upgrade has not reached yet.
pub fn is_upgrade_pending(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.is::<UpgradePending>()
            || cause
                .downcast_ref::<ai_hist::Error>()
                .is_some_and(ai_hist::Error::is_stale_schema)
    })
}

pub(super) fn open_connection(path: &std::path::Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(BUSY_TIMEOUT)?;
    // The store's own writers enable this so a REPLACE fires the change
    // feed's delete stamping; any write through this connection must too.
    conn.pragma_update(None, "recursive_triggers", true)?;
    Ok(conn)
}

/// Refuse, as [`UpgradePending`], a store the collector has not yet upgraded
/// to the schema this build reads, including one that predates only the
/// change feed, which opens and lists sessions as before.
pub fn ensure_current(path: &std::path::Path) -> Result<()> {
    feed::ensure_current(path)
}

/// The durable upload journal adjacent to this install's history database.
pub fn upload_queue_path(history: &std::path::Path) -> std::path::PathBuf {
    sidecar::path(history)
}

/// Open evidence and upload state as the install's owner: the caller holds
/// the collector lock. Migrates evidence and moves current upload state to
/// the adjacent queue. Retired export journals keep their unresolved request
/// identities and bytes while restarting capture from the change feed.
pub fn open_db(path: &std::path::Path) -> Result<Connection> {
    #[cfg(test)]
    DATABASE_OPENS.with(|count| count.set(count.get() + 1));
    let conn = open_for_capture(path)?;
    // Derived baselines may read retained payloads or build historical indexes.
    // Only the unkillable collector owner performs their first initialization.
    counters::init(&conn)?;
    pending::init(&conn)?;
    Ok(conn)
}

#[cfg(test)]
thread_local! { pub(super) static DATABASE_OPENS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
/// Open evidence and upload state for a control action that may run beside a
/// collector: pause, resume, compaction. Creates missing tables but never
/// rebuilds or migrates, which only the collector lock's holder may do: a
/// store written by an older build is [`UpgradePending`] until it has.
pub fn open_control_db(path: &std::path::Path) -> Result<Connection> {
    create_parent(path)?;
    if path.exists() {
        feed::ensure_current(path)?;
    } else {
        // A new store has nothing to migrate, so creating it is quick.
        let mut options = ai_hist::StoreOptions::default();
        options.db_path = Some(path.to_path_buf());
        ai_hist::SessionStore::open(options)?;
        return open_for_capture(path);
    }
    open_upload_state(path)
}
pub(super) fn create_parent(path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}
fn open_upload_state(path: &std::path::Path) -> Result<Connection> {
    let conn = sidecar::open(path, false)?;
    schema::init(&conn)?;
    policy::init(&conn)?;
    Ok(conn)
}
/// Read upload state without writing, for status and listings.
pub fn open_db_readonly(path: &std::path::Path) -> Result<Connection> {
    sidecar::open(path, true)
}
