//! The `cursor-mcp` helper Cursor runs: the Copilot helper's stdio framing,
//! with JSON-RPC batches refused, handing every request to Core's
//! [`super::ROUTE`]. No token, key or proof crosses this process.
use crate::copilot_dispatch::stdio::{binding_with, reply_as, serve_with};
use agent_relay_node::relay::cli::{session_hook_socket, socket_response};
use agent_relay_node::relay::outbound::shell_quote;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

/// What a refusal of class `class` tells the chat. `whoami` is the command
/// that shows, or after a moment registers, the chat's relay identity.
pub(super) fn message(class: &str, probe: &Path, socket: Option<&Path>) -> String {
    let whoami = || {
        let socket = socket
            .map(|socket| format!(" --socket {}", shell_quote(&socket.to_string_lossy())))
            .unwrap_or_default();
        format!("{} whoami{socket}", shell_quote(&probe.to_string_lossy()))
    };
    match class {
        "cursor_intent_missing" | "cursor_chat_unknown" | "cursor_intent_ambiguous" => format!(
            "Agent Relay could not tie this call to a Cursor chat on the relay. Run {} from this \
             chat's shell to put the chat on the relay, then retry. Do not repeat an ambiguous \
             send.",
            whoami()
        ),
        "cursor_args_too_large" => {
            "Agent Relay cannot attest a call with more than 32 KB of arguments.".to_owned()
        }
        _ => "Agent Relay could not attest or complete this Cursor call. Do not repeat an \
              ambiguous send."
            .to_owned(),
    }
}

/// `reply` with a Desktop refusal's message replaced by what its class tells
/// the chat, naming the class: Core's own dispatch refusals reach the helper
/// as a reply, so `reply_as` passes their generic message through.
fn with_class(mut reply: Value, message: &dyn Fn(&str) -> String) -> Value {
    if let Some(class) = reply["error"]["data"]["class"].as_str().map(str::to_owned) {
        reply["error"]["message"] = json!(format!("{} ({class})", message(&class)));
    }
    reply
}

pub(super) fn run(args: &[String]) -> i32 {
    if args == ["--help"] || args == ["-h"] {
        println!(
            "cursor-mcp --site <origin> --workspace <id> --account <id> [--socket <path>] [--dev]\n\
             App-owned Cursor MCP transport. No credential arguments."
        );
        return 0;
    }
    let Some((binding, development, named)) = binding_with(args, true) else {
        eprintln!("Invalid Cursor MCP configuration.");
        return 2;
    };
    if named.as_deref().is_some_and(|socket| !socket.is_absolute()) {
        eprintln!("Invalid Cursor MCP configuration.");
        return 2;
    }
    let build = crate::relay_build::hook_build(development);
    let Some(socket) = session_hook_socket(named.as_deref(), &build) else {
        eprintln!("Agent Relay is not available.");
        return 2;
    };
    let probe = std::env::current_exe().unwrap_or_else(|_| "agent-relay-probe".into());
    let dispatch = Arc::new(move |request: &Value| {
        let body =
            serde_json::to_vec(&json!({"binding":binding,"request":request})).unwrap_or_default();
        let answer = socket_response(
            &socket,
            "POST",
            super::ROUTE,
            &body,
            super::route::LOCAL_RESPONSE_BUDGET,
        );
        let message = |class: &str| message(class, &probe, named.as_deref());
        with_class(reply_as(&request["id"], answer, &message), &message)
    });
    match serve_with(std::io::stdin().lock(), std::io::stdout(), dispatch, true) {
        Ok(()) => 0,
        Err(()) => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::Mutex;

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
    fn a_batch_is_refused_without_dispatching_any_of_it() {
        let output = Output(Arc::default());
        let capture = output.clone();
        let input = concat!(
            r#"[{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"send_relay_message"}}]"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
            "\n"
        );
        serve_with(
            std::io::Cursor::new(input),
            output,
            Arc::new(|r: &Value| {
                assert!(r.is_object(), "a batch never reaches Core");
                json!({"jsonrpc":"2.0","id":r["id"],"result":{}})
            }),
            true,
        )
        .unwrap();
        let bytes = capture.0.lock().unwrap();
        let rows: Vec<Value> = String::from_utf8_lossy(&bytes)
            .lines()
            .map(|row| serde_json::from_str(row).unwrap())
            .collect();
        assert_eq!(rows.len(), 2);
        let batch = rows.iter().find(|row| row["id"].is_null()).unwrap();
        assert_eq!(batch["error"]["code"], -32600);
        assert!(rows
            .iter()
            .any(|row| row["id"] == 2 && row["result"] == json!({})));
    }

    #[test]
    fn the_command_line_takes_a_socket_but_no_credentials() {
        let parse = |args: &[&str]| {
            binding_with(
                &args.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
                true,
            )
        };
        let base = [
            "--site",
            "https://agentrelay.example",
            "--workspace",
            "w",
            "--account",
            "a",
        ];
        let (_, dev, socket) =
            parse(&[&base[..], &["--socket", "/tmp/r.sock", "--dev"]].concat()).unwrap();
        assert!(dev);
        assert_eq!(socket.as_deref(), Some(Path::new("/tmp/r.sock")));
        assert!(parse(&[&base[..], &["--token", "secret"]].concat()).is_none());
        assert!(parse(&[&base[..], &["--socket", "/a", "--socket", "/b"]].concat()).is_none());
    }

    #[test]
    fn a_dispatch_refusal_tells_the_chat_its_class() {
        let probe = Path::new("/opt/relay/agent-relay-probe");
        let message = |class: &str| message(class, probe, None);
        let refusal = crate::copilot_dispatch::classified(
            json!({"jsonrpc":"2.0","id":7,"error":{"code":-32000,"message":"generic"}}),
            "cursor_intent_missing",
        );
        let reply = json!({"ok": true, "data": refusal})
            .to_string()
            .into_bytes();
        let out = with_class(
            reply_as::<()>(&json!(7), Ok((200, reply)), &message),
            &message,
        );
        let text = out["error"]["message"].as_str().unwrap();
        assert!(text.ends_with(" (cursor_intent_missing)"), "{text}");
        assert!(text.contains("agent-relay-probe whoami"), "{text}");
        assert_eq!(out["error"]["data"]["class"], "cursor_intent_missing");
        let hosted = json!({"jsonrpc":"2.0","id":8,"error":{"code":-32602,"message":"Cloud"}});
        assert_eq!(with_class(hosted.clone(), &message), hosted);
    }

    #[test]
    fn a_missing_intent_names_the_command_that_puts_the_chat_on_the_relay() {
        let probe = Path::new("/opt/relay/agent-relay-probe");
        let refused = br#"{"ok":false,"error":{"code":"cursor_intent_missing","message":"x"}}"#;
        let out = reply_as::<()>(&json!(4), Ok((409, refused.to_vec())), &|class| {
            message(class, probe, Some(Path::new("/tmp/p s/relay.sock")))
        });
        assert_eq!(out["error"]["data"]["class"], "cursor_intent_missing");
        assert!(out["error"]["message"]
            .as_str()
            .unwrap()
            .contains("/opt/relay/agent-relay-probe whoami --socket '/tmp/p s/relay.sock'"));
        assert!(!message("refused", probe, None).contains("whoami"));
        assert!(message("cursor_chat_unknown", probe, None)
            .contains("/opt/relay/agent-relay-probe whoami from"));
    }
}
