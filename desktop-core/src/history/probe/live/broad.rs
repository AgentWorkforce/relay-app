//! Fair hot-path capture and resumable cold reconciliation.
use super::*;
mod cold;
mod initial;
mod remote;
mod startup;
pub(super) use remote::{remote, Remote};
pub(super) use startup::capture;
#[cfg(test)]
pub(super) use startup::{start, Started};

/// How long after a sweep that took `took` the next may start:
/// `duty_factor` times as long, at least `min_gap`. Expensive passes keep
/// their full recovery gap even when it exceeds the reconciliation timer.
pub(super) fn gap(took: Duration, options: &LiveOptions) -> Duration {
    took.saturating_mul(options.duty_factor)
        .max(options.min_gap)
}

pub(super) struct Context<'a> {
    pub directory: &'a Path,
    pub store: ai_hist::SessionStore,
    pub watch: Watching,
    pub received: &'a mpsc::Receiver<Noticed>,
    pub last: Swept,
    pub cancelled: &'a Arc<AtomicBool>,
    pub shared: &'a Shared,
    pub progress: Option<super::super::progress::Monitor>,
    pub options: &'a LiveOptions,
}

struct Scheduler<'a> {
    context: Context<'a>,
    owed: Option<Owed>,
    lost: bool,
    backstop: Instant,
    rounds: history::Reconciliations,
    partial_pass_finished: bool,
    cold_due: Instant,
    hot_since_cold: bool,
    audit_at: Instant,
    // Actual loss/full notices require immediate coverage after this round.
    // Periodic deadlines instead restart from completion, never synthetic loss.
    rescan_again: bool,
    recent: history::RecentPaths,
    // Once a changed stat was queued, later polls remember that stat. Keep
    // its acquisition obligation until success, apart from native-event churn.
    recent_hot: Vec<PathBuf>,
    safety: history::SafetyPaths,
    poll_at: Instant,
    // The complete watch no longer covers every provider root (one was
    // created, or a registration broke), as the last recent poll found.
    watch_outdated: bool,
    roots: history::RootSettings,
    refresh_after: Instant,
    // The initial rescan's pace until the first complete pass ends, and how
    // many of its closing maintenances were deferred or failed.
    initial: Option<LiveOptions>,
    initial_unfinished_closings: u32,
}

pub(super) fn follow(context: Context<'_>, roots: history::RootSettings) {
    Scheduler::new(context, roots).run();
}

impl<'a> Scheduler<'a> {
    fn new(context: Context<'a>, roots: history::RootSettings) -> Self {
        let now = Instant::now();
        context
            .shared
            .update(|state| state.metrics.initial_pass = true);
        Self {
            initial: Some(initial::initial_options(context.options)),
            initial_unfinished_closings: 0,
            context,
            owed: None,
            lost: false,
            backstop: now,
            rounds: Default::default(),
            partial_pass_finished: false,
            cold_due: now,
            hot_since_cold: false,
            audit_at: now,
            rescan_again: false,
            recent: Default::default(),
            recent_hot: Default::default(),
            safety: Default::default(),
            poll_at: now,
            watch_outdated: false,
            roots,
            refresh_after: now,
        }
    }

    fn run(&mut self) {
        while !stopping(self.context.cancelled) {
            if !self.prepare() {
                std::thread::sleep(STOP_POLL);
                continue;
            }
            let now = Instant::now();
            self.refresh(now);
            self.poll_recent(now);
            let hot_due = self.hot_due();
            let cold_ready = !self.rounds.is_empty() && now >= self.cold_due;
            // A due archive slice gets a turn after at most one hot pass.
            if cold_ready && (self.hot_since_cold || hot_due.is_none_or(|due| due > now)) {
                self.cold();
            } else if hot_due.is_some_and(|due| now >= due) {
                self.hot();
            } else {
                self.wait(now, hot_due);
            }
        }
    }

    fn hot_due(&self) -> Option<Instant> {
        self.owed
            .as_ref()
            .filter(|pending| !pending.paths.is_empty())
            .map(|pending| {
                (pending.first + self.context.options.debounce)
                    .max(self.context.last.ended + gap(self.context.last.took, self.options()))
            })
    }

    /// Admit queued events and validate consent before doing any capture.
    fn prepare(&mut self) -> bool {
        self.context.watch.observe(self.context.shared);
        for _ in 0..128 {
            let Ok(noticed) = self.context.received.try_recv() else {
                break;
            };
            self.context
                .watch
                .fold(noticed, &mut self.owed, &mut self.lost);
        }
        let checked = self.roots.last_checked();
        match self.roots.refresh(&mut self.context.store) {
            Ok(true) => self.reset_roots(Instant::now()),
            Ok(false) => {}
            Err(error) => {
                if self.roots.last_checked() != checked {
                    self.context
                        .shared
                        .judge(Some(collector::capture_error_class(&error).into()));
                }
                return false;
            }
        }
        true
    }

    fn reset_roots(&mut self, now: Instant) {
        self.context
            .watch
            .refresh_roots(&self.context.store, &self.roots, true);
        self.rounds.clear();
        self.partial_pass_finished = false;
        self.owed = None;
        self.recent = Default::default();
        self.recent_hot.clear();
        self.report_recent_hints();
        self.backstop = now;
        self.cold_due = now;
        self.poll_at = now;
        self.watch_outdated = false;
        self.refresh_after = now;
        self.rescan_again = false;
        self.audit_at = now;
        self.safety = Default::default();
    }

    fn poll_recent(&mut self, now: Instant) {
        if now < self.poll_at {
            return;
        }
        self.poll_at = now
            + self
                .context
                .options
                .fallback_poll
                .max(Duration::from_millis(100));
        if let Ok(paths) = self
            .roots
            .extra_claude_roots(&self.context.store)
            .and_then(|extra| {
                // A complete watcher covers only roots that existed when it
                // registered. A newly created or broken root is a loss, as in
                // selected inventory: reconcile now rather than at the long
                // watched backstop. Only the transition asks, so a root that
                // cannot be registered falls back to the short backstop
                // instead of reconciling back to back.
                let outdated = self.context.watch.watch.as_ref().is_ok_and(|watch| {
                    watch.complete()
                        && watch.outdated_paths(&history::watched_paths_for(
                            &self.context.store,
                            extra.clone(),
                        ))
                });
                if outdated && !self.watch_outdated {
                    self.lost = true;
                }
                self.watch_outdated = outdated;
                self.recent.poll_with_roots(&self.context.store, extra)
            })
        {
            for (source, path) in paths {
                let pending = self.owed.get_or_insert_with(|| Owed {
                    first: now,
                    sources: Default::default(),
                    full: false,
                    fs_event: false,
                    priority: Default::default(),
                    paths: Default::default(),
                });
                pending.sources.insert(source);
                if pending.paths.contains(&path) || pending.paths.len() < 4096 {
                    pending.paths.insert(path.clone());
                    if !self.recent_hot.contains(&path) && self.recent_hot.len() < 32 {
                        self.recent_hot.push(path);
                    }
                } else {
                    pending.full = true;
                }
            }
        }
        self.report_recent_hints();
    }

    fn report_recent_hints(&self) {
        self.context.shared.update(|state| {
            state.metrics.pending_empty_creates = self.recent.pending_empty_creates() as u64;
        });
    }

    /// How long after a healthy pass the next full round is due. Only a
    /// complete, current native watch spaces periodic coverage out; partial
    /// watches, polling and an outdated watch keep the short backstop. Real
    /// losses, directory and overflow notices still reconcile at once.
    fn coverage_interval(&self) -> Duration {
        let options = self.context.options;
        let watched = !self.watch_outdated
            && self
                .context
                .watch
                .watch
                .as_ref()
                .is_ok_and(inventory::Watch::complete);
        if watched {
            options.watched_backstop.max(options.backstop)
        } else {
            options.backstop
        }
    }

    fn refresh(&mut self, now: Instant) {
        let full_notice = self.owed.as_ref().is_some_and(|pending| pending.full);
        if now < self.refresh_after || (now < self.backstop && !self.lost && !full_notice) {
            return;
        }
        self.context
            .watch
            .refresh_roots(&self.context.store, &self.roots, false);
        // A rescan never discards an unfinished round's directory position.
        // A loss during a round schedules a fresh round afterward, so a rename
        // into an already visited directory is not missed until the backstop.
        if self.rounds.is_empty() {
            match history::reconciliation(&self.context.store, &self.roots) {
                Ok(next) => self.rounds = next,
                Err(error) => {
                    self.context
                        .shared
                        .judge(Some(collector::capture_error_class(&error).into()));
                    self.lost = true;
                    self.refresh_after =
                        now + self.context.options.min_gap.max(Duration::from_millis(100));
                    return;
                }
            }
        } else if self.rounds.healthy_pass_exhausted() {
            if let Err(error) = self.rounds.refresh(&self.context.store, &self.roots) {
                self.context
                    .shared
                    .judge(Some(collector::capture_error_class(&error).into()));
                self.lost = true;
                self.refresh_after =
                    now + self.context.options.min_gap.max(Duration::from_millis(100));
                return;
            }
            self.partial_pass_finished = false;
            self.rescan_again = false;
        } else if self.lost || full_notice {
            self.rescan_again = true;
        }
        if full_notice {
            if let Some(pending) = self.owed.as_mut() {
                pending.full = false;
            }
            if self
                .owed
                .as_ref()
                .is_some_and(|pending| pending.paths.is_empty())
            {
                self.owed = None;
            }
        }
        self.lost = false;
        self.backstop = now + self.coverage_interval();
    }

    fn hot(&mut self) {
        let Some(pending) = self.owed.as_ref() else {
            return;
        };
        let fs_event = pending.fs_event;
        let paths = pending.paths_with_recent(&self.recent_hot);
        if paths.is_empty() {
            if !pending.full {
                self.owed = None;
            }
            return;
        }
        if let Ok(watch) = &self.context.watch.watch {
            self.recent.remember(
                paths
                    .iter()
                    .flat_map(|path| watch.sources(path).map(|source| (source, path.clone()))),
            );
        }
        let paced = self.context.last.ended + gap(self.context.last.took, self.options());
        let debounced = pending.first + self.context.options.debounce;
        let waited = Some(paced.saturating_duration_since(debounced.max(self.context.last.ended)));
        let started = Instant::now();
        let stop = self.context.cancelled.clone();
        let outcome = self
            .roots
            .extra_claude_roots(&self.context.store)
            .and_then(|extra| {
                if fs_event {
                    history::sweep_paths_with_roots(
                        &self.context.store,
                        true,
                        Some(pending.exact_sources()),
                        Some(paths.clone()),
                        extra,
                        move || stopping(&stop),
                    )
                } else {
                    self.safety.sweep(
                        &self.context.store,
                        extra,
                        Some(pending.exact_sources()),
                        paths.clone(),
                        move || stopping(&stop),
                    )
                }
            });
        self.context.last = Swept {
            ended: Instant::now(),
            took: started.elapsed(),
        };
        self.hot_since_cold = true;
        let attempted: Vec<_> = self
            .recent_hot
            .iter()
            .filter(|path| paths.contains(path))
            .cloned()
            .collect();
        self.recent_hot.retain(|path| !paths.contains(path));
        if !matches!(&outcome, Ok(Some(_))) {
            // A contended or failed path still owes acquisition, but cannot
            // monopolize the recent lane ahead of every other changed file.
            self.recent_hot.extend(attempted);
        }
        // Publish only after the exact acquisition returned, so a controlled
        // empty-create fixture can observe its own attempted capture.
        self.report_recent_hints();
        if matches!(&outcome, Ok(Some(_))) {
            if let Some(pending) = self.owed.as_mut() {
                pending.completed(&paths);
            }
            if self
                .owed
                .as_ref()
                .is_some_and(|pending| pending.paths.is_empty() && !pending.full)
            {
                self.owed = None;
            }
        }
        let report = outcome.map(|report| Tick {
            trigger: if fs_event {
                Trigger::FsEvent
            } else {
                Trigger::Poll
            },
            swept: report.as_ref().is_some_and(|report| report.swept),
            contended: report.is_none(),
            changed: report.map(|report| report.changed).unwrap_or_default(),
            took: self.context.last.took,
            waited,
        });
        // Counted with ticks_paced: only a sweep that returned a tick.
        if report.is_ok() {
            self.initial_wait(waited);
        }
        if !report.as_ref().is_err_and(|error| {
            stopping(self.context.cancelled) && collector::capture_error_class(error) == "cancelled"
        }) {
            fold(
                self.context.directory,
                report,
                &self.context.store,
                self.context.shared,
            );
        }
    }

    fn wait(&mut self, now: Instant, hot_due: Option<Instant>) {
        let mut due = hot_due.map_or(self.backstop, |hot| self.backstop.min(hot));
        if !self.rounds.is_empty() {
            due = due.min(self.cold_due);
        }
        due = due.min(self.poll_at);
        if let Ok(noticed) = self.context.received.recv_timeout(
            due.max(self.refresh_after)
                .saturating_duration_since(now)
                .max(Duration::from_millis(10))
                .min(STOP_POLL),
        ) {
            self.context
                .watch
                .fold(noticed, &mut self.owed, &mut self.lost);
        }
    }
}

#[cfg(test)]
mod scheduling_tests {
    use super::*;
    #[test]
    fn a_notice_after_ready_survives_the_first_scheduler_handoff() {
        let Some(home) = crate::history::probe::live::tests::isolated_home(
            "history::probe::live::broad::scheduling_tests::a_notice_after_ready_survives_the_first_scheduler_handoff",
        ) else { return; };
        use agent_relay_node::relay::{
            claude_dirs::ClaudeDirs, process::FakeProcesses, workspace::relay_home,
        };
        let env = |key: &str| std::env::var(key).ok();
        ClaudeDirs::from_env(&env, &home, &relay_home(&env, &home))
            .remember_running(&FakeProcesses::default());
        let directory = home.join("probe");
        let db = directory.join("history.db");
        let project = home.join(".claude/projects/handoff");
        std::fs::create_dir_all(&project).unwrap();
        let store = history::open(&db).unwrap();
        let (events, received) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let shared = Shared::default();
        let options = LiveOptions::default();
        let Started {
            store,
            roots,
            watch,
        } = start(store, events.clone(), &cancelled, &shared, &options).unwrap();
        assert_eq!(shared.state().metrics.driver, Some("fs-events"));
        assert!(watch.complete(), "require a registered provider watch");

        // Controlled ready/first-round barrier: the producer writes only after
        // readiness is published, and explicitly enqueues the exact notice.
        // Neither OS callback timing nor a cold scan can make this pass.
        let active = project.join("active.jsonl");
        crate::history::probe::live::tests::append(
            &active,
            &crate::history::probe::live::tests::transcript_line("active", "handoff-one"),
        );
        events.send((Instant::now(), Some(active.clone()))).unwrap();
        let mut scheduler = Scheduler::new(
            Context {
                directory: &directory,
                store,
                watch,
                received: &received,
                last: Swept {
                    ended: Instant::now(),
                    took: Duration::ZERO,
                },
                cancelled: &cancelled,
                shared: &shared,
                progress: None,
                options: &options,
            },
            roots,
        );
        assert!(scheduler.prepare());
        assert!(
            scheduler
                .owed
                .as_ref()
                .is_some_and(|owed| owed.paths.contains(&active)),
            "a notice admitted after readiness must survive initial root validation"
        );
        assert!(scheduler.rounds.is_empty());
        scheduler.hot();
        assert_eq!(
            crate::history::probe::live::tests::events(&db, "active"),
            1,
            "the active write must use the hot path before any archive slice"
        );
        assert_eq!(shared.state().metrics.cold_slices, 0);

        // A real consent change still discards old-generation work, even for
        // an explicitly queued path, and cannot acquire its next event.
        crate::history::probe::live::tests::append(
            &active,
            &crate::history::probe::live::tests::transcript_line("active", "handoff-denied"),
        );
        events.send((Instant::now(), Some(active.clone()))).unwrap();
        ClaudeDirs::from_env(&env, &home, &relay_home(&env, &home))
            .set_shared(
                home.join(".claude").to_str().unwrap(),
                false,
                &home.join(".claude"),
            )
            .unwrap();
        scheduler.roots.invalidate();
        assert!(scheduler.prepare());
        assert!(
            scheduler.owed.is_none(),
            "real policy resets must discard queued old-generation paths"
        );
        scheduler.context.watch.fold(
            (Instant::now(), Some(active)),
            &mut scheduler.owed,
            &mut scheduler.lost,
        );
        assert!(
            scheduler.owed.is_none(),
            "revoked paths cannot be readmitted"
        );
        assert_eq!(crate::history::probe::live::tests::events(&db, "active"), 1);
    }

    #[test]
    fn malformed_startup_settings_retry_before_publishing_a_watch() {
        let Some(home) = crate::history::probe::live::tests::isolated_home(
            "history::probe::live::broad::scheduling_tests::malformed_startup_settings_retry_before_publishing_a_watch",
        ) else { return; };
        use agent_relay_node::relay::{
            claude_dirs::{ClaudeDirs, FILE},
            process::FakeProcesses,
            workspace::relay_home,
        };
        let env = |key: &str| std::env::var(key).ok();
        let relay = relay_home(&env, &home);
        ClaudeDirs::from_env(&env, &home, &relay).remember_running(&FakeProcesses::default());
        std::fs::create_dir_all(home.join(".claude/projects/invalid")).unwrap();
        let store = history::open(&home.join("probe/history.db")).unwrap();
        std::fs::create_dir_all(&relay).unwrap();
        let settings = relay.join(FILE);
        std::fs::write(&settings, b"{").unwrap();
        let (events, _received) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared::default());
        let (finished, outcome) = mpsc::channel();
        let thread = {
            let cancelled = cancelled.clone();
            let shared = shared.clone();
            std::thread::spawn(move || {
                let started = start(store, events, &cancelled, &shared, &LiveOptions::default());
                finished.send(started.is_some()).unwrap();
            })
        };
        // judge() signals this barrier only once the invalid settings were
        // rejected. The timeout is a deadlock guard, not an event deadline.
        let rejected = shared.wait(Instant::now() + Duration::from_secs(5), || false);
        let failure = shared.verdict();
        let premature_driver = shared.state().metrics.driver;
        std::fs::write(&settings, b"{}").unwrap();
        let recovered = outcome.recv_timeout(Duration::from_secs(5));
        cancelled.store(true, Ordering::Relaxed);
        thread.join().unwrap();
        assert!(rejected && failure.is_some_and(|verdict| verdict.is_some()));
        assert_eq!(
            premature_driver, None,
            "invalid consent must not publish a watch"
        );
        assert!(
            recovered.unwrap(),
            "valid settings must resume startup without restarting capture"
        );
        assert_eq!(shared.state().metrics.driver, Some("fs-events"));
    }

    pub(super) fn with_native_scheduler(
        label: &str,
        use_events: bool,
        check: impl FnOnce(&mut Scheduler<'_>, &Path, &[PathBuf]),
    ) {
        let Some(home) = crate::history::probe::live::tests::isolated_home(label) else {
            return;
        };
        use agent_relay_node::relay::{
            claude_dirs::ClaudeDirs, process::FakeProcesses, workspace::relay_home,
        };
        let env = |key: &str| std::env::var(key).ok();
        ClaudeDirs::from_env(&env, &home, &relay_home(&env, &home))
            .remember_running(&FakeProcesses::default());
        let directory = home.join("probe");
        let db = directory.join("history.db");
        let project = home.join(".claude/projects/native");
        std::fs::create_dir_all(&project).unwrap();
        let paths: Vec<_> = ["first", "second"]
            .into_iter()
            .map(|id| {
                let path = project.join(format!("{id}.jsonl"));
                crate::history::probe::live::tests::append(
                    &path,
                    &crate::history::probe::live::tests::transcript_line(id, "one"),
                );
                path
            })
            .collect();
        let store = history::open(&db).unwrap();
        store
            .sync(ai_hist::SyncOptions::default().sources([Source::Claude]))
            .unwrap();
        let (events, received) = mpsc::channel();
        let watch = Watching::start(&db, events, use_events);
        let cancelled = Arc::new(AtomicBool::new(false));
        let shared = Shared::default();
        let options = LiveOptions::default();
        let mut scheduler = Scheduler::new(
            Context {
                directory: &directory,
                store,
                watch,
                received: &received,
                last: Swept {
                    ended: Instant::now(),
                    took: Duration::ZERO,
                },
                cancelled: &cancelled,
                shared: &shared,
                progress: None,
                options: &options,
            },
            Default::default(),
        );
        scheduler
            .roots
            .refresh(&mut scheduler.context.store)
            .unwrap();
        check(&mut scheduler, &home, &paths);
    }

    #[test]
    fn full_request_keeps_exact_paths_during_reconciliation_backoff() {
        with_native_scheduler(
            "history::probe::live::broad::scheduling_tests::full_request_keeps_exact_paths_during_reconciliation_backoff",
            false,
            |scheduler, _, paths| {
                let db = scheduler.context.store.db_path().to_path_buf();
                scheduler.owed = Some(Owed {
                    first: Instant::now(),
                    sources: HashSet::from([Source::Claude]),
                    full: true,
                    fs_event: true,
                    priority: Default::default(),
                    paths: paths.iter().cloned().collect(),
                });
                scheduler.context.store = history::open_read(&db).unwrap();
                let now = Instant::now();
                scheduler.refresh(now);
                assert!(scheduler.lost && scheduler.rounds.is_empty());
                assert!(scheduler.refresh_after > now);
                scheduler.context.store = history::open(&db).unwrap();
                for (path, id) in paths.iter().zip(["first", "second"]) {
                    crate::history::probe::live::tests::append(path, &crate::history::probe::live::tests::transcript_line(id, "two"));
                }
                scheduler.refresh(now);
                assert!(scheduler.rounds.is_empty(), "reconciliation retry remains deferred");
                scheduler.hot();
                for id in ["first", "second"] {
                    assert_eq!(crate::history::probe::live::tests::events(&db, id), 2, "a full request must retain exact appended evidence during retry backoff");
                }
                let pending = scheduler.owed.as_ref().expect("the full rescan remains owed after exact work drains");
                assert!(pending.full && pending.paths.is_empty());
                assert!(scheduler.hot_due().is_none(), "a full-only obligation must not spin on a due hot deadline");
                scheduler.refresh(scheduler.refresh_after);
                assert!(!scheduler.rounds.is_empty(), "the full request must still create a reconciliation round");
                assert!(scheduler.owed.is_none());
            },
        );
    }

    #[test]
    fn full_request_from_recent_overflow_keeps_bounded_hot_work() {
        with_native_scheduler(
            "history::probe::live::broad::scheduling_tests::full_request_from_recent_overflow_keeps_bounded_hot_work",
            false,
            |scheduler, _, paths| {
                let now = Instant::now();
                scheduler.refresh(now);
                assert!(!scheduler.rounds.is_empty());
                let project = paths[0].parent().unwrap();
                let mut queued: HashSet<_> = (0..4094).map(|n| project.join(format!("queued-{n}.jsonl"))).collect();
                queued.extend(paths.iter().cloned());
                let selected = queued.iter().take(8).find(|path| !paths.contains(path)).unwrap().clone();
                crate::history::probe::live::tests::append(&selected, &crate::history::probe::live::tests::transcript_line("queued", "one"));
                let overflow = project.join("overflow.jsonl");
                crate::history::probe::live::tests::append(&overflow, &crate::history::probe::live::tests::transcript_line("overflow", "one"));
                let mut discovery = ai_hist::DiscoveryOptions::default().sources([Source::Claude]);
                discovery.paths = Some(vec![overflow]);
                scheduler.context.store.discover(discovery).unwrap();
                scheduler.owed = Some(Owed {
                    first: now,
                    sources: HashSet::from([Source::Claude]),
                    full: false,
                    fs_event: true,
                    priority: Default::default(),
                    paths: queued,
                });
                scheduler.poll_recent(now);
                assert!(scheduler.owed.as_ref().unwrap().full, "exercise actual recent-poll overflow after refresh");
                scheduler.hot();
                let db = scheduler.context.store.db_path();
                assert_eq!(crate::history::probe::live::tests::events(db, "queued"), 1, "queued exact work must survive a later full notice");
                assert_eq!(crate::history::probe::live::tests::events(db, "overflow"), 0, "overflowed paths belong to reconciliation, outside the bounded exact batch");
                let pending = scheduler.owed.as_ref().unwrap();
                assert!(pending.full);
                assert_eq!(pending.paths.len(), 4088, "one hot turn acquires at most eight queued paths");
                scheduler.refresh(now);
                assert!(scheduler.rescan_again, "full coverage during a round must retain its successor obligation");
                assert!(!scheduler.owed.as_ref().unwrap().full);
            },
        );
    }

    #[test]
    fn full_request_preserves_exact_provider_scope() {
        with_native_scheduler(
            "history::probe::live::broad::scheduling_tests::full_request_preserves_exact_provider_scope",
            false,
            |scheduler, _, paths| {
                let db = scheduler.context.store.db_path().to_path_buf();
                let codex = scheduler
                    .context
                    .store
                    .roots()
                    .codex
                    .join("sessions/2026/10/25/rollout-scope.jsonl");
                std::fs::create_dir_all(codex.parent().unwrap()).unwrap();
                std::fs::write(&codex, format!("{}\n{}\n",
                    serde_json::json!({"timestamp":"2026-10-25T00:00:00Z","type":"session_meta","payload":{"id":"scoped-codex","cwd":"/tmp/project","source":"cli"}}),
                    serde_json::json!({"timestamp":"2026-10-25T00:00:01Z","type":"event_msg","payload":{"type":"agent_message","message":"codex scope control"}}),
                )).unwrap();
                crate::history::probe::live::tests::append(
                    &paths[0],
                    &crate::history::probe::live::tests::transcript_line("first", "two"),
                );
                scheduler.owed = Some(Owed {
                    first: Instant::now(),
                    sources: HashSet::from([Source::Claude]),
                    full: true,
                    fs_event: true,
                    priority: Default::default(),
                    paths: HashSet::from([paths[0].clone(), codex.clone()]),
                });
                scheduler.hot();
                assert_eq!(crate::history::probe::live::tests::events(&db, "first"), 2);
                assert_eq!(
                    crate::history::probe::live::tests::events(&db, "scoped-codex"),
                    0,
                    "a full-rescan request must not widen the exact batch's provider allowlist"
                );
                assert!(scheduler.owed.as_ref().is_some_and(|pending| pending.full));
                scheduler
                    .context
                    .store
                    .sync(
                        ai_hist::SyncOptions::default()
                            .sources([Source::Codex])
                            .paths([codex])
                            .force(true),
                    )
                    .unwrap();
                assert_eq!(
                    crate::history::probe::live::tests::events(&db, "scoped-codex"),
                    1,
                    "the excluded provider fixture must contain real capturable evidence"
                );
            },
        );
    }

    #[test]
    fn a_changed_recent_transcript_precedes_the_archive_notice_backlog() {
        with_native_scheduler(
            "history::probe::live::broad::scheduling_tests::a_changed_recent_transcript_precedes_the_archive_notice_backlog",
            false,
            |scheduler, _, paths| {
                let project = paths[0].parent().unwrap();
                let queued: HashSet<_> = (0..4096)
                    .map(|n| project.join(format!("queued-{n}.jsonl")))
                    .collect();
                // Deliberately outside the next arbitrary HashSet batch: the
                // recent poll, rather than luck or archive completion, must
                // give the live writer a turn.
                let active = queued.iter().nth(16).unwrap().clone();
                let append = |marker| {
                    crate::history::probe::live::tests::append(
                        &active,
                        &crate::history::probe::live::tests::transcript_line("active-priority", marker),
                    );
                };
                append("first");
                scheduler.context.store.sync(
                    ai_hist::SyncOptions::default().sources([Source::Claude]).paths([active.clone()]).force(true),
                ).unwrap();
                scheduler.recent.poll(&scheduler.context.store).unwrap();
                scheduler.owed = Some(Owed {
                    first: Instant::now(),
                    sources: HashSet::from([Source::Claude]),
                    full: false,
                    fs_event: false,
                    priority: Default::default(),
                    paths: queued,
                });
                append("second");
                scheduler.poll_recent(Instant::now());
                let pending = scheduler.owed.as_mut().unwrap();
                let later_notices: Vec<_> = pending.paths.iter()
                    .filter(|path| *path != &active).take(64).cloned().collect();
                for path in later_notices {
                    pending.prioritize(&path);
                }
                scheduler.hot();
                assert_eq!(
                    crate::history::probe::live::tests::events(scheduler.context.store.db_path(), "active-priority"),
                    2,
                    "a changed recent transcript must not wait behind archive notices",
                );
                assert_eq!(scheduler.owed.as_ref().unwrap().paths.len(), 4088);
            },
        );
    }

    #[test]
    fn full_request_without_exact_paths_has_no_hot_deadline() {
        with_native_scheduler(
            "history::probe::live::broad::scheduling_tests::full_request_without_exact_paths_has_no_hot_deadline",
            false,
            |scheduler, _, _| {
                scheduler.owed = Some(Owed {
                    first: Instant::now(),
                    sources: Default::default(),
                    full: true,
                    fs_event: true,
                    priority: Default::default(),
                    paths: Default::default(),
                });
                scheduler.refresh_after = Instant::now() + Duration::from_secs(30);
                scheduler.refresh(Instant::now());
                assert!(scheduler.owed.as_ref().unwrap().full);
                assert!(scheduler.hot_due().is_none(), "deferred full-only work must wait for reconciliation rather than repeatedly call hot");
            },
        );
    }

    #[test]
    fn mixed_native_and_safety_paths_keep_forced_acquisition() {
        with_native_scheduler(
            "history::probe::live::broad::scheduling_tests::mixed_native_and_safety_paths_keep_forced_acquisition",
            true,
            |scheduler, _, paths| {
                scheduler.owed = Some(Owed {
                    first: Instant::now(),
                    sources: HashSet::from([Source::Claude]),
                    full: false,
                    fs_event: false,
                    priority: Default::default(),
                    paths: HashSet::from([paths[0].clone()]),
                });
                scheduler.context.watch.fold(
                    (Instant::now(), Some(paths[1].clone())),
                    &mut scheduler.owed,
                    &mut scheduler.lost,
                );
                assert!(scheduler.owed.as_ref().unwrap().fs_event);
                scheduler.hot();
                let count: i64 = rusqlite::Connection::open(scheduler.context.store.db_path())
                    .unwrap()
                    .query_row(
                        "SELECT COUNT(*) FROM session_hydration_checkpoints",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(count,2,"mixed/native notices must keep the forced route, even for native-only unchanged evidence");
                assert_eq!(scheduler.safety.context_count(), 0);
            },
        );
    }

    #[test]
    fn safety_context_is_dropped_after_global_audit_and_root_revocation() {
        with_native_scheduler("history::probe::live::broad::scheduling_tests::safety_context_is_dropped_after_global_audit_and_root_revocation",false,|scheduler,home,paths| {
            scheduler.recent.poll(&scheduler.context.store).unwrap();
            scheduler.safety.sweep(&scheduler.context.store,vec![],Some(vec![Source::Claude]),paths.to_vec(),||false).unwrap().unwrap();
            assert_eq!(scheduler.safety.context_count(),1);
            scheduler.finish_round();
            assert!(scheduler.audit_at>Instant::now()+Duration::from_secs(3600),"require actual successful global maintenance");
            assert_eq!(scheduler.safety.context_count(),0,"the old native proof snapshot must not survive a global audit");
            scheduler.safety.sweep(&scheduler.context.store,vec![],Some(vec![Source::Claude]),paths.to_vec(),||false).unwrap().unwrap();
            assert_eq!(scheduler.safety.context_count(),1);
            use agent_relay_node::relay::{claude_dirs::ClaudeDirs,workspace::relay_home};
            let env=|key:&str|std::env::var(key).ok();
            ClaudeDirs::from_env(&env,home,&relay_home(&env,home)).set_shared(home.join(".claude").to_str().unwrap(),false,&home.join(".claude")).unwrap();
            scheduler.roots.invalidate();
            assert!(scheduler.roots.refresh(&mut scheduler.context.store).unwrap());
            scheduler.reset_roots(Instant::now());
            assert_eq!(scheduler.safety.context_count(),0);
            crate::history::probe::live::tests::append(&paths[0],&crate::history::probe::live::tests::transcript_line("first","denied-two"));
            let _=scheduler.safety.sweep(&scheduler.context.store,vec![],Some(vec![Source::Claude]),paths.to_vec(),||false);
            let count:i64=rusqlite::Connection::open(scheduler.context.store.db_path()).unwrap()
                .query_row("SELECT COUNT(*) FROM session_events WHERE session_id='first'",[],|row|row.get(0)).unwrap();
            assert_eq!(count,1,"revoked base roots cannot acquire the new evidence through a retained proof");
        });
    }

    #[test]
    fn a_queued_write_skips_audit_and_invalidates_cached_consent() {
        let Some(home) = crate::history::probe::live::tests::isolated_home("history::probe::live::broad::scheduling_tests::a_queued_write_skips_audit_and_invalidates_cached_consent") else { return; };
        let directory = home.join("probe");
        let db = directory.join("history.db");
        let store = history::open(&db).unwrap();
        let path = home.join(".claude/projects/queued/queued.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        crate::history::probe::live::tests::append(
            &path,
            &crate::history::probe::live::tests::transcript_line("queued", "one"),
        );
        let (events, received) = mpsc::channel();
        let watch = Watching::start(&db, events.clone(), true);
        let cancelled = Arc::new(AtomicBool::new(false));
        let shared = Shared::default();
        let options = LiveOptions::default();
        let mut scheduler = Scheduler::new(
            Context {
                directory: &directory,
                store,
                watch,
                received: &received,
                last: Swept {
                    ended: Instant::now(),
                    took: Duration::ZERO,
                },
                cancelled: &cancelled,
                shared: &shared,
                progress: None,
                options: &options,
            },
            Default::default(),
        );
        scheduler
            .roots
            .refresh(&mut scheduler.context.store)
            .unwrap();
        assert!(scheduler.roots.last_checked().is_some());
        // A write arrives during the last cold slice, after its normal drain.
        events.send((Instant::now(), Some(path.clone()))).unwrap();
        scheduler.finish_round();
        assert!(scheduler
            .owed
            .as_ref()
            .is_some_and(|owed| owed.paths.contains(&path)));
        assert!(scheduler.roots.last_checked().is_none());
        let rows: i64 = rusqlite::Connection::open(&db)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM session_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            rows, 0,
            "maintenance must yield before acquiring the queued transcript"
        );
    }

    #[test]
    fn a_complete_watch_spaces_full_rounds_until_a_new_root_appears() {
        let Some(home) = crate::history::probe::live::tests::isolated_home("history::probe::live::broad::scheduling_tests::a_complete_watch_spaces_full_rounds_until_a_new_root_appears") else { return; };
        let directory = home.join("probe");
        let db = directory.join("history.db");
        std::fs::create_dir_all(home.join(".claude/projects/p")).unwrap();
        let store = history::open(&db).unwrap();
        let (events, received) = mpsc::channel();
        let watch = Watching::start(&db, events, true);
        assert!(watch.complete(), "require a registered provider watch");
        let cancelled = Arc::new(AtomicBool::new(false));
        let shared = Shared::default();
        let options = LiveOptions {
            watched_backstop: Duration::from_secs(30 * 60),
            ..LiveOptions::default()
        };
        let mut scheduler = Scheduler::new(
            Context {
                directory: &directory,
                store,
                watch,
                received: &received,
                last: Swept {
                    ended: Instant::now(),
                    took: Duration::ZERO,
                },
                cancelled: &cancelled,
                shared: &shared,
                progress: None,
                options: &options,
            },
            Default::default(),
        );
        scheduler
            .roots
            .refresh(&mut scheduler.context.store)
            .unwrap();
        scheduler.rounds =
            history::reconciliation(&scheduler.context.store, &scheduler.roots).unwrap();
        scheduler.audit_at = Instant::now() + Duration::from_secs(86400);
        let finishing = Instant::now();
        while !scheduler.rounds.is_empty() {
            scheduler.cold();
        }
        assert!(!scheduler.lost);
        assert!(
            scheduler.backstop >= finishing + options.watched_backstop,
            "a complete watch need not re-walk every tree each minute"
        );
        scheduler.poll_recent(Instant::now());
        assert!(!scheduler.lost, "an unchanged watch is not a loss");

        // A provider root created after registration is not watched yet:
        // the next recent poll reconciles at once and keeps the short
        // backstop until the watcher covers it.
        std::fs::create_dir_all(home.join(".codex/sessions")).unwrap();
        scheduler.poll_recent(scheduler.poll_at);
        assert!(
            scheduler.lost,
            "a new provider root must not wait for the long backstop"
        );
        assert_eq!(scheduler.coverage_interval(), options.backstop);
        scheduler.lost = false;
        scheduler.poll_recent(scheduler.poll_at);
        assert!(
            !scheduler.lost,
            "only the transition requests reconciliation"
        );
        // The requested loss is still pending when the scheduler refreshes.
        scheduler.lost = true;
        scheduler.refresh(Instant::now());
        assert!(!scheduler.lost);
        assert!(
            !scheduler.rounds.is_empty(),
            "the new root is reconciled at once"
        );
        scheduler.poll_recent(scheduler.poll_at);
        assert_eq!(
            scheduler.coverage_interval(),
            options.watched_backstop,
            "re-registration restores the watched interval"
        );
    }

    #[test]
    fn periodic_deadline_waits_after_completion_but_real_loss_retries_immediately() {
        let Some(home) = crate::history::probe::live::tests::isolated_home("history::probe::live::broad::scheduling_tests::periodic_deadline_waits_after_completion_but_real_loss_retries_immediately") else { return; };
        let directory = home.join("probe");
        let db = directory.join("history.db");
        let store = history::open(&db).unwrap();
        let (events, received) = mpsc::channel();
        let watch = Watching::start(&db, events, false);
        let cancelled = Arc::new(AtomicBool::new(false));
        let shared = Shared::default();
        let options = LiveOptions::default();
        let mut scheduler = Scheduler::new(
            Context {
                directory: &directory,
                store,
                watch,
                received: &received,
                last: Swept {
                    ended: Instant::now(),
                    took: Duration::ZERO,
                },
                cancelled: &cancelled,
                shared: &shared,
                progress: None,
                options: &options,
            },
            Default::default(),
        );
        scheduler
            .roots
            .refresh(&mut scheduler.context.store)
            .unwrap();
        scheduler.rounds =
            history::reconciliation(&scheduler.context.store, &scheduler.roots).unwrap();
        let head = scheduler.context.store.head_revision().unwrap();
        // Deterministic elapsed-time control: this unchanged SDK round spans
        // three periodic deadlines without a watcher loss or directory notice.
        scheduler.refresh(scheduler.backstop + options.backstop * 3);
        assert!(!scheduler.rescan_again, "a timer is not lost coverage");
        scheduler.audit_at = Instant::now() + Duration::from_secs(86400);
        let finishing = Instant::now();
        while !scheduler.rounds.is_empty() {
            scheduler.cold();
        }
        assert_eq!(scheduler.context.store.head_revision().unwrap(), head);
        assert!(!scheduler.lost, "quiet completion must not synthesize loss");
        let next = scheduler.backstop;
        assert!(next >= finishing + options.backstop);
        scheduler.refresh(next - Duration::from_nanos(1));
        assert!(
            scheduler.rounds.is_empty(),
            "retain a post-completion quiet interval"
        );
        scheduler.refresh(next);
        assert!(
            !scheduler.rounds.is_empty(),
            "the periodic backstop is still owed"
        );
        scheduler.lost = true;
        scheduler.refresh(next);
        assert!(
            scheduler.rescan_again,
            "real loss during coverage needs a successor"
        );
        while !scheduler.rounds.is_empty() {
            scheduler.cold();
        }
        assert!(scheduler.lost, "real loss must survive round completion");
        scheduler.context.store = history::open_read(&db).unwrap();
        let now = Instant::now();
        scheduler.refresh(now);
        assert!(scheduler.lost && scheduler.rounds.is_empty());
        assert!(scheduler.refresh_after > now);
        scheduler.context.store = history::open(&db).unwrap();
        scheduler.refresh(scheduler.refresh_after);
        assert!(!scheduler.lost && !scheduler.rounds.is_empty());
    }
    #[test]
    fn complete_watch_still_stats_a_held_open_recent_transcript() {
        let Some(home) = crate::history::probe::live::tests::isolated_home("history::probe::live::broad::scheduling_tests::complete_watch_still_stats_a_held_open_recent_transcript") else {return};
        let directory = home.join("probe");
        let db = directory.join("history.db");
        let path = home.join(".claude/projects/recent/recent.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        crate::history::probe::live::tests::append(
            &path,
            &crate::history::probe::live::tests::transcript_line("recent", "one"),
        );
        let store = history::open(&db).unwrap();
        history::sweep_paths_once(
            &store,
            false,
            Some(vec![Source::Claude]),
            Some(vec![path.clone()]),
            || false,
        )
        .unwrap();
        let (events, received) = mpsc::channel();
        let watch = Watching::start(&db, events, true);
        assert!(
            watch.complete(),
            "exercise healthy registration, not fallback"
        );
        let cancelled = Arc::new(AtomicBool::new(false));
        let shared = Shared::default();
        let options = LiveOptions::default();
        let mut scheduler = Scheduler::new(
            Context {
                directory: &directory,
                store,
                watch,
                received: &received,
                last: Swept {
                    ended: Instant::now(),
                    took: Duration::ZERO,
                },
                cancelled: &cancelled,
                shared: &shared,
                progress: None,
                options: &options,
            },
            Default::default(),
        );
        scheduler
            .roots
            .refresh(&mut scheduler.context.store)
            .unwrap();
        scheduler.poll_recent(Instant::now());
        scheduler.owed = None;
        // Drain no notifications: successful watch registration cannot prove a
        // flushed, still-open descriptor generated a notification.
        let mut writer = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        use std::io::Write;
        writer
            .write_all(
                crate::history::probe::live::tests::transcript_line("recent", "two").as_bytes(),
            )
            .unwrap();
        writer.flush().unwrap();
        scheduler.poll_recent(scheduler.poll_at);
        assert!(scheduler
            .owed
            .as_ref()
            .is_some_and(|owed| owed.paths.contains(&path)));
        scheduler.hot();
        let count: usize = rusqlite::Connection::open(&db)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM session_events WHERE session_id='recent'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
        assert!(!shared
            .state()
            .metrics
            .ticks_by_trigger
            .contains_key("fs-event"));
        assert_eq!(
            shared.state().metrics.ticks_by_trigger.get("poll"),
            Some(&1)
        );
        // Unchanged stats do not repeatedly re-enqueue that known file.
        scheduler.poll_recent(scheduler.poll_at);
        assert!(scheduler.owed.is_none());
        drop(writer);
    }

    #[test]
    fn partial_reconciliation_revisits_healthy_roots_without_completing_failed_work() {
        with_native_scheduler(
            "history::probe::live::broad::scheduling_tests::partial_reconciliation_revisits_healthy_roots_without_completing_failed_work",
            false,
            |scheduler, home, paths| {
                let bad = home.join(".codex/archived_sessions");
                std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
                std::fs::write(&bad, b"not a directory").unwrap();
                let db = scheduler.context.store.db_path().to_path_buf();
                scheduler.refresh(Instant::now());
                for _ in 0..120 { scheduler.cold(); }
                assert!(!scheduler.rounds.is_empty(), "failed enumeration remains owed");
                assert_eq!(scheduler.context.shared.state().metrics.sweeps, 0);
                let partial_deadline = scheduler.backstop;
                scheduler.cold();
                assert_eq!(scheduler.backstop, partial_deadline, "retry-only turns must not slide the next healthy pass forever");
                assert!(scheduler.audit_at <= Instant::now(), "partial coverage must not run successful maintenance");
                let new = paths[0].parent().unwrap().join("periodic-new.jsonl");
                crate::history::probe::live::tests::append(&new, &crate::history::probe::live::tests::transcript_line("periodic-new", "one"));
                let retry_due = scheduler.cold_due;
                scheduler.refresh(scheduler.backstop);
                assert_eq!(scheduler.cold_due, retry_due, "fresh coverage must not postpone the existing failed retry deadline");
                for _ in 0..120 { scheduler.cold(); }
                assert_eq!(crate::history::probe::live::tests::events(&db, "periodic-new"), 1,
                    "periodic coverage must revisit healthy roots while failed work stays queued");
                assert!(!scheduler.rounds.is_empty());
                assert_eq!(scheduler.context.shared.state().metrics.sweeps, 0);
                // A loss admitted while healthy traversal is active must survive
                // that partial boundary and trigger a successor promptly.
                scheduler.refresh(scheduler.backstop);
                scheduler.lost = true;
                scheduler.refresh(Instant::now());
                assert!(scheduler.rescan_again);
                for _ in 0..120 { scheduler.cold(); }
                assert!(scheduler.lost, "partial exhaustion must release the real-loss successor");
                let lost = new.with_file_name("loss-new.jsonl");
                crate::history::probe::live::tests::append(&lost, &crate::history::probe::live::tests::transcript_line("loss-new", "one"));
                scheduler.lost = true;
                scheduler.refresh(Instant::now());
                for _ in 0..120 { scheduler.cold(); }
                assert_eq!(crate::history::probe::live::tests::events(&db, "loss-new"), 1,
                    "loss recovery must not wait for unresolved failure to disappear");
                // Repair the same retained obligation; only then may completion succeed.
                std::fs::remove_file(&bad).unwrap();
                std::fs::create_dir_all(&bad).unwrap();
                for _ in 0..120 {
                    if scheduler.rounds.is_empty() { break; }
                    scheduler.cold();
                }
                assert!(scheduler.rounds.is_empty());
                assert_eq!(scheduler.context.shared.state().metrics.sweeps, 1);
            },
        );
    }

    #[test]
    fn partial_reconciliation_keeps_exact_new_thread_capture_on_the_hot_lane() {
        with_native_scheduler(
            "history::probe::live::broad::scheduling_tests::partial_reconciliation_keeps_exact_new_thread_capture_on_the_hot_lane",
            true,
            |scheduler, home, paths| {
                let bad = home.join(".codex/archived_sessions");
                std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
                std::fs::write(&bad, b"not a directory").unwrap();
                scheduler.refresh(Instant::now());
                for _ in 0..120 { scheduler.cold(); }
                let new = paths[0].parent().unwrap().join("hot-new.jsonl");
                crate::history::probe::live::tests::append(&new, &crate::history::probe::live::tests::transcript_line("hot-new", "one"));
                scheduler.context.watch.fold((Instant::now(), Some(new.clone())), &mut scheduler.owed, &mut scheduler.lost);
                assert!(scheduler.hot_due().is_some(), "the exact admitted event must be independently runnable");
                let slices = scheduler.context.shared.state().metrics.cold_slices;
                let retry_due = scheduler.cold_due;
                scheduler.hot();
                assert_eq!(crate::history::probe::live::tests::events(scheduler.context.store.db_path(), "hot-new"), 1);
                assert_eq!(scheduler.context.shared.state().metrics.cold_slices, slices,
                    "a hot write must not need a fresh cold coverage pass");
                assert_eq!(scheduler.cold_due, retry_due, "hot capture cannot defer failed cold obligations");
                assert!(!scheduler.rounds.is_empty());
            },
        );
    }
}
