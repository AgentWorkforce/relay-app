use super::*;
use crate::history::delivery::{record_failure_after, FailureWait, StoredUpload};

/// Receiver-classified failure. `retry_after_ms` is an absolute epoch-ms retry
/// time, exactly as `record_failure` interprets its own `retry_after_ms`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiverFailure {
    pub failure: DeliveryFailure,
    pub retry_after_ms: Option<i64>,
    /// A fixed wait in ms that this computer chose, not the receiver: it
    /// replaces the backoff, yet Retry wakes it. `retry_after_ms` outranks it.
    pub local_wait_ms: Option<i64>,
    pub recovery: Option<ConflictRecovery>,
    /// Sessions whose records must be held back for acquisition: the
    /// receiver refused them, or they have no grant to present. The rest of
    /// the batch is sent again under a new batch id.
    pub hold: Option<Vec<HeldSession>>,
    /// An object the receiver stored for this batch whose admission it has
    /// not decided yet: kept on the batch so a later pass, after a restart
    /// too, asks about it instead of storing it again. `None` leaves what
    /// the batch holds.
    pub upload: Option<StoredUpload>,
}
impl From<DeliveryFailure> for ReceiverFailure {
    fn from(failure: DeliveryFailure) -> Self {
        Self {
            failure,
            retry_after_ms: None,
            local_wait_ms: None,
            recovery: None,
            hold: None,
            upload: None,
        }
    }
}
impl ReceiverFailure {
    /// Record this failure under `lease`, waiting as it asks.
    pub(crate) fn record(
        &self,
        conn: &Connection,
        lease: &DeliveryLease,
        clock: &dyn Fn() -> i64,
    ) -> Result<DeliveryStatus> {
        let wait = match (self.retry_after_ms, self.local_wait_ms) {
            (None, Some(wait_ms)) => FailureWait::Local(wait_ms),
            (retry_after_ms, _) => FailureWait::of(retry_after_ms),
        };
        record_failure_after(conn, lease, self.failure, wait, self.upload.as_ref(), clock)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// When both are set the receiver's own time wins: `retry_after_ms`
    /// outranks `local_wait_ms`, and it is recorded as a receiver wait.
    #[test]
    fn retry_after_outranks_a_local_wait() {
        let directory = tempfile::tempdir().unwrap();
        let conn = crate::history::delivery::open_db(&directory.path().join("history.db")).unwrap();
        let now = 1_000_000;
        crate::history::delivery::test_evidence(&conn).execute(
            "INSERT INTO session_events(source,session_id,event_uid,ts_ms,role,kind,text) VALUES ('claude','a','a',42,'user','text','synthetic')", [],
        ).unwrap();
        let config = crate::history::delivery::DeliveryJobConfig {
            destination_id: "relayhistory".into(),
            instance_id: "probe".into(),
            account_id: "relayhistory:account".into(),
            mapping_version: "1".into(),
            selection: crate::history::delivery::ExportSelection {
                all_sources: true,
                kinds: vec!["session_event".into()],
                ..Default::default()
            },
            limits: crate::history::delivery::DeliveryLimits::default(),
        };
        let job = crate::history::delivery::create_job(&conn, &config, now).unwrap();
        crate::history::delivery::prepare_batch(&conn, &job.job_id, now).unwrap();
        let claim =
            crate::history::delivery::claim_batch(&conn, &job.job_id, "first", 60_000, &|| now)
                .unwrap()
                .unwrap();
        let failure = ReceiverFailure {
            retry_after_ms: Some(now + 5_000),
            local_wait_ms: Some(60_000),
            ..DeliveryFailure::Transient.into()
        };
        let status = failure.record(&conn, &claim.lease, &|| now).unwrap();
        assert_eq!(status.next_attempt_ms, now + 5_000);
        let receiver_wait: i64 = conn
            .query_row(
                "SELECT receiver_retry_after_ms FROM delivery_jobs WHERE id=?",
                [&claim.lease.job_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(receiver_wait, now + 5_000);
    }
}
