//! Cached catalog counts shared by content-free progress monitors.
use super::*;

/// How long a count of captured sessions is reused. Counting walks the whole
/// catalog, and the count changes only while a capture runs, so a report
/// every few seconds reuses it, and the end of a capture pass counts afresh.
const CAPTURED_SESSIONS_REFRESH: Duration = Duration::from_secs(30);

/// The last count of captured sessions in each store, shared by every
/// monitor in the process: each delivery pass starts its own.
pub(super) fn captured_sessions(db: &Path, fresh: bool) -> i64 {
    static COUNTS: OnceLock<Mutex<HashMap<PathBuf, (Instant, i64)>>> = OnceLock::new();
    let counts = COUNTS.get_or_init(Default::default);
    if !fresh {
        let counts = counts.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, count)) = counts.get(db) {
            if at.elapsed() < CAPTURED_SESSIONS_REFRESH {
                return *count;
            }
        }
    }
    let Ok(count) = crate::history::probe::history::captured_sessions(db) else {
        return 0;
    };
    counts
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(db.to_owned(), (Instant::now(), count));
    count
}

impl Progress {
    /// Read the counts a report shows. `fresh` counts captured sessions again
    /// rather than reusing a recent count.
    pub(super) fn read_counts(&mut self, db: &Path, job: Option<&str>, fresh: bool) {
        self.backlog_complete = false;
        if self.phase == "watching" {
            self.phase = "delivery".into();
        }
        self.sessions_captured = captured_sessions(db, fresh);
        if job.is_none() {
            return;
        }
        let Ok(conn) = delivery::open_db_readonly(db) else {
            return;
        };
        let _ = conn.busy_timeout(Duration::from_millis(100));
        if let Some(job) = job {
            if let Ok(status) = delivery::progress_status(&conn, job) {
                self.apply_delivery(&status);
            }
        }
    }
}
