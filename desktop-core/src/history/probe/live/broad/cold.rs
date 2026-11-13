//! Cold reconciliation slices and the maintenance that ends a round.
use super::*;

impl Scheduler<'_> {
    pub(super) fn cold(&mut self) {
        let Some(mut round) = self.rounds.pop_front() else {
            return;
        };
        let started = Instant::now();
        let before = self.context.store.head_revision().ok();
        let stop = self.context.cancelled.clone();
        let outcome = history::reconcile_step(&mut round, move || stopping(&stop));
        if outcome.as_ref().is_ok_and(|step| step.complete) {
            self.rounds.completed(round);
        } else {
            self.rounds.push_back(round);
        }
        self.hot_since_cold = false;
        let took = started.elapsed();
        self.context.shared.update(|state| {
            state.metrics.cold_slices += 1;
            state.metrics.cold_slice_ms_last = Some(millis(took));
            state.metrics.cold_slice_ms_max = state.metrics.cold_slice_ms_max.max(millis(took));
            if let Ok(step) = &outcome {
                state.metrics.cold_sqlite_busy_wait_ms_last = Some(millis(step.sqlite_busy_wait));
                state.metrics.cold_sqlite_busy_wait_ms_total = state
                    .metrics
                    .cold_sqlite_busy_wait_ms_total
                    .saturating_add(millis(step.sqlite_busy_wait));
                state.metrics.cold_entries += step.entries as u64;
                state.metrics.cold_paths += step.paths as u64;
            }
        });
        if let Some(progress) = &self.context.progress {
            (progress.observer())(ai_hist::CaptureProgress {
                source: "history".into(),
                processed_files: self.context.shared.state().metrics.cold_paths as usize,
                total_files: None,
            });
        }
        match outcome {
            Ok(step) => {
                if let Some(report) = step.report {
                    tick(
                        &Tick {
                            trigger: Trigger::Poll,
                            swept: before
                                .is_none_or(|before| before.revision != report.head_revision),
                            contended: false,
                            changed: report.changed,
                            took,
                            waited: None,
                        },
                        &self.context.store,
                        self.context.shared,
                    );
                }
                self.cold_due = Instant::now()
                    + history::cold_gap(
                        took,
                        step.sqlite_busy_wait,
                        self.options().cold_duty_factor,
                    );
            }
            Err(error) => {
                self.cold_due = Instant::now() + gap(took, self.options());
                // A cooperative user stop retains the last completed cycle;
                // it is not a new capture failure for delivery to publish.
                if stopping(self.context.cancelled)
                    && collector::capture_error_class(&error) == "cancelled"
                {
                    return;
                }
                if error
                    .downcast_ref::<ai_hist::Error>()
                    .is_some_and(|error| matches!(error, ai_hist::Error::SyncLocked { .. }))
                {
                    self.context
                        .shared
                        .update(|state| state.metrics.ticks_contended += 1);
                    return;
                }
                self.context
                    .shared
                    .judge(Some(collector::capture_error_class(&error).into()));
            }
        }
        if self.rounds.is_empty() {
            self.rounds.clear();
            self.partial_pass_finished = false;
            self.finish_round();
        } else if self.rounds.healthy_pass_exhausted() {
            // A partial pass owes retries, not successful maintenance. Keep the
            // failed objects and their existing cold deadline; only the fresh
            // healthy-coverage timer rests, once per pass rather than per retry.
            if !self.partial_pass_finished {
                self.partial_pass_finished = true;
                self.backstop = Instant::now() + self.coverage_interval();
                // A partial pass runs no maintenance: its healthy coverage is
                // complete, and objects that keep failing must not hold the
                // initial budget for the rest of the run.
                self.end_initial_pass();
            }
            if std::mem::take(&mut self.rescan_again) {
                self.lost = true;
                self.backstop = Instant::now();
            }
        }
    }

    pub(super) fn finish_round(&mut self) {
        let started = Instant::now();
        self.backstop = started + self.coverage_interval();
        // Full-sweep destination-loss repair and parser generation seals are
        // maintenance, once after startup reconciliation then at most daily.
        // They retain the existing provider/atomic-file boundary; this is not
        // a claim of a hard wall-clock budget for one provider snapshot.
        let due = Instant::now() >= self.audit_at;
        // Capture the epoch BEFORE draining: a callback in the gap before
        // the stop guard starts must remain new work, not become its baseline.
        let activity_before_drain = self
            .context
            .watch
            .watch
            .as_ref()
            .ok()
            .map(|watch| watch.activity().load(Ordering::Relaxed));
        for _ in 0..128 {
            let Ok(noticed) = self.context.received.try_recv() else {
                break;
            };
            self.context
                .watch
                .fold(noticed, &mut self.owed, &mut self.lost);
        }
        let queued_hot = self.owed.is_some();
        let audit = if due && queued_hot {
            super::super::maintenance::Audit {
                result: Ok(None),
                interrupted: true,
            }
        } else if due {
            super::super::maintenance::run(
                &self.context.store,
                self.context.cancelled,
                &self.context.watch,
                activity_before_drain,
                &self.recent,
                &self.roots,
            )
        } else {
            super::super::maintenance::Audit {
                result: Ok(Some(ai_hist::SyncReport::default())),
                interrupted: false,
            }
        };
        if audit.interrupted {
            // A consent interruption can precede the ordinary 1s refresh. Do
            // not let that cache authorize another pass with the old roots.
            self.roots.invalidate();
            // Per-file checkpoints survive cancellation. Keep maintenance due,
            // then service the event or fallback poll before retrying it.
            self.audit_at = Instant::now() + self.context.options.min_gap;
        }
        let maintained = due && !audit.interrupted && matches!(audit.result, Ok(Some(_)));
        match audit.result {
            Ok(Some(_)) => {
                if due {
                    self.audit_at = Instant::now() + Duration::from_secs(24 * 60 * 60);
                    self.safety = Default::default();
                }
                self.context.shared.judge(None);
                if let Some(progress) = self.context.progress.take() {
                    progress.finish(true);
                }
            }
            Ok(None) => self.audit_at = Instant::now() + self.context.options.min_gap,
            Err(_) if audit.interrupted => {}
            Err(error) => self
                .context
                .shared
                .judge(Some(collector::capture_error_class(&error).into())),
        }
        // An expensive audit contributes to cold duty pacing, never hot pacing.
        self.cold_due = self.cold_due.max(
            Instant::now()
                + started
                    .elapsed()
                    .saturating_mul(self.options().cold_duty_factor),
        );
        self.close_initial_pass(due, maintained);
        self.context.shared.update(|state| {
            state.metrics.sweeps += 1;
            state.metrics.sweep_ms_last = Some(millis(started.elapsed()));
            state.metrics.sweep_ms_max = state.metrics.sweep_ms_max.max(millis(started.elapsed()));
            state.metrics.last_capture_ms = Some(collector::now());
        });
        if std::mem::take(&mut self.rescan_again) {
            self.lost = true;
            self.backstop = Instant::now();
        }
    }
}
