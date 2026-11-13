//! Reading what RelayHistory answers: a batch's receipt, and what a
//! user-facing command (deletion check, upload selection, reset) shares, one
//! deadline and a bounded answer.

use super::{fail, require, DeliveryFailure, MAX_RESPONSE_BYTES};
use anyhow::Result;
use std::io::Read;
use std::time::{Duration, Instant};

/// The receipt in `response`. A success that is not 200, or a body that is not
/// JSON, is something between here and the receiver (a proxy, a captive
/// portal, an edge mid-deploy) and clears by itself, so it is retried; a JSON
/// receipt that fails validation is the receiver's and stops the batch.
///
/// Nothing is logged here: a batch logs its retried answer (see
/// [`AnswerFailure::logged`]); the limits fetch drops it.
pub(super) fn response_json(response: ureq::Response) -> Result<serde_json::Value> {
    read_answer(response).map_err(AnswerFailure::into_error)
}

/// [`response_json`], with why an answer could not be read kept apart.
pub(super) fn read_answer(response: ureq::Response) -> Result<serde_json::Value, AnswerFailure> {
    let status = response.status();
    let body = if status == 200 {
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    } else {
        Ok(Vec::new())
    };
    parse_answer(status, body)
}

/// What `status` and the `body` read for it make of an answer. A body is
/// only read for a 200.
fn parse_answer(
    status: u16,
    body: std::io::Result<Vec<u8>>,
) -> Result<serde_json::Value, AnswerFailure> {
    let unreadable = |cause| AnswerFailure::Unreadable { status, cause };
    if status != 200 {
        return Err(unreadable("unexpected_status"));
    }
    let bytes = body.map_err(|error| unreadable(super::io_cause(&error)))?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(AnswerFailure::Oversize);
    }
    serde_json::from_slice(&bytes).map_err(|_| unreadable("invalid_json"))
}

/// Why an answer is no receipt.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum AnswerFailure {
    /// Something between here and the receiver answered: retried. `cause`
    /// is `unexpected_status`, `invalid_json`, or the read's `tls`,
    /// `timeout` or `io`.
    Unreadable { status: u16, cause: &'static str },
    /// A body over `MAX_RESPONSE_BYTES`: the receiver's, not retried.
    Oversize,
}
impl AnswerFailure {
    /// The one log line a retried answer writes: its status and cause, never
    /// the body.
    pub(super) fn log_line(&self) -> Option<String> {
        match self {
            Self::Unreadable { status, cause } => Some(format!(
                "receiver answer unreadable; retrying later status={status} cause={cause}"
            )),
            Self::Oversize => None,
        }
    }
    pub(super) fn into_error(self) -> anyhow::Error {
        match self {
            Self::Unreadable { .. } => fail(DeliveryFailure::Transient),
            Self::Oversize => fail(DeliveryFailure::InvalidPayload),
        }
    }
    /// [`Self::into_error`], logging a retried answer first.
    pub(super) fn logged(self) -> anyhow::Error {
        if let Some(line) = self.log_line() {
            eprintln!("{line}");
        }
        self.into_error()
    }
}

/// What is left of a command's deadline: its auth refresh's lock and request
/// share the command's timeout.
pub(super) fn command_budget(timeout: Duration) -> impl Fn() -> Result<Duration> {
    let started = Instant::now();
    move || {
        let remaining = timeout.saturating_sub(started.elapsed());
        require(!remaining.is_zero(), DeliveryFailure::Transient)?;
        Ok(remaining)
    }
}

/// A command's JSON answer, bounded like a receipt: an oversize body is an
/// error of its own, never a truncated document.
pub(super) fn command_answer(response: ureq::Response) -> Result<serde_json::Value> {
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_RESPONSE_BYTES,
        "Cloud's answer was too large"
    );
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    /// An answer that is no receipt names its status and cause, and the log
    /// line carries nothing of the body, whatever the body holds.
    #[test]
    fn an_unreadable_answer_is_named_by_its_cause_and_never_logs_the_body() {
        let secret = br#"{"token":"secret-bearer"} <html>secret-portal</html>"#;
        let tls = || Error::new(ErrorKind::InvalidData, rustls::Error::General("x".into()));
        for (status, body, cause) in [
            (202, Ok(b"{}".to_vec()), "unexpected_status"),
            (204, Ok(Vec::new()), "unexpected_status"),
            (503, Ok(secret.to_vec()), "unexpected_status"),
            (200, Ok(secret[26..].to_vec()), "invalid_json"),
            (200, Ok(Vec::new()), "invalid_json"),
            (200, Err(Error::from(ErrorKind::ConnectionReset)), "io"),
            (200, Err(Error::from(ErrorKind::TimedOut)), "timeout"),
            (200, Err(tls()), "tls"),
        ] {
            let failure = parse_answer(status, body).unwrap_err();
            assert_eq!(failure, AnswerFailure::Unreadable { status, cause });
            let line = failure.log_line().unwrap();
            assert_eq!(
                line,
                format!("receiver answer unreadable; retrying later status={status} cause={cause}")
            );
            for private in ["secret", "token", "html", "portal"] {
                assert!(!line.contains(private), "{line}");
            }
            assert_eq!(
                super::super::failure_of(&failure.into_error()),
                Some(DeliveryFailure::Transient)
            );
        }
    }

    /// A receipt reads; an oversize body is the receiver's and is not
    /// retried, so it logs no retry.
    #[test]
    fn a_receipt_reads_and_an_oversize_answer_is_not_retried() {
        assert_eq!(
            parse_answer(200, Ok(br#"{"ok":true}"#.to_vec())).unwrap(),
            serde_json::json!({"ok": true})
        );
        let oversize =
            parse_answer(200, Ok(vec![b' '; MAX_RESPONSE_BYTES as usize + 1])).unwrap_err();
        assert_eq!(oversize, AnswerFailure::Oversize);
        assert_eq!(oversize.log_line(), None);
        assert_eq!(
            super::super::failure_of(&oversize.into_error()),
            Some(DeliveryFailure::InvalidPayload)
        );
    }
}
