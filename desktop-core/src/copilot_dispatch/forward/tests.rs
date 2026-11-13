use super::*;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
mod sse;

const SECRET: &str = "synthetic-private-proof-and-authorization";

fn call() -> Value {
    json!({"jsonrpc":"2.0","id":"actual-request","method":"tools/call",
        "params":{"arguments":{"_agent_relay_attestation":SECRET}}})
}

fn read_request(stream: &mut TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let count = stream.read(&mut buffer).unwrap();
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        assert!(bytes.len() < MAX_REQUEST_BYTES + 8192);
        if let Some(end) = bytes.windows(4).position(|x| x == b"\r\n\r\n") {
            let header = String::from_utf8_lossy(&bytes[..end]);
            let accept = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("accept").then_some(value.trim())
                })
                .unwrap();
            assert!(accept.contains("application/json") && accept.contains("text/event-stream"));
            let length: usize = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            if bytes.len() >= end + 4 + length {
                break;
            }
        }
    }
    assert!(bytes.starts_with(b"POST "));
}

fn fixture(
    status: u16,
    body: Vec<u8>,
    content_type: &str,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/fixed", listener.local_addr().unwrap());
    let content_type = content_type.to_owned();
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_request(&mut stream);
        let _ = write!(stream, "HTTP/1.1 {status} Fixture\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        let _ = stream.write_all(&body);
    });
    (url, worker)
}

#[test]
fn dropped_complete_post_is_never_retried() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/fixed", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_request(&mut stream);
        seen.fetch_add(1, Ordering::SeqCst);
        drop(stream);
        listener.set_nonblocking(true).unwrap();
        let until = Instant::now() + Duration::from_millis(250);
        while Instant::now() < until {
            if let Ok((stream, _)) = listener.accept() {
                seen.fetch_add(1, Ordering::SeqCst);
                drop(stream);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    let result = post_at(&url, SECRET, &call(), Duration::from_secs(2));
    worker.join().unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(result.unwrap_err(), IN_DOUBT);
}

#[test]
fn non_success_error_is_correlated_and_never_echoes_upstream_body() {
    for status in [400, 401, 403] {
        let body = json!({"jsonrpc":"2.0","id":null,"error":{"code":-32602,"message":SECRET}});
        let (url, worker) = fixture(status, body.to_string().into_bytes(), "application/json");
        let reply = post_at(&url, SECRET, &call(), Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        assert_eq!(reply["id"], "actual-request");
        assert!(reply.get("error").is_some());
        assert!(!reply.to_string().contains(SECRET));
    }
}

#[test]
fn redirect_is_not_followed_with_authorization() {
    let destination = TcpListener::bind("127.0.0.1:0").unwrap();
    destination.set_nonblocking(true).unwrap();
    let target = format!("http://{}/stolen", destination.local_addr().unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/fixed", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_request(&mut stream);
        write!(stream, "HTTP/1.1 307 Redirect\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    assert_eq!(
        post_at(&url, SECRET, &call(), Duration::from_secs(2)).unwrap_err(),
        IN_DOUBT
    );
    worker.join().unwrap();
    assert!(destination.accept().is_err());
}

#[test]
fn valid_json_result_and_original_id_are_preserved() {
    let reply =
        json!({"jsonrpc":"2.0","id":"actual-request","result":{"receipt":"fixture-receipt"}});
    let (url, worker) = fixture(
        200,
        reply.to_string().into_bytes(),
        "application/json; charset=utf-8",
    );
    assert_eq!(
        post_at(&url, SECRET, &call(), Duration::from_secs(2)).unwrap(),
        reply
    );
    worker.join().unwrap();
}

#[test]
fn malformed_or_uncorrelated_success_is_unknown_not_a_completed_receipt() {
    for reply in [
        json!({"jsonrpc":"2.0","id":"actual-request"}),
        json!({"jsonrpc":"2.0","id":"another-request","result":{}}),
        json!({"jsonrpc":"2.0","id":"actual-request","result":{},"error":{}}),
    ] {
        let (url, worker) = fixture(200, reply.to_string().into_bytes(), "application/json");
        assert_eq!(
            post_at(&url, SECRET, &call(), Duration::from_secs(2)).unwrap_err(),
            IN_DOUBT
        );
        worker.join().unwrap();
    }
}

#[test]
fn json_rpc_error_retains_id_and_code_but_not_message_or_data() {
    let reply = json!({"jsonrpc":"2.0","id":"actual-request","error":{"code":-32602,"message":SECRET,"data":SECRET}});
    let (url, worker) = fixture(200, reply.to_string().into_bytes(), "application/json");
    let actual = post_at(&url, SECRET, &call(), Duration::from_secs(2)).unwrap();
    worker.join().unwrap();
    assert_eq!(actual["id"], "actual-request");
    assert_eq!(actual["error"]["code"], -32602);
    assert!(!actual.to_string().contains(SECRET));
    assert!(actual["error"].get("data").is_none());
}

#[test]
fn notification_accepts_empty_202_but_request_does_not() {
    let (url, worker) = fixture(202, Vec::new(), "application/json");
    assert_eq!(
        post_at(
            &url,
            SECRET,
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            Duration::from_secs(2)
        )
        .unwrap(),
        Value::Null
    );
    worker.join().unwrap();
    let (url, worker) = fixture(202, Vec::new(), "application/json");
    assert_eq!(
        post_at(&url, SECRET, &call(), Duration::from_secs(2)).unwrap_err(),
        IN_DOUBT
    );
    worker.join().unwrap();
}

#[test]
fn oversized_request_is_refused_before_network_and_reply_is_bounded() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/fixed", listener.local_addr().unwrap());
    let oversized = json!({"id":1,"args":"x".repeat(MAX_REQUEST_BYTES)});
    assert_eq!(
        post_at(&url, SECRET, &oversized, Duration::from_secs(2)).unwrap_err(),
        "invalid_mcp_request"
    );
    assert!(listener.accept().is_err());
    let (url, worker) = fixture(
        200,
        vec![b'x'; MAX_REPLY_BYTES as usize + 1],
        "application/json",
    );
    assert_eq!(
        post_at(&url, SECRET, &call(), Duration::from_secs(2)).unwrap_err(),
        IN_DOUBT
    );
    worker.join().unwrap();
}

#[test]
fn stalled_body_is_bounded_without_a_second_post() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/fixed", listener.local_addr().unwrap());
    let worker = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_request(&mut stream);
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{").unwrap();
        std::thread::sleep(Duration::from_millis(250));
    });
    assert_eq!(
        post_at(&url, SECRET, &call(), Duration::from_millis(50)).unwrap_err(),
        IN_DOUBT
    );
    worker.join().unwrap();
}

#[test]
fn slow_dns_returns_while_lookup_is_blocked_and_never_posts_later() {
    let destination = TcpListener::bind("127.0.0.1:0").unwrap();
    destination.set_nonblocking(true).unwrap();
    let address = destination.local_addr().unwrap();
    let (entered, entered_rx) = mpsc::sync_channel(1);
    let (release, release_rx) = mpsc::sync_channel(1);
    let (finished, finished_rx) = mpsc::sync_channel(1);
    let (result, result_rx) = mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_millis(100);
        let lookup = std::sync::Mutex::new(Some((entered, release_rx, finished)));
        let resolver = move |host: &str| {
            let (entered, release_rx, finished) = lookup.lock().unwrap().take().unwrap();
            resolve_until(host, deadline, move |_| {
                entered.send(()).unwrap();
                release_rx.recv().unwrap();
                finished.send(()).unwrap();
                Ok(vec![address])
            })
        };
        let outcome = post_with_resolver(
            "http://offline.invalid/fixed",
            SECRET,
            &call(),
            deadline,
            resolver,
        );
        result.send(outcome).unwrap();
    });
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let outcome = result_rx.recv_timeout(Duration::from_secs(2));
    release.send(()).unwrap();
    finished_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    worker.join().unwrap();
    assert_eq!(outcome.unwrap().unwrap_err(), IN_DOUBT);
    assert!(destination.accept().is_err());
}

#[test]
fn invalid_remote_binding_is_refused_before_network() {
    let binding = Binding {
        site_url: "http://not-loopback.invalid".into(),
        workspace_id: "workspace".into(),
        account_id: "account".into(),
    };
    assert_eq!(
        post(&binding, SECRET, &call()).unwrap_err(),
        "hosted_endpoint_unavailable"
    );
}

#[test]
#[ignore = "explicit owned loopback fixture running the actual MCP JavaScript server transport"]
fn actual_mcp_sdk_initialize_over_json_http() {
    let endpoint = std::env::var("COPILOT_MCP_OFFLINE_FIXTURE_URL")
        .expect("owned local MCP fixture URL required");
    assert!(endpoint.starts_with("http://127.0.0.1:"));
    let binding = Binding {
        site_url: endpoint
            .strip_suffix("/cloud/api/v1/mcp/shared-sessions")
            .expect("fixed hosted MCP path required")
            .into(),
        workspace_id: "workspace".into(),
        account_id: "account".into(),
    };
    let request = json!({"jsonrpc":"2.0","id":"offline-initialize","method":"initialize",
        "params":{"protocolVersion":"2025-06-18","capabilities":{},
        "clientInfo":{"name":"offline-forward-fixture","version":"1"}}});
    let response = post_before(
        &binding,
        SECRET,
        &request,
        Instant::now() + Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(response["id"], "offline-initialize");
    assert_eq!(
        response["result"]["serverInfo"]["name"],
        "offline-copilot-forward-fixture"
    );
}

#[test]
fn expired_original_deadline_is_not_restarted_and_never_opens_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let binding = Binding {
        site_url: format!("http://{}", listener.local_addr().unwrap()),
        workspace_id: "workspace".into(),
        account_id: "account".into(),
    };
    let original_deadline = Instant::now() - Duration::from_secs(1);
    assert_eq!(
        post_before(&binding, SECRET, &call(), original_deadline).unwrap_err(),
        IN_DOUBT
    );
    assert!(listener.accept().is_err());
}
