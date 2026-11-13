//! `claude-attestation-hook`: Agent Relay's Claude Code PreToolUse
//! hook, run before each hosted Agent Relay MCP tool call.
use agent_relay_node::relay::cli::{session_hook_socket, socket_response};
use anyhow::Result;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(2);
/// A registered join waits for its desktop identity to sync before signing.
const JOIN_TIMEOUT: Duration = Duration::from_secs(15);

/// Adds the current chat's proof. A join without verified desktop identity is
/// denied with local registration guidance. Registration itself remains a
/// separate command under Claude's normal permission policy; a PreToolUse hook
/// must not register a chat before the person can approve the tool call.
pub fn run(socket: Option<&Path>, development: bool) -> Result<()> {
    let mut input = Vec::new();
    let _ = std::io::stdin().take(4 << 20).read_to_end(&mut input);
    let output = serde_json::from_slice::<Value>(&input)
        .ok()
        .and_then(|input| {
            // A profile build's hook always names its socket (`--socket`); only
            // Agent Relay's and Dev's read their own pointer.
            let build = crate::relay_build::hook_build(development);
            let socket = session_hook_socket(socket, &build);
            let timeout = if is_join(&input) {
                JOIN_TIMEOUT
            } else {
                TIMEOUT
            };
            output_at(&input, socket.as_deref(), |body| {
                let socket = socket.as_deref()?;
                let (status, response) =
                    socket_response(socket, "POST", super::ROUTE, body, timeout).ok()?;
                matches!(status, 200 | 400 | 403 | 409).then_some(response)
            })
        });
    if let Some(output) = output {
        let mut stdout = std::io::stdout();
        stdout.write_all(&serde_json::to_vec(&output)?)?;
        stdout.write_all(b"\n")?;
    }
    Ok(())
}

fn is_join(input: &Value) -> bool {
    input["hook_event_name"] == "PreToolUse"
        && input["tool_name"].as_str().and_then(super::hook_tool) == Some("join_relay")
}

fn denied(reason: impl Into<String>) -> Value {
    json!({"hookSpecificOutput": {
        "hookEventName": "PreToolUse", "permissionDecision": "deny",
        "permissionDecisionReason": reason.into(),
    }})
}

const RETRY: &str = "Agent Relay could not verify this chat's desktop identity. Keep the Agent Relay desktop app open and retry this join. Do not substitute a hosted account identity or report that this chat has joined.";

#[cfg(test)]
pub(crate) fn output(input: &Value, ask: impl FnOnce(&[u8]) -> Option<Vec<u8>>) -> Option<Value> {
    output_at(input, None, ask)
}

pub(crate) fn output_at(
    input: &Value,
    socket: Option<&Path>,
    ask: impl FnOnce(&[u8]) -> Option<Vec<u8>>,
) -> Option<Value> {
    verified_output(input, socket, ask).or_else(|| is_join(input).then(|| denied(RETRY)))
}

fn verified_output(
    input: &Value,
    socket: Option<&Path>,
    ask: impl FnOnce(&[u8]) -> Option<Vec<u8>>,
) -> Option<Value> {
    let request = super::hook_request(input)?;
    let response: Value =
        serde_json::from_slice(&ask(&serde_json::to_vec(&request).ok()?)?).ok()?;
    let Some(proof) = response["data"]["attestation"].as_str().filter(|proof| {
        let parts: Vec<_> = proof.split('.').collect();
        parts.len() == 3
            && parts.iter().all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            })
    }) else {
        if !is_join(input) {
            return None;
        }
        let reason = match response["error"]["code"].as_str()? {
            "not_a_relay_session" | "registration_required" => {
                let name = input["tool_input"]["name"].as_str().filter(|name| !name.trim().is_empty());
                if let (Some(socket), Some(name)) = (socket, name) {
                    let command = registration_command_for(&std::env::current_exe().ok()?, socket, &input["tool_input"])?;
                    format!("This Claude chat must register locally under the approved name {name:?} before hosted Relay can verify its identity. Run {command} from this chat's own Bash tool, through the normal command permission check; do not bypass that check. This registers only this chat and shares its conversation with the workspace. After it succeeds, retry join_relay without name or description. Do not run the command from a separate terminal, substitute another chat ID, or use a hosted account identity.")
                } else {
                    "This Claude chat has not joined Agent Relay Desktop. Ask the person what name to use, then call join_relay with that name to get its local registration command. Do not substitute a hosted account identity.".into()
                }
            }
            "invalid_name" | "name_taken" => "This Claude chat could not join under the requested name. Ask the person to choose an available name using 2–48 lowercase letters, digits and single dashes, then retry join_relay with that name.".into(),
            "not_allowed" => "This Claude chat cannot register while Let sessions put themselves on the relay is off. Ask the person to enable it in Agent Relay Settings or put this session on the relay in the app, then retry.".into(),
            _ => return None,
        };
        return Some(denied(reason));
    };
    if request.get("registration").is_some() && response["data"]["native_join"] != true {
        return None;
    }
    let mut output = super::hook_output(input, proof)?;
    if is_join(input) && response["data"]["native_join"] == true {
        // The core verified that the requested name is already this chat's
        // local name. Hosted native joins use that identity, never a new name.
        let arguments = output["hookSpecificOutput"]["updatedInput"].as_object_mut()?;
        arguments.remove("name");
        arguments.remove("description");
    }
    Some(output)
}

pub(super) fn registration_command_for(
    executable: &Path,
    socket: &Path,
    arguments: &Value,
) -> Option<String> {
    if !executable.is_absolute() || !socket.is_absolute() {
        return None;
    }
    let mut body = json!({
        "name": arguments.get("name")?,
        "mcp": agent_relay_node::relay::hosted_mcp::SERVER_NAME,
    });
    if let Some(description) = arguments.get("description") {
        body["description"] = description.clone();
    }
    let body = serde_json::to_string(&body).ok()?;
    let parts = [
        executable.to_str()?,
        "socket-request",
        "--socket",
        socket.to_str()?,
        "--method",
        "POST",
        "--path",
        "/register",
        "--body",
        &body,
    ];
    Some(
        parts
            .iter()
            .map(|part| agent_relay_node::relay::outbound::shell_quote(part))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// `claude-attestation-hook [--dev] [--socket <path>]`.
pub fn main(arguments: &[String]) -> i32 {
    let dev = arguments.iter().any(|argument| argument == "--dev");
    let socket = arguments
        .iter()
        .position(|argument| argument == "--socket")
        .and_then(|at| arguments.get(at + 1))
        .map(Path::new);
    match run(socket, dev) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}
