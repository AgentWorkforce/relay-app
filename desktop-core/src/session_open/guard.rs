//! One open per session at a time: a second concurrent click for the same
//! session is refused, in this process (a set) and across processes (the
//! macOS app runs each click as its own helper process: an exclusive lock
//! on a file per session, released when the holder exits).
use std::collections::BTreeSet;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::sync::Mutex;

use session_navigation::model::SessionRef;
use session_navigation_native::common::reservations::file_name;

static OPENING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Another open of this session is running.
    Busy,
    /// The lock could not be made: nothing can be reserved there either.
    Store,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Busy => "another open of this session is running",
            Self::Store => "the open lock could not be made",
        })
    }
}

impl std::error::Error for Refusal {}

/// Held while one open runs.
#[derive(Debug)]
pub struct Opening {
    name: String,
    _lock: File,
}

impl Drop for Opening {
    fn drop(&mut self) {
        if let Ok(mut set) = OPENING.lock() {
            set.remove(&self.name);
        }
    }
}

fn claim(name: &str) -> Result<(), Refusal> {
    let mut set = OPENING.lock().map_err(|_| Refusal::Store)?;
    if set.insert(name.to_owned()) {
        Ok(())
    } else {
        Err(Refusal::Busy)
    }
}

fn unclaim(name: &str) {
    if let Ok(mut set) = OPENING.lock() {
        set.remove(name);
    }
}

fn lock(dir: &Path, name: &str) -> Result<File, Refusal> {
    std::fs::create_dir_all(dir).map_err(|_| Refusal::Store)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(format!("{name}.lock")))
        .map_err(|_| Refusal::Store)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(Refusal::Busy),
        Err(TryLockError::Error(_)) => Err(Refusal::Store),
    }
}

/// Start an open of `session`, with lock files under `dir`.
pub fn begin(dir: &Path, session: &SessionRef) -> Result<Opening, Refusal> {
    let file = file_name(&session.normalized()).ok_or(Refusal::Store)?;
    // The set is keyed by the lock's path, so two data directories (tests,
    // profiles) never block each other.
    let name = dir.join(&file).to_string_lossy().into_owned();
    claim(&name)?;
    match lock(dir, &file) {
        Ok(file) => Ok(Opening { name, _lock: file }),
        Err(refusal) => {
            unclaim(&name);
            Err(refusal)
        }
    }
}
