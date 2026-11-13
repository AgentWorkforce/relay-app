//! What a session job's preparation already holds: revisions in its
//! unresolved batches, and records kept back for their oops window.
use super::*;

/// The revisions a preparation must not admit again: those held by the
/// job's unresolved batches prepared before its last membership change
/// (`membership_seq`), including the batches that replaced them. Under one
/// membership no path reaches a change another path already passed, so
/// batches prepared since are not read. With none left, the change is
/// forgotten.
pub(super) fn held_elsewhere(conn: &Connection, job_id: &str) -> Result<HashSet<String>> {
    #[derive(Deserialize)]
    struct Held {
        records: Vec<Revision>,
    }
    #[derive(Deserialize)]
    struct Revision {
        revision_id: String,
    }
    let changed: i64 = conn.query_row(
        "SELECT membership_seq FROM delivery_session_jobs WHERE job_id=?",
        [job_id],
        |r| r.get(0),
    )?;
    let mut held = HashSet::new();
    if changed == 0 {
        return Ok(held);
    }
    let mut statement = conn.prepare(&format!(
        "SELECT payload FROM delivery_batches WHERE job_id=? AND state IN {UNRESOLVED} AND {CLAIM_SEQ}<=?"
    ))?;
    let mut rows = statement.query(params![job_id, changed])?;
    let mut any = false;
    while let Some(row) = rows.next()? {
        any = true;
        let batch: Held = serde_json::from_str(&row.get::<_, String>(0)?)?;
        held.extend(batch.records.into_iter().map(|record| record.revision_id));
    }
    if !any {
        conn.execute(
            "UPDATE delivery_session_jobs SET membership_seq=0 WHERE job_id=?",
            [job_id],
        )?;
    }
    Ok(held)
}

/// `offered` scrubbed, or `None` when it is already accounted for: an
/// unresolved batch or this one holds it, or its session is in its oops
/// window and the scrubbed record is kept back until it finishes (and is
/// then held, so no path admits it again). A record already held never
/// reaches the scrub engine, so a refusal can only block a record that
/// would be queued.
pub(super) fn unaccounted(
    held: &mut HashSet<String>,
    hold: &mut Option<hold::Hold<'_>>,
    filling: &Filling,
    offered: Offered,
    identity: &[serde_json::Value],
) -> Result<Option<HistoryExportRecord>> {
    if held.contains(&offered.record().revision_id) {
        return Ok(None);
    }
    let record = filling.scrubbed(offered)?;
    if hold::kept(hold, filling, &record, identity)? {
        held.insert(record.revision_id.clone());
        return Ok(None);
    }
    Ok(Some(record))
}
