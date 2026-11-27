//! The flush's delivery: its generation, and the drain until the queue is
//! empty or the deadline passes.
use super::*;

/// Wall time of one delivery pass. Debug builds take an override, so a test
/// can span several passes without waiting out the release budget.
fn pass_budget() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var("AGENT_RELAY_PROBE_FLUSH_PASS_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|ms| *ms > 0)
    {
        return Duration::from_millis(ms);
    }
    collector::DRAIN_TIME_BUDGET
}

/// The flush's delivery generation, sharing everything: an earlier flush's
/// in the same state directory is resumed, one for another destination is a
/// conflict.
pub(super) fn job(db: &Path, account: &str) -> Result<String> {
    let conn = delivery::open_db(db)?;
    let config = delivery::DeliveryJobConfig {
        destination_id: collector::DESTINATION.into(),
        instance_id: collector::INSTANCE.into(),
        account_id: account.into(),
        mapping_version: destination::MAPPING_VERSION.into(),
        selection: collector::selection(true),
        limits: Default::default(),
    };
    let jobs: Vec<_> = delivery::list_jobs(&conn)?
        .into_iter()
        .filter(|job| job.state != "cancelled")
        .collect();
    if let Some(job) = jobs.iter().find(|job| job.config == config) {
        if job.state == "blocked" {
            delivery::retry_job(&conn, &job.job_id)?;
        }
        return Ok(job.job_id.clone());
    }
    if !jobs.is_empty() {
        return Err(refused(
            Exit::Blocked,
            "The state directory holds another destination's delivery.",
        ));
    }
    collector::record_baseline(&conn, true)?;
    Ok(delivery::create_job(&conn, &config, collector::now())?.job_id)
}

/// Drain `job_id` through `receiver` until its queue is empty (`Drained`)
/// or `deadline` passes (`Deadline`).
pub(super) fn drain(
    db: &Path,
    receiver: &destination::RelayHistoryReceiver,
    job_id: &str,
    worker_id: &str,
    deadline: Instant,
    counts: &Counts,
) -> Result<Exit> {
    let expired = move || Instant::now() >= deadline;
    let receivers =
        worker::SingleReceiver::new(collector::DESTINATION, collector::INSTANCE, receiver);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(Exit::Deadline);
        }
        let result = worker::drain(
            db,
            &receivers,
            &collector::deadline_options(job_id.into(), worker_id.into(), pass_budget(), deadline),
            &worker::system_clock,
            &expired,
        )?;
        let status = result
            .statuses
            .iter()
            .find(|status| status.job_id == job_id)
            .context("delivery job missing")?;
        counts.of(status);
        match status.state.as_str() {
            "active" => {}
            "blocked" if status.failure.as_deref() == Some("authentication_required") => {
                return Err(refused(
                    Exit::Unauthorized,
                    "RelayHistory refused the session.",
                ));
            }
            _ => {
                return Err(refused(
                    Exit::Blocked,
                    "Delivery is blocked. No unacknowledged data was discarded.",
                ))
            }
        }
        if let Some(failure) = result.failure {
            return Err(failure.into_error());
        }
        if !result.issues.is_empty() {
            return Err(refused(
                Exit::Failed,
                "Session delivery needs attention. No unacknowledged data was discarded.",
            ));
        }
        if status.bootstrap_complete && status.pending_records == 0 && status.unqueued_changes == 0
        {
            return Ok(Exit::Drained);
        }
        // A pass that moved nothing waits for the worker's retry deadline,
        // never past the flush's own.
        let wait = if status.next_attempt_ms > collector::now() {
            Duration::from_millis((status.next_attempt_ms - collector::now()) as u64)
        } else if result.metrics.batches_sent == 0 {
            Duration::from_millis(100)
        } else {
            Duration::ZERO
        };
        std::thread::sleep(wait.min(deadline.saturating_duration_since(Instant::now())));
    }
}
