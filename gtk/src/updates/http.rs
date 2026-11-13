//! The updater's one HTTP client: a GET of a public URL ([`fetch`]) of at
//! most [`MAX_DOWNLOAD`] bytes and a request with headers that reports
//! every status ([`request`]) within the request's own budget, both with
//! [`TIMEOUT`] for connecting and for each read. Bodies are counted as they
//! arrive, whatever their `Content-Length`, and dropped as soon as they
//! pass their budget. A refused answer's body is not read, except a `403`'s
//! (for `not_entitled`), which gets [`REFUSAL_BODY_DEADLINE`] in all.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use super::{FetchError, MAX_DOWNLOAD, TIMEOUT};

/// A request to the site's release feed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedRequest {
    /// `GET` or `POST`.
    pub method: &'static str,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: Option<Vec<u8>>,
    /// The most a `200` answer's body may be: a small budget for a manifest
    /// or `download-url` answer, the asset's signed size for a download.
    pub max_body: u64,
}

/// What the feed answered: any status, with its body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Sends a [`FeedRequest`]; an HTTP error status is an answer, not an error.
pub type Request<'a> = dyn Fn(&FeedRequest) -> Result<FeedResponse, FetchError> + 'a;

fn agent(redirects: u32) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(TIMEOUT)
        .timeout_read(TIMEOUT)
        .redirects(redirects)
        .user_agent(&format!("AgentRelay/{}", agent_relay_node::VERSION))
        .build()
}

/// The most of a refused answer's body that is kept, for `not_entitled`.
const MAX_REFUSAL_BODY: u64 = 64 * 1024;

/// The longest a `403` body is read for, however slowly it arrives. The
/// Mac's is `CloudFeed.refusalBodyDeadline`.
pub const REFUSAL_BODY_DEADLINE: Duration = Duration::from_secs(10);

/// At most `limit` bytes of `reader`, cut there when `truncating`, else
/// refused once one more byte arrives; nothing past that byte is read.
fn read_at_most(reader: impl Read, limit: u64, truncating: bool) -> Result<Vec<u8>, FetchError> {
    let mut data = Vec::new();
    reader
        .take(limit + 1)
        .read_to_end(&mut data)
        .map_err(|error| FetchError::Unreachable(error.to_string()))?;
    if data.len() as u64 > limit {
        if !truncating {
            return Err(FetchError::TooLarge);
        }
        data.truncate(limit as usize);
    }
    Ok(data)
}

/// The body of a `status` answer read from `reader`: a `200` body of at
/// most `max_body` bytes; a `403` body's first [`MAX_REFUSAL_BODY`] bytes
/// that arrive within `deadline`, for `not_entitled`; and nothing of any
/// other status, whose body no caller reads, so a `401` is retried and a
/// `501` streamed as soon as it is answered, however slow its body.
pub(super) fn answer_body(
    status: u16,
    reader: impl Read + Send + 'static,
    max_body: u64,
    deadline: Duration,
) -> Result<Vec<u8>, FetchError> {
    match status {
        200 => read_at_most(reader, max_body.min(MAX_DOWNLOAD), false),
        403 => Ok(refusal_body(reader, deadline)),
        _ => Ok(Vec::new()),
    }
}

/// What of a refused answer's body arrives within `deadline`, up to
/// [`MAX_REFUSAL_BODY`] bytes. The body is only informative: one that is
/// slow, unfinished or fails keeps what arrived, and its status stands.
/// The reading thread stops at its next read once this has returned.
fn refusal_body(reader: impl Read + Send + 'static, deadline: Duration) -> Vec<u8> {
    let end = Instant::now() + deadline;
    let (sender, receiver) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        let mut reader = reader.take(MAX_REFUSAL_BODY);
        let mut buffer = [0; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => return,
                Ok(read) => {
                    if sender.send(buffer[..read].to_vec()).is_err() || Instant::now() >= end {
                        return;
                    }
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => return,
            }
        }
    });
    let mut data = Vec::new();
    // Until the body ends (the sender is dropped) or the deadline passes.
    while let Ok(chunk) = receiver.recv_timeout(end.saturating_duration_since(Instant::now())) {
        data.extend_from_slice(&chunk);
    }
    data
}

/// A GET of `url`'s body, following redirects.
pub fn fetch(url: &str) -> Result<Vec<u8>, FetchError> {
    let response = agent(5)
        .get(url)
        .set("Accept", "*/*")
        .call()
        .map_err(|error| FetchError::Unreachable(error.to_string()))?;
    read_at_most(response.into_reader(), MAX_DOWNLOAD, false)
}

/// Sends `request` without following redirects, so its credentials reach
/// only the URL it names; a redirect comes back as its own status.
pub fn request(request: &FeedRequest) -> Result<FeedResponse, FetchError> {
    request_within(request, REFUSAL_BODY_DEADLINE)
}

/// [`request`], reading a `403` body for at most `deadline`.
fn request_within(request: &FeedRequest, deadline: Duration) -> Result<FeedResponse, FetchError> {
    let mut call = agent(0).request(request.method, &request.url);
    for (name, value) in &request.headers {
        call = call.set(name, value);
    }
    let answer = match &request.body {
        Some(data) => call.send_bytes(data),
        None => call.call(),
    };
    let response = match answer {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(error) => return Err(FetchError::Unreachable(error.to_string())),
    };
    let status = response.status();
    if status == 200
        && response
            .header("Content-Length")
            .and_then(|length| length.parse::<u64>().ok())
            .is_some_and(|length| length > request.max_body.min(MAX_DOWNLOAD))
    {
        return Err(FetchError::TooLarge);
    }
    Ok(FeedResponse {
        status,
        body: answer_body(status, response.into_reader(), request.max_body, deadline)?,
    })
}
