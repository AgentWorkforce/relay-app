//! Follow-up sweeps after a capture, until one has nothing left to walk.
//!
//! One sweep is bounded: Codex's SQLite threads are read 50 at a time behind
//! a cursor the next sweep continues, and a thread updated meanwhile sorts
//! above that cursor, so only the traversal after reads it. A sweep that left
//! such a page does not seal the source fingerprint, so the next one walks
//! again. `SyncReport` has no explicit continuation field, so the first
//! sweep that reports `swept == false` is the most this can show.
use super::*;

/// What [`sync_until_settled`] can say about capture continuation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureContinuation {
    /// Capture stopped before a sweep reported nothing to walk: the budget or
    /// the sweep limit ran out, a sweep failed or found the store locked, or
    /// a stop came. A sweep may have left a page for the next one.
    Incomplete,
    /// A sweep found its source fingerprint unchanged since a sealed sweep.
    /// The SDK seals it only after a sweep that read every source, but no
    /// explicit continuation field confirms it.
    Unknown,
}
impl CaptureContinuation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Incomplete => "incomplete",
            Self::Unknown => "unknown",
        }
    }
}

/// Sweep `db` until one reports unswept, at most `max_sweeps` times and only
/// before `budget_end`. One stop watcher follows `budget_end` for every
/// sweep, and each waits for the sync lock no longer than the budget left.
/// Nothing here is an error: what earlier sweeps stored stays for delivery.
pub fn sync_until_settled(
    db: &Path,
    budget_end: Instant,
    max_sweeps: usize,
) -> CaptureContinuation {
    let Ok(store) = open(db) else {
        return CaptureContinuation::Incomplete;
    };
    let stop = Stop::follow(move || Instant::now() >= budget_end);
    settle(&store, &stop, max_sweeps, || {
        budget_end.saturating_duration_since(Instant::now())
    })
}

fn settle(
    store: &SessionStore,
    stop: &Stop,
    max_sweeps: usize,
    budget_left: impl Fn() -> Duration,
) -> CaptureContinuation {
    for _ in 0..max_sweeps {
        if stop.stopped() {
            break;
        }
        let left = budget_left();
        if left.is_zero() {
            break;
        }
        let mut options = SyncOptions::default();
        options.lock_timeout_ms = u64::try_from(left.as_millis()).unwrap_or(u64::MAX);
        options.stop = Some(stop.token());
        match store.sync(options) {
            // A stop that came during the sweep makes its report untrustworthy.
            Ok(_) if stop.stopped() => break,
            Ok(report) if !report.swept => return CaptureContinuation::Unknown,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    CaptureContinuation::Incomplete
}

#[cfg(test)]
mod tests {
    use super::super::super::live::tests::isolated_home;
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn lock_holder(home: &Path) -> fs::File {
        let holder = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(home.join("probe/history.db.sync.lock"))
            .unwrap();
        fs2::FileExt::lock_exclusive(&holder).unwrap();
        holder
    }

    /// A sync lock another holder takes after capture is waited for within
    /// the budget left, and one held past it ends settling in time, never in
    /// an error that would skip counting and delivery.
    #[test]
    fn a_late_sync_lock_is_waited_for_within_the_budget() {
        let Some(home) = isolated_home(
            "history::probe::history::settle::tests::a_late_sync_lock_is_waited_for_within_the_budget",
        ) else {
            return;
        };
        let db = home.join("probe/history.db");
        fs::create_dir_all(db.parent().unwrap()).unwrap();
        drop(open(&db).unwrap());

        let holder = lock_holder(&home);
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(holder);
        });
        let started = Instant::now();
        let result = sync_until_settled(&db, started + Duration::from_secs(10), 16);
        release.join().unwrap();
        assert_eq!(
            result,
            CaptureContinuation::Unknown,
            "settled once released"
        );

        let _held = lock_holder(&home);
        let started = Instant::now();
        let budget = Duration::from_millis(800);
        let result = sync_until_settled(&db, started + budget, 16);
        assert_eq!(result, CaptureContinuation::Incomplete);
        assert!(
            started.elapsed() < budget + Duration::from_secs(2),
            "the lock wait is capped at the budget left: {:?}",
            started.elapsed()
        );
    }

    /// A stop requested after the loop's own check, as a sweep starts: that
    /// sweep's unswept report must not count as settled.
    #[test]
    fn a_late_stop_is_not_reported_settled() {
        let Some(home) = isolated_home(
            "history::probe::history::settle::tests::a_late_stop_is_not_reported_settled",
        ) else {
            return;
        };
        let db = home.join("probe/history.db");
        fs::create_dir_all(db.parent().unwrap()).unwrap();
        let store = open(&db).unwrap();
        let requested = Arc::new(AtomicBool::new(false));
        let stop = Stop::follow({
            let requested = requested.clone();
            move || requested.load(Ordering::SeqCst)
        });
        let ample = || Duration::from_secs(60);
        // The control, on the same store: an idle home settles.
        assert_eq!(
            settle(&store, &stop, 16, ample),
            CaptureContinuation::Unknown
        );
        let result = settle(&store, &stop, 16, || {
            requested.store(true, Ordering::SeqCst);
            ample()
        });
        assert_eq!(result, CaptureContinuation::Incomplete);
    }

    #[test]
    fn settling_stops_at_its_sweep_limit() {
        let Some(home) = isolated_home(
            "history::probe::history::settle::tests::settling_stops_at_its_sweep_limit",
        ) else {
            return;
        };
        let db = home.join("probe/history.db");
        fs::create_dir_all(db.parent().unwrap()).unwrap();
        let far = Instant::now() + Duration::from_secs(60);
        assert_eq!(
            sync_until_settled(&db, far, 0),
            CaptureContinuation::Incomplete
        );
    }
}
