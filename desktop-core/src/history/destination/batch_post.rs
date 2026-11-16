//! A batch POST, its auth/encoding resends and complete validated receipt.
use super::*;

pub(super) fn send_request(
    stage: Stage<'_>,
    url: &str,
    content_type: &str,
    body: &str,
    batch: &HistoryExportBatch,
    budget: &CallBudget<'_>,
) -> Result<DeliveryAcknowledgment> {
    budget.remaining()?;
    let auth = stage.session()?;
    check_account(&auth, &batch.account_id)?;
    // Do not wait while holding a claimed batch: a busy POST slot or shared
    // Retry-After returns to the worker, which releases its lease and checks
    // consent again on the next claim. Hold the OS lock through the complete
    // response, auth-refresh resends and receipt validation.
    let mut post_gate = post_gate::PostGate::acquire(&auth)?;
    let stored = WireBody {
        bytes: body.as_bytes(),
        encoding: None,
    };
    // Each attempt, an auth-refresh retry included, sends in the coding the
    // receiver publishes at that moment: a coding the server refused earlier
    // in this send is never posted again. The compressed bytes are kept per
    // coding, so a retry in the same coding does not compress again.
    let compressed: Mutex<Option<(ContentEncoding, Option<Vec<u8>>)>> = Mutex::new(None);
    let encoded = |encoding: ContentEncoding| {
        let mut kept = compressed.lock().unwrap_or_else(|e| e.into_inner());
        if kept.as_ref().map(|(coding, _)| *coding) != Some(encoding) {
            *kept = Some((encoding, encoding.encode(body.as_bytes())));
        }
        kept.as_ref().and_then(|(_, bytes)| bytes.clone())
    };
    // A refused coding with no budget left to resend the stored bytes says
    // nothing about the batch, so it ends the send as the budget does.
    let refused_unresent = std::sync::atomic::AtomicBool::new(false);
    let response = send_authenticated(
        &stage,
        |current, timeout| {
            let timeout = timeout.expect("receiver budget supplies a timeout");
            let post = |wire: WireBody<'_>, timeout: Duration| {
                post_batch(
                    url,
                    current,
                    content_type,
                    wire,
                    timeout,
                    &|| budget.remaining(),
                )
            };
            let Some((encoding, bytes)) = stage
                .published
                .request_encoding()
                .and_then(|encoding| encoded(encoding).map(|bytes| (encoding, bytes)))
            else {
                return post(stored, timeout);
            };
            let wire = WireBody {
                bytes: &bytes,
                encoding: Some(encoding),
            };
            match post(wire, timeout) {
                // A server that no longer decodes the coding it published (a
                // rollback, or a proxy in front of it) refuses the body before
                // reading the batch. The stored bytes go once more as they
                // are, and later attempts and batches stay uncompressed until
                // a receipt publishes a coding again.
                Err(error) if matches!(*error, ureq::Error::Status(400 | 415, _)) => {
                    stage.published.publish_encoding(None);
                    match budget.remaining() {
                        Ok(timeout) => post(stored, timeout),
                        Err(_) => {
                            refused_unresent.store(true, std::sync::atomic::Ordering::SeqCst);
                            Err(error)
                        }
                    }
                }
                result => result,
            }
        },
        |error| http_error(error, body),
        |current| check_account(current, &batch.account_id),
        &|| budget.remaining(),
    );
    if let Err(error) = &response {
        if let Some(rejection) = error.downcast_ref::<ReceiverRejection>() {
            post_gate.defer_until(rejection.retry_after_ms, rejection.failure)?;
        }
    }
    let response = response.map_err(guard::retry_unresolved).map_err(|error| {
        if refused_unresent.load(std::sync::atomic::Ordering::SeqCst) {
            budget.remaining().err().unwrap_or(error)
        } else if failure_of(&error).is_some() {
            error
        } else if let Err(stopped) = budget.remaining() {
            stopped
        } else {
            fail(DeliveryFailure::AuthenticationRequired)
        }
    })?;
    budget.remaining()?;
    // Only a batch retries an unreadable answer, so only a batch logs it.
    let response = read_answer(response).map_err(AnswerFailure::logged)?;
    budget.remaining()?;
    let acknowledgment = receipt(&response, batch)?;
    // Each receipt says what the server accepts now; one that says nothing
    // is an older server, which accepts the protocol's defaults and no
    // request-body coding.
    stage.published.publish(published_limits(&response));
    stage.published.enrolled_from(response.get("limits"));
    stage
        .published
        .publish_encoding(response.get("limits").and_then(published_encoding));
    Ok(acknowledgment)
}
