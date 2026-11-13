//! How a send that got no answer is logged, and the time and cancellation
//! budget every receiver call of one send shares.

use super::*;

/// The one log line a request that got no response writes: the kind of
/// failure only. A transport error's text carries the request URL, so it is
/// never logged.
pub(super) fn transport_log(transport: &ureq::Transport) -> String {
    let source = std::error::Error::source(transport)
        .and_then(|source| source.downcast_ref::<std::io::Error>());
    format!(
        "receiver request failed before a response; retrying later cause={}",
        transport_cause(transport.kind(), source)
    )
}
/// The kind of a transport failure, as the log names it: `dns`, `connect`,
/// `tls`, `timeout`, `io`, `protocol` (a response ureq could not read as
/// HTTP) or `other`. A TLS failure or a timeout is named as such whichever
/// step it ended.
pub(super) fn transport_cause(
    kind: ureq::ErrorKind,
    source: Option<&std::io::Error>,
) -> &'static str {
    match source.map(io_cause) {
        Some(cause @ ("tls" | "timeout")) => cause,
        _ => match kind {
            ureq::ErrorKind::Dns => "dns",
            ureq::ErrorKind::ConnectionFailed | ureq::ErrorKind::ProxyConnect => "connect",
            ureq::ErrorKind::Io => "io",
            ureq::ErrorKind::BadStatus | ureq::ErrorKind::BadHeader => "protocol",
            _ => "other",
        },
    }
}
/// The kind of an I/O failure on the receiver's connection: `tls` when
/// rustls refused it, `timeout`, or `io`.
pub(super) fn io_cause(error: &std::io::Error) -> &'static str {
    if error
        .get_ref()
        .is_some_and(|inner| inner.is::<rustls::Error>())
    {
        "tls"
    } else if matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        "timeout"
    } else {
        "io"
    }
}
pub(super) struct CallBudget<'a> {
    pub(super) started: Instant,
    pub(super) timeout: Duration,
    pub(super) cancelled: &'a dyn Fn() -> bool,
    /// Whether the budget ending has been logged: it is checked many times
    /// per send, and logged once.
    pub(super) reported: std::sync::atomic::AtomicBool,
}
impl<'a> CallBudget<'a> {
    pub(super) fn new(context: &ReceiverContext<'a>) -> Self {
        Self {
            started: Instant::now(),
            timeout: Duration::from_millis(context.timeout_ms.max(0) as u64),
            cancelled: context.cancelled,
            reported: std::sync::atomic::AtomicBool::new(false),
        }
    }
    pub(super) fn remaining(&self) -> Result<Duration> {
        self.remaining_logging_to(std::io::stderr())
    }
    /// [`Self::remaining`], writing the budget's end to `log` the first time
    /// a check finds it ended.
    pub(super) fn remaining_logging_to(&self, mut log: impl std::io::Write) -> Result<Duration> {
        let remaining = self.timeout.saturating_sub(self.started.elapsed());
        let cancelled = (self.cancelled)();
        if (cancelled || remaining.is_zero())
            && !self
                .reported
                .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            let _ = writeln!(
                log,
                "receiver request ended by its budget; retrying later cause={}",
                if cancelled { "cancelled" } else { "timeout" }
            );
        }
        require(
            !cancelled && !remaining.is_zero(),
            DeliveryFailure::Transient,
        )?;
        Ok(remaining)
    }
}
