//! Coalescing and provider scope for the broad capture watcher.
use super::{collector, inventory, Shared};
use ai_hist::Source;
use serde_json::json;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Instant,
};

/// A coalesced burst retains its provider identity until a sweep reads it.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Owed {
    pub(super) first: Instant,
    pub(super) sources: HashSet<Source>,
    pub(super) full: bool,
    pub(super) fs_event: bool,
    pub(super) paths: HashSet<PathBuf>,
    pub(super) priority: Vec<PathBuf>,
}
impl Owed {
    #[cfg(test)]
    pub(super) fn paths(&self) -> Vec<PathBuf> {
        self.paths_with_recent(&[])
    }

    pub(super) fn paths_with_recent(&self, recent: &[PathBuf]) -> Vec<PathBuf> {
        // A safety poll's obligation survives later filesystem notice bursts.
        // With all three classes busy, reserve4 recent,2 native and2 backlog
        // slots; spare capacity remains available to every class.
        let mut paths = Vec::new();
        Self::extend_paths(
            &mut paths,
            recent.iter().filter(|path| self.paths.contains(*path)),
            4,
        );
        let native_limit = paths.len() + if paths.is_empty() { 4 } else { 2 };
        Self::extend_paths(&mut paths, self.priority.iter(), native_limit);
        Self::extend_paths(
            &mut paths,
            self.paths
                .iter()
                .filter(|path| !self.priority.contains(*path) && !recent.contains(*path)),
            8,
        );
        Self::extend_paths(
            &mut paths,
            recent.iter().filter(|path| self.paths.contains(*path)),
            8,
        );
        Self::extend_paths(&mut paths, self.priority.iter(), 8);
        Self::extend_paths(&mut paths, self.paths.iter(), 8);
        paths
    }

    fn extend_paths<'a>(
        chosen: &mut Vec<PathBuf>,
        candidates: impl IntoIterator<Item = &'a PathBuf>,
        limit: usize,
    ) {
        for path in candidates {
            if chosen.len() >= limit {
                break;
            }
            if !chosen.contains(path) {
                chosen.push(path.clone());
            }
        }
    }
    pub(super) fn prioritize(&mut self, path: &Path) {
        self.priority.retain(|known| known != path);
        self.priority.insert(0, path.to_path_buf());
        self.priority.truncate(32);
    }
    pub(super) fn completed(&mut self, paths: &[PathBuf]) {
        for path in paths {
            self.paths.remove(path);
        }
        self.priority.retain(|path| self.paths.contains(path));
    }
    #[cfg(test)]
    pub(super) fn sources(&self) -> Option<Vec<Source>> {
        (!self.full).then(|| self.exact_sources())
    }
    pub(super) fn exact_sources(&self) -> Vec<Source> {
        Source::ALL
            .iter()
            .copied()
            .filter(|source| self.sources.contains(source))
            .collect()
    }
}

/// What the broad watcher sends the capture thread: when it noticed it, and
/// the path a write event named, or `None` when the watcher reported an
/// error and a write may have gone unreported.
pub(super) type Noticed = (Instant, Option<PathBuf>);

/// The filesystem watch over every path the store's own watch covers, and
/// what the thread last reported about how it is driven.
pub(super) struct Watching {
    pub(super) watch: Result<inventory::Watch, inventory::Unwatched>,
    events: mpsc::Sender<Noticed>,
    use_fs_events: bool,
    reported: Option<(&'static str, Option<&'static str>)>,
}
impl Watching {
    /// Arm broad capture only after its sharing snapshot was validated.
    pub(super) fn start_roots(
        store: &ai_hist::SessionStore,
        roots: &crate::history::probe::history::RootSettings,
        events: mpsc::Sender<Noticed>,
        use_fs_events: bool,
    ) -> Self {
        let mut watching = Self::unregistered(events, use_fs_events);
        watching.refresh_roots(store, roots, false);
        watching
    }

    fn unregistered(events: mpsc::Sender<Noticed>, use_fs_events: bool) -> Self {
        Self {
            watch: Err(inventory::Unwatched::NoRoots),
            events,
            use_fs_events,
            reported: None,
        }
    }

    #[cfg(test)]
    pub(super) fn start(db: &Path, events: mpsc::Sender<Noticed>, use_fs_events: bool) -> Self {
        let mut watching = Self::unregistered(events, use_fs_events);
        if use_fs_events {
            watching.watch = watching.register(db);
        }
        watching
    }

    #[cfg(test)]
    fn register(&self, db: &Path) -> Result<inventory::Watch, inventory::Unwatched> {
        let events = self.events.clone();
        inventory::Watch::start(db, move |path| {
            let _ = events.send((Instant::now(), path));
        })
    }

    /// Register afresh when that would watch something more: a provider root
    /// created since, or one whose registration failed. A watcher that
    /// cannot be replaced keeps the one already running.
    #[cfg(test)]
    fn refresh(&mut self, db: &Path) {
        if !self.use_fs_events {
            return;
        }
        if self.watch.as_ref().is_ok_and(|watch| !watch.outdated(db)) {
            return;
        }
        match self.register(db) {
            Ok(watch) => self.watch = Ok(watch),
            Err(reason) if self.watch.is_err() => self.watch = Err(reason),
            Err(_) => {}
        }
    }

    /// Refresh coverage from the collector's validated sharing snapshot.
    pub(super) fn refresh_roots(
        &mut self,
        store: &ai_hist::SessionStore,
        roots: &crate::history::probe::history::RootSettings,
        reset: bool,
    ) {
        if reset {
            self.watch = Err(inventory::Unwatched::NoRoots);
            self.reported = None;
        }
        if !self.use_fs_events {
            return;
        }
        let Ok(extra) = roots.extra_claude_roots(store) else {
            return;
        };
        let paths = crate::history::probe::history::watched_paths_for(store, extra);
        if self
            .watch
            .as_ref()
            .is_ok_and(|watch| !watch.outdated_paths(&paths))
        {
            return;
        }
        let events = self.events.clone();
        match inventory::Watch::start_paths(paths, move |path| {
            let _ = events.send((Instant::now(), path));
        }) {
            Ok(watch) => self.watch = Ok(watch),
            Err(reason) if self.watch.is_err() => self.watch = Err(reason),
            Err(_) => {}
        }
    }

    /// Whether `noticed` owes a forced sweep: a write to a path some source
    /// reads, or a watcher error.
    fn owes(&self, noticed: &Noticed) -> bool {
        match (&self.watch, &noticed.1) {
            (Ok(watch), Some(path)) => watch.covers(path),
            (Ok(_), None) => true,
            (Err(_), _) => false,
        }
    }

    /// Fold one notice into what is owed: a write some source reads owes a
    /// forced sweep from when it was noticed, and a loss also asks for the
    /// watcher to be registered again.
    pub(super) fn fold(&self, noticed: Noticed, owed: &mut Option<Owed>, lost: &mut bool) {
        if self.owes(&noticed) {
            *lost |= noticed.1.is_none();
            let pending = owed.get_or_insert_with(|| Owed {
                first: noticed.0,
                sources: HashSet::new(),
                full: false,
                fs_event: true,
                priority: Default::default(),
                paths: HashSet::new(),
            });
            pending.fs_event = true;
            match (&self.watch, noticed.1.as_deref()) {
                (Ok(watch), Some(path)) => {
                    let sources: Vec<_> = watch.sources(path).collect();
                    let unknown = sources.is_empty();
                    pending.sources.extend(sources);
                    // Bound event memory as well as per-pass work. An overflow
                    // requests resumable reconciliation instead of losing paths.
                    if path.is_file() && !unknown {
                        if pending.paths.contains(path) || pending.paths.len() < 4096 {
                            pending.paths.insert(path.to_path_buf());
                            pending.prioritize(path);
                        } else {
                            pending.full = true;
                        }
                    }
                    // Directories/deletions may change roots or rename a subtree.
                    pending.full |= !path.is_file() || unknown;
                }
                _ => pending.full = true,
            }
        }
    }

    #[cfg(test)]
    pub(super) fn drain(
        &self,
        received: &mpsc::Receiver<Noticed>,
        owed: &mut Option<Owed>,
        lost: &mut bool,
        trigger: super::Trigger,
    ) -> super::Trigger {
        while let Ok(noticed) = received.try_recv() {
            self.fold(noticed, owed, lost);
        }
        if owed.is_some() {
            super::Trigger::FsEvent
        } else {
            trigger
        }
    }

    /// Every full fallback adopts any provider roots that appeared, including
    /// directory/deletion notices as well as watcher losses and backstops.
    #[cfg(test)]
    pub(super) fn prepare_sources(
        &mut self,
        db: &Path,
        pending: Option<&Owed>,
        full: bool,
    ) -> Option<Vec<Source>> {
        let sources = if full {
            None
        } else {
            pending.and_then(Owed::sources)
        };
        if sources.is_none() {
            self.refresh(db);
        }
        sources
    }

    #[cfg(test)]
    pub(super) fn complete(&self) -> bool {
        self.watch.as_ref().is_ok_and(inventory::Watch::complete)
    }

    /// Report the driver when it is first known or changes: with the time
    /// it started watching, and the reason whenever it polls.
    pub(super) fn observe(&mut self, shared: &Shared) {
        let now = match &self.watch {
            Ok(watch) if watch.complete() => ("fs-events", None),
            Ok(_) => ("fs-events", Some("partial_fs_events")),
            Err(_) if !self.use_fs_events => ("polling", Some("disabled")),
            Err(inventory::Unwatched::NoRoots) => ("polling", Some("no_provider_roots")),
            Err(inventory::Unwatched::Unavailable) => ("polling", Some("fs_events_unavailable")),
        };
        if self.reported == Some(now) {
            return;
        }
        self.reported = Some(now);
        let (name, reason) = now;
        shared.update(|state| {
            state.metrics.driver = Some(name);
            state.metrics.fallback_reason = reason;
            state
                .metrics
                .watching_since_ms
                .get_or_insert_with(collector::now);
        });
        eprintln!(
            "{}",
            json!({"operation":"capture","stage":"live_capture","mode":"watch","driver":name,"fallback_reason":reason})
        );
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    #[test]
    fn recent_priority_is_bounded_and_keeps_a_backlog_turn() {
        let mut pending = Owed {
            first: Instant::now(),
            sources: Default::default(),
            full: false,
            fs_event: true,
            priority: Default::default(),
            paths: (0..100)
                .map(|n| PathBuf::from(format!("path-{n}")))
                .collect(),
        };
        for n in 0..100 {
            pending.prioritize(&PathBuf::from(format!("path-{n}")));
        }
        assert_eq!(pending.priority.len(), 32);
        let turn = pending.paths();
        assert_eq!(turn.len(), 8);
        assert_eq!(&turn[..4], &pending.priority[..4]);
        assert!(turn[4..]
            .iter()
            .all(|path| !pending.priority.contains(path)));
        let recent: Vec<_> = (0..8).map(|n| PathBuf::from(format!("path-{n}"))).collect();
        let mixed = pending.paths_with_recent(&recent);
        assert_eq!(mixed.len(), 8);
        assert_eq!(&mixed[..4], &recent[..4]);
        assert_eq!(
            mixed
                .iter()
                .filter(|path| pending.priority.contains(path))
                .count(),
            2
        );
        assert_eq!(
            mixed
                .iter()
                .filter(|path| !recent.contains(path) && !pending.priority.contains(path))
                .count(),
            2
        );
        pending.completed(&turn);
        assert_eq!(pending.paths.len(), 92);
        assert_eq!(pending.priority.len(), 28);
    }

    #[test]
    fn repeated_admitted_path_at_capacity_does_not_request_reconciliation() {
        let Some(home) = crate::history::probe::live::tests::isolated_home("history::probe::live::watching::admission_tests::repeated_admitted_path_at_capacity_does_not_request_reconciliation") else { return; };
        let db = home.join("probe/history.db");
        crate::history::probe::history::open(&db).unwrap();
        let project = home.join(".claude/projects/capacity");
        std::fs::create_dir_all(&project).unwrap();
        let path = project.join("known.jsonl");
        std::fs::write(&path, "").unwrap();
        let (events, _received) = mpsc::channel();
        let watching = Watching::start(&db, events, true);
        let mut pending = Owed {
            first: Instant::now(),
            sources: Default::default(),
            full: false,
            fs_event: true,
            priority: Default::default(),
            paths: (0..4095)
                .map(|n| project.join(format!("{n}.jsonl")))
                .collect(),
        };
        pending.paths.insert(path.clone());
        let mut owed = Some(pending);
        let mut lost = false;
        watching.fold((Instant::now(), Some(path.clone())), &mut owed, &mut lost);
        assert!(!owed.as_ref().unwrap().full);
        assert!(owed.as_ref().unwrap().paths().contains(&path));
        let new = project.join("new.jsonl");
        std::fs::write(&new, "").unwrap();
        watching.fold((Instant::now(), Some(new)), &mut owed, &mut lost);
        assert!(
            owed.as_ref().unwrap().full,
            "a distinct overflow must still reconcile"
        );
    }

    #[test]
    fn unrelated_probe_directory_is_ignored_and_provider_directory_requests_rescan() {
        let Some(home)=crate::history::probe::live::tests::isolated_home("history::probe::live::watching::admission_tests::unrelated_probe_directory_is_ignored_and_provider_directory_requests_rescan") else {return};
        let directory = home.join("probe");
        std::fs::create_dir_all(&directory).unwrap();
        let project = home.join(".claude/projects/p");
        std::fs::create_dir_all(&project).unwrap();
        let db = directory.join("history.db");
        crate::history::probe::history::open(&db).unwrap();
        let (events, _received) = mpsc::channel();
        let watching = Watching::start(&db, events, true);
        assert!(watching.watch.is_ok());
        let (mut owed, mut lost) = (None, false);
        watching.fold((Instant::now(), Some(directory)), &mut owed, &mut lost);
        assert!(owed.is_none() && !lost);
        watching.fold((Instant::now(), Some(project)), &mut owed, &mut lost);
        assert!(owed.is_some_and(|pending| pending.full));
    }
}
