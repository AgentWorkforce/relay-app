//! Local, content-free evidence for work that follows the public cycle report.
//! At most one aggregate per minute, plus a nonempty stop flush, uses stderr.
//! Unix rotates the collector log while running. Other platforms omit summaries
//! when the current log has no room for the whole line; rotation waits for restart.
use crate::history::delivery::CensusRefresh;
use anyhow::Result;
use serde_json::{json, Value};
use std::{
    io::Write,
    path::Path,
    time::{Duration, Instant},
};

const REPORT_INTERVAL: Duration = Duration::from_secs(60);

#[cfg(unix)]
use process_cpu_unix as process_cpu;
#[cfg(windows)]
use process_cpu_windows as process_cpu;

pub(super) struct Phases {
    pub pass: Duration,
    pub delivery: Duration,
    pub compaction: Option<Duration>,
    pub retention: Duration,
    pub census: Duration,
}

#[derive(Default)]
struct Counts {
    passes: u64,
    pass: Duration,
    delivery: Duration,
    compactions: u64,
    compaction: Duration,
    retention: Duration,
    census: Duration,
    current: u64,
    followed: u64,
    taken: u64,
    errors: u64,
    skipped: u64,
    changes: u64,
    judged: u64,
    sessions: u64,
}

pub(super) struct Window {
    started: Instant,
    cpu: Option<Duration>,
    counts: Counts,
}

impl Window {
    pub fn start() -> Self {
        Self::new(Instant::now(), process_cpu())
    }

    #[cfg(test)]
    pub(super) fn start_due() -> Self {
        Self::new(Instant::now() - REPORT_INTERVAL, process_cpu())
    }

    fn new(started: Instant, cpu: Option<Duration>) -> Self {
        Self {
            started,
            cpu,
            counts: Counts::default(),
        }
    }

    pub fn record(&mut self, phases: Phases, census: Option<&Result<CensusRefresh>>) {
        let counts = &mut self.counts;
        counts.passes = counts.passes.saturating_add(1);
        counts.pass = counts.pass.saturating_add(phases.pass);
        counts.delivery = counts.delivery.saturating_add(phases.delivery);
        counts.retention = counts.retention.saturating_add(phases.retention);
        counts.census = counts.census.saturating_add(phases.census);
        if let Some(compaction) = phases.compaction {
            counts.compactions = counts.compactions.saturating_add(1);
            counts.compaction = counts.compaction.saturating_add(compaction);
        }
        match census {
            Some(Ok(CensusRefresh::Current)) => counts.current = counts.current.saturating_add(1),
            Some(Ok(CensusRefresh::Followed { changes, judged })) => {
                counts.followed = counts.followed.saturating_add(1);
                counts.changes = counts.changes.saturating_add(*changes as u64);
                counts.judged = counts.judged.saturating_add(*judged as u64);
            }
            Some(Ok(CensusRefresh::Taken { sessions })) => {
                counts.taken = counts.taken.saturating_add(1);
                counts.sessions = counts.sessions.saturating_add(*sessions as u64);
            }
            Some(Err(_)) => counts.errors = counts.errors.saturating_add(1),
            None => counts.skipped = counts.skipped.saturating_add(1),
        }
    }

    pub fn report_if_due(&mut self, directory: &Path, output: impl Write) {
        self.report_if_due_at(directory, Instant::now(), output);
    }

    fn report_if_due_at(&mut self, directory: &Path, now: Instant, output: impl Write) {
        if now.saturating_duration_since(self.started) >= REPORT_INTERVAL {
            self.write(directory, now, process_cpu(), cfg!(unix), output);
        }
    }

    /// Preserve a short run's evidence once on a clean stop, including its wait.
    pub fn finish(&mut self, directory: &Path, output: impl Write) {
        self.write(directory, Instant::now(), process_cpu(), cfg!(unix), output);
    }

    fn write(
        &mut self,
        directory: &Path,
        now: Instant,
        cpu: Option<Duration>,
        rotates: bool,
        mut output: impl Write,
    ) {
        if let Some(report) = self.take(now, cpu) {
            let line = format!("{report}\n");
            // Diagnostic failures must never interrupt collection or retries.
            if report_fits(directory, line.len(), rotates) {
                let _ = output.write_all(line.as_bytes());
            }
        }
    }

    fn take(&mut self, now: Instant, cpu: Option<Duration>) -> Option<Value> {
        if self.counts.passes == 0 {
            return None;
        }
        let elapsed = now.saturating_duration_since(self.started);
        let cpu_delta = self
            .cpu
            .zip(cpu)
            .and_then(|(before, after)| after.checked_sub(before));
        let cpu_percent = cpu_delta
            .filter(|_| !elapsed.is_zero())
            .map(|cpu| (cpu.as_secs_f64() / elapsed.as_secs_f64() * 1_000.0).round() / 10.0);
        let c = std::mem::take(&mut self.counts);
        self.started = now;
        self.cpu = cpu;
        Some(json!({
            "operation": "collector_performance", "schema_version": 1, "at_ms": super::now(),
            "version": agent_relay_node::VERSION, "role": "sessions",
            "window_elapsed_ms": elapsed.as_millis(),
            "process_cpu_ms": cpu_delta.map(|cpu| cpu.as_millis()),
            "process_cpu_percent_one_core": cpu_percent,
            "cpu_scope": "collector_process_all_threads",
            "passes": c.passes,
            "pass_active_elapsed_ms": c.pass.as_millis(),
            "delivery_elapsed_ms": c.delivery.as_millis(),
            "compaction_calls": c.compactions,
            "compaction_elapsed_ms": c.compaction.as_millis(),
            "retention_calls": c.passes,
            "retention_elapsed_ms": c.retention.as_millis(),
            "census_elapsed_ms": c.census.as_millis(),
            "census": {
                "current": c.current, "followed": c.followed, "taken": c.taken,
                "errors": c.errors, "skipped": c.skipped,
                "changes": c.changes, "sessions_judged": c.judged,
                "sessions_taken": c.sessions,
            },
        }))
    }
}

/// Non-Unix collectors rotate only at startup. New summaries must fit without
/// pushing that log past its cap, including a stop flush. Other writers retain
/// their existing behavior; this check only bounds these additional summaries.
fn report_fits(directory: &Path, bytes: usize, rotates: bool) -> bool {
    if rotates {
        return true;
    }
    let length = match std::fs::metadata(directory.join(super::LOG)) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(_) => return false,
    };
    length
        .checked_add(bytes as u64)
        .is_some_and(|length| length <= super::LOG_CAP)
}

/// Cumulative CPU used by this process's threads, excluding child processes.
#[cfg(unix)]
fn process_cpu_unix() -> Option<Duration> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes usage on success; no pointer escapes.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: the successful call above initialized the value.
    let usage = unsafe { usage.assume_init() };
    let time = |value: libc::timeval| {
        Some(
            Duration::from_secs(u64::try_from(value.tv_sec).ok()?)
                + Duration::from_micros(u64::try_from(value.tv_usec).ok()?),
        )
    };
    time(usage.ru_utime)?.checked_add(time(usage.ru_stime)?)
}

#[cfg(windows)]
fn process_cpu_windows() -> Option<Duration> {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::Threading::{GetCurrentProcess, GetProcessTimes},
    };
    let mut created = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exited = created;
    let mut kernel = created;
    let mut user = created;
    // SAFETY: GetCurrentProcess returns a valid pseudo-handle and all outputs
    // point to live FILETIME values. The pseudo-handle must not be closed.
    if unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    } == 0
    {
        return None;
    }
    let ticks =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    let duration = |time| {
        let ticks = ticks(time);
        Duration::new(ticks / 10_000_000, ((ticks % 10_000_000) * 100) as u32)
    };
    duration(kernel).checked_add(duration(user))
}

#[cfg(not(any(unix, windows)))]
fn process_cpu() -> Option<Duration> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phases() -> Phases {
        Phases {
            pass: Duration::from_millis(11),
            delivery: Duration::from_millis(5),
            compaction: Some(Duration::from_millis(1)),
            retention: Duration::from_millis(2),
            census: Duration::from_millis(3),
        }
    }

    #[test]
    fn summary_counts_outcomes_and_resets_without_recording_error_contents() {
        let started = Instant::now();
        let mut window = Window::new(started, Some(Duration::from_secs(1)));
        for result in [
            Ok(CensusRefresh::Current),
            Ok(CensusRefresh::Followed {
                changes: 7,
                judged: 2,
            }),
            Ok(CensusRefresh::Taken { sessions: 19 }),
            Err(anyhow::anyhow!("private-session /secret/path payload")),
        ] {
            window.record(phases(), Some(&result));
        }
        window.record(phases(), None);
        let end = started + REPORT_INTERVAL;
        let report = window.take(end, Some(Duration::from_secs(16))).unwrap();
        assert_eq!(report["process_cpu_ms"], 15_000);
        assert_eq!(report["process_cpu_percent_one_core"], 25.0);
        assert_eq!(report["window_elapsed_ms"], 60_000);
        assert_eq!(report["passes"], 5);
        assert_eq!(report["retention_elapsed_ms"], 10);
        assert_eq!(report["census_elapsed_ms"], 15);
        assert_eq!(
            report["census"],
            json!({"current":1,"followed":1,"taken":1,"errors":1,"skipped":1,"changes":7,"sessions_judged":2,"sessions_taken":19})
        );
        assert!(!report.to_string().contains("secret"));
        assert!(!report.to_string().contains("payload"));
        assert!(window.take(end, Some(Duration::from_secs(16))).is_none());
        window.record(phases(), Some(&Ok(CensusRefresh::Current)));
        let next = window
            .take(end + REPORT_INTERVAL, Some(Duration::from_secs(17)))
            .unwrap();
        assert_eq!(next["passes"], 1);
        assert_eq!(next["process_cpu_ms"], 1_000);
        assert_eq!(next["census"]["changes"], 0);
    }

    #[test]
    fn missing_or_regressing_cpu_samples_are_unavailable_then_recover() {
        let started = Instant::now();
        let mut window = Window::new(started, Some(Duration::from_secs(5)));
        for (i, cpu) in [None, Some(2), Some(1), Some(3)].into_iter().enumerate() {
            window.record(phases(), None);
            let report = window
                .take(
                    started + REPORT_INTERVAL * (i as u32 + 1),
                    cpu.map(Duration::from_secs),
                )
                .unwrap();
            if i < 3 {
                assert!(report["process_cpu_ms"].is_null());
                assert!(report["process_cpu_percent_one_core"].is_null());
            } else {
                assert_eq!(report["process_cpu_ms"], 2_000);
            }
        }
    }

    #[test]
    fn fast_passes_emit_one_bounded_line_per_window_and_one_stop_flush() {
        let directory = tempfile::tempdir().unwrap();
        let started = Instant::now() - REPORT_INTERVAL;
        let mut window = Window::new(started, process_cpu());
        let mut output = Vec::new();
        for tick in 0..10_000 {
            window.record(phases(), Some(&Ok(CensusRefresh::Current)));
            window.report_if_due_at(
                directory.path(),
                started + Duration::from_micros(tick),
                &mut output,
            );
        }
        assert!(output.is_empty());
        window.report_if_due_at(directory.path(), started + REPORT_INTERVAL, &mut output);
        assert_eq!(output.iter().filter(|byte| **byte == b'\n').count(), 1);
        assert!(output.len() < 2_048);
        let report: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(report["passes"], 10_000);
        window.finish(directory.path(), &mut output);
        assert_eq!(output.iter().filter(|byte| **byte == b'\n').count(), 1);
        window.record(phases(), None);
        window.finish(directory.path(), &mut output);
        assert_eq!(output.iter().filter(|byte| **byte == b'\n').count(), 2);
        window.finish(directory.path(), &mut output);
        assert_eq!(output.iter().filter(|byte| **byte == b'\n').count(), 2);
    }

    #[test]
    fn nonrotating_log_leaves_room_for_whole_periodic_and_stop_lines() {
        let directory = tempfile::tempdir().unwrap();
        assert!(report_fits(
            directory.path(),
            super::super::LOG_CAP as usize,
            false
        ));
        assert!(!report_fits(
            directory.path(),
            super::super::LOG_CAP as usize + 1,
            false
        ));
        let log = std::fs::File::create(directory.path().join(super::super::LOG)).unwrap();
        log.set_len(super::super::LOG_CAP - 1).unwrap();
        assert!(report_fits(directory.path(), 1, false));
        assert!(!report_fits(directory.path(), 2, false));
        assert!(report_fits(directory.path(), 2, true));
        let mut window = Window::start_due();
        let mut output = Vec::new();
        for _ in 0..2 {
            window.record(phases(), None);
            // Periodic reporting and the stop flush use this same write gate.
            window.write(
                directory.path(),
                Instant::now(),
                process_cpu(),
                false,
                &mut output,
            );
        }
        assert!(output.is_empty());
        assert_eq!(window.counts.passes, 0);
        log.set_len(super::super::LOG_CAP).unwrap();
        assert!(!report_fits(directory.path(), 1, false));
        assert!(!report_fits(&directory.path().join("\0"), 1, false));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn native_process_cpu_is_available_and_monotonic() {
        let before = process_cpu().expect("process CPU sample");
        let after = process_cpu().expect("process CPU sample");
        assert!(after >= before);
    }

    /// Synthetic reporting overhead, with no history reads, uploads or waits.
    #[test]
    #[ignore = "opt-in telemetry overhead experiment"]
    fn telemetry_overhead_benchmark() {
        const PASSES: u64 = 10_000;
        let directory = tempfile::tempdir().unwrap();
        for sample in 0..5 {
            let started = Instant::now();
            for _ in 0..PASSES {
                let _ = std::hint::black_box(Ok::<_, anyhow::Error>(CensusRefresh::Current));
            }
            let disabled = started.elapsed();
            let mut window = Window::start();
            let started = Instant::now();
            for _ in 0..PASSES {
                // Include the delivery loop's phase clock reads; no actual
                // delivery or database operation belongs in this experiment.
                let pass_started = Instant::now();
                let delivery = pass_started.elapsed();
                let retention_started = Instant::now();
                let retention = retention_started.elapsed();
                let census_started = Instant::now();
                let census = std::hint::black_box(Ok(CensusRefresh::Current));
                let phases = Phases {
                    pass: pass_started.elapsed(),
                    delivery,
                    compaction: None,
                    retention,
                    census: census_started.elapsed(),
                };
                window.record(phases, Some(&census));
                window.report_if_due(directory.path(), std::io::sink());
            }
            window.finish(directory.path(), std::io::sink());
            let enabled = started.elapsed();
            eprintln!(
                "{}",
                json!({
                    "experiment": "collector_telemetry_overhead", "sample": sample, "passes": PASSES,
                    "disabled_wall_us": disabled.as_micros(), "enabled_wall_us": enabled.as_micros(),
                    "added_wall_ns_per_pass": enabled.saturating_sub(disabled).as_nanos() / u128::from(PASSES),
                })
            );
        }
    }
}
