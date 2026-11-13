use super::{super::live, *};

/// The delivery loop: one bounded pass at a time, woken early by the capture
/// thread when local history gained something to deliver. A pass that moved
/// a backlog is followed at once; otherwise the next waits for the capture
/// thread's wake or the pass interval, whichever comes first.
pub(super) fn deliver_until_stopped(
    directory: &Path,
    startup_id: &str,
    config: &Config,
    cancelled: &Arc<AtomicBool>,
    live: &live::Shared,
) {
    let mut session: Option<worker::DrainSession> = None;
    let mut performance = performance::Window::start();
    let mut deferral: Option<BusyDeferral> = None;
    while !stopping(cancelled) {
        let pass_started = Instant::now();
        // Whatever the capture thread wrote before this pass, the pass reads.
        live.clear();
        let delivery = (|| {
            if session.is_none() {
                session = Some(worker::DrainSession::open(&directory.join("history.db"))?);
            }
            deliver_captured_on(
                directory,
                config,
                false,
                cancelled,
                session.as_mut().context("delivery session unavailable")?,
            )
        })();
        let delivery_elapsed = pass_started.elapsed();
        // A user stop is not a failed/offline cycle and must not start delivery.
        if stopping(cancelled) {
            break;
        }
        let metrics = PassMetrics::take();
        // The pass's last drain already read the queue it left; only a pass
        // that ran none reads it again.
        let backlog = metrics
            .as_ref()
            .map_or_else(|| backlog_pending(directory, config), |m| m.backlog);
        let moved = delivery.is_ok()
            && metrics
                .as_ref()
                .is_some_and(|metrics| metrics.drain.batches_acknowledged > 0);
        let interval = pass_interval(backlog, moved);
        // Between passes, never beside one: the rewrite holds the store's
        // write lock for its whole length. Retention then publishes how far
        // uploads were acknowledged and evicts expired history no upload
        // still needs.
        let compaction = if live.take_compact_due() {
            let started = Instant::now();
            live::compact_store(directory, &directory.join("history.db"));
            Some(started.elapsed())
        } else {
            None
        };
        let retention_started = Instant::now();
        crate::history::retention::maintain(directory, cancelled);
        let retention_elapsed = retention_started.elapsed();
        // The session census follows what this pass captured, so status
        // counts sessions without walking every identity the store holds.
        let census_started = Instant::now();
        let census = session.as_mut().map(|session| {
            session
                .connection()
                .and_then(delivery::refresh_session_census)
        });
        let census_elapsed = census_started.elapsed();
        report_pass(
            &mut performance,
            directory,
            performance::Phases {
                pass: pass_started.elapsed(),
                delivery: delivery_elapsed,
                compaction,
                retention: retention_elapsed,
                census: census_elapsed,
            },
            census.as_ref(),
            std::io::stderr(),
        );
        // The next attempt is due one interval from here, after the pass's
        // maintenance: the report names the same deadline the loop waits on.
        let deadline = Instant::now() + interval;
        deferral = busy_step(verdict(&delivery).as_deref(), deferral, now(), interval);
        // A pass that found local history busy is rescheduled, not failed:
        // the report says when the next attempt is due.
        persist_live_report(
            directory,
            startup_id,
            live,
            &delivery,
            metrics.as_ref(),
            deferral,
            std::io::stderr(),
        );
        live.wait(deadline, || stopping(cancelled));
    }
    performance.finish(directory, std::io::stderr());
}

/// One pass's step through a run of busy passes: the deferral that follows
/// `previous` after a pass that ended with `delivery` at `now_ms`, with the
/// next attempt due `interval` later. `None` when the pass was not busy.
fn busy_step(
    delivery: Option<&str>,
    previous: Option<BusyDeferral>,
    now_ms: i64,
    interval: Duration,
) -> Option<BusyDeferral> {
    let next_attempt_ms = now_ms.saturating_add(interval.as_millis().min(i64::MAX as u128) as i64);
    BusyDeferral::after(delivery, previous, now_ms, next_attempt_ms)
}

fn report_pass(
    performance: &mut performance::Window,
    directory: &Path,
    phases: performance::Phases,
    census: Option<&Result<delivery::CensusRefresh>>,
    mut output: impl Write,
) {
    if census.is_some_and(|result| result.is_err()) {
        let _ = writeln!(
            output,
            "Session counts could not be updated. The next pass updates them."
        );
    }
    performance.record(phases, census);
    performance.report_if_due(directory, output);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Busy passes count up in a row from the first one, and any other
    /// outcome ends the run: busy, busy, ok, busy is 1, 2, 0, 1.
    #[test]
    fn busy_passes_count_a_run_and_reset_after_any_other_outcome() {
        let interval = Duration::from_secs(2);
        let mut deferral = None;
        let mut seen = Vec::new();
        for (at, outcome) in [
            (1_000, Some("database_busy")),
            (4_000, Some("database_busy")),
            (7_000, None),
            (10_000, Some("database_busy")),
        ] {
            deferral = busy_step(outcome, deferral, at, interval);
            seen.push(deferral.map(|d| (d.passes, d.since_ms, d.next_attempt_ms)));
        }
        assert_eq!(
            seen,
            [
                Some((1, 1_000, 3_000)),
                Some((2, 1_000, 6_000)),
                None,
                Some((1, 10_000, 12_000)),
            ]
        );
        // A failure other than busy also ends the run.
        let busy = busy_step(Some("database_busy"), None, 0, interval);
        assert_eq!(busy_step(Some("network"), busy, 1, interval), None);
    }

    #[test]
    fn census_failure_diagnostic_does_not_wait_for_a_performance_window() {
        let directory = tempfile::tempdir().unwrap();
        let mut performance = performance::Window::start();
        let mut output = Vec::new();
        report_pass(
            &mut performance,
            directory.path(),
            performance::Phases {
                pass: Duration::ZERO,
                delivery: Duration::ZERO,
                compaction: None,
                retention: Duration::ZERO,
                census: Duration::ZERO,
            },
            Some(&Err(anyhow::anyhow!("private error details"))),
            &mut output,
        );
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Session counts could not be updated. The next pass updates them.\n"
        );
    }

    #[test]
    fn collector_pass_reports_census_failure_immediately_and_one_safe_summary() {
        let directory = tempfile::tempdir().unwrap();
        let mut performance = performance::Window::start_due();
        let mut output = Vec::new();
        report_pass(
            &mut performance,
            directory.path(),
            performance::Phases {
                pass: Duration::from_millis(11),
                delivery: Duration::from_millis(5),
                compaction: None,
                retention: Duration::from_millis(2),
                census: Duration::from_millis(3),
            },
            Some(&Err(anyhow::anyhow!(
                "secret-session /private/path payload"
            ))),
            &mut output,
        );
        let text = String::from_utf8(output).unwrap();
        let mut lines = text.lines();
        assert_eq!(
            lines.next(),
            Some("Session counts could not be updated. The next pass updates them.")
        );
        let summary: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(summary["operation"], "collector_performance");
        assert_eq!(summary["passes"], 1);
        assert_eq!(summary["census"]["errors"], 1);
        assert_eq!(summary["retention_elapsed_ms"], 2);
        assert_eq!(summary["census_elapsed_ms"], 3);
        assert!(lines.next().is_none());
        for private in ["secret-session", "/private/path", "payload"] {
            assert!(!text.contains(private));
        }
    }
}
