//! The caps a batch is prepared under: a job's configured limits, as its
//! destination's published limits and an incomplete bootstrap adjust them.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeliveryLimits {
    pub max_batch_records: usize,
    pub max_batch_bytes: usize,
    /// Bounds the feed changes one preparation examines, including
    /// excluded and unselected ones.
    pub max_scan_records: usize,
    /// Bounds the destination body as retained: the JSON envelope around it,
    /// not the body's own length.
    pub max_prepared_bytes: usize,
    /// While the job's bootstrap is incomplete, the bytes past which a batch
    /// takes no further record (see [`DeliveryLimits::for_bootstrap`]). It
    /// paces batches only: a record is still cut at `max_batch_bytes`, so a
    /// first sync sends the same content a later pass would. Set at
    /// preparation, never stored.
    #[serde(skip)]
    pub bootstrap_batch_bytes: Option<usize>,
}
/// The receiver's request cap, 2 MiB, less room for the request envelope
/// around the batch (`{"protocolVersion":1,"batch":…}`) and the wire repair
/// of a body prepared by an older build.
pub(super) const DEFAULT_BATCH_BYTES: usize = 2_097_152 - REQUEST_ROOM;
/// Room a batch leaves below the receiver's request cap.
pub(super) const REQUEST_ROOM: usize = 65_536;
/// A retained body is the request escaped as a JSON string, which can double
/// it, inside a small envelope. This cap lets every body the receiver's 2 MiB
/// request cap admits be retained; that cap itself is enforced on the body.
pub(super) const DEFAULT_PREPARED_BYTES: usize = 2 * 2_097_152 + 65_536;
impl Default for DeliveryLimits {
    fn default() -> Self {
        Self {
            max_batch_records: DEFAULT_BATCH_RECORDS,
            max_batch_bytes: DEFAULT_BATCH_BYTES,
            max_scan_records: 400,
            max_prepared_bytes: DEFAULT_PREPARED_BYTES,
            bootstrap_batch_bytes: None,
        }
    }
}
impl DeliveryLimits {
    /// The defaults of builds before batches grew toward the receiver's
    /// request cap: 1 MiB batches, 2 MiB retained bodies.
    pub(super) const FORMER_DEFAULT: Self = Self {
        max_batch_records: 100,
        max_batch_bytes: 1_048_576,
        max_scan_records: 400,
        max_prepared_bytes: 2_097_152,
        bootstrap_batch_bytes: None,
    };
    /// The limits a stored generation runs with. A generation created with a
    /// former build's defaults never chose them, so it runs with the current
    /// defaults, which every body it already holds also fits; the stored row
    /// is left as written, so an older build reading it sees what it wrote.
    /// Limits anyone set explicitly are kept.
    pub(super) fn effective(self) -> Self {
        if self == Self::FORMER_DEFAULT {
            Self::default()
        } else {
            self
        }
    }
}
/// The protocol's record cap per batch, which a job uses until its
/// destination publishes another.
pub const DEFAULT_BATCH_RECORDS: usize = 100;
/// The most records per batch a destination's published limit can raise the
/// cap to, whatever it publishes.
pub const MAX_PUBLISHED_BATCH_RECORDS: usize = 500;

/// The most records a batch holds while its job's bootstrap is incomplete
/// (see [`DeliveryLimits::for_bootstrap`]).
pub const BOOTSTRAP_BATCH_RECORDS: usize = 100;
/// The most bytes a batch holds while its job's bootstrap is incomplete: a
/// 256 KiB request, less the same room for the request envelope that
/// [`DEFAULT_BATCH_BYTES`] leaves below the receiver's cap.
pub const BOOTSTRAP_BATCH_BYTES: usize = 262_144 - REQUEST_ROOM;

/// Feed changes the default limits examine per record they admit.
pub(super) const SCAN_PER_RECORD: usize = 4;
/// The most feed changes one preparation may examine.
pub(super) const MAX_SCAN_RECORDS: usize = 10_000;

/// Batch limits a destination published with its last receipt. They size the
/// batches prepared next, never one already prepared: a batch is immutable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PublishedLimits {
    /// The most records the destination accepts in one batch.
    pub max_records: Option<usize>,
    /// The most bytes the destination accepts in one request.
    pub max_request_bytes: Option<usize>,
}
impl DeliveryLimits {
    /// These limits as a destination's published ones adjust them. A
    /// published record limit replaces the protocol default, up to
    /// [`MAX_PUBLISHED_BATCH_RECORDS`], and lowers any other cap a job was
    /// configured with but never raises it. The scan budget grows with a
    /// raised record cap, keeping the default's four changes examined per
    /// record admitted, so a raised cap can actually be filled. A published
    /// request size only ever lowers the byte cap, keeping the default's room
    /// for the request envelope. Nothing published leaves them unchanged.
    pub fn with_published(mut self, published: PublishedLimits) -> Self {
        if let Some(records) = published.max_records {
            let records = records.max(1);
            self.max_batch_records = if self.max_batch_records == DEFAULT_BATCH_RECORDS {
                records.min(MAX_PUBLISHED_BATCH_RECORDS)
            } else {
                self.max_batch_records.min(records)
            };
            self.max_scan_records = self
                .max_scan_records
                .max(self.max_batch_records * SCAN_PER_RECORD)
                .min(MAX_SCAN_RECORDS);
        }
        if let Some(bytes) = published.max_request_bytes {
            self.max_batch_bytes = self
                .max_batch_bytes
                .min(bytes.saturating_sub(REQUEST_ROOM).max(512));
        }
        self
    }
    /// These limits for a job whose bootstrap is incomplete. A first sync
    /// moves a computer's whole history in back-to-back batches, so each is
    /// kept to [`BOOTSTRAP_BATCH_RECORDS`] and [`BOOTSTRAP_BATCH_BYTES`] to
    /// keep every request short at the receiver. Lower caps a job or its
    /// destination already set are kept; once bootstrap completes the job
    /// runs with its own limits again. A single record larger than the
    /// bootstrap byte cap still goes, alone, and is cut only where a later
    /// batch would cut it.
    pub fn for_bootstrap(mut self) -> Self {
        self.max_batch_records = self.max_batch_records.min(BOOTSTRAP_BATCH_RECORDS);
        self.bootstrap_batch_bytes = Some(self.max_batch_bytes.min(BOOTSTRAP_BATCH_BYTES));
        self
    }
}
