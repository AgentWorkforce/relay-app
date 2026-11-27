//! The batch a prepare fills, and the scrub step every record passes
//! through before any copy of it reaches the delivery queue.

use super::*;
use crate::history::scrub::{ScrubContext, ScrubSurface};

/// A batch being filled, with the length it serializes to kept current.
/// Serialized, a batch is its envelope with an empty `records` array, plus
/// each record, plus a comma between records, so admitting a record costs
/// that record's serialization rather than the whole batch's again.
/// Every record it is offered passes through `scrub` before it is kept or
/// batched: see [`Filling::scrubbed`].
pub(super) struct Filling {
    pub(super) batch: HistoryExportBatch,
    pub(super) bytes: usize,
    scrub: ScrubEngine,
    /// Records the engine panicked on during this preparation, for
    /// [`quarantine`] to record in the preparation's transaction.
    quarantined: Vec<Quarantined>,
}
impl Filling {
    pub(super) fn new(batch: HistoryExportBatch, scrub: ScrubEngine) -> Result<Self> {
        ensure!(batch.records.is_empty(), "a batch is filled from empty");
        let bytes = serialized_len(&batch)?;
        Ok(Self {
            batch,
            bytes,
            scrub,
            quarantined: Vec::new(),
        })
    }
    /// `offered`'s record with its payload through the scrub engine, before
    /// any copy of it is written to the queue (a batch row or an
    /// oops-window copy). A copy released from the oops window is returned
    /// as it was stored only when it was kept under an engine of this
    /// engine's name; one kept under another engine, or by a build that
    /// recorded none, is scrubbed like a record read. A refusal is a
    /// [`ScrubRefused`] error: the caller's preparation fails, its
    /// transaction rolls back with the cursor where it was, and [`refused`]
    /// blocks the job so the record is offered again on a retry, never stored
    /// raw and never counted as suppressed.
    ///
    /// `None` when the engine panicked on the record: it is quarantined for
    /// this engine (see [`quarantine`]) and the preparation moves past it,
    /// so one record the engine cannot handle never stops the rest. Nothing
    /// of its content is kept, logged or queued; a later engine offers it
    /// again ([`retry_quarantined`]).
    pub(super) fn scrubbed(&mut self, offered: Offered) -> Result<Option<HistoryExportRecord>> {
        let mut record = match offered {
            Offered::Held(record, Some(engine)) if engine == self.scrub.name() => {
                return Ok(Some(record))
            }
            Offered::Held(record, _) | Offered::Read(record) => record,
        };
        if record.payload.is_null() {
            return Ok(Some(record));
        }
        let context = ScrubContext::new(ScrubSurface::LocalStore)
            .source(&record.source)
            .session(record.session_id.as_deref());
        match self.scrub.scrub_json(&record.payload, &context) {
            Ok(payload) => {
                record.payload = payload;
                Ok(Some(record))
            }
            Err(error) if error.panicked() => {
                eprintln!(
                    "delivery record quarantined batch_ref={} record_ref={} reason={error}",
                    hash(self.batch.batch_id.as_bytes()),
                    hash(record.revision_id.as_bytes()),
                );
                self.quarantined.push(Quarantined {
                    revision_id: record.revision_id,
                    record_id: record.record_id,
                    revision: record.revision,
                    kind: record.kind,
                    source: record.source,
                });
                Ok(None)
            }
            Err(error) => {
                eprintln!(
                    "delivery record not queued batch_ref={} reason={error}",
                    hash(self.batch.batch_id.as_bytes()),
                );
                Err(ScrubRefused.into())
            }
        }
    }
    /// The name of the engine every record offered is scrubbed with.
    pub(super) fn engine(&self) -> &str {
        self.scrub.name()
    }
    /// The batch's serialized length with a record of `record` bytes added.
    pub(super) fn with(&self, record: usize) -> usize {
        self.bytes + record + usize::from(!self.batch.records.is_empty())
    }
}

/// A record offered to a preparation, by where it comes from.
pub(super) enum Offered {
    /// Read from the store, and not yet scrubbed.
    Read(HistoryExportRecord),
    /// Released from the oops window, with the name of the engine that
    /// scrubbed it when it was kept (`None` from a build that recorded none).
    Held(HistoryExportRecord, Option<String>),
}
impl Offered {
    pub(super) fn record(&self) -> &HistoryExportRecord {
        match self {
            Self::Read(record) | Self::Held(record, _) => record,
        }
    }
    /// Whether `selection` takes the record.
    pub(super) fn selected(&self, selection: &ExportSelection) -> bool {
        let record = self.record();
        let session = record.session_id.as_deref();
        selected(selection, &record.kind, &record.source, session)
    }
}

/// `offered`, or `None` when the job's consent keeps it back: the caller
/// counts a `None` as suppressed. Nothing reaches the scrub engine here.
pub(super) fn consented(consent: &mut Consent<'_>, offered: Offered) -> Result<Option<Offered>> {
    if !consent.admits(&RecordScope::of(offered.record()))? {
        return Ok(None);
    }
    Ok(Some(offered))
}

/// A record the scrub engine panicked on: its identity and where the feed
/// holds it, never its content.
pub(super) struct Quarantined {
    revision_id: String,
    record_id: String,
    revision: i64,
    kind: String,
    source: String,
}

/// The `delivery_quarantine.current_digest` of a record quarantined because
/// the scrub engine panicked on it, apart from a receiver conflict's.
pub(super) const SCRUB_PANICKED: &str = "scrub_panicked";

/// The `parent_batch_id` job `job_id`'s scrub quarantine is kept under:
/// there is no batch, and the key keeps each job's rows apart.
fn quarantine_key(job_id: &str) -> String {
    format!("{SCRUB_PANICKED}:{job_id}")
}

/// Records each record `filling`'s engine panicked on in job `job_id`'s
/// quarantine (`delivery_quarantine`, counted in the job's
/// `quarantined_records` and so in progress.json's `recordsQuarantined`), in
/// the preparation's transaction `conn`. A row holds the record's identity,
/// its feed position under `origin`, and the engine's name: no payload. The
/// engine name is what [`retry_quarantined`] compares, so a later engine
/// offers the record again. A record already quarantined is updated, so the
/// count stays one per record.
pub(super) fn quarantine(
    conn: &Connection,
    job_id: &str,
    origin: &feed::Origin,
    filling: &Filling,
    now_ms: i64,
) -> Result<()> {
    let key = quarantine_key(job_id);
    for record in &filling.quarantined {
        let marker = serde_json::json!({
            "quarantine": SCRUB_PANICKED,
            "engine": filling.engine(),
            "feed_epoch": origin.feed_epoch,
            "feed_revision": record.revision - origin.revision_base,
            "kind": record.kind,
            "source": record.source,
        });
        conn.execute(
            "INSERT INTO delivery_quarantine(job_id,parent_batch_id,revision_id,record_id,record_json,submitted_digest,current_digest,created_ms) VALUES (?,?,?,?,?,?,?,?) \
             ON CONFLICT(parent_batch_id,revision_id) DO UPDATE SET record_json=excluded.record_json,submitted_digest=excluded.submitted_digest,created_ms=excluded.created_ms",
            params![job_id, key, record.revision_id, record.record_id, marker.to_string(), filling.engine(), SCRUB_PANICKED, now_ms],
        )?;
    }
    Ok(())
}

/// The most quarantined records one preparation reads and offers again. A
/// record that panics the engine again, or that is passed over, still costs
/// its read, so a long quarantine is worked through a slice per preparation
/// and never holds one up; each slice leaves the records it offered marked
/// with this engine or gone, so the next preparation reads the next slice.
pub(super) const RETRY_BUDGET: usize = 16;

/// Offers job `job_id`'s records quarantined under an engine other than
/// `filling`'s to `admit` again, at most [`RETRY_BUDGET`] of them, read afresh
/// from `store`'s feed: whether the batch filled. A record `admit` takes
/// (into the batch, the hold, as suppressed or as not this job's) leaves the
/// quarantine; one that panics the engine again is quarantined anew under
/// this engine. One whose feed position no longer holds it — the feed was
/// renumbered, or a later revision superseded it and reaches the job through
/// the feed — is dropped from the quarantine, since nothing of it remains to
/// send.
pub(super) fn retry_quarantined(
    (conn, store, origin): (&Connection, &ai_hist::SessionStore, &feed::Origin),
    job_id: &str,
    selection: &ExportSelection,
    max_batch_records: usize,
    filling: &mut Filling,
    mut admit: impl FnMut(&mut Filling, Offered, &[serde_json::Value]) -> Result<bool>,
) -> Result<bool> {
    let key = quarantine_key(job_id);
    let rows = conn
        .prepare("SELECT revision_id,record_json FROM delivery_quarantine WHERE parent_batch_id=? AND submitted_digest<>? ORDER BY created_ms,revision_id LIMIT ?")?
        .query_map(params![key, filling.engine(), RETRY_BUDGET as i64], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (revision_id, marker) in rows {
        if filling.batch.records.len() >= max_batch_records {
            return Ok(true);
        }
        if let Some((record, key)) = reread(store, origin, selection, &revision_id, &marker)? {
            if !admit(filling, Offered::Read(record), &key)? {
                return Ok(true);
            }
        }
        conn.execute(
            "DELETE FROM delivery_quarantine WHERE parent_batch_id=? AND revision_id=?",
            params![quarantine_key(job_id), revision_id],
        )?;
    }
    Ok(false)
}

/// The quarantined record `revision_id` with its feed key, read again from
/// the feed position its `marker` names, or `None` when that position no
/// longer holds that revision.
fn reread(
    store: &ai_hist::SessionStore,
    origin: &feed::Origin,
    selection: &ExportSelection,
    revision_id: &str,
    marker: &str,
) -> Result<Option<(HistoryExportRecord, Vec<serde_json::Value>)>> {
    // The epoch is not compared: a feed the origin adopted keeps its
    // numbering, and a replaced one restarts every job from the start, which
    // reads the record afresh. The revision id check tells them apart.
    #[derive(Deserialize)]
    struct Marker {
        feed_revision: i64,
    }
    let Ok(Marker { feed_revision }) = serde_json::from_str::<Marker>(marker) else {
        return Ok(None);
    };
    // A position past the store's head (a replaced, renumbered store) holds
    // nothing; reading from it is an error that would fail every preparation.
    let head = store.head_revision()?.revision;
    if feed_revision <= 0 || u64::try_from(feed_revision).is_ok_and(|at| at > head) {
        return Ok(None);
    }
    let query = feed::query(selection)?.batch(1);
    let mut changes = store.changes_since(origin.position(feed_revision - 1), query)?;
    let Some(change) = changes.next().transpose()? else {
        return Ok(None);
    };
    if feed::revision(change.revision)? != feed_revision {
        return Ok(None);
    }
    let record = feed::record_from_change(origin, &change)?;
    Ok((record.revision_id == revision_id).then_some((record, change.key)))
}

/// Ends a preparation of job `job_id` in its transaction `conn`: records what
/// `filling`'s engine panicked on ([`quarantine`]), then queues the batch
/// ending at feed position `cursor` when it holds any record. The batch's id
/// when it was queued.
pub(super) fn seal(
    conn: &Connection,
    job_id: &str,
    origin: &feed::Origin,
    filling: Filling,
    cursor: i64,
    now_ms: i64,
) -> Result<Option<String>> {
    quarantine(conn, job_id, origin, &filling, now_ms)?;
    let batch = filling.batch;
    if batch.records.is_empty() {
        return Ok(None);
    }
    insert_batch(conn, &batch, cursor, now_ms)?;
    Ok(Some(batch.batch_id))
}

/// The scrub engine refused a record a preparation read. It carries no
/// record content.
#[derive(Debug)]
pub(super) struct ScrubRefused;
impl std::fmt::Display for ScrubRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the scrub engine refused a record")
    }
}
impl std::error::Error for ScrubRefused {}

/// Blocks job `job_id` after a preparation the scrub engine refused, as an
/// upload the engine refuses blocks its batch: `invalid_payload`, kept until
/// the job is retried (by hand, or at a new build's first start). The
/// refused preparation rolled back, so the job's cursor still stands before
/// the refused record and the retry offers it to the engine again. Nothing
/// is counted as suppressed.
pub(super) fn refused(conn: &Connection, job_id: &str) -> Result<PrepareResult> {
    let tx = write_transaction(conn)?;
    let job = job(&tx, job_id)?;
    let scoped = is_session_job(&tx, job_id)?;
    let bootstrap_complete = bootstrap_complete(&tx, &job, job_id, scoped)?;
    tx.execute(
        "UPDATE delivery_jobs SET state='blocked',failure=? WHERE id=? AND state='active'",
        params![DeliveryFailure::InvalidPayload.code(), job_id],
    )?;
    tx.commit()?;
    eprintln!(
        "delivery job blocked job_ref={} reason=scrub_refused",
        hash(job_id.as_bytes())
    );
    Ok(PrepareResult {
        batch_id: None,
        scanned_records: 0,
        bootstrap_complete,
    })
}
