use super::*;
use serde_json::json;
use std::io::{self, Cursor};
use std::time::Duration;

fn reply() -> Value {
    json!({"jsonrpc":"2.0","id":"request","result":{"receipt":"fixture"}})
}
fn frame(value: &Value) -> String {
    format!("data: {value}\n\n")
}
fn parse(bytes: impl Read) -> Result<Value, &'static str> {
    receive(
        bytes,
        &json!("request"),
        Instant::now() + Duration::from_secs(2),
    )
}
struct Chunks {
    bytes: Cursor<Vec<u8>>,
    size: usize,
}
impl Read for Chunks {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let size = buf.len().min(self.size);
        self.bytes.read(&mut buf[..size])
    }
}

#[test]
fn utf8_bom_line_endings_and_multiline_frames_survive_every_chunk_boundary() {
    for ending in ["\n", "\r\n", "\r"] {
        let stream=format!("\u{feff}: keepalive{ending}id: ignored{ending}event: message{ending}retry: 100{ending}unknown: ignored{ending}data: {{\"jsonrpc\":\"2.0\",{ending}data: \"id\":\"request\",\"result\":{{\"text\":\"café ☕\"}}}}{ending}{ending}");
        for size in 1..=stream.len() {
            let result = parse(Chunks {
                bytes: Cursor::new(stream.as_bytes().to_vec()),
                size,
            })
            .unwrap();
            assert_eq!(
                result["result"]["text"], "café ☕",
                "ending{ending:?}chunk{size}"
            );
        }
    }
}
#[test]
fn notifications_and_valid_other_responses_never_complete_the_wrong_call() {
    let mut stream =
        frame(&json!({"jsonrpc":"2.0","method":"notifications/progress","params":{"progress":1}}));
    stream += &frame(&json!({"jsonrpc":"2.0","id":7,"result":{"receipt":"wrong"}}));
    stream +=
        &frame(&json!({"jsonrpc":"2.0","id":"other","error":{"code":-32602,"message":"foreign"}}));
    stream += &frame(&reply());
    assert_eq!(parse(Cursor::new(stream)).unwrap(), reply());
}

#[test]
fn custom_events_are_not_receipts_and_event_type_resets_at_every_blank_block() {
    let fake = json!({"jsonrpc":"2.0","id":"request","result":{"receipt":"fake-custom-event"}});
    let stream = format!(
        "event: custom\ndata: {fake}\n\nevent: custom\n\n{}",
        frame(&reply())
    );
    assert_eq!(parse(Cursor::new(stream)).unwrap(), reply());
    let stream = format!(
        "event: message\nevent: custom\ndata: {fake}\n\nevent:\n{}",
        frame(&reply())
    );
    assert_eq!(parse(Cursor::new(stream)).unwrap(), reply());
}
#[test]
fn matching_error_uses_the_existing_sanitizer() {
    let stream = frame(
        &json!({"jsonrpc":"2.0","id":"request","error":{"code":-32602,"message":"synthetic-secret","data":"synthetic-secret"}}),
    );
    let result = parse(Cursor::new(stream)).unwrap();
    assert_eq!(result["id"], "request");
    assert_eq!(result["error"]["code"], -32602);
    assert!(!result.to_string().contains("synthetic-secret"));
    assert!(result["error"].get("data").is_none());
}
#[test]
fn malformed_or_unsupported_messages_fail_closed_before_a_later_receipt() {
    for invalid in [
        "not json".to_owned(),
        "[]".into(),
        "null".into(),
        json!({"jsonrpc":"1.0","id":"request","result":{}}).to_string(),
        json!({"jsonrpc":"2.0","result":{}}).to_string(),
        json!({"jsonrpc":"2.0","id":"other"}).to_string(),
        json!({"jsonrpc":"2.0","id":"other","result":{},"error":{}}).to_string(),
        json!({"jsonrpc":"2.0","method":"notifications/progress","params":null}).to_string(),
        json!({"jsonrpc":"2.0","method":"sampling/createMessage","id":9,"params":{}}).to_string(),
    ] {
        let stream = format!("data: {invalid}\n\n{}", frame(&reply()));
        assert_eq!(parse(Cursor::new(stream)).unwrap_err(), IN_DOUBT);
    }
}
#[test]
fn empty_or_truncated_frames_do_not_turn_eof_into_a_receipt() {
    for stream in [
        String::new(),
        ": keepalive\n\n".into(),
        "data:\n\n".into(),
        format!("data: {}", reply()),
        format!("data: {}\n", reply()),
    ] {
        assert_eq!(parse(Cursor::new(stream)).unwrap_err(), IN_DOUBT);
    }
    let mut invalid = b"data: ".to_vec();
    invalid.push(0xff);
    invalid.extend_from_slice(b"\n\n");
    assert_eq!(parse(Cursor::new(invalid)).unwrap_err(), IN_DOUBT);
}
#[test]
fn cumulative_wire_cap_includes_comments_and_discarded_events() {
    let mut comments = vec![b':'; MAX_REPLY_BYTES as usize];
    comments.extend_from_slice(format!("\n\n{}", frame(&reply())).as_bytes());
    assert_eq!(parse(Cursor::new(comments)).unwrap_err(), IN_DOUBT);
    let mut events = String::new();
    let ignored = frame(&json!({"jsonrpc":"2.0","method":"notifications/progress"}));
    while events.len() <= MAX_REPLY_BYTES as usize {
        events += &ignored;
    }
    events += &frame(&reply());
    assert_eq!(parse(Cursor::new(events)).unwrap_err(), IN_DOUBT);
    let exact = frame(&reply());
    let prefix = format!(
        ":{}\n\n",
        "x".repeat(MAX_REPLY_BYTES as usize - exact.len() - 3)
    );
    assert_eq!(parse(Cursor::new(prefix + &exact)).unwrap(), reply());
}
#[test]
fn original_deadline_is_checked_before_reading_and_after_a_slow_read() {
    struct Slow;
    impl Read for Slow {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            std::thread::sleep(Duration::from_millis(30));
            let bytes = frame(&reply());
            out[..bytes.len()].copy_from_slice(bytes.as_bytes());
            Ok(bytes.len())
        }
    }
    assert_eq!(
        receive(
            Slow,
            &json!("request"),
            Instant::now() + Duration::from_millis(5)
        )
        .unwrap_err(),
        IN_DOUBT
    );
    struct NoRead;
    impl Read for NoRead {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            panic!("expired deadline must not read")
        }
    }
    assert_eq!(
        receive(
            NoRead,
            &json!("request"),
            Instant::now() - Duration::from_secs(1)
        )
        .unwrap_err(),
        IN_DOUBT
    );
}
