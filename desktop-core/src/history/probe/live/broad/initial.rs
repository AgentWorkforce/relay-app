//! The larger, bounded capture budget of a capture run's first complete pass.
use super::*;

/// The initial rescan's pace: the first complete broad pass of a capture run
/// (the cold backfill, its reconciliation and the maintenance that ends it)
/// starts each hot sweep and cold slice no sooner than this many times the
/// last one's duration after it ended, so each lane holds at most about
/// `1 / (1 + factor)`, half of one core. Both lanes share the one capture
/// thread, so together they never hold more than one core. Once that pass
/// completes, [`SWEEP_DUTY_FACTOR`] and the cold factor pace capture again.
/// A capture run starts on every enrolment, re-enrolment and probe start.
pub(in crate::history::probe::live) const INITIAL_DUTY_FACTOR: u32 = 1;

/// How many full rounds whose closing maintenance was deferred or failed the
/// initial budget lasts before it ends anyway.
pub(super) const INITIAL_UNFINISHED_CLOSINGS: u32 = 3;

/// `options` with both duty factors capped at [`INITIAL_DUTY_FACTOR`]: never
/// slower than the steady pace, and test overrides below it are kept.
pub(super) fn initial_options(options: &LiveOptions) -> LiveOptions {
    LiveOptions {
        duty_factor: options.duty_factor.min(INITIAL_DUTY_FACTOR),
        cold_duty_factor: options.cold_duty_factor.min(INITIAL_DUTY_FACTOR),
        ..options.clone()
    }
}

impl Scheduler<'_> {
    /// The pace in force: the initial budget until the first pass completes.
    pub(super) fn options(&self) -> &LiveOptions {
        self.initial.as_ref().unwrap_or(self.context.options)
    }

    /// The first complete pass ended: return to the steady factors.
    pub(super) fn end_initial_pass(&mut self) {
        if self.initial.take().is_some() {
            self.context
                .shared
                .update(|state| state.metrics.initial_pass = false);
        }
    }

    /// A full round finished. The first complete pass ends with its closing
    /// maintenance: when that ran to completion, or was not due and none was
    /// left owed. A deferred or failed one stays due and keeps the initial
    /// budget for the next round, at most [`INITIAL_UNFINISHED_CLOSINGS`]
    /// times, so hot work queued at every round end or a maintenance that
    /// keeps failing cannot hold it forever.
    pub(super) fn close_initial_pass(&mut self, due: bool, maintained: bool) {
        if self.initial.is_none() {
            return;
        }
        if due && !maintained {
            self.initial_unfinished_closings += 1;
        }
        if maintained
            || (!due && self.initial_unfinished_closings == 0)
            || self.initial_unfinished_closings >= INITIAL_UNFINISHED_CLOSINGS
        {
            self.end_initial_pass();
        }
    }

    /// Count a hot sweep that waited for the pace under the initial budget.
    pub(super) fn initial_wait(&self, waited: Option<Duration>) {
        if self.initial.is_some() && waited.is_some_and(|waited| !waited.is_zero()) {
            self.context
                .shared
                .update(|state| state.metrics.initial_pass_ticks_paced += 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::scheduling_tests::with_native_scheduler;
    use super::*;

    /// The first pass runs at the initial budget; once it completes, capture
    /// is paced by the steady factors again, and the metrics say which.
    #[test]
    fn the_initial_pass_uses_the_larger_budget_until_it_completes() {
        with_native_scheduler(
            "history::probe::live::broad::initial::tests::the_initial_pass_uses_the_larger_budget_until_it_completes",
            false,
            |scheduler, _, _| {
                // Known recent paths: the closing maintenance runs to completion.
                scheduler.recent.poll(&scheduler.context.store).unwrap();
                let metrics = || scheduler.context.shared.state().metrics.report();
                assert_eq!(metrics()["initial_pass"], true);
                assert_eq!(scheduler.options().duty_factor, INITIAL_DUTY_FACTOR);
                assert_eq!(scheduler.options().cold_duty_factor, INITIAL_DUTY_FACTOR);
                let took = Duration::from_secs(3);
                assert_eq!(gap(took, scheduler.options()), took.max(SWEEP_MIN_GAP));
                scheduler.refresh(Instant::now());
                assert!(!scheduler.rounds.is_empty());
                scheduler.cold();
                assert_eq!(
                    scheduler.options().cold_duty_factor,
                    INITIAL_DUTY_FACTOR,
                    "an unfinished backfill keeps the initial budget"
                );
                for _ in 0..120 {
                    if scheduler.rounds.is_empty() {
                        break;
                    }
                    scheduler.cold();
                }
                assert!(scheduler.rounds.is_empty());
                assert_eq!(metrics()["sweeps"], 1);
                assert_eq!(metrics()["initial_pass"], false);
                assert_eq!(scheduler.options().duty_factor, SWEEP_DUTY_FACTOR);
                assert_eq!(scheduler.options().cold_duty_factor, COLD_DUTY_FACTOR);
                assert_eq!(gap(took, scheduler.options()), took * SWEEP_DUTY_FACTOR);
                // A later round never brings the initial budget back.
                scheduler.lost = true;
                scheduler.refresh(Instant::now());
                for _ in 0..120 {
                    if scheduler.rounds.is_empty() {
                        break;
                    }
                    scheduler.cold();
                }
                assert_eq!(metrics()["sweeps"], 2);
                assert_eq!(metrics()["initial_pass"], false);
                assert_eq!(scheduler.options().cold_duty_factor, COLD_DUTY_FACTOR);
            },
        );
    }

    fn owed(paths: &[PathBuf]) -> Owed {
        Owed {
            first: Instant::now(),
            sources: HashSet::from([Source::Claude]),
            full: false,
            fs_event: true,
            priority: Default::default(),
            paths: paths.iter().cloned().collect(),
        }
    }

    /// Run cold slices until the round in progress ends.
    fn drain(scheduler: &mut Scheduler<'_>) {
        for _ in 0..120 {
            if scheduler.rounds.is_empty() {
                break;
            }
            scheduler.cold();
        }
    }

    /// An object that keeps failing keeps its round owed, but once the
    /// healthy part of the first pass is exhausted the initial budget ends.
    #[test]
    fn a_permanently_failing_object_still_ends_the_initial_pass() {
        with_native_scheduler(
            "history::probe::live::broad::initial::tests::a_permanently_failing_object_still_ends_the_initial_pass",
            false,
            |scheduler, home, _| {
                let bad = home.join(".codex/archived_sessions");
                std::fs::create_dir_all(bad.parent().unwrap()).unwrap();
                std::fs::write(&bad, b"not a directory").unwrap();
                scheduler.refresh(Instant::now());
                for _ in 0..120 {
                    scheduler.cold();
                }
                let metrics = scheduler.context.shared.state().metrics.report();
                assert!(!scheduler.rounds.is_empty(), "failed enumeration remains owed");
                assert_eq!(metrics["sweeps"], 0);
                assert_eq!(metrics["initial_pass"], false);
                assert_eq!(scheduler.options().cold_duty_factor, COLD_DUTY_FACTOR);
            },
        );
    }

    /// A closing maintenance deferred for queued hot work does not end the
    /// first pass; a later one that runs to completion does.
    #[test]
    fn a_deferred_closing_maintenance_keeps_the_initial_budget() {
        with_native_scheduler(
            "history::probe::live::broad::initial::tests::a_deferred_closing_maintenance_keeps_the_initial_budget",
            false,
            |scheduler, _, paths| {
                let metrics = || scheduler.context.shared.state().metrics.report();
                scheduler.recent.poll(&scheduler.context.store).unwrap();
                scheduler.owed = Some(owed(paths));
                scheduler.refresh(Instant::now());
                drain(scheduler);
                assert_eq!(metrics()["sweeps"], 1);
                assert!(scheduler.audit_at < Instant::now() + Duration::from_secs(3600));
                assert_eq!(metrics()["initial_pass"], true);
                assert_eq!(scheduler.options().cold_duty_factor, INITIAL_DUTY_FACTOR);
                scheduler.owed = None;
                scheduler.audit_at = Instant::now();
                scheduler.lost = true;
                scheduler.refresh(Instant::now());
                drain(scheduler);
                assert_eq!(metrics()["sweeps"], 2);
                assert!(
                    scheduler.audit_at > Instant::now() + Duration::from_secs(3600),
                    "require actual successful maintenance"
                );
                assert_eq!(metrics()["initial_pass"], false);
                assert_eq!(scheduler.options().cold_duty_factor, COLD_DUTY_FACTOR);
            },
        );
    }

    /// Hot work queued at every round end cannot hold the budget forever.
    #[test]
    fn repeatedly_deferred_maintenance_ends_the_initial_pass_after_a_bound() {
        with_native_scheduler(
            "history::probe::live::broad::initial::tests::repeatedly_deferred_maintenance_ends_the_initial_pass_after_a_bound",
            false,
            |scheduler, _, paths| {
                let metrics = || scheduler.context.shared.state().metrics.report();
                scheduler.owed = Some(owed(paths));
                for round in 1..=INITIAL_UNFINISHED_CLOSINGS {
                    assert_eq!(metrics()["initial_pass"], true);
                    // The next round's maintenance is due again (backstop).
                    scheduler.audit_at = Instant::now();
                    scheduler.lost = true;
                    scheduler.refresh(Instant::now());
                    drain(scheduler);
                    assert_eq!(metrics()["sweeps"], u64::from(round));
                }
                assert!(scheduler.owed.is_some());
                assert_eq!(metrics()["initial_pass"], false);
                assert_eq!(scheduler.options().duty_factor, SWEEP_DUTY_FACTOR);
            },
        );
    }

    /// Hot sweeps that wait for the pace are counted under the initial budget
    /// and not after it ends; ticks_paced counts both.
    #[test]
    fn hot_sweeps_count_initial_pacing_only_during_the_initial_pass() {
        with_native_scheduler(
            "history::probe::live::broad::initial::tests::hot_sweeps_count_initial_pacing_only_during_the_initial_pass",
            false,
            |scheduler, _, paths| {
                let metrics = || scheduler.context.shared.state().metrics.report();
                let sweep = |scheduler: &mut Scheduler<'_>, line: &str| {
                    for (path, id) in paths.iter().zip(["first", "second"]) {
                        crate::history::probe::live::tests::append(
                            path,
                            &crate::history::probe::live::tests::transcript_line(id, line),
                        );
                    }
                    // A long last sweep, so this one waits for the pace.
                    scheduler.context.last = Swept {
                        ended: Instant::now(),
                        took: Duration::from_secs(10),
                    };
                    scheduler.owed = Some(owed(paths));
                    scheduler.hot();
                };
                sweep(scheduler, "two");
                assert_eq!(metrics()["ticks_paced"], 1);
                assert_eq!(metrics()["initial_pass_ticks_paced"], 1);
                scheduler.end_initial_pass();
                sweep(scheduler, "three");
                assert_eq!(metrics()["ticks_paced"], 2);
                assert_eq!(metrics()["initial_pass_ticks_paced"], 1);
            },
        );
    }

    #[test]
    fn the_initial_budget_never_slows_a_faster_override() {
        let options = LiveOptions {
            duty_factor: 0,
            cold_duty_factor: 0,
            ..LiveOptions::default()
        };
        let initial = initial_options(&options);
        assert_eq!((initial.duty_factor, initial.cold_duty_factor), (0, 0));
        let initial = initial_options(&LiveOptions::default());
        assert_eq!(
            (initial.duty_factor, initial.cold_duty_factor),
            (INITIAL_DUTY_FACTOR, INITIAL_DUTY_FACTOR)
        );
    }
}
