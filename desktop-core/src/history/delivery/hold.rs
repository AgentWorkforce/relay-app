//! The oops window: under an organization's upload control, a finished
//! session's records wait locally before they upload, so the person who ran
//! it can still decide it must not leave the machine.
//!
//! A session is *finished* once it has been quiet for the whole window: at
//! `now_ms >= last_activity_ms + window_ms`, where `last_activity_ms` is the
//! store's record of the session's latest activity. Activity that resumes
//! moves `last_activity_ms` and so restarts the window. A session the store
//! has no activity time for is measured from when its first record was held.
//! A relationship waits for both of the sessions it names.
//!
//! A record a preparation would send while its session is in its window is
//! copied to `delivery_held_records` instead, and the job's position moves
//! past it in the same transaction, exactly as for a record sent. Each later
//! preparation first releases the copies whose sessions have finished, in
//! revision order, through the same selection and consent checks as any
//! record, and drops the copies those checks now refuse. A copy leaves the
//! table in the transaction that puts it into a batch, so nothing is sent
//! twice, and the table is the whole state: a restart, or a pass made after
//! a long time offline, sees the same holds.
//! A window the organization lifts releases every copy at the next pass.
use super::*;

/// The window a preparation holds finished sessions for, and the time it
/// measures them at.
pub(super) struct Hold<'a> {
    conn: &'a Connection,
    job_id: &'a str,
    /// `None` once there is no window: nothing more is held, and what was
    /// held is released.
    window_ms: Option<i64>,
    now_ms: i64,
    waiting: HashMap<(String, String), bool>,
}

/// A copy a preparation releases: the record, its change key, and the name
/// of the engine that scrubbed it (`None` from a build that recorded none).
pub(super) struct Released {
    pub(super) record: HistoryExportRecord,
    pub(super) key: Vec<serde_json::Value>,
    pub(super) engine: Option<String>,
}

impl<'a> Hold<'a> {
    /// `None` when there is no window and nothing was held. Without a window
    /// nothing more is held, and copies held under one that has since been
    /// lifted are all due.
    pub(super) fn new(
        conn: &'a Connection,
        job_id: &'a str,
        window_ms: Option<i64>,
        now_ms: i64,
    ) -> Result<Option<Self>> {
        let window_ms = window_ms.filter(|window| *window > 0);
        if window_ms.is_none() {
            let any: bool = conn
                .prepare_cached(
                    "SELECT EXISTS(SELECT 1 FROM delivery_held_records WHERE job_id=?)",
                )?
                .query_row([job_id], |r| r.get(0))?;
            if !any {
                return Ok(None);
            }
        }
        Ok(Some(Self {
            conn,
            job_id,
            window_ms,
            now_ms,
            waiting: HashMap::new(),
        }))
    }

    /// Whether `session` of `source` is still in its window.
    fn session_waiting(&mut self, source: &str, session_id: &str) -> Result<bool> {
        let Some(window_ms) = self.window_ms else {
            return Ok(false);
        };
        let key = (source.to_owned(), session_id.to_owned());
        if let Some(&waiting) = self.waiting.get(&key) {
            return Ok(waiting);
        }
        let quiet_since = match last_activity(self.conn, source, session_id)? {
            Some(last) => last,
            None => self
                .conn
                .prepare_cached(
                    "SELECT MIN(held_ms) FROM delivery_held_records WHERE job_id=? AND \
                     ((source=?2 AND session_id=?3) OR (child_source=?2 AND child_session_id=?3))",
                )?
                .query_row(params![self.job_id, source, session_id], |r| {
                    r.get::<_, Option<i64>>(0)
                })?
                .unwrap_or(self.now_ms),
        };
        let waiting = self.now_ms < quiet_since.saturating_add(window_ms);
        self.waiting.insert(key, waiting);
        Ok(waiting)
    }

    /// Whether `record` must wait: its session, or the child a relationship
    /// names, is in its window. A record of no session never waits.
    pub(super) fn holds(&mut self, record: &HistoryExportRecord) -> Result<bool> {
        let Some(session_id) = record.session_id.as_deref() else {
            return Ok(false);
        };
        if self.session_waiting(&record.source, session_id)? {
            return Ok(true);
        }
        match (record.kind.as_str(), sessions_child(record)) {
            ("relationship", Some(child)) => self.session_waiting(&record.source, child),
            _ => Ok(false),
        }
    }

    /// Keep a copy of `record`, scrubbed by the engine named `engine`, until
    /// its sessions finish. A revision already kept stays as it was.
    pub(super) fn keep(
        &self,
        record: &HistoryExportRecord,
        key: &[serde_json::Value],
        engine: &str,
    ) -> Result<()> {
        let session_id = record.session_id.as_deref().unwrap_or_default();
        let child = match record.kind.as_str() {
            "relationship" => sessions_child(record),
            _ => None,
        };
        self.conn.prepare_cached(
            "INSERT OR IGNORE INTO delivery_held_records(job_id,revision_id,revision,source,session_id,child_source,child_session_id,record_json,key_json,held_ms,scrub_engine) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
        )?.execute(params![
            self.job_id,
            record.revision_id,
            record.revision,
            record.source,
            session_id,
            child.map(|_| record.source.as_str()),
            child,
            serde_json::to_string(record)?,
            serde_json::to_string(key)?,
            self.now_ms,
            engine,
        ])?;
        Ok(())
    }

    /// The revisions of the copies whose sessions have finished, oldest
    /// first. The caller takes each with [`Hold::take`] and removes it with
    /// [`Hold::forget`] in the transaction that sends or drops it.
    pub(super) fn due(&mut self) -> Result<Vec<String>> {
        let rows: Vec<HeldRow> = self
            .conn
            .prepare_cached(
                "SELECT revision_id,source,session_id,child_source,child_session_id FROM delivery_held_records WHERE job_id=? ORDER BY revision,revision_id",
            )?
            .query_map([self.job_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut due = Vec::new();
        for (revision_id, source, session_id, child_source, child_session_id) in rows {
            if self.session_waiting(&source, &session_id)? {
                continue;
            }
            if let (Some(child_source), Some(child_session_id)) = (child_source, child_session_id) {
                if self.session_waiting(&child_source, &child_session_id)? {
                    continue;
                }
            }
            due.push(revision_id);
        }
        Ok(due)
    }

    /// The held copy of `revision_id`.
    pub(super) fn take(&self, revision_id: &str) -> Result<Released> {
        let (record, key, engine): (String, String, Option<String>) = self
            .conn
            .prepare_cached(
                "SELECT record_json,key_json,scrub_engine FROM delivery_held_records WHERE job_id=? AND revision_id=?",
            )?
            .query_row(params![self.job_id, revision_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
        Ok(Released {
            record: serde_json::from_str(&record)?,
            key: serde_json::from_str(&key)?,
            engine,
        })
    }

    /// Remove the copy of `revision_id`: it is in a batch, or will never be.
    pub(super) fn forget(&self, revision_id: &str) -> Result<()> {
        self.conn
            .prepare_cached("DELETE FROM delivery_held_records WHERE job_id=? AND revision_id=?")?
            .execute(params![self.job_id, revision_id])?;
        Ok(())
    }
}

/// [`prepare_batch`](super::prepare_batch) through `store`, holding each session's records for `oops_window_ms`
/// after its last activity, when that is some positive window: see the
/// module. A record waiting is kept locally and the job moves past it;
/// a later preparation sends it once its session has finished.
pub(crate) fn prepare_batch_with(
    conn: &Connection,
    store: &ai_hist::SessionStore,
    job_id: &str,
    now_ms: i64,
    window: usize,
    published: PublishedLimits,
    oops_window_ms: Option<i64>,
) -> Result<PrepareResult> {
    let scrub = ScrubEngine::default_engine();
    let intake = Intake {
        oops_window_ms,
        scrub: &scrub,
    };
    prepare_batch_scrubbed(conn, store, job_id, now_ms, window, published, intake)
}

/// What a preparation does with a record it reads before queueing it: how
/// long its session's records are held, and the engine that scrubs it.
#[derive(Clone, Copy)]
pub(crate) struct Intake<'s> {
    /// The oops window: see [`prepare_batch_with`].
    pub(crate) oops_window_ms: Option<i64>,
    /// Every record passes through this engine before any copy is queued.
    pub(crate) scrub: &'s ScrubEngine,
}

/// [`prepare_batch_with`] scrubbing every record with the intake's engine
/// rather than [`ScrubEngine::default_engine`], as the drain does with its
/// own engine.
pub(crate) fn prepare_batch_scrubbed(
    conn: &Connection,
    store: &ai_hist::SessionStore,
    job_id: &str,
    now_ms: i64,
    window: usize,
    published: PublishedLimits,
    intake: Intake<'_>,
) -> Result<PrepareResult> {
    match prepare(conn, store, job_id, now_ms, window, published, intake) {
        Err(error) if error.downcast_ref::<filling::ScrubRefused>().is_some() => {
            filling::refused(conn, job_id).context(PreparationFailed)
        }
        prepared => prepared.context(PreparationFailed),
    }
}

/// Keeps `record`, scrubbed by `filling`'s engine, under its key when `hold`
/// holds its session: whether it did, so the preparation moves past it.
pub(super) fn kept(
    hold: &mut Option<Hold<'_>>,
    filling: &Filling,
    record: &HistoryExportRecord,
    key: &[serde_json::Value],
) -> Result<bool> {
    let Some(hold) = hold.as_mut() else {
        return Ok(false);
    };
    if !hold.holds(record)? {
        return Ok(false);
    }
    hold.keep(record, key, filling.engine())?;
    Ok(true)
}

/// Offers each copy whose session has finished to `admit`, oldest first,
/// before anything else goes into the batch: whether the batch filled. The
/// copies are offered as [`Offered::Held`] with the engine that scrubbed
/// them when they were kept: one kept under the current engine is not
/// scrubbed again (see [`Filling::scrubbed`]). A
/// copy `admit` adds or drops is forgotten; one it has no room for stays
/// held for the next batch.
pub(super) fn release_due<'a>(
    hold: &mut Option<Hold<'a>>,
    filling: &mut Filling,
    max_batch_records: usize,
    mut admit: impl FnMut(
        &mut Filling,
        Offered,
        &[serde_json::Value],
        &mut Option<Hold<'a>>,
    ) -> Result<bool>,
) -> Result<bool> {
    let Some(due) = hold.as_mut().map(Hold::due).transpose()? else {
        return Ok(false);
    };
    for revision_id in due {
        if filling.batch.records.len() >= max_batch_records {
            return Ok(true);
        }
        let Some(taken) = hold
            .as_ref()
            .map(|hold| hold.take(&revision_id))
            .transpose()?
        else {
            break;
        };
        let offered = Offered::Held(taken.record, taken.engine);
        if !admit(filling, offered, &taken.key, hold)? {
            return Ok(true);
        }
        if let Some(hold) = hold.as_ref() {
            hold.forget(&revision_id)?;
        }
    }
    Ok(false)
}

/// Opens an all-sources job's hold under `(window_ms, now_ms)` and releases
/// its due copies into the batch before the feed (see [`release_due`]): the
/// hold, and whether the batch filled.
pub(super) fn release_to_feed<'a>(
    conn: &'a Connection,
    job_id: &'a str,
    config: &DeliveryJobConfig,
    (window_ms, now_ms): (Option<i64>, i64),
    filling: &mut Filling,
    consent: &mut Consent<'_>,
    suppressed: &mut i64,
) -> Result<(Option<Hold<'a>>, bool)> {
    let mut hold = Hold::new(conn, job_id, window_ms, now_ms)?;
    let full = release_due(
        &mut hold,
        filling,
        config.limits.max_batch_records,
        |filling, copy, key, _| admit(filling, copy, key, config, consent, &mut None, suppressed),
    )?;
    Ok((hold, full))
}

/// An all-sources job's offer of a record, from the feed or released from
/// `hold` (passed empty then): one its selection does not take is passed
/// over (for a copy, the job's own selection changed), one kept back counts
/// as suppressed, one whose session `hold` holds is kept locally, and the
/// rest go into the batch: whether there was room. A record read from the
/// feed is scrubbed once it is known to be kept or batched; a released copy
/// already was.
pub(super) fn admit(
    filling: &mut Filling,
    offered: Offered,
    key: &[serde_json::Value],
    config: &DeliveryJobConfig,
    consent: &mut Consent<'_>,
    hold: &mut Option<Hold<'_>>,
    suppressed: &mut i64,
) -> Result<bool> {
    if !offered.selected(&config.selection) {
        return Ok(true);
    }
    if consent.record_excluded(offered.record())? {
        *suppressed += 1;
        return Ok(true);
    }
    let record = filling.scrubbed(offered)?;
    if kept(hold, filling, &record, key)? {
        return Ok(true);
    }
    match push_record(filling, record, key, &config.limits)? {
        Admitted::Added => Ok(true),
        Admitted::Full => Ok(false),
        Admitted::Suppressed => {
            *suppressed += 1;
            Ok(true)
        }
    }
}

/// A held copy's revision, its session, and the child session an edge names.
type HeldRow = (String, String, String, Option<String>, Option<String>);

fn sessions_child(record: &HistoryExportRecord) -> Option<&str> {
    record.payload.get("child_session_id")?.as_str()
}

/// The store's latest activity for a session, when it records one.
pub(super) fn last_activity(
    conn: &Connection,
    source: &str,
    session_id: &str,
) -> Result<Option<i64>> {
    // Resolve the same read-only evidence attachment that the SELECT reads;
    // the upload queue's main schema intentionally contains no sessions.
    if !schema::column_exists(conn, "sessions", "last_activity_ms")? {
        return Ok(None);
    }
    Ok(conn
        .prepare_cached("SELECT last_activity_ms FROM sessions WHERE source=? AND session_id=?")?
        .query_row(params![source, session_id], |r| r.get::<_, Option<i64>>(0))
        .optional()?
        .flatten())
}

/// Whether the job holds copies of `session`'s records, or of edges naming it.
pub fn held_for_session(conn: &Connection, source: &str, session_id: &str) -> Result<bool> {
    if !conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='delivery_held_records')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(false);
    }
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM delivery_held_records WHERE (source=?1 AND session_id=?2) OR (child_source=?1 AND child_session_id=?2))",
        params![source, session_id],
        |r| r.get(0),
    )?)
}

/// Drop every held copy that discloses `session`, in the caller's
/// transaction: it is excluded, or left the job.
pub(super) fn drop_session(
    conn: &Connection,
    job_id: Option<&str>,
    session: &SessionIdentity,
) -> Result<()> {
    conn.execute(
        "DELETE FROM delivery_held_records WHERE (?1 IS NULL OR job_id=?1) AND \
         ((source=?2 AND session_id=?3) OR (child_source=?2 AND child_session_id=?3))",
        params![job_id, session.source, session.session_id],
    )?;
    Ok(())
}

/// Drop every copy the job holds, in the caller's transaction.
pub(super) fn drop_job(conn: &Connection, job_id: &str) -> Result<()> {
    conn.execute("DELETE FROM delivery_held_records WHERE job_id=?", [job_id])?;
    Ok(())
}

/// When `session`'s oops window ends under a window of `window_ms`: its last
/// activity plus the window, or, when the store records no activity for it,
/// when its first record was held plus the window. `None` when neither is
/// known: nothing of it waits yet.
pub fn waiting_until(
    conn: &Connection,
    session: &SessionIdentity,
    window_ms: i64,
) -> Result<Option<i64>> {
    if let Some(last) = last_activity(conn, &session.source, &session.session_id)? {
        return Ok(Some(last.saturating_add(window_ms)));
    }
    if !conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='delivery_held_records')",
        [],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(None);
    }
    let first: Option<i64> = conn.query_row(
        "SELECT MIN(held_ms) FROM delivery_held_records WHERE (source=?1 AND session_id=?2) OR (child_source=?1 AND child_session_id=?2)",
        params![session.source, session.session_id],
        |r| r.get(0),
    )?;
    Ok(first.map(|first| first.saturating_add(window_ms)))
}

/// Whether the oops window of `window_ms` has ended at `now_ms` for any of
/// `sessions`: under the organization's upload control a session may be kept
/// back only while it waits, so excluding it is refused.
pub fn window_ended(
    conn: &Connection,
    sessions: &[SessionIdentity],
    window_ms: i64,
    now_ms: i64,
) -> Result<bool> {
    for session in sessions {
        if waiting_until(conn, session, window_ms)?.is_some_and(|until| now_ms >= until) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests;
