//! How long a failed delivery waits before its next attempt, and how an
//! explicit retry or an update shortens that wait.
use super::{
    apply_failure, check_lease, job, status, write_transaction, DeliveryFailure, DeliveryLease,
    DeliveryStatus, LeaseCheck,
};
use anyhow::{ensure, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

/// Longest wait after a transient failure (a 5xx, a non-conflict 409, a
/// transport or local error). A receiver that recovers is tried again within
/// this, plus jitter, however long it was down: an outage of hours used to
/// leave the uploader idle for up to the twenty-minute step it had reached.
/// It matches `SUSTAINED_FAILURE_MS`, so a failing upload is retried about as
/// often as the app re-evaluates whether it is stalled, and one request every
/// five minutes per computer is no load on a receiver that is down.
pub const TRANSIENT_BACKOFF_CEILING_MS: i64 = 5 * 60 * 1_000;
/// Longest wait after a rate limit (HTTP 408, 425 or 429) that named no wait
/// of its own. The receiver asked for less traffic, so the backoff keeps
/// growing to an hour. A receiver `Retry-After` replaces this backoff.
pub const RATE_LIMITED_BACKOFF_CEILING_MS: i64 = 60 * 60 * 1_000;

/// Another worker owns the claim. The core refuses every write under a lease
/// it has fenced, so this names the refusal apart from a rejected write.
#[derive(Debug)]
pub(super) struct LeaseFenced;
impl std::fmt::Display for LeaseFenced {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("delivery lease was fenced by another worker")
    }
}
impl std::error::Error for LeaseFenced {}

/// Whether a failed write is SQLite refusing it because another connection
/// holds the lock, rather than the write itself being rejected.
///
/// "I could not write just now" and "this write is not allowed" are the same
/// `Err` at the call site, and the delivery keepalive read the first as the
/// second: a renewal that merely lost a race for the database marked the claim
/// lost, discarding a send that had already succeeded.
pub(super) fn lost_a_race_for_the_database(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<rusqlite::Error>()
        .is_some_and(|error| {
            matches!(
                error,
                rusqlite::Error::SqliteFailure(failure, _)
                    if matches!(
                        failure.code,
                        rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                    )
            )
        })
}

/// What an acknowledgment that would not settle makes of the attempt.
///
/// Lock contention and a lost lease clear by themselves, so the batch retries,
/// and so does local storage failing (a full disk, an I/O error): that says
/// nothing about the acknowledgment. A corrupt database or the acknowledgment
/// or stored batch breaking an invariant is not transient. A resend reaches
/// the same verdict, so the job blocks at this batch, for a person to retry,
/// rather than hold the head of the line.
pub(super) fn settle_failure(error: &anyhow::Error) -> DeliveryFailure {
    if let Some(cause) = deferral_cause(error) {
        eprintln!("{}", deferral_log(cause));
        return DeliveryFailure::Transient;
    }
    if error.chain().any(local_storage_failed) {
        eprintln!("delivery acknowledgment deferred: {error:#}");
        return DeliveryFailure::Transient;
    }
    eprintln!("delivery acknowledgment refused: {error:#}");
    DeliveryFailure::InvalidPayload
}

/// Why an acknowledgment that would not settle is deferred without a word on
/// its storage: `lease_fenced` when another worker owns the claim,
/// `database_busy` when another connection held the lock. `None` for any
/// other failure.
fn deferral_cause(error: &anyhow::Error) -> Option<&'static str> {
    if error.is::<LeaseFenced>() {
        Some("lease_fenced")
    } else if lost_a_race_for_the_database(error) {
        Some("database_busy")
    } else {
        None
    }
}

/// The log line of a deferred acknowledgment: its cause, never the error's
/// text.
fn deferral_log(cause: &str) -> String {
    format!("delivery acknowledgment deferred: cause={cause}")
}

/// Whether `cause` is the local database or file system failing to write,
/// rather than the database being corrupt.
fn local_storage_failed(cause: &(dyn std::error::Error + 'static)) -> bool {
    match cause.downcast_ref::<rusqlite::Error>() {
        Some(rusqlite::Error::SqliteFailure(failure, _)) => !matches!(
            failure.code,
            rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
        ),
        _ => cause.is::<std::io::Error>(),
    }
}

/// Which failures wait out a backoff rather than block the job, and the
/// code each is stored and reported as.
impl DeliveryFailure {
    pub(crate) fn retryable(self) -> bool {
        matches!(
            self,
            Self::Transient | Self::RateLimited | Self::LegacyUploaderActive
        )
    }
    pub(super) fn code(self) -> &'static str {
        match self {
            Self::Transient => "transient",
            Self::RateLimited => "rate_limited",
            Self::AuthenticationRequired => "authentication_required",
            Self::PermissionDenied => "permission_denied",
            Self::InvalidPayload => "invalid_payload",
            Self::ReceiverRejected => "receiver_rejected",
            Self::UnsupportedEvidence => "unsupported_evidence",
            Self::MappingVersionMismatch => "mapping_version_mismatch",
            Self::DeliveryConflict => "delivery_conflict",
            Self::LegacyUploaderActive => "legacy_uploader_active",
        }
    }
}

/// The wait before the next attempt after `attempts` consecutive claims:
/// 1 s doubled per attempt up to the failure's ceiling, plus up to a fifth
/// of jitter, seeded by the batch so concurrent computers spread out.
pub(super) fn retry_backoff_ms(failure: DeliveryFailure, attempts: i64, seed: &str) -> i64 {
    let ceiling = match failure {
        DeliveryFailure::RateLimited => RATE_LIMITED_BACKOFF_CEILING_MS,
        _ => TRANSIENT_BACKOFF_CEILING_MS,
    };
    let backoff = 1_000_i64
        .saturating_mul(1_i64 << attempts.clamp(0, 12))
        .min(ceiling);
    let digest = Sha256::digest(format!("{seed}:{attempts}"));
    let jitter = (u16::from_be_bytes([digest[0], digest[1]]) as i64) * backoff / 327_675;
    backoff + jitter
}

/// When a failed attempt is tried next.
///
/// `Receiver` is a receiver's Retry-After, an absolute epoch-ms deadline: it
/// replaces the backoff, and restarts and Retry leave it standing. `Local` is
/// a fixed wait in ms that this computer chose for itself, such as a local TLS
/// configuration refusal: it replaces the backoff too, but the receiver never
/// asked for it, so Retry wakes it like any backoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureWait {
    Backoff,
    Receiver(i64),
    Local(i64),
}

impl FailureWait {
    pub(crate) fn of(retry_after_ms: Option<i64>) -> Self {
        retry_after_ms.map_or(Self::Backoff, Self::Receiver)
    }

    pub(super) fn receiver_directed(self) -> bool {
        matches!(self, Self::Receiver(_))
    }

    /// The epoch-ms deadline of the next attempt, dated from `now_ms`.
    pub(super) fn deadline(
        self,
        failure: DeliveryFailure,
        attempts: i64,
        seed: &str,
        now_ms: i64,
    ) -> i64 {
        match self {
            Self::Backoff => now_ms.saturating_add(retry_backoff_ms(failure, attempts, seed)),
            Self::Receiver(at_ms) => at_ms,
            Self::Local(wait_ms) => now_ms.saturating_add(wait_ms),
        }
    }
}

/// What a receiver asks the queue to keep of an object it stored for a
/// batch while its own decision on that object is pending: the object's
/// key, which names it and carries no authority, or nothing, so the next
/// send starts over. Never a credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredUpload {
    Pending(String),
    Forgotten,
}

/// The key of the object kept on `batch_id` (see [`StoredUpload`]), if any.
pub(super) fn upload_key(tx: &Connection, batch_id: &str) -> Result<Option<String>> {
    Ok(tx.query_row(
        "SELECT upload_key FROM delivery_batches WHERE id=?",
        [batch_id],
        |row| row.get(0),
    )?)
}

/// Record a receiver outcome, dating the retry from *inside* the write
/// transaction.
///
/// Ownership is fence-only here (`LeaseCheck::Owned`), so an expired but
/// unclaimed worker may still record its own outcome. That is precisely why
/// the clock must be read after the lock: acquiring it can block for as long
/// as the busy policy allows, and `apply_failure` schedules the next attempt
/// from `now_ms`. Dated from before the wait, a two-second backoff committed
/// after a thirty-second wait is already twenty-eight seconds overdue, and the
/// batch is claimable the moment the transaction commits — contention is the
/// one situation backoff exists for, and it was the one situation that
/// defeated it. `acknowledge` reads its clock the same way, because partial
/// and unsupported acceptance schedule a retry through the same path.
pub fn record_failure(
    conn: &Connection,
    lease: &DeliveryLease,
    failure: DeliveryFailure,
    retry_after_ms: Option<i64>,
    clock: &dyn Fn() -> i64,
) -> Result<DeliveryStatus> {
    let wait = FailureWait::of(retry_after_ms);
    record_failure_after(conn, lease, failure, wait, None, clock)
}

/// [`record_failure`] with the wait named outright; the clock is read
/// inside the write transaction for the reason given there. An `upload` the
/// receiver named is kept on (or cleared from) the batch in the same write;
/// `None` leaves what the batch holds.
pub(crate) fn record_failure_after(
    conn: &Connection,
    lease: &DeliveryLease,
    failure: DeliveryFailure,
    wait: FailureWait,
    upload: Option<&StoredUpload>,
    clock: &dyn Fn() -> i64,
) -> Result<DeliveryStatus> {
    let tx = write_transaction(conn)?;
    let now_ms = clock();
    check_lease(&tx, lease, now_ms, LeaseCheck::Owned)?;
    apply_failure(&tx, lease, failure, wait, now_ms)?;
    if let Some(upload) = upload {
        let key = match upload {
            StoredUpload::Pending(key) => Some(key.as_str()),
            StoredUpload::Forgotten => None,
        };
        tx.execute(
            "UPDATE delivery_batches SET upload_key=? WHERE id=?",
            params![key, &lease.batch_id],
        )?;
    }
    tx.commit()?;
    status(conn, &lease.job_id)
}

/// An explicit retry from the person: restart a blocked job, or wake an
/// active one waiting out a transient failure's backoff so a receiver that
/// has recovered is tried at once instead of after the rest of the wait.
/// Returns whether the job changed.
///
/// The check and the reset share one write transaction. A paused or
/// cancelled job is left as it is, and so is a job with a live lease: a
/// batch is in flight and its own outcome decides what happens next, so the
/// wake must not fence it. A receiver-directed wait stands, however long it
/// is. A client-generated rate-limit wait longer than
/// `TRANSIENT_BACKOFF_CEILING_MS` is woken: it is doubling backoff an older
/// build grew, and would otherwise idle uploads for up to an hour after the
/// contention passed. The wake keeps `attempts`, so a receiver that is still
/// down fails straight back to its ceiling rather than starting over.
pub fn retry_requested(conn: &Connection, job_id: &str, now_ms: i64) -> Result<bool> {
    let tx = write_transaction(conn)?;
    let (state, failure, next_attempt_ms, receiver_retry_after_ms, lease_until_ms): (
        String,
        Option<String>,
        i64,
        i64,
        Option<i64>,
    ) = tx
        .query_row(
            "SELECT state,failure,next_attempt_ms,receiver_retry_after_ms,lease_until_ms FROM delivery_jobs WHERE id=?",
            [job_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?
        .context("delivery job not found")?;
    let receiver_directed = receiver_retry_after_ms == next_attempt_ms;
    // An older uploader's hold is woken too: the person has just removed it.
    let wakes = !receiver_directed
        && match failure.as_deref() {
            Some(code)
                if code == DeliveryFailure::Transient.code()
                    || code == DeliveryFailure::LegacyUploaderActive.code() =>
            {
                next_attempt_ms > now_ms
            }
            Some(code) if code == DeliveryFailure::RateLimited.code() => {
                next_attempt_ms > now_ms.saturating_add(TRANSIENT_BACKOFF_CEILING_MS)
            }
            _ => false,
        };
    // A blocked job keeps its lease while a sibling batch of the window that
    // blocked it is in flight (`end_claim`); that batch's outcome lands first.
    let retried = (state == "blocked" || (state == "active" && wakes))
        && lease_until_ms.is_none_or(|until| until <= now_ms);
    if retried {
        reset_job_retry(&tx, job_id)?;
    }
    tx.commit()?;
    Ok(retried)
}

/// Bring a client-generated transient or rate-limited wait, whose backoff an
/// older build grew to an hour, within the transient ceiling. A receiver's
/// explicit Retry-After, a paused or blocked job and a live lease stand. The
/// collector does this as it starts (`retry_job_for_build`).
///
/// Builds before 2026.10.62 never honored a server `Retry-After`: every
/// rate limit's wait came from the attempts-based schedule, out to an hour,
/// and installing a build that honors it left that wait standing. The new
/// column defaults legacy rows to client-generated; all newly explicit
/// waits are marked and never shortened. `attempts` is kept, so a receiver
/// still answering 429 restores either its Retry-After or the full schedule.
pub fn clamp_retry_wait(conn: &Connection, job_id: &str, now_ms: i64) -> Result<bool> {
    let tx = write_transaction(conn)?;
    let changed = clamp_in(&tx, job_id, now_ms)?;
    tx.commit()?;
    Ok(changed)
}

fn clamp_in(conn: &Connection, job_id: &str, now_ms: i64) -> Result<bool> {
    let latest = now_ms.saturating_add(TRANSIENT_BACKOFF_CEILING_MS);
    Ok(conn.execute(
        "UPDATE delivery_jobs SET next_attempt_ms=?2 WHERE id=?1 AND state='active' AND failure IN ('transient','rate_limited') AND receiver_retry_after_ms<>next_attempt_ms AND next_attempt_ms>?2 AND COALESCE(lease_until_ms<=?3,1)",
        params![job_id, latest, now_ms],
    )? == 1)
}

pub fn retry_job(conn: &Connection, job_id: &str) -> Result<DeliveryStatus> {
    let tx = write_transaction(conn)?;
    reset_job_retry(&tx, job_id)?;
    tx.commit()?;
    status(conn, job_id)
}

/// Recorded among a job's builds once a `permission_denied` block from
/// before the older-uploader check was retried (`retry_job_for_build`).
const LEGACY_PERMISSION_RETRY: &str = "legacy-permission-denied-retry";

/// Retry a blocked verdict once per build, atomically recording the build with
/// the job transition. History retains every build, including after rollbacks.
/// A legacy file marker may be imported only when it explicitly names this job.
/// Active, paused and cancelled jobs record the build without changing state,
/// except that a long retry wait is clamped (`clamp_retry_wait`): the
/// collector runs this as it starts, which an update always does.
pub fn retry_job_for_build(
    conn: &Connection,
    job_id: &str,
    build: &str,
    legacy_build: Option<&str>,
) -> Result<()> {
    let tx = write_transaction(conn)?;
    let current = job(&tx, job_id)?;
    let recorded: Option<String> = tx.query_row(
        "SELECT retry_build FROM delivery_jobs WHERE id=?",
        [job_id],
        |row| row.get(0),
    )?;
    // Keep the former single-build column for older probes. Import it here as
    // well as during migration in case an older probe has run since migration.
    for previous in recorded.as_deref().into_iter().chain(legacy_build) {
        tx.execute(
            "INSERT OR IGNORE INTO delivery_job_builds(job_id,build) VALUES (?,?)",
            params![job_id, previous],
        )?;
    }
    let first_run = tx.execute(
        "INSERT OR IGNORE INTO delivery_job_builds(job_id,build) VALUES (?,?)",
        params![job_id, build],
    )? == 1;
    // Earlier builds blocked a job as `permission_denied` when one
    // `launchctl` or `crontab` read failed: the older-uploader check, which
    // is now retried. Nothing stored tells that apart from an account
    // mismatch or a server 403, so such a job is retried at a new build's
    // first start and once more at a later start, recorded with
    // `LEGACY_PERMISSION_RETRY` among its builds; a refusal that stands after
    // that stays blocked until a new build.
    let failure: Option<String> = tx.query_row(
        "SELECT failure FROM delivery_jobs WHERE id=?",
        [job_id],
        |row| row.get(0),
    )?;
    let refused = !first_run
        && current.state == "blocked"
        && failure.as_deref() == Some(DeliveryFailure::PermissionDenied.code())
        && tx.execute(
            "INSERT OR IGNORE INTO delivery_job_builds(job_id,build) VALUES (?,?)",
            params![job_id, LEGACY_PERMISSION_RETRY],
        )? == 1;
    if (first_run || refused) && current.state == "blocked" {
        reset_job_retry(&tx, job_id)?;
    }
    clamp_in(&tx, job_id, chrono::Utc::now().timestamp_millis())?;
    tx.execute(
        "UPDATE delivery_jobs SET retry_build=?1 WHERE id=?2 AND retry_build IS NOT ?1",
        params![build, job_id],
    )?;
    tx.commit()?;
    Ok(())
}

fn reset_job_retry(conn: &Connection, job_id: &str) -> Result<()> {
    ensure!(
        job(conn, job_id)?.state != "cancelled",
        "delivery job is cancelled"
    );
    conn.execute("UPDATE delivery_jobs SET state='active',failure=NULL,next_attempt_ms=0,receiver_retry_after_ms=0,fence=fence+1,worker_id=NULL,lease_until_ms=NULL WHERE id=?",[job_id])?;
    conn.execute("UPDATE delivery_batches SET state='pending' WHERE job_id=? AND state IN ('leased','blocked','retry_wait')",[job_id])?;
    // Provisional singleton rejections may all have come from a receiver-wide
    // or schema failure. An explicit/build retry re-tests them after the
    // receiver or probe changes; accepted and proven-suppressed progress stays.
    conn.execute(
        "UPDATE delivery_batches SET rejected_revision_ids='[]',isolation_frontier='[]' WHERE job_id=?",
        [job_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    fn sqlite(code: i32) -> anyhow::Error {
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None).into()
    }

    /// A deferred acknowledgment is logged by its cause: a fenced lease or
    /// a lost race for the database. Anything else is no deferral of this
    /// kind, and the line never carries the error's own text.
    #[test]
    fn a_deferred_acknowledgment_is_named_by_its_cause() {
        let secret = || {
            rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                Some("secret-session /private/path".into()),
            )
        };
        for (error, cause) in [
            (anyhow::Error::new(LeaseFenced), Some("lease_fenced")),
            (
                anyhow::Error::new(LeaseFenced).context("settle secret-session"),
                Some("lease_fenced"),
            ),
            (sqlite(rusqlite::ffi::SQLITE_BUSY), Some("database_busy")),
            (sqlite(rusqlite::ffi::SQLITE_LOCKED), Some("database_busy")),
            (anyhow::Error::from(secret()), Some("database_busy")),
            (sqlite(rusqlite::ffi::SQLITE_FULL), None),
            (sqlite(rusqlite::ffi::SQLITE_CORRUPT), None),
            (anyhow!("secret-session"), None),
        ] {
            assert_eq!(deferral_cause(&error), cause, "{error:#}");
            if let Some(cause) = cause {
                assert_eq!(settle_failure(&error), DeliveryFailure::Transient);
                let line = deferral_log(cause);
                assert_eq!(
                    line,
                    format!("delivery acknowledgment deferred: cause={cause}")
                );
                for private in ["secret", "private", "fenced by"] {
                    assert!(!line.contains(private), "{line}");
                }
            }
        }
    }

    #[test]
    fn local_failures_retry_a_refused_acknowledgment_and_invariants_block_it() {
        for retried in [
            sqlite(rusqlite::ffi::SQLITE_BUSY),
            sqlite(rusqlite::ffi::SQLITE_LOCKED),
            LeaseFenced.into(),
            sqlite(rusqlite::ffi::SQLITE_FULL),
            sqlite(rusqlite::ffi::SQLITE_IOERR),
            sqlite(rusqlite::ffi::SQLITE_INTERRUPT),
            sqlite(rusqlite::ffi::SQLITE_FULL).context("commit settlement"),
            std::io::Error::other("disk unavailable").into(),
        ] {
            assert_eq!(settle_failure(&retried), DeliveryFailure::Transient);
        }
        let mismatch = || -> Result<()> {
            ensure!(false, "acknowledgment batch mismatch");
            Ok(())
        };
        for blocked in [
            anyhow!("invalid acknowledgment revision"),
            mismatch().unwrap_err(),
            serde_json::from_str::<u8>("{").unwrap_err().into(),
            sqlite(rusqlite::ffi::SQLITE_CORRUPT),
            sqlite(rusqlite::ffi::SQLITE_NOTADB),
        ] {
            assert_eq!(settle_failure(&blocked), DeliveryFailure::InvalidPayload);
        }
    }
}
