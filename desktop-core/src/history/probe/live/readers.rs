use super::*;

/// The store as selected mode reads it: the delivery tables, and the
/// catalog through `ai-hist`, whose handle keeps its own read connections.
pub(super) struct Readers {
    pub(super) control: rusqlite::Connection,
    pub(super) store: ai_hist::SessionStore,
    /// The database file the control connection opened. A store replaced
    /// under it (a reset, a restored backup) is a different file, which a
    /// kept connection would go on reading.
    pub(super) file: Option<FileIdentity>,
    pub(super) queue_file: Option<FileIdentity>,
    pub(super) snapshot: bool,
}
impl Readers {
    pub(super) fn open(db: &Path) -> anyhow::Result<Self> {
        let file = FileIdentity::of(db);
        Ok(Self {
            control: delivery::open_db_readonly(db)?,
            store: history::open_read(db)?,
            file,
            queue_file: FileIdentity::of(&delivery::upload_queue_path(db)),
            snapshot: false,
        })
    }
    /// Whether `db` is still the file these readers opened.
    pub(super) fn current(&self, db: &Path) -> bool {
        self.file.is_some()
            && self.file == FileIdentity::of(db)
            && self.queue_file.is_some()
            && self.queue_file == FileIdentity::of(&delivery::upload_queue_path(db))
    }
}

/// A file's device and inode: what a path names, independent of its contents.
/// `None` where the platform gives no stable identity, so such readers are
/// never kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct FileIdentity {
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
