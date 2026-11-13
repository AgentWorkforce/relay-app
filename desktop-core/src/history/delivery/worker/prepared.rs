//! A claimed batch's body on its way out: mapped by the receiver and
//! scrubbed before it is retained, then scrubbed again immediately before it
//! is sent. A body the engine refuses blocks its batch and goes nowhere.

use super::super::{
    check_lease, end_claim, insert_batch, job, pending_scope, take_claim_place, write_transaction,
    Consent, LeaseCheck,
};
use super::*;
use crate::history::scrub::{ScrubContext, ScrubError, ScrubSurface};
use anyhow::ensure;
use rusqlite::params;

/// The receiver's mapping of a claimed batch, hashed and serialized as its
/// row retains it, or none when the batch already holds its persisted body.
/// It reads no local state, so a pipelined drain runs it on a sender thread.
/// Every local error here is a retryable transient: only the receiver
/// classifies the batch itself.
pub(super) fn map(
    receiver: &dyn Receiver,
    claim: &ClaimedBatch,
    max_prepared_bytes: usize,
    scrub: &ScrubEngine,
    context: &ReceiverContext<'_>,
) -> Mapped {
    let transient = || ReceiverFailure::from(DeliveryFailure::Transient);
    if (context.cancelled)() {
        return Err(transient());
    }
    if receiver.mapping_version() != claim.batch.mapping_version {
        return Err(DeliveryFailure::MappingVersionMismatch.into());
    }
    let kinds = receiver.supported_kinds();
    if claim.batch.records.iter().any(|record| {
        !kinds.contains(&record.kind.as_str())
            || (record.operation == "delete" && !receiver.supports_tombstones())
    }) {
        return Err(DeliveryFailure::UnsupportedEvidence.into());
    }
    if claim.prepared.is_some() {
        return Ok(None);
    }
    let prepared = receiver.prepare(&claim.batch, context)?;
    if (context.cancelled)() {
        return Err(transient());
    }
    if prepared.content_type.is_empty()
        || prepared.content_type.len() > 200
        || prepared.content_type.contains(['\r', '\n'])
    {
        return Err(DeliveryFailure::InvalidPayload.into());
    }
    // Only record payloads are session content. The receiver's surrounding
    // wire envelope carries protocol identity and, for the Relay receiver,
    // the acquisition grant intentionally presented to the service. Never
    // offer that envelope to a content scrubber.
    let body = scrubbed_body(
        scrub,
        &prepared.body,
        &claim.batch,
        ScrubSurface::LocalStore,
    )
    .map_err(|error| refused(&claim.batch, &error))?;
    let body = scrubbed_body(scrub, &body, &claim.batch, ScrubSurface::Upload)
        .map_err(|error| refused(&claim.batch, &error))?;
    let retained = RetainedPayload::new(PreparedPayload::stored(
        receiver.mapping_version(),
        &prepared.content_type,
        &body,
        Some(THIS_BUILD.as_str()),
    ))
    .map_err(|_| transient())?;
    if retained.len() > max_prepared_bytes {
        return Err(DeliveryFailure::InvalidPayload.into());
    }
    Ok(Some(retained))
}

/// Scrub only the exact `payload` path of each batch record in a receiver's
/// prepared JSON. Relay's production request wraps the batch at `/batch`;
/// the direct batch form is also used by core receivers and tests. The full
/// record envelope must match the claimed batch before its payload is
/// touched, so an unrelated nested `records` object can never become a
/// key-name allowlist. An opaque receiver format is scrubbed as text, while
/// JSON with no record paths gets the already-scrubbed batch content it
/// mapped unchanged; [`Filling::scrubbed`] is the mandatory scrub boundary
/// before any batch reaches a receiver.
///
/// [`Filling::scrubbed`]: super::super::filling::Filling::scrubbed
fn scrubbed_body(
    scrub: &ScrubEngine,
    body: &str,
    batch: &HistoryExportBatch,
    surface: ScrubSurface,
) -> Result<String, ScrubError> {
    let mut value: serde_json::Value = match serde_json::from_str(body) {
        Ok(value) => value,
        Err(_) => return scrub.scrub_text(body, &ScrubContext::new(surface)),
    };
    let records = if value.pointer("/batch/records").is_some() {
        value.pointer_mut("/batch/records")
    } else if value.pointer("/records").is_some() {
        value.pointer_mut("/records")
    } else {
        return Ok(body.to_owned());
    }
    .and_then(serde_json::Value::as_array_mut)
    .ok_or_else(|| ScrubError::new("prepared body records are not an array"))?;
    if records.len() != batch.records.len() {
        return Err(ScrubError::new(
            "prepared body records differ from its batch",
        ));
    }

    let mut changed = false;
    for (wire, record) in records.iter_mut().zip(&batch.records) {
        let mut expected = serde_json::to_value(record)
            .map_err(|_| ScrubError::new("batch record does not serialize"))?;
        let Some(wire_envelope) = wire.as_object() else {
            return Err(ScrubError::new("prepared body record is not an object"));
        };
        let Some(expected_envelope) = expected.as_object_mut() else {
            return Err(ScrubError::new("batch record is not an object"));
        };
        expected_envelope.remove("payload");
        if wire_envelope
            .iter()
            .filter(|(name, _)| name.as_str() != "payload")
            .ne(expected_envelope.iter())
        {
            return Err(ScrubError::new(
                "prepared body record envelope differs from its batch",
            ));
        }
        let payload = wire
            .get_mut("payload")
            .ok_or_else(|| ScrubError::new("prepared body record has no payload"))?;
        if payload.is_null() {
            continue;
        }
        let context = ScrubContext::new(surface)
            .source(&record.source)
            .session(record.session_id.as_deref());
        let scrubbed = scrub.scrub_json(payload, &context)?;
        if scrubbed != *payload {
            *payload = scrubbed;
            changed = true;
        }
    }
    if !changed {
        return Ok(body.to_owned());
    }
    serde_json::to_string(&value)
        .map_err(|_| ScrubError::new("scrubbed prepared body does not serialize"))
}

/// A body the scrub engine refused: the batch is blocked as an invalid
/// payload, its existing non-retryable failure, so neither the raw body nor
/// a retry of it goes anywhere.
fn refused(batch: &HistoryExportBatch, error: &ScrubError) -> ReceiverFailure {
    eprintln!(
        "delivery batch held batch_ref={} reason={error}",
        super::super::hash(batch.batch_id.as_bytes()),
    );
    DeliveryFailure::InvalidPayload.into()
}

/// A prepared body is immutable once it can have reached a receiver. A
/// newer scrubber may produce a replacement, but it must be persisted under
/// a new identity before any bytes leave this machine.
pub(super) fn scrub_for_upload(
    scrub: &ScrubEngine,
    payload: &PreparedPayload,
    batch: &HistoryExportBatch,
    remap: impl FnOnce(&HistoryExportBatch) -> Result<RetainedPayload, ReceiverFailure>,
) -> Result<Option<(HistoryExportBatch, RetainedPayload)>, ReceiverFailure> {
    if payload.sha256 != super::super::hash(&payload.body) {
        return Err(refused(
            batch,
            &ScrubError::new("prepared body digest mismatch"),
        ));
    }
    let body = scrubbed_body(scrub, &payload.body, batch, ScrubSurface::Upload)
        .map_err(|error| refused(batch, &error))?;
    if body == payload.body {
        return Ok(None);
    }
    if serde_json::from_str::<serde_json::Value>(&body).is_err() {
        let next = opaque_successor(scrub, &body, batch).map_err(|error| refused(batch, &error))?;
        let retained = remap(&next)?;
        return Ok(Some((next, retained)));
    }
    replacement(&body, payload, batch)
        .map(Some)
        .map_err(|error| refused(batch, &error))
}

/// An opaque mapping belongs to its receiver. Re-scrub the claimed records
/// and give the receiver the successor's identity before it maps new bytes.
fn opaque_successor(
    scrub: &ScrubEngine,
    body: &str,
    batch: &HistoryExportBatch,
) -> Result<HistoryExportBatch, ScrubError> {
    let invalid = || ScrubError::new("opaque successor batch does not serialize");
    let records = serde_json::to_string(batch).map_err(|_| invalid())?;
    let records = scrubbed_body(scrub, &records, batch, ScrubSurface::LocalStore)?;
    let records = scrubbed_body(scrub, &records, batch, ScrubSurface::Upload)?;
    let mut next: HistoryExportBatch = serde_json::from_str(&records).map_err(|_| invalid())?;
    if next.records.iter().zip(&batch.records).any(|(next, old)| {
        next.kind == "relationship"
            && next.payload.get("child_session_id") != old.payload.get("child_session_id")
    }) {
        return Err(invalid());
    }
    next.batch_id = super::super::hash(
        serde_json::to_vec(&(
            "delivery-opaque-scrub-replacement-v1",
            &batch.batch_id,
            super::super::hash(body),
            super::super::hash(&records),
        ))
        .map_err(|_| invalid())?,
    );
    Ok(next)
}

fn replacement(
    body: &str,
    payload: &PreparedPayload,
    batch: &HistoryExportBatch,
) -> Result<(HistoryExportBatch, RetainedPayload), ScrubError> {
    let invalid = || ScrubError::new("changed prepared body has no matching batch envelope");
    let mut value: serde_json::Value = serde_json::from_str(body).map_err(|_| invalid())?;
    let wire_batch = if value.get("batch").is_some() {
        value.get_mut("batch").ok_or_else(invalid)?
    } else {
        &mut value
    };
    let mut next: HistoryExportBatch =
        serde_json::from_value(wire_batch.clone()).map_err(|_| invalid())?;
    let mut envelope = next.clone();
    envelope.records.clone_from(&batch.records);
    if envelope != *batch
        || next.records.iter().zip(&batch.records).any(|(next, old)| {
            next.kind == "relationship"
                && next.payload.get("child_session_id") != old.payload.get("child_session_id")
        })
    {
        return Err(invalid());
    }
    // Key the replacement on the predecessor and the scrubbed bytes, so
    // retrying an interrupted local transition derives exactly the same id.
    next.batch_id = super::super::hash(
        serde_json::to_vec(&(
            "delivery-scrub-replacement-v1",
            &batch.batch_id,
            super::super::hash(body),
        ))
        .map_err(|_| invalid())?,
    );
    wire_batch["batch_id"] = serde_json::Value::String(next.batch_id.clone());
    let body = serde_json::to_string(&value).map_err(|_| invalid())?;
    let prepared =
        PreparedPayload::stored(&payload.mapping_version, &payload.content_type, &body, None);
    let retained = RetainedPayload::new(prepared).map_err(|_| invalid())?;
    Ok((next, retained))
}

/// Replace an unacknowledged frozen request before transport. The old id
/// never names the new bytes, and a restart claims the persisted successor.
pub(super) fn persist_successor(
    conn: &Connection,
    lease: &DeliveryLease,
    previous_sha256: &str,
    successor: &HistoryExportBatch,
    prepared: RetainedPayload,
    clock: &dyn Fn() -> i64,
) -> Result<()> {
    let tx = write_transaction(conn)?;
    check_lease(&tx, lease, clock(), LeaseCheck::Live)?;
    let job = job(&tx, &lease.job_id)?;
    let (current, stored) = pending_scope(&tx, lease)?.context("delivery batch missing")?;
    let current_prepared: PreparedPayload =
        serde_json::from_str(&stored.context("prepared body missing")?)?;
    ensure!(
        current_prepared.sha256 == previous_sha256
            && successor.batch_id != lease.batch_id
            && successor.job_id == lease.job_id
            && prepared.len() <= job.config.limits.max_prepared_bytes,
        "invalid prepared body replacement"
    );
    ensure!(
        Consent::new(&tx, &job.id, &job.config.selection)?.admits_all(&current.records)?,
        "delivery batch is now excluded"
    );
    let (journal_end, created_ms, accepted, suppressed, accepted_records): (
        i64, i64, String, String, i64,
    ) = tx.query_row(
        "SELECT journal_end,created_ms,accepted_revision_ids,suppressed_revision_ids,accepted_records
         FROM delivery_batches WHERE id=?",
        [&lease.batch_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
    )?;
    insert_batch(&tx, successor, journal_end, created_ms)?;
    take_claim_place(&tx, &successor.batch_id, &lease.batch_id)?;
    // Confirmed receipt progress transfers. An isolation frontier describes
    // rejections of the old bytes and must be reconsidered with the new body.
    tx.execute(
        "UPDATE delivery_batches SET prepared=?,accepted_revision_ids=?,suppressed_revision_ids=?,accepted_records=? WHERE id=?",
        params![prepared.text, accepted, suppressed, accepted_records, successor.batch_id],
    )?;
    tx.execute(
        "UPDATE delivery_batches SET state='suppressed',payload=NULL,prepared=NULL,
         accepted_revision_ids='[]',suppressed_revision_ids='[]',rejected_revision_ids='[]',isolation_frontier='[]',accepted_records=0
         WHERE id=?",
        [&lease.batch_id],
    )?;
    end_claim(&tx, lease)?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::delivery::HistoryExportRecord;
    use serde_json::json;

    const SESSION: &str = "bc-00000000-0000-0000-0000-0000000000aa";
    const GRANT: &str = "rhg_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const FAKE_KEY: &str = "sk-proj-ABCDEFGHIJKLMNOPQRSTUVWX1234567890abcd";

    struct Wire;
    impl Receiver for Wire {
        fn mapping_version(&self) -> &str {
            "1"
        }
        fn supported_kinds(&self) -> &[&str] {
            &["session_event"]
        }
        fn supports_tombstones(&self) -> bool {
            true
        }
        fn prepare(
            &self,
            batch: &HistoryExportBatch,
            _: &ReceiverContext<'_>,
        ) -> Result<PreparedBody, ReceiverFailure> {
            Ok(PreparedBody {
                content_type: "application/json".into(),
                body: json!({
                    "protocolVersion": 1,
                    "batch": batch,
                    "acquisition": [{
                        "source": "cursor",
                        "sessionId": SESSION,
                        "grantId": "00000000-0000-4000-8000-000000000001",
                        "ownerEpoch": "1",
                        "grant": GRANT,
                    }],
                })
                .to_string(),
            })
        }
        fn send(
            &self,
            _: &PreparedPayload,
            _: &HistoryExportBatch,
            _: &ReceiverContext<'_>,
        ) -> Result<DeliveryAcknowledgment, ReceiverFailure> {
            unreachable!()
        }
    }

    fn batch() -> HistoryExportBatch {
        HistoryExportBatch {
            schema_version: 1,
            origin_id: "origin".into(),
            batch_id: "batch".into(),
            job_id: "job".into(),
            generation: 1,
            destination_id: "relay".into(),
            instance_id: "default".into(),
            account_id: "relayhistory:synthetic".into(),
            mapping_version: "1".into(),
            records: vec![HistoryExportRecord {
                schema_version: 1,
                origin_id: "origin".into(),
                record_id: "record".into(),
                revision_id: "revision".into(),
                revision: 1,
                kind: "session_event".into(),
                source: "cursor".into(),
                session_id: Some(SESSION.into()),
                operation: "upsert".into(),
                payload: json!({"text": format!("OPENAI_API_KEY={FAKE_KEY}")}),
            }],
        }
    }

    fn assert_safe(body: &str) {
        let value: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(value["batch"]["records"][0]["session_id"], SESSION);
        assert_eq!(value["acquisition"][0]["sessionId"], SESSION);
        assert_eq!(value["acquisition"][0]["grant"], GRANT);
        assert!(crate::history::acquisition::valid_grant(
            value["acquisition"][0]["grant"].as_str().unwrap()
        ));
        let text = value["batch"]["records"][0]["payload"]["text"]
            .as_str()
            .unwrap();
        assert!(!text.contains(FAKE_KEY), "{text}");
        assert!(text.contains("[REDACTED]"), "{text}");
    }

    #[test]
    fn scrubber_upgrade_never_changes_a_frozen_batch_under_its_old_wire_id() {
        let mut batch = batch();
        batch.records[0].payload = json!({"text": "qualification-before-upgrade"});
        let body = json!({"protocolVersion": 1, "batch": batch}).to_string();
        let old = PreparedPayload::stored("1", "application/json", &body, Some("older-build"));
        let scrub = crate::history::scrub::testing::rewrite(
            "qualification-before-upgrade",
            "qualification-after-upgrade",
        );

        let (next, prepared) = scrub_for_upload(&scrub, &old, &batch, |_| unreachable!())
            .unwrap()
            .unwrap();
        let wire: serde_json::Value = serde_json::from_str(&prepared.payload.body).unwrap();
        assert_ne!(next.batch_id, batch.batch_id);
        assert_eq!(wire["batch"]["batch_id"], next.batch_id);
        assert_eq!(
            wire["batch"]["records"][0]["payload"]["text"],
            "qualification-after-upgrade"
        );
        assert_eq!(next.records[0].revision_id, batch.records[0].revision_id);
        let (retried, prepared_again) = scrub_for_upload(&scrub, &old, &batch, |_| unreachable!())
            .unwrap()
            .unwrap();
        assert_eq!(retried, next);
        assert_eq!(prepared_again.payload, prepared.payload);
    }

    #[test]
    fn a_corrupt_frozen_body_cannot_be_replaced_and_sent() {
        let batch = batch();
        let body = json!({"protocolVersion": 1, "batch": batch}).to_string();
        let mut old = PreparedPayload::stored("1", "application/json", &body, None);
        old.body.push(' ');
        assert_eq!(
            scrub_for_upload(
                &ScrubEngine::default_engine(),
                &old,
                &batch,
                |_| unreachable!()
            )
            .err()
            .unwrap()
            .failure,
            DeliveryFailure::InvalidPayload
        );
    }

    #[test]
    fn mapping_and_upload_scrub_content_without_changing_wire_identity_or_grant() {
        let batch = batch();
        let claim = ClaimedBatch {
            lease: DeliveryLease {
                job_id: "job".into(),
                batch_id: "batch".into(),
                worker_id: "worker".into(),
                fence: 1,
                expires_at_ms: 10,
            },
            batch: batch.clone(),
            prepared: None,
            accepted_revision_ids: vec![],
            suppressed_revision_ids: vec![],
            rejected_revision_ids: vec![],
            isolation_frontier: vec![],
            upload_key: None,
        };
        let context = ReceiverContext {
            cancelled: &|| false,
            timeout_ms: 1_000,
            idempotency_key: "batch",
            accepted_revision_ids: &[],
            suppressed_revision_ids: &[],
            rejected_revision_ids: &[],
            isolation_frontier: &[],
            upload_key: None,
        };
        let retained = map(
            &Wire,
            &claim,
            1_000_000,
            &ScrubEngine::default_engine(),
            &context,
        )
        .unwrap()
        .unwrap();
        assert_safe(&retained.payload.body);

        assert!(scrub_for_upload(
            &ScrubEngine::default_engine(),
            &retained.payload,
            &batch,
            |_| unreachable!()
        )
        .unwrap()
        .is_none());
    }
}
