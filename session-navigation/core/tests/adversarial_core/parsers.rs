use super::*;

// ============================================================== Parsers ===

/// STRENGTH: hostile registry bytes never panic and never yield a record.
/// Evidence: Claude facts F (torn files; first JSON value only).
#[test]
fn hostile_registry_inputs_are_rejected_without_panicking() {
    let deep = "[".repeat(100_000);
    let huge = vec![b' '; parse::claude::REGISTRY_LIMIT + 1];
    let inputs: Vec<Vec<u8>> = vec![
        deep.into_bytes(),
        huge,
        b"\0\0\0".to_vec(),
        br#"{"pid":43210,"pid":1,"sessionId":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","procStart":"Wed Sep 30 10:00:00 2026"}"#.to_vec(),
        br#"{"pid":"43210","sessionId":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","procStart":"Wed Sep 30 10:00:00 2026"}"#.to_vec(),
        br#"{"pid":43210,"sessionId":"-aaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","procStart":"Wed Sep 30 10:00:00 2026"}"#.to_vec(),
        br#"{"pid":43210,"sessionId":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","procStart":"Thu Sep 30 10:00:00 2026"}"#.to_vec(),
        br#"{"pid":4294967297,"sessionId":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","procStart":"Wed Sep 30 10:00:00 2026"}"#.to_vec(),
        br#"{"pid":43210,"sessionId":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","procStart":"Wed Sep 30 10:00:00 2026","hostSessionId":"local_../../x"}"#.to_vec(),
    ];
    for bytes in inputs {
        assert!(
            parse::claude::parse_registry(&bytes).is_err(),
            "{:?}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(80)])
        );
    }
}
