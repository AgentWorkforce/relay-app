//! The filesystem watch over selected members' transcripts.
use super::*;

/// What a watcher callback reports: each written path of a write event,
/// and a loss for a rescan or a failure.
pub(super) fn observed(event: notify::Result<notify::Event>) -> Vec<Observed> {
    match event {
        Ok(event) if !event.need_rescan() => {
            if !inventory::writes(&event.kind) {
                return Vec::new();
            }
            event.paths.into_iter().map(Observed::Written).collect()
        }
        _ => vec![Observed::Lost],
    }
}

/// What the member watch reports.
#[derive(Debug)]
pub(super) enum Observed {
    /// A write at this path.
    Written(PathBuf),
    /// Events were dropped (a queue overflow, or a kernel-coalesced rescan
    /// naming only the watched directory), or the watcher failed: a write to
    /// any member's transcript may have gone unreported.
    Lost,
}

/// The filesystem watch over selected members' transcripts. Each
/// transcript's directory is registered without recursing: a watch on the
/// file itself ends with the next atomic rewrite, and a provider's whole
/// tree would report every other session's writes.
pub(super) struct MemberWatch {
    watcher: notify::RecommendedWatcher,
    /// The directories registered.
    directories: HashSet<PathBuf>,
    /// Directories whose registration failed, retried each round: one that
    /// does not exist yet, or a watcher out of watches.
    unregistered: HashSet<PathBuf>,
    /// The transcripts followed, by member, as last asked.
    following: HashMap<delivery::SessionIdentity, PathBuf>,
    /// The members whose transcript's directory is registered.
    pub(super) covered: HashSet<delivery::SessionIdentity>,
    /// Each covered transcript, in every spelling an event can name it by.
    files: HashMap<PathBuf, delivery::SessionIdentity>,
}
impl MemberWatch {
    pub(super) fn start(events: mpsc::Sender<Observed>) -> notify::Result<Self> {
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            for observed in observed(event) {
                let _ = events.send(observed);
            }
        })?;
        Ok(Self {
            watcher,
            directories: HashSet::new(),
            unregistered: HashSet::new(),
            following: HashMap::new(),
            covered: HashSet::new(),
            files: HashMap::new(),
        })
    }

    /// Follow `files`, each member's transcript: register the directories
    /// not registered yet and give up those no member's transcript is in.
    /// Returns the members to compare now because the registrations moved:
    /// every covered member, since a registration made now reports no write
    /// from before it held, and some backends restart their stream for every
    /// path they add or remove.
    pub(super) fn follow(
        &mut self,
        files: HashMap<delivery::SessionIdentity, PathBuf>,
    ) -> HashSet<delivery::SessionIdentity> {
        use notify::Watcher;
        if files == self.following && self.unregistered.is_empty() {
            return HashSet::new();
        }
        let mut by_directory: HashMap<PathBuf, Vec<delivery::SessionIdentity>> = HashMap::new();
        for (member, file) in &files {
            if let Some(directory) = file.parent() {
                by_directory
                    .entry(directory.to_path_buf())
                    .or_default()
                    .push(member.clone());
            }
        }
        let mut moved = false;
        let unused: Vec<PathBuf> = self
            .directories
            .iter()
            .filter(|directory| !by_directory.contains_key(*directory))
            .cloned()
            .collect();
        for directory in unused {
            let _ = self.watcher.unwatch(&directory);
            self.directories.remove(&directory);
            moved = true;
        }
        self.unregistered.clear();
        self.covered.clear();
        self.files.clear();
        for (directory, members) in by_directory {
            if !self.directories.contains(&directory) {
                if self
                    .watcher
                    .watch(&directory, notify::RecursiveMode::NonRecursive)
                    .is_err()
                {
                    self.unregistered.insert(directory);
                    continue;
                }
                self.directories.insert(directory);
                moved = true;
            }
            for member in members {
                for spelling in spellings(&files[&member]) {
                    self.files.insert(spelling, member.clone());
                }
                self.covered.insert(member);
            }
        }
        self.following = files;
        if moved {
            self.covered.clone()
        } else {
            HashSet::new()
        }
    }

    /// Whether an event will name a write to `member`'s transcript.
    #[cfg(test)]
    pub(super) fn covers(&self, member: &delivery::SessionIdentity) -> bool {
        self.covered.contains(member)
    }

    /// The member whose transcript `path` is.
    pub(super) fn member(&self, path: &Path) -> Option<&delivery::SessionIdentity> {
        self.files.get(path)
    }
}

/// A file's path as given and with its directory's symlinks resolved, which
/// is how some backends report it (FSEvents names `/var` as `/private/var`).
fn spellings(file: &Path) -> Vec<PathBuf> {
    let resolved = file
        .parent()
        .and_then(|parent| Some(parent.canonicalize().ok()?.join(file.file_name()?)));
    let mut spellings = vec![file.to_path_buf()];
    spellings.extend(resolved.filter(|resolved| resolved != file));
    spellings
}
