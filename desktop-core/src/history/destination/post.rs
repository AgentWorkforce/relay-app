//! The receiver's HTTP agents, and the one resend a request gets when a
//! kept-alive connection closes under it before any response.
use super::{cloud, encoding::WireBody, Result, MAX_IN_FLIGHT};
use crate::history::operator_policy::{pass_revision, REVISION_HEADER};
use std::{sync::LazyLock, time::Duration};

/// One HTTP agent for every receiver in the process, so consecutive batches
/// reuse a kept-alive connection instead of paying a TCP and TLS handshake
/// each. It holds no credentials: the bearer is set on each request, so a
/// rotated token takes effect on the next send. Redirects stay disabled, and
/// every request sets its own deadline from the worker's budget. A pooled
/// connection the server has since closed is detected before reuse and
/// replaced. A changed platform trust snapshot replaces the whole pool.
pub(super) fn http_agent() -> ureq::Agent {
    // Keep a connection for every batch a drain may have in flight at once,
    // rather than ureq's default of one per host, which would close all but
    // one after each round of concurrent sends.
    static AGENT: LazyLock<cloud::HistoryAgentPool> = LazyLock::new(|| {
        cloud::HistoryAgentPool::new(|builder| {
            builder
                .redirects(0)
                .max_idle_connections_per_host(MAX_IN_FLIGHT)
        })
    });
    AGENT.get()
}

/// An agent that never reuses a connection, for the one resend
/// [`post_batch`] makes after a connection closed under a request.
pub(super) fn fresh_agent() -> ureq::Agent {
    static AGENT: LazyLock<cloud::HistoryAgentPool> = LazyLock::new(|| {
        cloud::HistoryAgentPool::new(|builder| builder.redirects(0).max_idle_connections(0))
    });
    AGENT.get()
}

/// POST one prepared batch. A kept-alive connection the server closes just as
/// a request is written on it fails the request with a reset, or with the
/// connection ending before any response; ureq resends a POST only when the
/// close is seen before its body is written. The receiver answers an
/// identical batch id and revision with its original receipt, so the request
/// is sent once more, at once, on a new connection, while the budget lasts,
/// rather than failing the batch into the worker's backoff. A timeout is not
/// resent: the server may still be working on it.
///
/// The request names the operator policy revision of the current delivery
/// pass for `auth`'s workspace (see [`with_policy_revision`]).
pub(super) fn post_batch(
    url: &str,
    auth: &cloud::StoredAuth,
    content_type: &str,
    body: WireBody<'_>,
    timeout: Duration,
    remaining: &dyn Fn() -> Result<Duration>,
) -> Sent {
    // Captured once: the first send and the one resend below name the same
    // revision, the one this delivery pass decided eligibility under.
    let revision = auth.workspace_id.as_deref().and_then(pass_revision);
    let post = |agent: &ureq::Agent, timeout: Duration| {
        let request = agent
            .post(url)
            .timeout(timeout)
            .set("Authorization", &format!("Bearer {}", auth.access_token))
            .set("Content-Type", content_type);
        let request = with_policy_revision(request, revision);
        match body.encoding {
            Some(encoding) => request.set("Content-Encoding", encoding.name()),
            None => request,
        }
        .send_bytes(body.bytes)
        .map_err(Box::new)
    };
    resend_once(&[http_agent(), fresh_agent()], &post, timeout, remaining)
}

/// `request` naming the operator policy `revision` in
/// [`REVISION_HEADER`], when there is one. The ingest edge and Control
/// compare it with the organization's current revision.
pub(super) fn with_policy_revision(request: ureq::Request, revision: Option<i64>) -> ureq::Request {
    match revision {
        Some(revision) => request.set(REVISION_HEADER, &revision.to_string()),
        None => request,
    }
}

/// A request's response, or the error that ended it.
pub(super) type Sent = std::result::Result<ureq::Response, Box<ureq::Error>>;

/// Send with `agents[0]`, and once more with `agents[1]` while the budget
/// lasts when the connection closed before a response: see [`post_batch`].
/// Only for requests the server answers the same when they arrive twice.
pub(super) fn resend_once(
    agents: &[ureq::Agent; 2],
    send: &dyn Fn(&ureq::Agent, Duration) -> Sent,
    timeout: Duration,
    remaining: &dyn Fn() -> Result<Duration>,
) -> Sent {
    match send(&agents[0], timeout) {
        Err(error) if connection_closed(&error) => match remaining() {
            Ok(timeout) => {
                eprintln!("receiver connection closed before its response; resending the batch");
                send(&agents[1], timeout)
            }
            Err(_) => Err(error),
        },
        result => result,
    }
}

/// The context ureq attaches to a read error while it waits for a response's
/// status line, before any part of the response has been accepted.
const STATUS_LINE_READ: &str = "Error encountered in the status line";

/// Whether a request failed because its connection was reset or ended before
/// a response arrived, not because it timed out or could not connect.
///
/// Over plain TCP ureq reports a connection that ends before a status line as
/// `ConnectionAborted`. Over TLS, rustls reports a peer that closes without
/// `close_notify` (the usual idle keep-alive close) as `UnexpectedEof`, and
/// ureq passes it through with the context of the line it was reading; it
/// counts only while that line is the status line, so an end that cuts off
/// the headers of a response is not taken for a stale connection.
fn connection_closed(error: &ureq::Error) -> bool {
    let ureq::Error::Transport(transport) = error else {
        return false;
    };
    transport.kind() == ureq::ErrorKind::Io
        && std::error::Error::source(transport)
            .and_then(|source| source.downcast_ref::<std::io::Error>())
            .is_some_and(|io| match io.kind() {
                std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::BrokenPipe => true,
                std::io::ErrorKind::UnexpectedEof => {
                    io.get_ref()
                        .and_then(|inner| inner.downcast_ref::<ureq::Error>())
                        .and_then(|inner| match inner {
                            ureq::Error::Transport(transport) => transport.message(),
                            ureq::Error::Status(..) => None,
                        })
                        == Some(STATUS_LINE_READ)
                }
                _ => false,
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The TLS layer as rustls 0.23 presents it to ureq: a peer that closes
    /// the connection without `close_notify` reads as `UnexpectedEof`.
    #[derive(Debug)]
    struct EofWithoutCloseNotify(Box<dyn ureq::ReadWrite>);
    impl std::io::Read for EofWithoutCloseNotify {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.0.read(buf)? {
                0 if !buf.is_empty() => Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "peer closed connection without sending TLS close_notify",
                )),
                read => Ok(read),
            }
        }
    }
    impl std::io::Write for EofWithoutCloseNotify {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.0.flush()
        }
    }
    impl ureq::ReadWrite for EofWithoutCloseNotify {
        fn socket(&self) -> Option<&std::net::TcpStream> {
            self.0.socket()
        }
    }
    struct Tls;
    impl ureq::TlsConnector for Tls {
        fn connect(
            &self,
            _: &str,
            io: Box<dyn ureq::ReadWrite>,
        ) -> std::result::Result<Box<dyn ureq::ReadWrite>, ureq::Error> {
            Ok(Box::new(EofWithoutCloseNotify(io)))
        }
    }

    /// Answer each connection with the next of `replies` after reading its
    /// whole request, then close it. Returns the URL and the request count.
    fn serve_replies(
        replies: Vec<&'static str>,
    ) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{BufRead, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!(
            "https://{}/v1/delivery/batches",
            listener.local_addr().unwrap()
        );
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = requests.clone();
        std::thread::spawn(move || {
            for reply in replies {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = std::io::BufReader::new(stream);
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                reader.read_exact(&mut vec![0; length]).unwrap();
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                reader.get_mut().write_all(reply.as_bytes()).unwrap();
            }
        });
        (url, requests)
    }

    fn tls_agent(pool: bool) -> ureq::Agent {
        let builder = ureq::AgentBuilder::new()
            .redirects(0)
            .tls_connector(std::sync::Arc::new(Tls));
        if pool {
            builder
        } else {
            builder.max_idle_connections(0)
        }
        .build()
    }

    fn post_over_tls(url: &str) -> Sent {
        let post = |agent: &ureq::Agent, timeout: Duration| {
            agent
                .post(url)
                .timeout(timeout)
                .send_bytes(b"{}")
                .map_err(Box::new)
        };
        resend_once(
            &[tls_agent(true), tls_agent(false)],
            &post,
            Duration::from_secs(5),
            &|| Ok(Duration::from_secs(5)),
        )
    }

    #[test]
    fn a_tls_connection_ending_before_the_status_line_is_resent_once() {
        let ok = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
        let (url, requests) = serve_replies(vec!["", ok]);
        let response = post_over_tls(&url).unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn a_tls_connection_ending_inside_the_headers_is_not_resent() {
        let ok = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
        let (url, requests) = serve_replies(vec!["HTTP/1.1 200 OK\r\nContent-Le", ok]);
        let error = post_over_tls(&url).unwrap_err();
        assert!(!connection_closed(&error), "{error}");
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
