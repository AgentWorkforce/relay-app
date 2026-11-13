//! App-owned stdio framing. No token, key, proof or HTTP response header
//! crosses this process; only the final MCP response leaves Core.
use super::forward::error;
use crate::mcp_settings::Binding;
use agent_relay_node::relay::cli::{session_hook_socket, socket_response};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::{BufRead, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const FRAME: u64 = 1 << 20;
type Dispatch = Arc<dyn Fn(&Value) -> Value + Send + Sync>;

fn binding(args: &[String]) -> Option<(Binding, bool)> {
    let mut site = None;
    let mut workspace = None;
    let mut account = None;
    let mut dev = false;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--dev" {
            if dev {
                return None;
            }
            dev = true;
            i += 1;
            continue;
        }
        let value = args.get(i + 1)?.clone();
        let field = match args[i].as_str() {
            "--site" => &mut site,
            "--workspace" => &mut workspace,
            "--account" => &mut account,
            _ => return None,
        };
        if field.replace(value).is_some() {
            return None;
        }
        i += 2;
    }
    let binding = Binding {
        site_url: site?,
        workspace_id: workspace?,
        account_id: account?,
    };
    binding.endpoint()?;
    Some((binding, dev))
}

fn identifier(request: &Value) -> Option<String> {
    match request.get("id")? {
        Value::String(s) if !s.is_empty() && s.len() <= 128 => Some(format!("s:{s}")),
        Value::Number(n) if n.is_i64() || n.is_u64() => Some(format!("n:{n}")),
        _ => None,
    }
}

fn write(output: &mut impl Write, value: &Value) -> Result<(), ()> {
    serde_json::to_writer(&mut *output, value).map_err(|_| ())?;
    output
        .write_all(b"\n")
        .and_then(|()| output.flush())
        .map_err(|_| ())
}

fn frame(input: &mut impl BufRead) -> Result<Option<Option<Value>>, ()> {
    let mut line = String::new();
    if input.take(FRAME + 1).read_line(&mut line).map_err(|_| ())? == 0 {
        return Ok(None);
    }
    if line.len() as u64 > FRAME || !line.ends_with('\n') {
        return Err(());
    }
    match serde_json::from_str(&line) {
        Ok(value) => Ok(Some(Some(value))),
        Err(_) => Ok(Some(None)),
    }
}

fn work(
    request: Value,
    id: String,
    notification: bool,
    output: Arc<Mutex<impl Write>>,
    active: Arc<Mutex<BTreeSet<String>>>,
    dispatch: Dispatch,
) {
    let response = dispatch(&request);
    if !notification {
        if let Ok(mut output) = output.lock() {
            let _ = write(&mut *output, &response);
        }
    }
    if let Ok(mut active) = active.lock() {
        active.remove(&id);
    }
}

fn serve(
    mut input: impl BufRead,
    output: impl Write + Send + 'static,
    dispatch: Dispatch,
) -> Result<(), ()> {
    let output = Arc::new(Mutex::new(output));
    let active = Arc::new(Mutex::new(BTreeSet::new()));
    let mut workers = Vec::<std::thread::JoinHandle<()>>::new();
    let mut notifications = 0u64;
    loop {
        let Some(request) = frame(&mut input)? else {
            break;
        };
        let Some(request) = request else {
            write(
                &mut *output.lock().map_err(|_| ())?,
                &error(&Value::Null, -32700, "Invalid MCP JSON."),
            )?;
            continue;
        };
        let notification = request.get("id").is_none();
        let id = if notification {
            notifications = notifications.checked_add(1).ok_or(())?;
            format!("notification:{notifications}")
        } else if let Some(id) = identifier(&request) {
            id
        } else {
            write(
                &mut *output.lock().map_err(|_| ())?,
                &error(&Value::Null, -32600, "Invalid MCP request identifier."),
            )?;
            continue;
        };
        let mut live = active.lock().map_err(|_| ())?;
        if live.len() >= 8 || !live.insert(id.clone()) {
            if !notification {
                write(
                    &mut *output.lock().map_err(|_| ())?,
                    &error(
                        &request["id"],
                        -32000,
                        "MCP request capacity or duplicate identifier.",
                    ),
                )?;
            }
            continue;
        }
        drop(live);
        workers.retain(|worker| !worker.is_finished());
        let (output, active, dispatch) = (output.clone(), active.clone(), dispatch.clone());
        workers.push(std::thread::spawn(move || {
            work(request, id, notification, output, active, dispatch)
        }));
    }
    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}

/// The core's answer, or a refusal carrying only the core's static class.
fn reply<E>(id: &Value, answer: Result<(u16, Vec<u8>), E>) -> Value {
    let class = match answer {
        Ok((status, bytes)) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(value)
                if status == 200
                    && value["ok"] == true
                    && (value["data"].is_object() || value["data"].is_null()) =>
            {
                return value["data"].clone();
            }
            Ok(value) => match value["error"]["code"].as_str() {
                Some(code) => super::refusal_class(code).to_owned(),
                None => "core_refused".to_owned(),
            },
            Err(_) => "core_refused".to_owned(),
        },
        Err(_) => "core_unreachable".to_owned(),
    };
    super::classified(
        error(
            id,
            -32000,
            "Agent Relay could not verify or complete this native call. Do not repeat an ambiguous send.",
        ),
        &class,
    )
}

pub(super) fn run(args: &[String]) -> i32 {
    if args == ["--help"] || args == ["-h"] {
        println!("copilot-mcp --site <origin> --workspace <id> --account <id> [--dev]\nApp-owned native Copilot MCP transport. No credential arguments.");
        return 0;
    }
    let Some((binding, development)) = binding(args) else {
        eprintln!("Invalid Copilot MCP configuration.");
        return 2;
    };
    // `copilot-mcp` carries only `--dev`, so it reads Agent Relay's or Agent
    // Relay Dev's own pointer, never another build's.
    let build = crate::relay_build::hook_build(development);
    let Some(socket) = session_hook_socket(None, &build) else {
        eprintln!("Agent Relay is not available.");
        return 2;
    };
    let dispatch = Arc::new(move |request: &Value| {
        let body =
            serde_json::to_vec(&json!({"binding":binding,"request":request})).unwrap_or_default();
        let answer = socket_response(
            &socket,
            "POST",
            super::ROUTE,
            &body,
            Duration::from_secs(60),
        );
        reply(&request["id"], answer)
    });
    match serve(std::io::stdin().lock(), std::io::stdout(), dispatch) {
        Ok(()) => 0,
        Err(()) => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone)]
    struct Output(Arc<Mutex<Vec<u8>>>);
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn stdio_preserves_identifiers_and_never_answers_notifications() {
        let output = Output(Arc::default());
        let capture = output.clone();
        let input=b"{\"jsonrpc\":\"2.0\",\"id\":\"actual-id\",\"method\":\"ping\"}\n{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n";
        serve(
            std::io::Cursor::new(input),
            output,
            Arc::new(|r| json!({"jsonrpc":"2.0","id":r["id"],"result":{}})),
        )
        .unwrap();
        let bytes = capture.0.lock().unwrap();
        let rows = String::from_utf8_lossy(&bytes);
        let rows = rows.lines().collect::<Vec<_>>();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            serde_json::from_str::<Value>(rows[0]).unwrap()["id"],
            "actual-id"
        );
    }
    #[test]
    fn a_refused_native_call_carries_only_the_core_class() {
        let id = json!(7);
        let refused = br#"{"ok":false,"error":{"code":"authority_changed","message":"x"}}"#;
        let out = reply::<()>(&id, Ok((409, refused.to_vec())));
        assert_eq!(out["id"], 7);
        assert_eq!(out["error"]["code"], -32000);
        assert_eq!(out["error"]["data"], json!({"class":"authority_changed"}));
        assert!(out["error"]["message"]
            .as_str()
            .unwrap()
            .ends_with("Do not repeat an ambiguous send."));
        let leaky = br#"{"ok":false,"error":{"code":"ticket tk_secret /Users/me","message":"x"}}"#;
        let out = reply::<()>(&id, Ok((409, leaky.to_vec())));
        assert_eq!(out["error"]["data"]["class"], "refused");
        assert!(!out.to_string().contains("tk_secret"));
        for code in ["session_abc123", &"ab12cd34".repeat(8)] {
            let body = format!(r#"{{"ok":false,"error":{{"code":"{code}","message":"x"}}}}"#);
            let out = reply::<()>(&id, Ok((409, body.into_bytes())));
            assert_eq!(out["error"]["data"]["class"], "refused");
            assert!(!out.to_string().contains(code));
        }
        let out = reply::<()>(&id, Ok((500, b"not json".to_vec())));
        assert_eq!(out["error"]["data"]["class"], "core_refused");
        let out = reply::<()>(&id, Err(()));
        assert_eq!(out["error"]["data"]["class"], "core_unreachable");
        let ok = br#"{"ok":true,"data":{"jsonrpc":"2.0","id":7,"result":{}}}"#;
        let out = reply::<()>(&id, Ok((200, ok.to_vec())));
        assert_eq!(out["result"], json!({}));
        assert!(out.get("error").is_none());
    }
    #[test]
    fn stdio_rejects_oversized_frames_and_credential_or_socket_arguments() {
        let output = Output(Arc::default());
        assert!(serve(
            std::io::Cursor::new(vec![b'x'; FRAME as usize + 2]),
            output,
            Arc::new(|_| Value::Null)
        )
        .is_err());
        for args in [
            vec!["--token", "secret"],
            vec!["--socket", "/foreign"],
            vec!["--site", "https://x", "--site", "https://y"],
        ] {
            assert!(binding(&args.into_iter().map(str::to_owned).collect::<Vec<_>>()).is_none());
        }
    }
}
