use super::*;

#[test]
fn streamed_result_returns_before_the_http_connection_closes() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/fixed", listener.local_addr().unwrap());
    let (release, wait) = mpsc::sync_channel(1);
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_request(&mut stream);
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: TeXt/EvEnT-StReAm; charset=UTF-8\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive\r\n\r\n").unwrap();
        let response="\u{feff}: heartbeat\r\nevent: message\r\ndata: {\"jsonrpc\":\"2.0\",\r\ndata: \"id\":\"actual-request\",\"result\":{\"receipt\":\"streamed\"}}\r\n\r\n";
        for part in response.as_bytes().chunks(7) {
            write!(stream, "{:x}\r\n", part.len()).unwrap();
            stream.write_all(part).unwrap();
            stream.write_all(b"\r\n").unwrap();
            stream.flush().unwrap();
        }
        // No terminating chunk and no EOF until the client has already replied.
        wait.recv_timeout(Duration::from_secs(3)).unwrap();
    });
    let (result, reply) = mpsc::sync_channel(1);
    let client = std::thread::spawn(move || {
        result
            .send(post_at(&url, SECRET, &call(), Duration::from_secs(2)))
            .unwrap();
    });
    let actual = reply.recv_timeout(Duration::from_secs(1));
    release.send(()).unwrap();
    client.join().unwrap();
    server.join().unwrap();
    let actual = actual
        .expect("a completed SSE frame must return before server close")
        .unwrap();
    assert_eq!(actual["id"], "actual-request");
    assert_eq!(actual["result"]["receipt"], "streamed");
}

#[test]
fn sse_http_timeout_is_bounded_and_does_not_reconnect_or_repeat_post() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/fixed", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_request(&mut stream);
        seen.fetch_add(1, Ordering::SeqCst);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n: heartbeat\n\n")
            .unwrap();
        stream.flush().unwrap();
        std::thread::sleep(Duration::from_millis(250));
        listener.set_nonblocking(true).unwrap();
        while let Ok((_stream, _)) = listener.accept() {
            seen.fetch_add(1, Ordering::SeqCst);
        }
    });
    assert_eq!(
        post_at(&url, SECRET, &call(), Duration::from_millis(50)).unwrap_err(),
        IN_DOUBT
    );
    server.join().unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
}
