//! `cursor-guide-hook`: Agent Relay Desktop's Cursor hook that hands a
//! Cursor chat the session socket commands for its relay identity.
//!
//! Cursor's MCP calls name no chat, so the core gives a Cursor session
//! socket commands alone (`hosted_mcp`'s `McpAccess::SocketOnly`) and Cloud
//! refuses the hosted MCP's identity tools for it. A chat cannot guess the
//! probe's path or, on a site profile build, its session socket, so this
//! hook tells it both:
//! - `sessionStart` adds the commands to the chat's context;
//! - `beforeMCPExecution` denies an identity tool of Agent Relay's MCP entry
//!   with the one command that does the same, and allows everything else.
//!
//! It never blocks Cursor: unknown events and unreadable input get `{}`.
use agent_relay_node::relay::outbound::shell_quote;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The subcommand the hook runs; also how its `hooks.json` entries are
/// recognized.
pub const SUBCOMMAND: &str = "cursor-guide-hook";

/// The Cursor hook events this hook answers.
pub const EVENTS: [&str; 2] = ["sessionStart", "beforeMCPExecution"];

/// The hosted MCP tools that act as the calling chat's relay identity,
/// which a Cursor chat cannot use.
pub const IDENTITY_TOOLS: [&str; 7] = [
    "join_relay",
    "send_relay_message",
    "post_relay_channel",
    "subscribe_relay_channel",
    "unsubscribe_relay_channel",
    "check_relay_inbox",
    "set_relay_agent_card",
];

/// Where the commands the hook names run: the probe and this build's
/// session socket, when it could be found.
pub struct Commands {
    probe: String,
    socket: Option<String>,
}

impl Commands {
    pub fn new(probe: &Path, socket: Option<&Path>) -> Self {
        Self {
            probe: shell_quote(&probe.to_string_lossy()),
            socket: socket.map(|socket| shell_quote(&socket.to_string_lossy())),
        }
    }

    /// `'<probe>' <subcommand> --socket '<sock>'<rest>`.
    fn line(&self, socket: &str, subcommand: &str, rest: &str) -> String {
        format!("{} {subcommand} --socket {socket}{rest}", self.probe)
    }

    /// The sessionStart context.
    pub fn guidance(&self) -> String {
        let head = "Agent Relay: this Cursor chat has no relay identity through the Agent Relay \
                    MCP server, so its relay identity tools (join_relay, send_relay_message, \
                    post_relay_channel, check_relay_inbox and the like) do not work here. Relay \
                    messages for this chat arrive in the chat through Agent Relay's Cursor hooks.";
        let Some(socket) = self.socket.as_deref() else {
            return format!(
                "{head} Agent Relay is not running, so this chat's relay session socket could \
                 not be found: start Agent Relay, then start a new chat to get the commands."
            );
        };
        format!(
            "{head} Act on the relay with these commands, run from this chat's shell:\n\
             - this chat's own relay name: {}\n\
             - the agents it can message: {}\n\
             - send a message: {} (add --reply-to '<message id>' to answer a message)\n\
             - read a message by its reference: {}",
            self.line(socket, "whoami", ""),
            self.line(socket, "agents", ""),
            self.line(socket, "send", " --to '<name>' --body '<text>'"),
            self.line(socket, "read", " '<ref>'"),
        )
    }

    /// The command doing what `tool` would, as the deny message names it.
    fn instead(&self, socket: &str, tool: &str) -> String {
        let (subcommand, rest, note) = match tool {
            "send_relay_message" => (
                "send",
                " --to '<name>' --body '<text>'",
                " (add --reply-to '<message id>' to answer a message)",
            ),
            "post_relay_channel" => (
                "send",
                " --channel '<channel>' --body '<text>'",
                " (add --thread '<message id>' to answer in a thread)",
            ),
            "subscribe_relay_channel" => ("subscribe-channel", " '<channel>'", ""),
            "unsubscribe_relay_channel" => ("subscribe-channel", " '<channel>' --remove", ""),
            "check_relay_inbox" => (
                "read",
                " '<ref>'",
                "; relay messages arrive in this chat on their own",
            ),
            _ => ("whoami", "", " shows this chat's relay identity"),
        };
        format!("{}{note}", self.line(socket, subcommand, rest))
    }

    /// The beforeMCPExecution deny message for `tool`.
    pub fn denial(&self, tool: &str) -> String {
        let head = format!(
            "Agent Relay: a Cursor chat has no relay identity through MCP, so {tool} cannot \
             work here."
        );
        match self.socket.as_deref() {
            Some(socket) => format!(
                "{head} Run this from this chat's shell instead: {}",
                self.instead(socket, tool)
            ),
            None => format!(
                "{head} Agent Relay is not running: start it, then start a new chat to get \
                 the commands to use."
            ),
        }
    }
}

/// Whether a beforeMCPExecution call is an identity tool of `server`.
fn identity_call(input: &Value, server: &str) -> Option<String> {
    let tool = input["tool_name"].as_str()?;
    (input["mcp_server_name"].as_str() == Some(server) && IDENTITY_TOOLS.contains(&tool))
        .then(|| tool.to_string())
}

/// The hook's answer to `input` for `server`, Agent Relay's MCP entry.
/// `commands` is asked only for an event that names them.
pub fn answer(input: &[u8], server: &str, commands: impl FnOnce() -> Commands) -> Value {
    let Ok(input) = serde_json::from_slice::<Value>(input) else {
        return json!({});
    };
    match input["hook_event_name"].as_str() {
        Some("sessionStart") => json!({"additional_context": commands().guidance()}),
        Some("beforeMCPExecution") => match identity_call(&input, server) {
            Some(tool) => json!({"permission": "deny", "agent_message": commands().denial(&tool)}),
            None => json!({"permission": "allow"}),
        },
        _ => json!({}),
    }
}

/// The value after `flag` in `arguments`.
fn value_of<'a>(arguments: &'a [String], flag: &str) -> Option<&'a str> {
    let at = arguments.iter().position(|argument| argument == flag)?;
    arguments.get(at + 1).map(String::as_str)
}

/// This build's session socket: the one the hook command names (a site
/// profile build's), else the one its build's pointer names, as the other
/// hooks find it.
fn session_socket(arguments: &[String]) -> Option<PathBuf> {
    let development = arguments.iter().any(|argument| argument == "--dev");
    agent_relay_node::relay::cli::session_hook_socket(
        value_of(arguments, "--socket").map(Path::new),
        &crate::relay_build::hook_build(development),
    )
}

/// `cursor-guide-hook [--socket <path>] [--dev]`: answers one Cursor hook
/// payload from standard input, and always exits 0.
pub fn main(arguments: &[String]) -> i32 {
    let mut input = Vec::new();
    let _ = std::io::stdin().take(4 << 20).read_to_end(&mut input);
    let output = answer(&input, crate::mcp_settings::NAME, || {
        let probe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("agent-relay-probe"));
        Commands::new(&probe, session_socket(arguments).as_deref())
    });
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(output.to_string().as_bytes());
    let _ = stdout.write_all(b"\n");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERVER: &str = "agent-relay";

    fn commands(socket: Option<&str>) -> impl FnOnce() -> Commands + '_ {
        move || {
            Commands::new(
                Path::new("/opt/relay/agent-relay-probe"),
                socket.map(Path::new),
            )
        }
    }

    fn mcp_call(server: &str, tool: &str) -> Vec<u8> {
        json!({
            "hook_event_name": "beforeMCPExecution",
            "conversation_id": "c1",
            "tool_name": tool,
            "tool_input": "{}",
            "mcp_server_name": server,
            "url": "https://example.invalid/mcp",
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn session_start_names_the_probe_and_the_socket() {
        let input =
            br#"{"hook_event_name":"sessionStart","conversation_id":"c1","workspace_roots":[]}"#;
        let output = answer(input, SERVER, commands(Some("/tmp/profile run/relay.sock")));
        let context = output["additional_context"].as_str().unwrap();
        assert_eq!(output.as_object().unwrap().len(), 1);
        for command in [
            "/opt/relay/agent-relay-probe whoami --socket '/tmp/profile run/relay.sock'",
            "/opt/relay/agent-relay-probe agents --socket '/tmp/profile run/relay.sock'",
            "/opt/relay/agent-relay-probe send --socket '/tmp/profile run/relay.sock' --to '<name>' --body '<text>'",
            "--reply-to '<message id>'",
            "/opt/relay/agent-relay-probe read --socket '/tmp/profile run/relay.sock' '<ref>'",
        ] {
            assert!(context.contains(command), "{command} missing from {context}");
        }
        assert!(context.contains("no relay identity through the Agent Relay MCP server"));
        assert!(context.contains("arrive in the chat through Agent Relay's Cursor hooks"));
    }

    #[test]
    fn session_start_without_a_socket_says_to_start_agent_relay() {
        let output = answer(
            br#"{"hook_event_name":"sessionStart"}"#,
            SERVER,
            commands(None),
        );
        let context = output["additional_context"].as_str().unwrap();
        assert!(context.contains("Agent Relay is not running"));
        assert!(!context.contains("--socket"));
    }

    #[test]
    fn identity_tools_of_the_relay_server_are_denied_with_their_command() {
        for tool in IDENTITY_TOOLS {
            let output = answer(
                &mcp_call(SERVER, tool),
                SERVER,
                commands(Some("/tmp/r s.sock")),
            );
            assert_eq!(output["permission"], "deny", "{tool}");
            let message = output["agent_message"].as_str().unwrap();
            assert!(!message.contains('\n'), "{tool}: one line");
            assert!(
                message.contains("/opt/relay/agent-relay-probe ")
                    && message.contains(" --socket '/tmp/r s.sock'"),
                "{tool}: {message}"
            );
        }
        let send = answer(
            &mcp_call(SERVER, "send_relay_message"),
            SERVER,
            commands(Some("/tmp/r s.sock")),
        );
        assert!(send["agent_message"].as_str().unwrap().contains(
            "/opt/relay/agent-relay-probe send --socket '/tmp/r s.sock' --to '<name>' --body '<text>'"
        ));
        let leave = answer(
            &mcp_call(SERVER, "unsubscribe_relay_channel"),
            SERVER,
            commands(Some("/tmp/r s.sock")),
        );
        assert!(leave["agent_message"]
            .as_str()
            .unwrap()
            .contains("subscribe-channel --socket '/tmp/r s.sock' '<channel>' --remove"));
    }

    #[test]
    fn other_servers_and_tools_are_allowed_without_reading_the_socket() {
        let never = || -> Commands { panic!("an allowed call names no commands") };
        for input in [
            mcp_call("other-server", "send_relay_message"),
            mcp_call(SERVER, "list_relay_agents"),
            mcp_call(SERVER, "search_shared_sessions"),
            br#"{"hook_event_name":"beforeMCPExecution"}"#.to_vec(),
        ] {
            assert_eq!(
                answer(&input, SERVER, never),
                json!({"permission": "allow"})
            );
        }
    }

    #[test]
    fn unknown_events_and_junk_get_an_empty_answer() {
        let never = || -> Commands { panic!("no commands for an empty answer") };
        for input in [
            &br#"{"hook_event_name":"stop","status":"completed"}"#[..],
            br#"{"conversation_id":"c1"}"#,
            b"not json",
            b"",
            b"[1,2]",
        ] {
            assert_eq!(answer(input, SERVER, never), json!({}));
        }
    }

    #[test]
    fn the_socket_comes_from_the_command_line_when_named() {
        let arguments = ["--socket".to_string(), "/tmp/named.sock".into()];
        assert_eq!(value_of(&arguments, "--socket"), Some("/tmp/named.sock"));
        assert_eq!(
            session_socket(&arguments),
            Some(PathBuf::from("/tmp/named.sock"))
        );
        assert_eq!(value_of(&["--socket".to_string()], "--socket"), None);
    }
}
