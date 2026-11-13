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
}
impl Filling {
    pub(super) fn new(batch: HistoryExportBatch, scrub: ScrubEngine) -> Result<Self> {
        ensure!(batch.records.is_empty(), "a batch is filled from empty");
        let bytes = serialized_len(&batch)?;
        Ok(Self {
            batch,
            bytes,
            scrub,
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
    pub(super) fn scrubbed(&self, offered: Offered) -> Result<HistoryExportRecord> {
        let mut record = match offered {
            Offered::Held(record, Some(engine)) if engine == self.scrub.name() => {
                return Ok(record)
            }
            Offered::Held(record, _) | Offered::Read(record) => record,
        };
        if record.payload.is_null() {
            return Ok(record);
        }
        let context = ScrubContext::new(ScrubSurface::LocalStore)
            .source(&record.source)
            .session(record.session_id.as_deref());
        match self.scrub.scrub_json(&record.payload, &context) {
            Ok(payload) => {
                record.payload = payload;
                Ok(record)
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
