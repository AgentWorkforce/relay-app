//! The flush's capture: one full sweep, then bounded follow-up sweeps within
//! a share of the time left, so delivery keeps the rest of the deadline.
use super::*;

/// Follow-up sweeps take at most this fraction (1/n) of the time left after
/// the first sweep; delivery keeps the rest.
const SETTLE_SHARE: u32 = 2;
/// At most this many follow-up sweeps: 16 pages of Codex's 50-thread SQLite
/// sweep, beyond whatever the first sweep read.
const SETTLE_SWEEPS_MAX: usize = 16;

pub(super) fn update_counts(db: &Path, job_id: &str, counts: &Counts) -> Result<()> {
    counts
        .sessions_captured
        .store(history::captured_sessions(db)?, Ordering::SeqCst);
    counts.of(&delivery::status(&delivery::open_db(db)?, job_id)?);
    Ok(())
}

/// Capture into `db` and count what it holds. `None` when the deadline ended
/// the first sweep; otherwise what the follow-up sweeps could show. A
/// follow-up sweep that fails, finds the store locked or runs out of budget
/// leaves capture `incomplete` but never stops delivery of what was stored.
pub(super) fn run(
    db: &Path,
    job_id: &str,
    deadline: Instant,
    counts: &Counts,
) -> Result<Option<history::CaptureContinuation>> {
    let expired = move || Instant::now() >= deadline;
    counts.set_capture("incomplete");
    match history::sync(
        db,
        deadline.saturating_duration_since(Instant::now()),
        false,
        |_| {},
        expired,
    ) {
        Ok(true) => {}
        Ok(false) => {
            return Err(refused(
                Exit::Failed,
                "Another capture holds the state directory.",
            ))
        }
        Err(_) if expired() => {
            update_counts(db, job_id, counts)?;
            return Ok(None);
        }
        Err(error) => return Err(error),
    }
    let now = Instant::now();
    let budget_end = now + deadline.saturating_duration_since(now) / SETTLE_SHARE;
    let continuation = history::sync_until_settled(db, budget_end, SETTLE_SWEEPS_MAX);
    counts.set_capture(continuation.as_str());
    update_counts(db, job_id, counts)?;
    Ok(Some(continuation))
}

/// The flush's ending: a drained queue whose capture did not settle is
/// reported as `capture_incomplete`, never as drained.
pub(super) fn outcome(delivered: Exit, continuation: history::CaptureContinuation) -> Exit {
    match (delivered, continuation) {
        (Exit::Drained, history::CaptureContinuation::Incomplete) => Exit::CaptureIncomplete,
        (exit, _) => exit,
    }
}
