//! The feed client against a local server whose refused answers never end
//! their bodies, or break them: the status is acted on without waiting for
//! a body no caller reads, and a `403` body gets its deadline and keeps
//! what arrived. The Mac's are in CloudFeedUpdaterTests.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, Instant};

use super::*;

const NOT_ENTITLED: &[u8] = br#"{"error":"not_entitled"}"#;

/// How a served body goes on after its first chunk.
#[derive(Clone, Copy)]
enum Then {
    /// A byte every 100 ms, within every read timeout, never ending.
    Trickle,
    /// A malformed chunk, then the connection closes.
    Break,
}

fn chunk(stream: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
    write!(stream, "{:x}\r\n", data.len())?;
    stream.write_all(data)?;
    stream.write_all(b"\r\n")?;
    stream.flush()
}

/// The URL of a server that answers one request with `status` and a
/// chunked body: `first`, then as `then` says.
fn serve(status: &'static str, first: &'static [u8], then: Then) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let address = listener.local_addr().expect("address");
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut request = Vec::new();
        let mut byte = [0; 1];
        while !request.ends_with(b"\r\n\r\n") {
            if stream.read(&mut byte).unwrap_or(0) == 0 {
                return;
            }
            request.push(byte[0]);
        }
        let head = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n"
        );
        if stream.write_all(head.as_bytes()).is_err() || chunk(&mut stream, first).is_err() {
            return;
        }
        match then {
            // Stops when the client is gone, or after a minute.
            Then::Trickle => {
                for _ in 0..600 {
                    thread::sleep(Duration::from_millis(100));
                    if chunk(&mut stream, b" ").is_err() {
                        return;
                    }
                }
            }
            Then::Break => {
                let _ = stream.write_all(b"zz\r\n");
                let _ = stream.flush();
            }
        }
    });
    format!("http://{address}/cloud/api/v1/desktop/feed/manifest?channel=stable")
}

fn feed_request(url: String) -> FeedRequest {
    FeedRequest {
        method: "GET",
        url,
        headers: BTreeMap::from([("Authorization".to_owned(), "Bearer 1".to_owned())]),
        body: None,
        max_body: 1024,
    }
}

/// `request_within` `url` with a half-second `403` deadline, and how long
/// it took.
fn timed(url: String) -> (Result<FeedResponse, FetchError>, Duration) {
    let started = Instant::now();
    let answer = request_within(&feed_request(url), Duration::from_millis(500));
    (answer, started.elapsed())
}

#[test]
fn an_unending_401_or_501_is_answered_without_its_body() {
    for status in ["401 Unauthorized", "501 Not Implemented", "404 Not Found"] {
        let (answer, took) = timed(serve(status, b"{}", Then::Trickle));
        let code: u16 = status[..3].parse().expect("code");
        assert_eq!(
            answer,
            Ok(FeedResponse {
                status: code,
                body: Vec::new()
            })
        );
        assert!(took < Duration::from_secs(2), "{status} took {took:?}");
    }
}

#[test]
fn a_broken_401_body_keeps_its_status() {
    let (answer, took) = timed(serve("401 Unauthorized", b"{}", Then::Break));
    assert_eq!(answer.map(|answer| answer.status), Ok(401));
    assert!(took < Duration::from_secs(2), "took {took:?}");
}

#[test]
fn an_unending_403_body_is_read_until_its_deadline_and_kept() {
    let (answer, took) = timed(serve("403 Forbidden", NOT_ENTITLED, Then::Trickle));
    let answer = answer.expect("an answer");
    assert_eq!(answer.status, 403);
    assert!(answer.body.starts_with(NOT_ENTITLED), "{:?}", answer.body);
    assert!(took >= Duration::from_millis(500), "took {took:?}");
    assert!(took < Duration::from_secs(2), "took {took:?}");
}

#[test]
fn a_broken_403_body_keeps_its_status_and_what_arrived() {
    let (answer, took) = timed(serve("403 Forbidden", NOT_ENTITLED, Then::Break));
    assert_eq!(
        answer,
        Ok(FeedResponse {
            status: 403,
            body: NOT_ENTITLED.to_vec()
        })
    );
    assert!(took < Duration::from_secs(2), "took {took:?}");
}
