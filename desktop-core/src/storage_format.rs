//! Local upload-state format shared by migration witnesses and updater probes.
//! Advancing this value requires an upgrade path and release-transition tests;
//! it is independent of the application's release number.

pub const UPLOAD_QUEUE_FORMAT: u32 = 1;

/// Read-only capability advertisement. This must not open a store or core.
pub fn capabilities() -> String {
    format!("{{\"protocol\":1,\"upload_queue_formats\":[0,{UPLOAD_QUEUE_FORMAT}]}}\n")
}
