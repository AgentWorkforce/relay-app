//! Pending resumes recorded by the apps before this engine: macOS wrote
//! `session-pending-resumes.json` (`{"version":1,"pending":["claude:<id>"]}`)
//! and GTK `pending-session-resumes.json` (`["claude:<id>"]`), both in the
//! app's data directory, keyed `<source>:<session id>`. An entry still
//! blocks a duplicate launch after an upgrade: it reads as a launch
//! reservation with no tty, made at time zero in an unknown boot, so only an
//! owner the plan returns to (Windows) or the person's `launch_gone`
//! resolves it. An unreadable or damaged file blocks every launch
//! (`Unknown`), as it did before. Nothing here drops an entry silently:
//! [`take`] removes one only after the engine's own reservation replaced it,
//! or after a live owner was observed (what the apps did).
use serde::{Deserialize, Serialize};
use session_navigation::evidence::{Fact, Pending, Reservation};
use session_navigation::model::SessionRef;
use session_navigation_native::system::{join, FsError, System};

/// The macOS app's file.
pub const MAC_FILE: &str = "session-pending-resumes.json";
/// The GTK app's file.
pub const GTK_FILE: &str = "pending-session-resumes.json";

const LIMIT: usize = 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MacRecord {
    version: u32,
    pending: Vec<String>,
}

#[derive(Clone, Copy)]
enum Shape {
    Mac,
    Gtk,
}

fn shape(name: &str) -> Shape {
    if name == MAC_FILE {
        Shape::Mac
    } else {
        Shape::Gtk
    }
}

fn parse(shape: Shape, bytes: &[u8]) -> Option<Vec<String>> {
    match shape {
        Shape::Mac => serde_json::from_slice::<MacRecord>(bytes)
            .ok()
            .filter(|r| r.version == 1)
            .map(|r| r.pending),
        Shape::Gtk => serde_json::from_slice(bytes).ok(),
    }
}

fn encode(shape: Shape, pending: Vec<String>) -> Option<Vec<u8>> {
    match shape {
        Shape::Mac => serde_json::to_vec(&MacRecord {
            version: 1,
            pending,
        })
        .ok(),
        Shape::Gtk => serde_json::to_vec_pretty(&pending).ok(),
    }
}

/// `<source>:<id>`, the apps' key.
pub fn key(source: &str, id: &str) -> String {
    format!("{source}:{id}")
}

/// One file's entries: `Known(empty)` when it does not exist.
fn entries<S: System>(sys: &S, path: &str, shape: Shape) -> Fact<Vec<String>> {
    match sys.read_file(path, LIMIT) {
        Err(FsError::NotFound) => Fact::Known(Vec::new()),
        Err(_) => Fact::Unknown,
        Ok(bytes) => parse(shape, &bytes).map_or(Fact::Unknown, Fact::Known),
    }
}

/// Whether either file holds one of `keys`.
pub fn pending<S: System>(sys: &S, dir: &str, keys: &[String]) -> Fact<bool> {
    let mut found = false;
    for name in [MAC_FILE, GTK_FILE] {
        match entries(sys, &join(dir, name), shape(name)) {
            Fact::Unknown => return Fact::Unknown,
            Fact::Known(list) => found |= list.iter().any(|entry| keys.contains(entry)),
        }
    }
    Fact::Known(found)
}

/// The reservation a legacy entry stands for.
pub fn reservation(session: &SessionRef) -> Reservation {
    Reservation {
        session: session.clone(),
        made_at: 0,
        tty: None,
        seen: false,
        boot: None,
        via: Pending::Launch,
    }
}

/// Remove `keys` from both files (atomic rewrite of each file that had
/// one). `Err` leaves the entry in place, still blocking.
pub fn take<S: System>(sys: &S, dir: &str, keys: &[String]) -> Result<(), FsError> {
    for name in [MAC_FILE, GTK_FILE] {
        let path = join(dir, name);
        let shape = shape(name);
        let Fact::Known(list) = entries(sys, &path, shape) else {
            return Err(FsError::Other);
        };
        if !list.iter().any(|entry| keys.contains(entry)) {
            continue;
        }
        let kept = list.into_iter().filter(|e| !keys.contains(e)).collect();
        let bytes = encode(shape, kept).ok_or(FsError::Other)?;
        sys.write_atomic(&path, &bytes)?;
    }
    Ok(())
}
