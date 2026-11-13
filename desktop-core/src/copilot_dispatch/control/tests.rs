use super::*;
use crate::copilot_dispatch::intent::{Owner, Peer};
use agent_relay_node::relay::process::ProcessInfo;
use serde_json::json;
use std::io::Write;

const TICKET: &str = "fixture-intent-only";
#[test]
fn completed_nonrecords_cannot_hide_a_manual_permission_requirement() {
    for invalid in [
        json!({"type":"permission.completed","data":null}),
        json!({"type":"permission.completed","data":{"requestId":"bad","result":null}}),
        json!(null),
        json!([]),
        json!({}),
        json!({"type":null}),
        json!({"type":42}),
        json!({"type":""}),
        json!({"type":"  "}),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        std::fs::write(&path, b"").unwrap();
        let call = call(&path);
        append(&path, &[pre(&call, "call-one"), invalid]);
        assert!(inspect(&path, &call, TICKET, now()).is_err());
    }
}

#[test]
fn malformed_permission_records_cannot_be_treated_as_no_prompt_policy() {
    for invalid in [
        json!({"type":"permission.requested","data":null}),
        json!({"type":"permission.requested","data":{"requestId":"bad","permissionRequest":null}}),
        json!({"type":"permission.requested","data":{"requestId":"bad","permissionRequest":{"kind":"mcp"}}}),
        json!({"type":"permission.requested","data":{"requestId":"bad","permissionRequest":{"kind":"mcp","toolCallId":42}}}),
        json!({"type":"permission.requested","data":{"permissionRequest":{"kind":"mcp","toolCallId":"other"}}}),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("events.jsonl");
        std::fs::write(&path, b"").unwrap();
        let call = call(&path);
        append(&path, &[pre(&call, "call-one"), invalid]);
        assert!(inspect(&path, &call, TICKET, now()).is_err());
    }
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    std::fs::write(&path, b"").unwrap();
    let call = call(&path);
    append(
        &path,
        &[
            pre(&call, "call-one"),
            json!({"type":"permission.requested","data":{"requestId":"foreign","permissionRequest":{"kind":"mcp","toolCallId":"other"}}}),
            json!({"type":"permission.requested","data":{"requestId":"shell","permissionRequest":{"kind":"shell"}}}),
        ],
    );
    assert!(matches!(
        inspect(&path, &call, TICKET, now()),
        Ok(Gate::Ready)
    ));
}
fn now() -> i64 {
    chrono::DateTime::parse_from_rfc3339("2026-11-07T05:26:45Z")
        .unwrap()
        .timestamp_millis()
}
fn call(path: &Path) -> Call {
    let journal = anchor(path).unwrap();
    Call {
        sid: "native-chat".into(),
        tool: "send_relay_message".into(),
        args: json!({"to":"peer","text":"fixture"}),
        owner: Owner {
            site: "https://example.test".into(),
            user: "u".into(),
            workspace: "w".into(),
            generation: 1,
            source: "s".into(),
            registration: "r".into(),
        },
        peer: Peer {
            runtime: ProcessInfo {
                pid: 1,
                parent: 0,
                uid: 501,
                start: 2,
            },
            hold: journal.file.clone(),
        },
        journal,
        authority: 1,
    }
}
fn args(call: &Call) -> Value {
    let mut v = call.args.clone();
    v[FIELD] = json!(TICKET);
    v
}
fn pre(call: &Call, id: &str) -> Value {
    json!({"type":"hook.start","data":{"hookType":"preMcpToolCall","input":{
        "sessionId":call.sid,"serverName":"agentrelay","toolName":call.tool,"toolCallId":id,"arguments":args(call)}}})
}
fn request(call: &Call) -> Value {
    json!({"type":"permission.requested","data":{"requestId":"approval","permissionRequest":{
        "kind":"mcp","toolCallId":"call-one","serverName":"agentrelay","toolName":format!("agentrelay-{}",call.tool),"args":args(call)}}})
}
fn completed(kind: &str) -> Value {
    json!({"type":"permission.completed","timestamp":"2026-11-07T05:26:45Z","data":{
        "requestId":"approval","toolCallId":"call-one","result":{"kind":kind}}})
}
fn append(path: &Path, values: &[Value]) {
    let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
    for v in values {
        writeln!(file, "{v}").unwrap();
    }
}
#[test]
fn outstanding_manual_approval_never_becomes_ready_by_waiting() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    std::fs::write(&path, b"").unwrap();
    let call = call(&path);
    append(&path, &[pre(&call, "call-one"), request(&call)]);
    assert!(matches!(
        inspect(&path, &call, TICKET, now() - 185_000),
        Ok(Gate::Waiting)
    ));
    assert!(matches!(
        inspect(&path, &call, TICKET, now()),
        Ok(Gate::Waiting)
    ));
    append(&path, &[completed("approved")]);
    assert!(matches!(
        inspect(&path, &call, TICKET, now()),
        Ok(Gate::Ready)
    ));
    assert!(inspect(&path, &call, TICKET, now() + 60_001).is_err());
}
#[test]
fn cancellation_before_mint_is_ignored_but_later_abort_invalidates() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    std::fs::write(&path, b"{\"type\":\"abort\"}\n").unwrap();
    let call = call(&path);
    append(&path, &[pre(&call, "call-one")]);
    assert!(matches!(
        inspect(&path, &call, TICKET, now()),
        Ok(Gate::Ready)
    ));
    append(&path, &[json!({"type":"abort"})]);
    assert!(inspect(&path, &call, TICKET, now()).is_err());
}
#[test]
fn ticket_cannot_be_borrowed_by_another_native_call_or_changed_arguments() {
    let call = Call {
        journal: Anchor {
            file: FileIdentity {
                device: 0,
                inode: 0,
                birth_ns: 0,
            },
            offset: 0,
        },
        ..{
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("events.jsonl");
            std::fs::write(&path, b"").unwrap();
            call(&path)
        }
    };
    let encode = |events: Vec<Value>| {
        events
            .into_iter()
            .map(|e| format!("{e}\n"))
            .collect::<String>()
            .into_bytes()
    };
    assert!(inspect_bytes(
        &encode(vec![pre(&call, "call-one"), pre(&call, "call-two")]),
        &call,
        TICKET,
        now()
    )
    .is_err());
    let mut wrong = pre(&call, "call-one");
    wrong["data"]["input"]["arguments"]["text"] = json!("different");
    assert!(inspect_bytes(&encode(vec![wrong]), &call, TICKET, now()).is_err());
    assert!(inspect_bytes(
        &encode(vec![
            pre(&call, "call-one"),
            request(&call),
            completed("denied")
        ]),
        &call,
        TICKET,
        now()
    )
    .is_err());
    assert!(inspect_bytes(b"{broken}\n", &call, TICKET, now()).is_err());
}
#[test]
fn replacing_or_truncating_control_file_cannot_authorize_dispatch() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    std::fs::write(&path, b"{}\n").unwrap();
    let call = call(&path);
    std::fs::write(&path, b"").unwrap();
    assert!(inspect(&path, &call, TICKET, now()).is_err());
    std::fs::rename(&path, temp.path().join("old")).unwrap();
    std::fs::write(&path, format!("{}\n", pre(&call, "call-one"))).unwrap();
    assert!(inspect(&path, &call, TICKET, now()).is_err());
}

#[test]
fn partially_flushed_permission_request_is_not_no_prompt_approval() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    std::fs::write(&path, b"").unwrap();
    let call = call(&path);
    let mut bytes = format!("{}\n", pre(&call, "call-one")).into_bytes();
    bytes.extend_from_slice(b"{\"type\":\"permission.requested\"");
    assert!(matches!(
        inspect_bytes(&bytes, &call, TICKET, now()),
        Ok(Gate::Waiting)
    ));
    append(
        &path,
        &[
            pre(&call, "call-one"),
            request(&call),
            completed("approved"),
        ],
    );
    assert!(matches!(
        inspect(&path, &call, TICKET, now()),
        Ok(Gate::Ready)
    ));
}

#[test]
fn hook_resolved_approval_correlates_by_request_id_without_inventing_call_id() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("events.jsonl");
    std::fs::write(&path, b"").unwrap();
    let call = call(&path);
    let mut requested = request(&call);
    requested["data"]["resolvedByHook"] = json!(true);
    let mut done = completed("approved");
    done["data"].as_object_mut().unwrap().remove("toolCallId");
    append(
        &path,
        &[pre(&call, "call-one"), requested.clone(), done.clone()],
    );
    assert!(matches!(
        inspect(&path, &call, TICKET, now()),
        Ok(Gate::Ready)
    ));
    done["data"]["toolCallId"] = json!("wrong-call");
    let bytes = [pre(&call, "call-one"), requested, done]
        .into_iter()
        .map(|e| format!("{e}\n"))
        .collect::<String>();
    assert!(inspect_bytes(bytes.as_bytes(), &call, TICKET, now()).is_err());
}

#[cfg(unix)]
#[test]
fn symlink_or_fifo_never_blocks_or_reads_as_control_input() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    std::fs::write(&target, b"{}\n").unwrap();
    let path = temp.path().join("events.jsonl");
    symlink(&target, &path).unwrap();
    assert!(anchor(&path).is_err());
    std::fs::remove_file(&path).unwrap();
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    // Only creates a disposable fixture; production opens with O_NONBLOCK.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert!(anchor(&path).is_err());
}
