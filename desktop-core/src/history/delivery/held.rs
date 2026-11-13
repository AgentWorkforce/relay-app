//! Batches held back for acquisition: a session's records wait, unresolved
//! but unclaimed, until the session's ownership is settled and the hold is
//! released.

use super::*;

type Recovery<T> = std::result::Result<T, ConflictRecoveryError>;

fn storage(error: anyhow::Error) -> ConflictRecoveryError {
    ConflictRecoveryError::Storage(error)
}

/// Hold back the records of `held` sessions from the leased batch, which
/// they kept from being sent: the receiver refused them for acquisition, or
/// they have no grant to present. The batch is suppressed (its records are
/// not counted as suppressed) and replaced by one held batch per session
/// and one batch of everything else, each under a new batch id and in the
/// parent's claim place, so a payload never changes beneath an idempotency
/// key that may have been used. Held batches stay unresolved, so the
/// acknowledged cursor never moves past them, but are neither claimed nor
/// counted in the window until [`release_held`] lets them go. An empty
/// `session_id` holds the source's records that carry no session.
///
/// Only revisions still unresolved are held. A revision the parent already
/// settled (accepted, suppressed, or provisionally rejected by isolation)
/// goes into the batch of everything else with its progress carried over,
/// so it is neither held nor sent again, and is counted once when that
/// batch is acknowledged.
pub(crate) fn hold_sessions(
    conn: &Connection,
    lease: &DeliveryLease,
    held: &[HeldSession],
    clock: &dyn Fn() -> i64,
) -> Recovery<()> {
    let tx = write_transaction(conn).map_err(|error| storage(error.into()))?;
    let now_ms = clock();
    let (parent, end) = leased_parent(&tx, lease, now_ms)?;
    let progress = batch_progress(&tx, &lease.batch_id).map_err(storage)?;
    let groups = group_records(&parent, held, &progress);
    if groups.iter().all(|(session, _)| session.is_none()) {
        return Err(ConflictRecoveryError::Rejected(anyhow::anyhow!(
            "no record of the batch belongs to a held session"
        )));
    }
    tx.execute(
        "UPDATE delivery_batches SET state='suppressed',payload=NULL,prepared=NULL WHERE id=?",
        [&lease.batch_id],
    )
    .map_err(|error| storage(error.into()))?;
    for (session, records) in groups {
        let mut batch = parent.clone();
        batch.batch_id = tx
            .query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0))
            .map_err(|error| storage(error.into()))?;
        batch.records = records;
        insert_batch(&tx, &batch, end, now_ms).map_err(storage)?;
        take_claim_place(&tx, &batch.batch_id, &lease.batch_id).map_err(storage)?;
        match session {
            Some(session) => mark_held(&tx, &batch.batch_id, session)?,
            // The settled progress the parent made on these records stands.
            None => carry_progress(&tx, &batch, &progress)?,
        }
    }
    end_claim(&tx, lease).map_err(storage)?;
    tx.commit().map_err(|error| storage(error.into()))?;
    Ok(())
}

/// The leased batch's payload and journal end, once the lease is still ours.
fn leased_parent(
    tx: &Transaction<'_>,
    lease: &DeliveryLease,
    now_ms: i64,
) -> Recovery<(HistoryExportBatch, i64)> {
    match check_lease(tx, lease, now_ms, LeaseCheck::Owned) {
        Ok(()) => {}
        Err(error) if error.is::<retry::LeaseFenced>() => {
            return Err(ConflictRecoveryError::Fenced)
        }
        Err(error) => return Err(storage(error)),
    }
    let (payload, end): (String, i64) = tx
        .query_row(
            "SELECT payload,journal_end FROM delivery_batches WHERE id=? AND job_id=? AND state='leased'",
            params![lease.batch_id, lease.job_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| storage(error.into()))?;
    let parent = serde_json::from_str(&payload)
        .map_err(|error| ConflictRecoveryError::Rejected(error.into()))?;
    Ok((parent, end))
}

/// The parent's records grouped by the held session each still waits for,
/// in first-seen order; `None` gathers everything else, settled records
/// included.
fn group_records<'a>(
    parent: &HistoryExportBatch,
    held: &'a [HeldSession],
    progress: &BatchProgress,
) -> Vec<(Option<&'a HeldSession>, Vec<HistoryExportRecord>)> {
    let settled: HashSet<&str> = progress
        .accepted
        .iter()
        .chain(&progress.suppressed)
        .chain(&progress.rejected)
        .map(String::as_str)
        .collect();
    let key = |record: &HistoryExportRecord| {
        if settled.contains(record.revision_id.as_str()) {
            return None;
        }
        held.iter().find(|held| {
            held.source == record.source
                && held.session_id == record.session_id.as_deref().unwrap_or("")
        })
    };
    let mut groups: Vec<(Option<&HeldSession>, Vec<HistoryExportRecord>)> = Vec::new();
    for record in &parent.records {
        let session = key(record);
        match groups.iter_mut().find(|(group, _)| *group == session) {
            Some((_, records)) => records.push(record.clone()),
            None => groups.push((session, vec![record.clone()])),
        }
    }
    groups
}

/// Carry the parent's settled progress onto the batch of everything else.
fn carry_progress(
    tx: &Transaction<'_>,
    batch: &HistoryExportBatch,
    progress: &BatchProgress,
) -> Recovery<()> {
    let carried = |ids: &[String]| -> Vec<String> {
        batch
            .records
            .iter()
            .filter(|record| ids.contains(&record.revision_id))
            .map(|record| record.revision_id.clone())
            .collect()
    };
    let accepted = carried(&progress.accepted);
    let suppressed = carried(&progress.suppressed);
    let rejected_ids = carried(&progress.rejected);
    if accepted.is_empty() && suppressed.is_empty() && rejected_ids.is_empty() {
        return Ok(());
    }
    let json =
        |ids: &Vec<String>| serde_json::to_string(ids).map_err(|error| storage(error.into()));
    tx.execute(
        "UPDATE delivery_batches SET accepted_revision_ids=?,suppressed_revision_ids=?,rejected_revision_ids=?,accepted_records=? WHERE id=?",
        params![
            json(&accepted)?,
            json(&suppressed)?,
            json(&rejected_ids)?,
            accepted.len() as i64,
            batch.batch_id
        ],
    )
    .map_err(|error| storage(error.into()))?;
    Ok(())
}

/// Mark one replacement batch as held for `session`.
fn mark_held(tx: &Transaction<'_>, batch_id: &str, session: &HeldSession) -> Recovery<()> {
    tx.execute(
        "UPDATE delivery_batches SET held_source=?,held_session=?,held_reason=? WHERE id=?",
        params![session.source, session.session_id, session.reason, batch_id],
    )
    .map_err(|error| storage(error.into()))?;
    Ok(())
}

/// Let the held records of one session go: they are claimed again in their
/// place and present whatever grant the session holds by then. Returns how
/// many batches were released.
pub fn release_held(conn: &Connection, source: &str, session_id: &str) -> Result<usize> {
    if !schema::column_exists(conn, "delivery_batches", "held_source")? {
        return Ok(0);
    }
    Ok(conn.execute(
        &format!("UPDATE delivery_batches SET held_source=NULL,held_session=NULL,held_reason=NULL WHERE held_source=? AND held_session=? AND state IN {UNRESOLVED}"),
        params![source, session_id],
    )?)
}

/// Change why one session's records are held, keeping them held. Returns
/// how many batches changed.
pub fn relabel_held(
    conn: &Connection,
    source: &str,
    session_id: &str,
    reason: &str,
) -> Result<usize> {
    if !schema::column_exists(conn, "delivery_batches", "held_source")? {
        return Ok(0);
    }
    Ok(conn.execute(
        &format!("UPDATE delivery_batches SET held_reason=? WHERE held_source=? AND held_session=? AND state IN {UNRESOLVED}"),
        params![reason, source, session_id],
    )?)
}

/// This install's current origin, once the collector has opened the store.
pub fn origin_id(conn: &Connection) -> Result<Option<String>> {
    if !schema::table_exists(conn, "delivery_origin")? {
        return Ok(None);
    }
    Ok(feed::Origin::read(conn).ok().map(|origin| origin.origin_id))
}

/// Every session with held records, once each per reason.
pub fn held_sessions(conn: &Connection) -> Result<Vec<HeldSession>> {
    if !schema::column_exists(conn, "delivery_batches", "held_source")? {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(&format!("SELECT DISTINCT held_source,held_session,held_reason FROM delivery_batches WHERE held_source IS NOT NULL AND state IN {UNRESOLVED} ORDER BY 1,2,3"))?;
    let rows = statement.query_map([], |row| {
        Ok(HeldSession {
            source: row.get(0)?,
            session_id: row.get(1)?,
            reason: row.get(2)?,
        })
    })?;
    rows.collect::<rusqlite::Result<_>>().map_err(Into::into)
}
