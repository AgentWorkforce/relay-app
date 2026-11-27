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
/// would be queued. A record the engine panicked on is quarantined and
/// `None` too: the preparation moves past it.
pub(super) fn unaccounted(
    held: &mut HashSet<String>,
    hold: &mut Option<hold::Hold<'_>>,
    filling: &mut Filling,
    offered: Offered,
    identity: &[serde_json::Value],
) -> Result<Option<HistoryExportRecord>> {
    if held.contains(&offered.record().revision_id) {
        return Ok(None);
    }
    let Some(record) = filling.scrubbed(offered)? else {
        return Ok(None);
    };
    if hold::kept(hold, filling, &record, identity)? {
        held.insert(record.revision_id.clone());
        return Ok(None);
    }
    Ok(Some(record))
}

/// [`filling::retry_quarantined`] for session job `root`, unless the batch is
/// already `full`: each record is offered to `admit` while a member still
/// owns it ([`owned`]); one no member owns any more leaves the quarantine
/// unsent. Whether the batch filled.
pub(super) fn retry_members<'h, F>(
    full: bool,
    feed: (&Connection, &SessionStore, &feed::Origin),
    root: &Job,
    members: &Members,
    filling: &mut Filling,
    hold: &mut Option<hold::Hold<'h>>,
    admit: &mut F,
) -> Result<bool>
where
    F: FnMut(
        &mut Filling,
        Offered,
        &[serde_json::Value],
        &mut Option<hold::Hold<'h>>,
    ) -> Result<bool>,
{
    if full {
        return Ok(true);
    }
    let selection = &root.config.selection;
    let max_batch_records = root.config.limits.max_batch_records;
    let retry = |filling: &mut Filling, offered: Offered, key: &[serde_json::Value]| {
        if !owned(members, offered.record()) {
            return Ok(true);
        }
        admit(filling, offered, key, hold)
    };
    filling::retry_quarantined(feed, &root.id, selection, max_batch_records, filling, retry)
}

/// Whether a member of the job still owns `record`: its session is a member,
/// or it is a relationship whose child session is one. A child's
/// incoming-edges phase sends an edge from a parent that is not a member, so
/// asking only after the record's own session would drop that edge for good:
/// the backfill has moved past it, and the feed never sends a non-member's
/// records.
fn owned(members: &Members, record: &HistoryExportRecord) -> bool {
    let own = members.added(&record.source, record.session_id.as_deref());
    let edge = (record.kind == "relationship")
        .then(|| members.added(&record.source, child(record)))
        .flatten();
    own.or(edge).is_some()
}
