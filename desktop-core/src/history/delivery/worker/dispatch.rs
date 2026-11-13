//! Persisted dispatch checks and durable replacement before transport.
use super::super::{claim_batch, release_stopped_claim, store_payload, validate_dispatch};
use super::prepared::{persist_successor, scrub_for_upload};
use super::*;

impl Worker<'_> {
    /// Mapping, payload persistence and the eligibility recheck: everything
    /// before transport, ending with the persisted body to send.
    pub(super) fn ready(
        &self,
        receiver: &dyn Receiver,
        claim: &ClaimedBatch,
        config: &DeliveryJobConfig,
        stopping: &dyn Fn() -> bool,
    ) -> Result<Option<PreparedPayload>, ReceiverFailure> {
        let mapped = map(
            receiver,
            claim,
            config.limits.max_prepared_bytes,
            &self.options.scrub,
            &self.context(claim, stopping),
        )?;
        self.dispatch(
            receiver,
            claim,
            mapped,
            config.limits.max_prepared_bytes,
            stopping,
        )
    }

    /// Persist a batch's mapped payload, if it has one, and recheck its
    /// eligibility, ending with the persisted body to send. A pipelined drain
    /// does this on its draining thread, which holds the journal connection,
    /// and hands the body to a sender thread at once. Every local error here
    /// is a retryable transient.
    pub(super) fn dispatch(
        &self,
        receiver: &dyn Receiver,
        claim: &ClaimedBatch,
        mapped: Option<RetainedPayload>,
        max_prepared_bytes: usize,
        stopping: &dyn Fn() -> bool,
    ) -> Result<Option<PreparedPayload>, ReceiverFailure> {
        let transient = || ReceiverFailure::from(DeliveryFailure::Transient);
        if stopping() {
            return Err(transient());
        }
        let freshly_mapped = mapped.is_some();
        if let Some(retained) = mapped {
            store_payload(&self.conn, &claim.lease, retained, self.clock)
                .map_err(|_| transient())?;
            if stopping() {
                return Err(transient());
            }
        }
        // Eligibility can change while a receiver maps its payload. Recheck it
        // in core immediately before transport and send the persisted bytes.
        let payload =
            validate_dispatch(&self.conn, &claim.lease, self.clock).map_err(|_| transient())?;
        if stopping() {
            return Err(transient());
        }
        if freshly_mapped {
            return Ok(Some(payload));
        }
        if let Some((successor, prepared)) =
            scrub_for_upload(&self.options.scrub, &payload, &claim.batch, |next| {
                self.remap(receiver, claim, next, max_prepared_bytes, stopping)
            })?
        {
            if stopping() {
                return Err(transient());
            }
            if prepared.len() > max_prepared_bytes {
                return Err(DeliveryFailure::InvalidPayload.into());
            }
            persist_successor(
                &self.conn,
                &claim.lease,
                &payload.sha256,
                &successor,
                prepared,
                self.clock,
            )
            .map_err(|_| transient())?;
            return Ok(None);
        }
        Ok(Some(payload))
    }

    /// A replacement occupies the predecessor's place in the journal. Once
    /// it is persisted, return all later unsent claims to pending, even if
    /// their mapping failed. The next turn claims the successor first.
    pub(super) fn dispatch_window(
        &self,
        receiver: &dyn Receiver,
        claim: &ClaimedBatch,
        mapped: Mapped,
        config: &DeliveryJobConfig,
        stopping: &dyn Fn() -> bool,
        defer: bool,
    ) -> Result<Option<PreparedPayload>, ReceiverFailure> {
        if defer {
            release_stopped_claim(&self.conn, &claim.lease, self.clock)
                .map_err(|_| ReceiverFailure::from(DeliveryFailure::Transient))?;
            return Ok(None);
        }
        mapped.and_then(|mapped| {
            self.dispatch(
                receiver,
                claim,
                mapped,
                config.limits.max_prepared_bytes,
                stopping,
            )
        })
    }

    fn remap(
        &self,
        receiver: &dyn Receiver,
        claim: &ClaimedBatch,
        next: &HistoryExportBatch,
        max_prepared_bytes: usize,
        stopping: &dyn Fn() -> bool,
    ) -> Result<RetainedPayload, ReceiverFailure> {
        let mut successor = claim.clone();
        successor.batch = next.clone();
        successor.lease.batch_id.clone_from(&next.batch_id);
        successor.prepared = None;
        successor.rejected_revision_ids.clear();
        successor.isolation_frontier.clear();
        map(
            receiver,
            &successor,
            max_prepared_bytes,
            &self.options.scrub,
            &self.context(&successor, stopping),
        )?
        .ok_or_else(|| DeliveryFailure::Transient.into())
    }

    /// Open a window: claim the job's oldest unresolved batch under a new job
    /// lease, as a one-batch drain does.
    pub(super) fn open_window(&self, job_id: &str) -> Result<NextClaim> {
        Ok(
            match claim_batch(
                &self.conn,
                job_id,
                &self.options.worker_id,
                self.options.lease_ms,
                self.clock,
            )? {
                Some(claim) => NextClaim::Claimed(Box::new(claim)),
                None => NextClaim::Empty,
            },
        )
    }
}
