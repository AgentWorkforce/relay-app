use agent_relay_node::relay::contract::account::SignInLink;
use agent_relay_node::relay::state::CaptureProgress;
use serde_json::Value;

/// The probe's `progress` event uses camelCase field names.
pub(super) fn capture_progress(progress: &Value) -> CaptureProgress {
    let count = |key: &str| progress[key].as_u64().unwrap_or(0);
    CaptureProgress {
        phase: progress["phase"].as_str().unwrap_or_default().into(),
        source: progress["source"].as_str().unwrap_or_default().into(),
        processed_files: count("processedFiles"),
        total_files: progress["totalFiles"].as_u64(),
        sessions_captured: count("sessionsCaptured"),
    }
}

/// The device-approval link in the probe's `approval` event.
pub(super) fn approval_link(event: &Value) -> SignInLink {
    let text = |key: &str| event[key].as_str().map(str::to_string);
    SignInLink {
        verification_url: text("verification_uri"),
        verification_url_complete: text("verification_uri_complete"),
        user_code: text("user_code"),
        expires_at: text("expires_at"),
        interval: event["interval"].as_u64(),
    }
}
