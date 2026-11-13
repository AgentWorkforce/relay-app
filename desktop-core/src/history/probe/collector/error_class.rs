//! The stable, public class of a capture or delivery failure, and the
//! allowlisted sentence each class shows. Classes never carry raw provider
//! text, paths, bodies, credentials or session ids.
//!
//! `ai_hist::Error` variants that name one condition are classified by
//! variant: `SyncLocked` is `sync_locked` (another sync holds the store's
//! lock; never `database_busy`), `SessionNotFound`, `SessionSourceUnavailable`
//! and `SourceMismatch` are `source_unavailable`, `HydrationUnsupported` is
//! `hydration_failed`, `ConnectorNotConfigured` is `connector_not_configured`,
//! `AuthenticationExpired` is `connector_auth_expired`, `ConnectorFailure` (a
//! remote connector failing outright) is `connector_failure`,
//! `EvidencePartial` is `evidence_partial` and `Cancelled` is `cancelled`.
//! The catch-all variants the engine wraps SQLite and I/O failures in
//! (`HydrationFailed`, `SyncFailed`, `Discovery`, `DatabaseOpen`, `Query`)
//! are first read for the local storage conditions below, then fall back to
//! `hydration_failed`, `source_sync_failed` (a local sweep that did not
//! finish) or `database_error`.
use crate::history::{delivery, destination};

/// Classify typed causes into stable public labels without exposing raw error details.
pub(in crate::history::probe) fn capture_error_class(error: &anyhow::Error) -> &'static str {
    if let Some(class) = super::legacy_schedules::class(error) {
        return class;
    }
    for cause in error.chain() {
        if let Some(store) = cause.downcast_ref::<ai_hist::Error>() {
            return store_error_class(store);
        }
        if let Some(transport) = cause.downcast_ref::<destination::TransportFailure>() {
            if transport.0 == delivery::DeliveryFailure::DeliveryConflict {
                return "delivery_conflict";
            }
        }
        if let Some(sql) = cause.downcast_ref::<rusqlite::Error>() {
            return sqlite_class(sql);
        }
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            return match io.kind() {
                std::io::ErrorKind::NotFound => "source_missing",
                std::io::ErrorKind::PermissionDenied => "permission_denied",
                std::io::ErrorKind::StorageFull => "disk_full",
                _ => "io_error",
            };
        }
        if cause.downcast_ref::<serde_json::Error>().is_some() {
            return "invalid_source_json";
        }
    }
    if error
        .downcast_ref::<delivery::PreparationFailed>()
        .is_some()
    {
        return "upload_preparation_failed";
    }
    "capture_error"
}

/// Classify a typed SQLite failure.
fn sqlite_class(sql: &rusqlite::Error) -> &'static str {
    match sql.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            "database_busy"
        }
        Some(rusqlite::ErrorCode::DiskFull) => "disk_full",
        Some(rusqlite::ErrorCode::ReadOnly | rusqlite::ErrorCode::PermissionDenied) => {
            "permission_denied"
        }
        Some(rusqlite::ErrorCode::CannotOpen) => "database_unavailable",
        Some(rusqlite::ErrorCode::DatabaseCorrupt) => "database_corrupt",
        _ => "database_error",
    }
}

/// Classify a `SessionStore` failure: by variant where the variant names the
/// condition, and by the store's message only for the catch-all variants
/// that wrap a SQLite or I/O failure (see the module documentation).
pub(super) fn store_error_class(error: &ai_hist::Error) -> &'static str {
    use ai_hist::Error as E;
    match error {
        E::Cancelled(_) => "cancelled",
        E::SyncLocked { .. } => "sync_locked",
        E::SessionNotFound(_) | E::SessionSourceUnavailable(_) | E::SourceMismatch(_) => {
            "source_unavailable"
        }
        E::HydrationUnsupported(_) => "hydration_failed",
        E::ConnectorNotConfigured(_) => "connector_not_configured",
        E::AuthenticationExpired(_) => "connector_auth_expired",
        E::ConnectorFailure(_) => "connector_failure",
        E::EvidencePartial(_) => "evidence_partial",
        E::HydrationFailed(_) => local_storage_class(error).unwrap_or("hydration_failed"),
        E::SyncFailed(_) | E::Discovery(_) => {
            local_storage_class(error).unwrap_or("source_sync_failed")
        }
        E::DatabaseOpen(_) | E::Query(_) => local_storage_class(error).unwrap_or("database_error"),
        // StaleSchema, InvalidArgument, UnsupportedOperation,
        // SourceRevisionConflict, WatermarkAheadOfStore,
        // ConsumerKindsMismatch, InsufficientSpace, and any variant a later
        // ai-hist adds (the enum is #[non_exhaustive]).
        _ => local_storage_class(error).unwrap_or("capture_error"),
    }
}

/// The OS error numbers Windows appends to a full disk (`ERROR_DISK_FULL`)
/// and a denied access (`ERROR_ACCESS_DENIED`). The same numbers name other
/// errors elsewhere (5 is `EIO` on Unix), so they are read on Windows only.
#[cfg(windows)]
const OS_DISK_FULL: &[&str] = &["(os error 112)"];
#[cfg(windows)]
const OS_DENIED: &[&str] = &["(os error 5)"];
#[cfg(not(windows))]
const OS_DISK_FULL: &[&str] = &[];
#[cfg(not(windows))]
const OS_DENIED: &[&str] = &[];

/// The local storage condition a store message names. The store carries
/// its cause as a message rather than the typed SQLite or I/O error, so
/// these are recognized by SQLite's and the platform's own fixed wording.
fn local_storage_class(error: &ai_hist::Error) -> Option<&'static str> {
    let message = error.message();
    let says = |phrases: &[&str]| phrases.iter().any(|phrase| message.contains(phrase));
    if says(&["database is locked", "database table is locked"]) {
        Some("database_busy")
    } else if says(&[
        "database or disk is full",
        "No space left on device",
        "There is not enough space on the disk",
    ]) || says(OS_DISK_FULL)
    {
        Some("disk_full")
    } else if says(&["database disk image is malformed", "file is not a database"]) {
        Some("database_corrupt")
    } else if says(&[
        "attempt to write a readonly database",
        "access permission denied",
        "Permission denied",
        "Access is denied",
    ]) || says(OS_DENIED)
    {
        Some("permission_denied")
    } else if says(&["unable to open database file"]) {
        Some("database_unavailable")
    } else if says(&["No such file or directory"]) {
        Some("source_missing")
    } else {
        None
    }
}

/// Return actionable, allowlisted storage guidance suitable for desktop status and stderr.
pub(in crate::history::probe) fn local_failure_message(
    error: &anyhow::Error,
) -> Option<&'static str> {
    class_failure_message(capture_error_class(error))
}

/// The guidance for a class, so a verdict read back from a report renders the
/// same sentence as the error it came from.
fn class_failure_message(class: &str) -> Option<&'static str> {
    match class {
        "database_corrupt" => Some("The local history database needs repair. Preserve the database before recovery; reconnecting will not repair it."),
        "disk_full" => Some("The local disk is full. Free disk space to resume session uploads."),
        "database_busy" => Some("Local history is busy. Agent Relay will retry when the other operation finishes."),
        "database_unavailable" => Some("Agent Relay cannot open local history. Check that its directory exists and local file permissions allow access."),
        "permission_denied" => Some("Agent Relay cannot access local history. Check local file permissions."),
        "upload_preparation_failed" => Some("Agent Relay could not prepare local history for upload. It keeps retrying; update Agent Relay if uploads stay stuck."),
        "delivery_conflict" => Some("History delivery conflict: receiver-rejected records are quarantined or await validated conflict details. Inspect delivery status; other records remain queued."),
        "legacy_schedules_unknown" => Some(destination::LEGACY_SCHEDULES_UNKNOWN_MESSAGE),
        "sync_locked" => Some("Another Agent Relay process is syncing local history. Capture resumes when it finishes; nothing is lost."),
        "hydration_failed" => Some("A session could not be read in full; it is retried on the next pass. Other sessions keep uploading."),
        "source_unavailable" => Some("A selected session could not be read; it is skipped this pass and retried. Other sessions keep uploading."),
        "connector_not_configured" => Some("A remote session source is selected but its connector is not set up. Set up the connector again, or turn off its sessions; other sessions keep uploading."),
        "connector_auth_expired" => Some("A remote session connector's sign-in expired. Sign in to the connector again; other sessions keep uploading."),
        "connector_failure" => Some("A remote session connector failed; it is retried on the next pass. Other sessions keep uploading."),
        "source_sync_failed" => Some("Reading local session files did not finish; it is retried on the next pass. Queued data is preserved."),
        "evidence_partial" => Some("A remote session connector returned only part of a session; it is retried on the next pass. Other sessions keep uploading."),
        _ => None,
    }
}

/// The capture side's sentence for the general classes, which name no one
/// local condition. A delivery failure with these classes is usually the
/// network, so they are capture-only: the probe's own stderr and a delivery
/// verdict keep their generic wording.
fn capture_only_message(class: &str) -> Option<&'static str> {
    match class {
        "capture_error" => Some("Session capture hit a problem; it is retried on the next pass. Queued data is preserved; update Agent Relay if this continues."),
        "cancelled" => Some("Session capture was interrupted and resumes on the next pass. Nothing is lost."),
        "source_missing" => Some("A session file was moved or deleted before it could be read. Capture continues with the remaining sessions."),
        "io_error" => Some("Agent Relay could not read a local file; it is retried on the next pass. Queued data is preserved."),
        "invalid_source_json" => Some("A session file could not be parsed; it is skipped this pass and retried. Other sessions keep uploading."),
        "database_error" => Some("Local history reported an error; capture retries on the next pass. Queued data is preserved."),
        _ => None,
    }
}

/// What a delivery verdict with no sentence of its own says.
const DELIVERY_RETRYING: &str = "Sync paused or offline. Retrying; local data remains queued.";

/// The sentence for one side's class: `capture` is the capture side, whose
/// every class has its own sentence.
pub(super) fn class_message(class: Option<&str>, capture: bool) -> Option<String> {
    class.map(|class| {
        class_failure_message(class)
            .or_else(|| capture.then(|| capture_only_message(class)).flatten())
            .unwrap_or(DELIVERY_RETRYING)
            .to_owned()
    })
}
