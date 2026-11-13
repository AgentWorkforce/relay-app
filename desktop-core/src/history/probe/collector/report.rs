//! The advisory cycle report: the verdict on each side of a pass, what the
//! pass moved, and a busy pass's rescheduling, written to `cycle.json` and the
//! collector log.

use super::*;

/// The last verdict on each side of a pass: the stable class of that outcome,
/// `None` where it succeeded.
#[derive(Default, Clone)]
pub(super) struct Verdicts {
    pub(super) capture: Option<String>,
    pub(super) delivery: Option<String>,
    /// The collector run that observed them, whose `startup_id` the report
    /// carries: `status` shows only the live run's report.
    pub(super) run: Option<String>,
}
impl Verdicts {
    #[cfg(test)]
    pub(super) fn capture(result: &Result<()>) -> Self {
        Self {
            capture: verdict(result),
            delivery: None,
            run: None,
        }
    }
    /// The one class the desktop acts on: a capture fault first, because it
    /// names a local condition, then a delivery fault.
    pub(super) fn effective(&self) -> Option<&str> {
        self.capture.as_deref().or(self.delivery.as_deref())
    }
}

/// Classify one side's result into its public label.
pub(super) fn verdict(result: &Result<()>) -> Option<String> {
    result
        .as_ref()
        .err()
        .map(|error| capture_error_class(error).to_owned())
}

/// Build the advisory cycle status from typed classes without including
/// provider data. `capture` and `delivery` carry each side; the top-level
/// fields are the effective verdict, which is what the desktop shows.
pub(super) fn cycle_report(verdicts: &Verdicts) -> serde_json::Value {
    let (capture, delivery) = (verdicts.capture.as_deref(), verdicts.delivery.as_deref());
    let side = |class: Option<&str>, is_capture| json!({"ok": class.is_none(), "error_class": class, "message": class_message(class, is_capture)});
    let effective = verdicts.effective();
    json!({
        "at_ms": now(), "startup_id": verdicts.run, "ok": effective.is_none(),
        "error_class": effective,
        "message": class_message(effective, capture.is_some()),
        "capture": side(capture, true),
        "delivery": side(delivery, false),
    })
}

/// The capture verdict the last report of run `run` holds. Another run's
/// verdict describes a capture this run has not made, and a report that
/// cannot be read carries none: the next capture pass re-establishes it.
pub(super) fn saved_capture_verdict(directory: &Path, run: &str) -> Option<String> {
    fs::read(directory.join("cycle.json"))
        .ok()
        .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
        .filter(|report| report["startup_id"] == run)
        .and_then(|report| report["capture"]["error_class"].as_str().map(str::to_owned))
}

/// What the drains of one pass moved, and the queue they left: content-free
/// counts, bytes and durations for the pass report. Drains record into the
/// thread that ran them, which is the thread that reports the pass, and the
/// report takes what they recorded.
#[derive(Default)]
pub(super) struct PassMetrics {
    pub(super) drains: u64,
    pub(super) drain: worker::DrainMetrics,
    /// The queue after the pass's last drain.
    pub(super) pending_records: i64,
    pub(super) unqueued_changes: i64,
    pub(super) oldest_pending_age_ms: Option<i64>,
    /// Whether that queue still holds work a pass can move now.
    pub(super) backlog: bool,
}
thread_local! {
    pub(super) static PASS_METRICS: std::cell::RefCell<Option<PassMetrics>> = const { std::cell::RefCell::new(None) };
}
impl PassMetrics {
    pub(super) fn record(drain: &worker::DrainMetrics, status: &delivery::DeliveryStatus) {
        PASS_METRICS.with(|pass| {
            let mut pass = pass.borrow_mut();
            let pass = pass.get_or_insert_with(PassMetrics::default);
            pass.drains += 1;
            pass.drain.add(drain);
            pass.pending_records = status.pending_records;
            pass.unqueued_changes = status.unqueued_changes;
            pass.oldest_pending_age_ms = status
                .oldest_pending_ms
                .map(|oldest| now().saturating_sub(oldest).max(0));
            pass.backlog = has_backlog(status);
        });
    }
    /// What this thread's drains recorded since the last take, if any ran.
    pub(super) fn take() -> Option<PassMetrics> {
        PASS_METRICS.with(|pass| pass.borrow_mut().take())
    }
    pub(super) fn report(&self) -> serde_json::Value {
        let drain = &self.drain;
        let per_second = |count: u64| {
            (drain.elapsed_ms > 0)
                .then(|| ((count as f64) * 1_000.0 / drain.elapsed_ms as f64 * 10.0).round() / 10.0)
        };
        json!({
            "drains": self.drains,
            "drain_ms": drain.elapsed_ms,
            "batches_sent": drain.batches_sent,
            "bytes_sent": drain.bytes_sent,
            "batches_acknowledged": drain.batches_acknowledged,
            "records_acknowledged": drain.records_acknowledged,
            "batches_per_second": per_second(drain.batches_acknowledged),
            "records_per_second": per_second(drain.records_acknowledged),
            "send_ms_total": drain.send_ms_total,
            "send_ms_mean": (drain.batches_sent > 0).then(|| drain.send_ms_total / drain.batches_sent),
            "send_ms_max": drain.send_ms_max,
            "queue_ms_max": drain.queue_ms_max,
            "event_lag_ms": drain.event_lag_ms,
            "in_flight_limit": DELIVERY_CONCURRENCY,
            "max_in_flight": drain.max_in_flight,
            "batches_retried": drain.batches_retried,
            "pending_records": self.pending_records,
            "unqueued_changes": self.unqueued_changes,
            "oldest_pending_age_ms": self.oldest_pending_age_ms,
        })
    }
}

/// Persist advisory status without interrupting retries when either status or logs cannot be written.
/// A pass that ran a drain reports what it moved as `delivery_metrics`.
pub(super) fn write_cycle_report(
    directory: &Path,
    verdicts: &Verdicts,
    metrics: Option<&PassMetrics>,
    diagnostics: impl Write,
) {
    write_report(directory, verdicts, metrics, None, None, diagnostics)
}
/// A delivery pass that ended because local history was busy: how many
/// passes in a row did, since when, and when the loop tries again. The report
/// carries it so status shows a rescheduled pass rather than a bare failure,
/// until the run of busy passes has lasted long enough to need attention.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BusyDeferral {
    pub(super) passes: u64,
    pub(super) since_ms: i64,
    pub(super) next_attempt_ms: i64,
}
impl BusyDeferral {
    /// The deferral after a pass that ended with `delivery`, following
    /// `previous` (the last pass's deferral, if it was busy too), at `now_ms`
    /// with the loop's next attempt due at `next_attempt_ms`. Any outcome but
    /// a busy one ends the run.
    pub(super) fn after(
        delivery: Option<&str>,
        previous: Option<Self>,
        now_ms: i64,
        next_attempt_ms: i64,
    ) -> Option<Self> {
        (delivery == Some("database_busy")).then(|| BusyDeferral {
            passes: previous.map_or(0, |previous| previous.passes) + 1,
            since_ms: previous.map_or(now_ms, |previous| previous.since_ms),
            next_attempt_ms,
        })
    }
}
/// [`write_cycle_report`], with the capture thread's `capture_metrics` when
/// it runs beside the delivery loop.
pub(super) fn write_report(
    directory: &Path,
    verdicts: &Verdicts,
    metrics: Option<&PassMetrics>,
    capture: Option<serde_json::Value>,
    deferral: Option<BusyDeferral>,
    mut diagnostics: impl Write,
) {
    let mut report = cycle_report(verdicts);
    if let Some(deferral) = deferral {
        report["delivery"]["busy_passes"] = json!(deferral.passes);
        report["delivery"]["busy_since_ms"] = json!(deferral.since_ms);
        report["delivery"]["next_attempt_ms"] = json!(deferral.next_attempt_ms);
    }
    legacy_schedules::report(&mut report);
    if let Some(metrics) = metrics {
        report["delivery_metrics"] = metrics.report();
    }
    if let Some(capture) = capture {
        report["capture_metrics"] = capture;
    }
    if let Err(error) = save_status_json(&directory.join("cycle.json"), &report) {
        let message = local_failure_message(&error)
            .unwrap_or("Sync status could not be saved. Retrying; local data remains queued.");
        // collector.log may be on the same full disk. Logging must not panic
        // or propagate another I/O error out of the retry loop either.
        let _ = writeln!(diagnostics, "{message}");
    }
    if let Some(message) = report["message"].as_str() {
        match deferral {
            // One line per busy pass, naming when delivery tries again.
            Some(deferral) => {
                let seconds = (deferral.next_attempt_ms - now())
                    .max(0)
                    .saturating_add(999)
                    / 1000;
                let _ = writeln!(
                    diagnostics,
                    "{message} Next delivery attempt in about {seconds} s (busy passes in a row: {}).",
                    deferral.passes
                );
            }
            None => {
                let _ = writeln!(diagnostics, "{message}");
            }
        }
    }
}

/// Report one pass. A pass that only delivered observed nothing about
/// capture, so the last capture verdict stands.
pub(super) fn persist_cycle_report(
    directory: &Path,
    run: &str,
    outcome: &PassOutcome,
    diagnostics: impl Write,
) {
    persist_pass_report(
        directory,
        run,
        outcome,
        PassMetrics::take().as_ref(),
        diagnostics,
    )
}
/// [`persist_cycle_report`] with the metrics the pass's drains recorded,
/// already taken.
pub(super) fn persist_pass_report(
    directory: &Path,
    run: &str,
    outcome: &PassOutcome,
    metrics: Option<&PassMetrics>,
    diagnostics: impl Write,
) {
    let verdicts = Verdicts {
        capture: match &outcome.capture {
            Some(result) => verdict(result),
            None => saved_capture_verdict(directory, run),
        },
        delivery: verdict(&outcome.delivery),
        run: Some(run.to_owned()),
    };
    write_cycle_report(directory, &verdicts, metrics, diagnostics);
}
/// Report one delivery pass of the background loop. Capture runs on its own
/// thread: its last verdict and counts are the report's capture side, and
/// before it has finished a capture the run's last reported verdict stands.
pub(super) fn persist_live_report(
    directory: &Path,
    run: &str,
    live: &super::super::live::Shared,
    delivery: &Result<()>,
    metrics: Option<&PassMetrics>,
    deferral: Option<BusyDeferral>,
    diagnostics: impl Write,
) {
    let verdicts = Verdicts {
        capture: live
            .verdict()
            .unwrap_or_else(|| saved_capture_verdict(directory, run)),
        delivery: verdict(delivery),
        run: Some(run.to_owned()),
    };
    write_report(
        directory,
        &verdicts,
        metrics,
        live.report(),
        deferral,
        diagnostics,
    );
}
