//! Writable stores kept across calls, keyed by database file identity.
use super::*;

/// How many writable stores the process keeps: one per extra Claude folder,
/// plus the selected-mode writer, is the expected population.
const KEPT_STORES: usize = 16;

/// A writable store kept for the process, with the database file it opened.
struct Kept {
    db: PathBuf,
    file: FileIdentity,
    roots: ProviderRoots,
    store: SessionStore,
}

/// Writable stores kept across calls, oldest first, at most `cap`.
struct KeptStores {
    entries: Vec<Kept>,
    cap: usize,
}
impl KeptStores {
    const fn new(cap: usize) -> Self {
        Self {
            entries: Vec::new(),
            cap,
        }
    }
    /// The store kept for `db` over `roots` while `db` is still `file`.
    /// Every store kept for an earlier file at the path is dropped first.
    fn get(
        &mut self,
        db: &Path,
        file: FileIdentity,
        roots: &ProviderRoots,
    ) -> Option<SessionStore> {
        self.entries
            .retain(|kept| kept.db != db || kept.file == file);
        self.entries
            .iter()
            .find(|kept| kept.db == db && kept.roots == *roots)
            .map(|kept| kept.store.clone())
    }
    fn insert(&mut self, db: &Path, file: FileIdentity, roots: ProviderRoots, store: SessionStore) {
        self.entries
            .retain(|kept| kept.db != db || kept.file == file);
        if self
            .entries
            .iter()
            .any(|kept| kept.db == db && kept.roots == roots)
        {
            return;
        }
        if self.entries.len() >= self.cap {
            self.entries.remove(0);
        }
        self.entries.push(Kept {
            db: db.to_path_buf(),
            file,
            roots,
            store,
        });
    }
}

/// Writable stores kept across calls on the recurring capture paths: each
/// hot pass and discovery over an extra Claude folder, and each selected
/// member's hydration. An open migrates and handshakes, and a store's first
/// sync fingerprints every catalog row, which a kept store does once and then
/// follows from the change feed (relayhistory's "Incremental embedder
/// scheduling"). One sync at a time per store still holds: every clone's
/// sync takes the store's own sync lock.
static KEPT: std::sync::Mutex<KeptStores> = std::sync::Mutex::new(KeptStores::new(KEPT_STORES));

/// [`open_with`], from a store kept while `db` is the file it opened. A file
/// replaced at the path (a reset, a restored backup) drops every store kept
/// for it, and the next call opens, and so migrates, the new file. Where the
/// platform gives no stable file identity, or the path names no file yet,
/// nothing is kept and every call opens, as before.
pub(super) fn kept_with(db: &Path, roots: ProviderRoots) -> Result<SessionStore> {
    kept_in(&KEPT, db, roots)
}

fn kept_in(
    kept: &std::sync::Mutex<KeptStores>,
    db: &Path,
    roots: ProviderRoots,
) -> Result<SessionStore> {
    let lock = || {
        kept.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    };
    let Some(file) = FileIdentity::of(db) else {
        return open_with(db, roots);
    };
    if let Some(store) = lock().get(db, file, &roots) {
        return Ok(store);
    }
    // Opened outside the lock: an open can wait on a migration.
    let store = open_with(db, roots.clone())?;
    // Kept only when the file the open migrated is still the one at the path.
    if FileIdentity::of(db) == Some(file) {
        lock().insert(db, file, roots, store.clone());
    }
    Ok(store)
}

/// A file's device and inode: what a path names, independent of its contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}
impl FileIdentity {
    #[cfg(unix)]
    fn of(path: &Path) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).ok()?;
        Some(Self {
            device: meta.dev(),
            inode: meta.ino(),
        })
    }
    #[cfg(not(unix))]
    fn of(_path: &Path) -> Option<Self> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn roots(home: &Path) -> ProviderRoots {
        let mut roots = ProviderRoots::from_home(home.to_path_buf(), home.join("opencode.db"));
        roots.claude = home.join(".claude-extra");
        roots
    }

    fn kept_file(kept: &Mutex<KeptStores>, db: &Path) -> Option<FileIdentity> {
        kept.lock()
            .unwrap()
            .entries
            .iter()
            .find(|kept| kept.db == db)
            .map(|kept| kept.file)
    }

    fn count(kept: &Mutex<KeptStores>, db: &Path) -> usize {
        let kept = kept.lock().unwrap();
        kept.entries.iter().filter(|kept| kept.db == db).count()
    }

    #[cfg(unix)]
    #[test]
    fn a_kept_store_is_reused_until_its_database_is_replaced() {
        let kept = Mutex::new(KeptStores::new(KEPT_STORES));
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("history.db");
        let roots = roots(dir.path());
        // The first call creates the file; only one that existed is kept.
        kept_in(&kept, &db, roots.clone()).unwrap();
        assert_eq!(count(&kept, &db), 0);
        kept_in(&kept, &db, roots.clone()).unwrap();
        let first = FileIdentity::of(&db);
        assert!(first.is_some());
        assert_eq!(kept_file(&kept, &db), first);
        kept_in(&kept, &db, roots.clone()).unwrap();
        assert_eq!(count(&kept, &db), 1, "a later call reuses the kept store");

        // Other roots over the same file are a store of their own.
        let mut other = roots.clone();
        other.claude = dir.path().join(".claude-other");
        kept_in(&kept, &db, other).unwrap();
        assert_eq!(count(&kept, &db), 2);

        // A store restored over the path is a different file: what was kept
        // for the old one is dropped, and the new file is opened, which
        // migrates it.
        let replacement = dir.path().join("replacement.db");
        open_with(&replacement, roots.clone()).unwrap();
        std::fs::rename(&replacement, &db).unwrap();
        let second = FileIdentity::of(&db);
        assert_ne!(first, second);
        kept_in(&kept, &db, roots.clone()).unwrap();
        assert_eq!(kept_file(&kept, &db), second);
        assert_eq!(count(&kept, &db), 1);
    }

    #[test]
    fn kept_stores_are_bounded_and_the_oldest_goes_first() {
        let kept = Mutex::new(KeptStores::new(4));
        let dir = tempfile::tempdir().unwrap();
        let dbs: Vec<_> = (0..6)
            .map(|n| dir.path().join(format!("history-{n}.db")))
            .collect();
        for db in &dbs {
            open_with(db, roots(dir.path())).unwrap();
            kept_in(&kept, db, roots(dir.path())).unwrap();
        }
        if FileIdentity::of(&dbs[0]).is_none() {
            // No stable identity on this platform: nothing is kept.
            assert!(kept.lock().unwrap().entries.is_empty());
            return;
        }
        assert_eq!(kept.lock().unwrap().entries.len(), 4);
        assert_eq!(count(&kept, &dbs[0]), 0);
        assert_eq!(count(&kept, &dbs[5]), 1);
    }
}
