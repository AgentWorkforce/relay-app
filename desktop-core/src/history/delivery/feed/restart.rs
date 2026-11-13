//! Restarting jobs when the feed is numbered afresh.
use super::*;

/// Restart every live job from the feed's start: an all-sources job drains
/// it again, and a session job backfills each member again from the start
/// while following the feed from `head`. Unsent batches, which hold the
/// records of the positions being discarded, go with them, settled receipts
/// no longer mark a position, and every claim is fenced.
pub(in crate::history::delivery) fn restart_jobs(conn: &Connection, head: u64) -> Result<()> {
    let head = revision(head)?;
    conn.execute(
        "DELETE FROM delivery_batches WHERE state IN ('pending','leased','retry_wait','blocked')",
        [],
    )?;
    // Settled receipts stay, but the feed positions they end at belong to
    // the numbering being discarded: the acknowledged position, which moves
    // through receipts' ends, must not pick one up again.
    conn.execute(
        "UPDATE delivery_batches SET journal_end=0 WHERE state IN ('acknowledged','suppressed','cancelled')",
        [],
    )?;
    conn.execute("UPDATE delivery_jobs SET attempts=0,next_attempt_ms=0,receiver_retry_after_ms=0 WHERE state<>'cancelled'", [])?;
    restart_cursors(conn, head)
}

/// Retired journal positions cannot acknowledge new feed rows. Keep every
/// batch, id, payload and prepared body, and claim the old requests before
/// newly prepared feed work (whose end is strictly positive).
pub(in crate::history::delivery) fn restart_preserving_batches(
    conn: &Connection,
    head: u64,
) -> Result<()> {
    conn.execute("UPDATE delivery_batches SET journal_end=0", [])?;
    restart_cursors(conn, revision(head)?)
}

fn restart_cursors(conn: &Connection, head: i64) -> Result<()> {
    conn.execute(
        "UPDATE delivery_jobs SET acknowledged_cursor=0,fence=fence+1,worker_id=NULL,lease_until_ms=NULL WHERE state<>'cancelled'",
        [],
    )?;
    conn.execute("UPDATE delivery_session_jobs SET last_member=''", [])?;
    conn.execute(
        "UPDATE delivery_jobs SET journal_cursor=0,bootstrap_done=0 WHERE state<>'cancelled' AND id NOT IN (SELECT job_id FROM delivery_session_jobs)",
        [],
    )?;
    conn.execute(
        "UPDATE delivery_jobs SET journal_cursor=?1,bootstrap_done=1 WHERE state<>'cancelled' AND id IN (SELECT job_id FROM delivery_session_jobs)",
        [head],
    )?;
    conn.execute(
        "UPDATE delivery_session_members SET cutoff=?1,cursor=0,phase=0,bootstrap_done=0",
        [head],
    )?;
    // Held copies carry the retired numbering. Replay holds or sends them
    // again under the new one; unresolved prepared requests remain intact.
    conn.execute("DELETE FROM delivery_held_records", [])?;
    Ok(())
}
