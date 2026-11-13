use super::*;
use crate::history::delivery::hold_sessions;

impl Worker<'_> {
    /// An acquisition hold is neither a conflict nor a transient failure:
    /// the named sessions wait for their ownership, the rest goes again at
    /// once. Returns how the batch concluded, or `None` when the hold was
    /// rejected and the failure is recorded as usual.
    pub(super) fn hold(
        &self,
        claim: &ClaimedBatch,
        held: &[HeldSession],
        retry_after_ms: Option<i64>,
    ) -> Result<Option<Concluded>> {
        match hold_sessions(&self.conn, &claim.lease, held, self.clock) {
            Ok(()) => Ok(Some(Concluded {
                settled: None,
                failed: false,
            })),
            Err(ConflictRecoveryError::Rejected(_)) => {
                eprintln!("acquisition hold rejected: no record of the batch was named");
                Ok(None)
            }
            Err(ConflictRecoveryError::Storage(_)) => {
                eprintln!("acquisition hold deferred: local storage unavailable");
                record_failure(
                    &self.conn,
                    &claim.lease,
                    DeliveryFailure::Transient,
                    retry_after_ms,
                    self.clock,
                )?;
                Ok(Some(Concluded {
                    settled: None,
                    failed: true,
                }))
            }
            Err(error @ ConflictRecoveryError::Fenced) => {
                eprintln!("acquisition hold fenced: lease ownership changed");
                Err(error.into())
            }
        }
    }
}
