//! What a refused cloud reset means. A History service that does not serve
//! reset at all (a self-hosted session ingest) says so with a 501 and
//! `error.code` `unsupported_by_backend`; that answer never changes on a
//! retry, so it ends the reset instead of keeping uploads stopped.
use super::rejection::{error_body_bytes, parse_error_body};

/// What the person is told when the History service does not serve cloud
/// reset. The same sentence on macOS, Linux and Windows.
pub const RESET_UNSUPPORTED_MESSAGE: &str =
    "This history service does not support cloud reset. Uploads continue.";
const RESET_FAILED_MESSAGE: &str =
    "Cloud reset could not be completed. Uploads remain stopped; retry the reset.";

/// The History service does not serve cloud reset: a terminal answer.
#[derive(Debug)]
pub struct ResetUnsupported;
impl std::fmt::Display for ResetUnsupported {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(RESET_UNSUPPORTED_MESSAGE)
    }
}
impl std::error::Error for ResetUnsupported {}

/// Maps a refused reset request. Only a 501 whose JSON body carries
/// `error.code` `unsupported_by_backend` is terminal; `reason` is not read.
/// Every other refusal keeps the plan for a retry.
pub(super) fn reset_error(error: ureq::Error) -> anyhow::Error {
    let ureq::Error::Status(501, response) = error else {
        return anyhow::anyhow!(RESET_FAILED_MESSAGE);
    };
    let correlation = response
        .header("x-correlation-id")
        .filter(|id| id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .unwrap_or("none")
        .to_owned();
    let code = error_body_bytes(response)
        .and_then(|bytes| parse_error_body(&bytes))
        .and_then(|body| body["error"]["code"].as_str().map(str::to_owned));
    if code.as_deref() != Some("unsupported_by_backend") {
        return anyhow::anyhow!(RESET_FAILED_MESSAGE);
    }
    eprintln!("history service does not serve cloud reset status=501 correlation_id={correlation}");
    ResetUnsupported.into()
}

/// Whether a limits answer allows cloud reset. Hosted History does not
/// send `sessionReset`; only an explicit `false` turns reset off.
pub fn reset_allowed(limits: &serde_json::Value) -> bool {
    limits.get("sessionReset") != Some(&serde_json::Value::Bool(false))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The self-hosted ingest's answer, as the session pipeline serves it (PR #431, head 57585337).
    const UNSUPPORTED: &str = r#"{"error":{"code":"unsupported_by_backend","message":"this route is not served by the self-host session ingest","reason":"route_not_served_selfhost"},"correlationId":"6f1c2a9e-3b4d-4e5f-8a7b-9c0d1e2f3a4b"}"#;

    fn refused(status: u16, body: &str) -> ureq::Error {
        let response = format!(
            "HTTP/1.1 {status} Refused\r\ncontent-type: application/json\r\nx-correlation-id: 6f1c2a9e-3b4d-4e5f-8a7b-9c0d1e2f3a4b\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        ureq::Error::Status(status, response.parse::<ureq::Response>().unwrap())
    }

    #[test]
    fn a_501_unsupported_by_backend_reset_is_terminal() {
        let error = reset_error(refused(501, UNSUPPORTED));
        assert!(error.is::<ResetUnsupported>());
        assert_eq!(error.to_string(), RESET_UNSUPPORTED_MESSAGE);
        // `reason` may change; only the code decides.
        let other_reason = UNSUPPORTED.replace("route_not_served_selfhost", "something_else");
        assert!(reset_error(refused(501, &other_reason)).is::<ResetUnsupported>());
    }

    #[test]
    fn every_other_reset_refusal_keeps_the_retry_answer() {
        let other_code = UNSUPPORTED.replace("unsupported_by_backend", "not_implemented");
        for error in [
            refused(501, &other_code),
            refused(501, "not json"),
            refused(501, r#"{"error":"unsupported_by_backend"}"#),
            refused(503, UNSUPPORTED),
            refused(500, UNSUPPORTED),
        ] {
            let mapped = reset_error(error);
            assert!(!mapped.is::<ResetUnsupported>());
            assert_eq!(mapped.to_string(), RESET_FAILED_MESSAGE);
        }
    }

    #[test]
    fn only_an_explicit_false_turns_cloud_reset_off() {
        use serde_json::json;
        assert!(reset_allowed(&json!({"sessionDeletion": true})));
        assert!(reset_allowed(&json!({"sessionReset": true})));
        assert!(!reset_allowed(
            &json!({"sessionDeletion": false, "sessionReset": false})
        ));
    }
}
