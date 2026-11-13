//! The window of unresolved batches a job may hold.
use super::*;

/// When the job's window of unresolved batches is full, what a preparation
/// reports without preparing more: the oldest unresolved batch, the one
/// covering the earliest stretch of the feed.
pub(super) fn full_window(
    conn: &Connection,
    job: &Job,
    window: usize,
    scoped: bool,
) -> Result<Option<PrepareResult>> {
    // The oldest unresolved batch, and how many the job holds: a full window
    // prepares nothing more until one of them resolves.
    let (existing, unresolved): (Option<String>, usize) = conn.query_row(
        &format!("SELECT (SELECT id FROM delivery_batches WHERE job_id=?1 AND state IN ('pending','leased','retry_wait','blocked') AND held_source IS NULL ORDER BY journal_end,{CLAIM_SEQ},seq LIMIT 1),(SELECT COUNT(*) FROM delivery_batches WHERE job_id=?1 AND state IN ('pending','leased','retry_wait','blocked') AND held_source IS NULL)"),
        [&job.id],
        |row| Ok((row.get(0)?, row.get::<_, i64>(1)? as usize)),
    )?;
    if unresolved < window.max(1) {
        return Ok(None);
    }
    let bootstrap_complete = if scoped {
        sessions::backfilled(conn, &job.id)?
    } else {
        job.caught_up
    };
    Ok(Some(PrepareResult {
        batch_id: existing,
        scanned_records: 0,
        bootstrap_complete,
    }))
}
