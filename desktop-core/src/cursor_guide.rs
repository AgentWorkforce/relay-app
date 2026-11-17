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
//!   with the one command that does the same, if there is one.
//!
//! Where Cursor reaches the hosted MCP through Agent Relay's `cursor-mcp`
//! helper and Cloud verifies Cursor chat proofs (`cursor_dispatch`), the
//! chat does have its relay identity through MCP: `beforeMCPExecution`
//! announces each identity tool call of Agent Relay's server to Core, which
//! signs that chat's proof into the call, and `sessionStart` says the
//! identity tools work. Any other call of the server carries no proof, so
//! it is neither announced nor signed. An identity call Core did not keep
//! is still left to Cursor while Cursor's calls are attested: the helper
//! then refuses it with Core's reason, which the chat sees as the call's
//! error. It is denied with its command only when they are not attested,
//! as the refusal's class says or, when the class cannot tell, Core's
//! status. The hook writes the class of every refusal to its standard
//! error.
//!
//! Every other call, of any server, gets `{}`: the hook is registered for
//! every MCP call, so it gives no opinion on them and leaves them to Cursor's
//! and the user's own approval. Unknown events and unreadable input get `{}`
//! too, so it never blocks Cursor.
use agent_relay_node::relay::outbound::shell_quote;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

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

/// Which Cursor chats can use Agent Relay. Cursor runs Cloud agents,
/// Projects and Remote Control (My Machines) chats in its cloud, where it
/// reads neither file, so only a local agent registers. The Mac's
/// `CursorHooks.localOnlyNote` and GTK's `CURSOR_HOOKS_LOCAL_ONLY_NOTE` say
/// the same for their platform.
pub const LOCAL_ONLY_NOTE: &str = "Only a local Cursor agent (This Mac or This PC) can use Agent Relay. Cloud agents, Projects and Remote Control (My Machines) chats cannot, because Cursor does not load ~/.cursor/hooks.json or ~/.cursor/mcp.json for them. Pick This Mac or This PC in the selector under the chat box, or make it the default in Cursor Settings > Agents > Conversation > Default Environment.";

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

    /// The sessionStart context while Cursor's calls are attested per chat.
    pub fn attested_guidance(&self) -> String {
        let head = "Agent Relay: this Cursor chat has its own relay identity through the Agent \
                    Relay MCP server: its relay identity tools (send_relay_message, \
                    post_relay_channel, check_relay_inbox and the like) act as this chat. Relay \
                    messages for this chat arrive in the chat through Agent Relay's Cursor hooks.";
        match self.socket.as_deref() {
            Some(socket) => format!(
                "{head} If such a call is refused because the chat is not on the relay, run {} \
                 from this chat's shell, then retry.",
                self.line(socket, "whoami", "")
            ),
            None => head.to_string(),
        }
    }

    /// The sessionStart context.
    pub fn guidance(&self) -> String {
        let head = format!(
            "Agent Relay: this Cursor chat has no relay identity through the Agent Relay MCP \
             server, so its relay identity tools (join_relay, send_relay_message, \
             post_relay_channel, check_relay_inbox and the like) do not work here. Relay \
             messages for this chat arrive in the chat through Agent Relay's Cursor hooks. \
             {LOCAL_ONLY_NOTE}"
        );
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
            _ => (
                "whoami",
                "",
                " shows the relay identity this chat already has",
            ),
        };
        format!("{}{note}", self.line(socket, subcommand, rest))
    }

    /// The beforeMCPExecution deny message for `tool`.
    pub fn denial(&self, tool: &str) -> String {
        let head = format!(
            "Agent Relay: a Cursor chat has no relay identity through MCP, so {tool} cannot \
             work here."
        );
        if tool == "set_relay_agent_card" {
            return format!(
                "{head} A Cursor chat's agent card is not set from the chat, and no probe \
                 command sets it."
            );
        }
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

/// What the hook asks Agent Relay's core.
pub trait Relay {
    /// Whether Cursor's calls of Agent Relay's server are attested per chat.
    fn attesting(&self) -> bool;
    /// Announces a `beforeMCPExecution` call of one of Agent Relay's
    /// identity tools: `Ok`
    /// when Core kept it, to sign the matching call as this chat, else the
    /// class of Core's refusal (`core_unreachable` without an answer).
    fn announce(&self, call: &Value) -> Result<(), String>;
}

/// Whether Cursor's calls are attested, as far as refusal `class` of an
/// announcement says: Core refuses with `cursor_attestation_unavailable`
/// when they are not, and checks that before the chat, the hook process,
/// the account and the deadline. Any other class leaves it unknown.
fn attesting_after(class: &str) -> Option<bool> {
    match class {
        "cursor_attestation_unavailable" => Some(false),
        "cursor_chat_unknown"
        | "cursor_hook_unverified"
        | "account_changed"
        | "deadline_expired"
        | "cursor_intent_capacity" => Some(true),
        _ => None,
    }
}

/// Whether a beforeMCPExecution call is an identity tool.
fn identity_call(input: &Value) -> Option<String> {
    let tool = input["tool_name"].as_str()?;
    IDENTITY_TOOLS.contains(&tool).then(|| tool.to_string())
}

/// The members of a `beforeMCPExecution` payload Core reads.
fn announcement(input: &Value) -> Value {
    let mut call = serde_json::Map::new();
    for name in [
        "conversation_id",
        "generation_id",
        "tool_use_id",
        "tool_name",
        "tool_input",
    ] {
        if let Some(value) = input.get(name) {
            call.insert(name.into(), value.clone());
        }
    }
    Value::Object(call)
}

/// The hook's answer to `input` for `server`, Agent Relay's MCP entry.
/// `relay` is asked only about session starts and Agent Relay's own
/// identity tools;
/// `commands` only for an event that names them.
pub fn answer(
    input: &[u8],
    server: &str,
    relay: &dyn Relay,
    commands: impl FnOnce() -> Commands,
) -> Value {
    let Ok(input) = serde_json::from_slice::<Value>(input) else {
        return json!({});
    };
    match input["hook_event_name"].as_str() {
        Some("sessionStart") if relay.attesting() => {
            json!({"additional_context": commands().attested_guidance()})
        }
        Some("sessionStart") => json!({"additional_context": commands().guidance()}),
        Some("beforeMCPExecution") if input["mcp_server_name"].as_str() == Some(server) => {
            // Only an identity tool carries a proof: any other call of the
            // server goes out unsigned and needs no announcement.
            let Some(tool) = identity_call(&input) else {
                return json!({});
            };
            let Err(class) = relay.announce(&announcement(&input)) else {
                return json!({});
            };
            eprintln!("[cursor-guide-hook] Agent Relay refused to attest the call: {class}");
            // While Cursor's calls are attested, the helper refuses this
            // call with Core's reason; the probe command is only for a chat
            // whose MCP calls carry no identity.
            if attesting_after(&class).unwrap_or_else(|| relay.attesting()) {
                return json!({});
            }
            let message = format!(
                "{} (Agent Relay refused the call: {class}.)",
                commands().denial(&tool)
            );
            json!({"permission": "deny", "agent_message": message})
        }
        _ => json!({}),
    }
}

/// How long one run of the hook may spend asking Core, inside the timeout
/// its `hooks.json` entries give Cursor.
const BUDGET: Duration = Duration::from_secs(4);

/// Agent Relay's core on this build's session socket, asked from `started`.
struct SessionSocket {
    socket: Option<PathBuf>,
    started: std::time::Instant,
}

impl SessionSocket {
    /// Core's `data`, or the class of its refusal.
    fn ask(&self, route: &str, body: &[u8], timeout: Duration) -> Result<Value, String> {
        let unreachable = || "core_unreachable".to_string();
        let socket = self.socket.as_deref().ok_or_else(unreachable)?;
        let timeout = timeout.min(BUDGET.saturating_sub(self.started.elapsed()));
        if timeout.is_zero() {
            return Err(unreachable());
        }
        let (status, bytes) =
            agent_relay_node::relay::cli::socket_response(socket, "POST", route, body, timeout)
                .map_err(|_| unreachable())?;
        let reply: Value = serde_json::from_slice(&bytes).map_err(|_| "core_refused")?;
        if status == 200 && reply["ok"] == true {
            return Ok(reply["data"].clone());
        }
        let code = reply["error"]["code"].as_str().unwrap_or("core_refused");
        Err(crate::copilot_dispatch::refusal_class(code).to_string())
    }
}

impl Relay for SessionSocket {
    fn attesting(&self) -> bool {
        self.ask(
            crate::cursor_dispatch::STATUS_ROUTE,
            b"{}",
            Duration::from_secs(2),
        )
        .is_ok_and(|data| data["attesting"] == true)
    }

    fn announce(&self, call: &Value) -> Result<(), String> {
        let data = self.ask(
            crate::cursor_dispatch::INTENT_ROUTE,
            call.to_string().as_bytes(),
            crate::cursor_dispatch::ANNOUNCE_TIMEOUT,
        )?;
        (data["accepted"] == true)
            .then_some(())
            .ok_or_else(|| "core_refused".to_string())
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
    let socket = session_socket(arguments);
    let relay = SessionSocket {
        socket: socket.clone(),
        started: std::time::Instant::now(),
    };
    let output = answer(&input, crate::mcp_settings::NAME, &relay, || {
        let probe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("agent-relay-probe"));
        Commands::new(&probe, socket.as_deref())
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

    /// Core's answers, and how often it was asked.
    struct Fake {
        attesting: bool,
        /// `None` when Core keeps an announcement, else its refusal class.
        refusal: Option<&'static str>,
        announced: std::cell::RefCell<Vec<Value>>,
        status_asked: std::cell::Cell<u32>,
    }

    impl Fake {
        fn off() -> Self {
            Fake::refusing(false, "cursor_attestation_unavailable")
        }

        fn new(attesting: bool, accepts: bool) -> Self {
            Fake::with(attesting, (!accepts).then_some("core_refused"))
        }

        fn refusing(attesting: bool, class: &'static str) -> Self {
            Fake::with(attesting, Some(class))
        }

        fn with(attesting: bool, refusal: Option<&'static str>) -> Self {
            Fake {
                attesting,
                refusal,
                announced: std::cell::RefCell::new(Vec::new()),
                status_asked: std::cell::Cell::new(0),
            }
        }
    }

    impl Relay for Fake {
        fn attesting(&self) -> bool {
            self.status_asked.set(self.status_asked.get() + 1);
            self.attesting
        }
        fn announce(&self, call: &Value) -> Result<(), String> {
            self.announced.borrow_mut().push(call.clone());
            self.refusal.map_or(Ok(()), |class| Err(class.to_string()))
        }
    }

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
        let output = answer(
            input,
            SERVER,
            &Fake::off(),
            commands(Some("/tmp/profile run/relay.sock")),
        );
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
        assert!(context.contains(LOCAL_ONLY_NOTE));
    }

    #[test]
    fn session_start_without_a_socket_says_to_start_agent_relay() {
        let output = answer(
            br#"{"hook_event_name":"sessionStart"}"#,
            SERVER,
            &Fake::off(),
            commands(None),
        );
        let context = output["additional_context"].as_str().unwrap();
        assert!(context.contains("Agent Relay is not running"));
        assert!(context.contains(LOCAL_ONLY_NOTE));
        assert!(!context.contains("--socket"));
    }

    #[test]
    fn identity_tools_of_the_relay_server_are_denied_with_their_command() {
        for tool in IDENTITY_TOOLS
            .into_iter()
            .filter(|tool| *tool != "set_relay_agent_card")
        {
            let output = answer(
                &mcp_call(SERVER, tool),
                SERVER,
                &Fake::off(),
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
            &Fake::off(),
            commands(Some("/tmp/r s.sock")),
        );
        assert!(send["agent_message"].as_str().unwrap().contains(
            "/opt/relay/agent-relay-probe send --socket '/tmp/r s.sock' --to '<name>' --body '<text>'"
        ));
        let leave = answer(
            &mcp_call(SERVER, "unsubscribe_relay_channel"),
            SERVER,
            &Fake::off(),
            commands(Some("/tmp/r s.sock")),
        );
        assert!(leave["agent_message"]
            .as_str()
            .unwrap()
            .contains("subscribe-channel --socket '/tmp/r s.sock' '<channel>' --remove"));
        let join = answer(
            &mcp_call(SERVER, "join_relay"),
            SERVER,
            &Fake::off(),
            commands(Some("/tmp/r s.sock")),
        );
        assert!(join["agent_message"].as_str().unwrap().contains(
            "whoami --socket '/tmp/r s.sock' shows the relay identity this chat already has"
        ));
    }

    #[test]
    fn the_agent_card_is_denied_without_naming_a_command() {
        for socket in [Some("/tmp/r s.sock"), None] {
            let output = answer(
                &mcp_call(SERVER, "set_relay_agent_card"),
                SERVER,
                &Fake::off(),
                commands(socket),
            );
            assert_eq!(output["permission"], "deny");
            let message = output["agent_message"].as_str().unwrap();
            assert!(message.contains("no probe command sets it"), "{message}");
            assert!(
                !message.contains("whoami") && !message.contains("--socket"),
                "{message}"
            );
        }
    }

    #[test]
    fn other_servers_and_tools_get_no_opinion_without_reading_the_socket() {
        let never = || -> Commands { panic!("a call left to Cursor names no commands") };
        for input in [
            mcp_call("other-server", "send_relay_message"),
            mcp_call(SERVER, "list_relay_agents"),
            mcp_call(SERVER, "search_shared_sessions"),
            br#"{"hook_event_name":"beforeMCPExecution"}"#.to_vec(),
        ] {
            assert_eq!(answer(&input, SERVER, &Fake::off(), never), json!({}));
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
            assert_eq!(answer(input, SERVER, &Fake::off(), never), json!({}));
        }
    }

    #[test]
    fn a_call_core_kept_is_left_to_cursor_and_announced_with_its_chat() {
        let relay = Fake::new(true, true);
        let never = || -> Commands { panic!("a kept call names no commands") };
        let mut input: Value =
            serde_json::from_slice(&mcp_call(SERVER, "send_relay_message")).unwrap();
        input["tool_use_id"] = json!("toolu_1");
        input["generation_id"] = json!("g1");
        let output = answer(input.to_string().as_bytes(), SERVER, &relay, never);
        assert_eq!(output, json!({}));
        assert_eq!(
            relay.announced.borrow().as_slice(),
            [json!({
                "conversation_id": "c1",
                "generation_id": "g1",
                "tool_use_id": "toolu_1",
                "tool_name": "send_relay_message",
                "tool_input": "{}",
            })]
        );
    }

    #[test]
    fn a_call_core_refused_keeps_the_identity_tool_denial_only_while_not_attested() {
        // Core cannot say why (an unclassified refusal): its status decides.
        let relay = Fake::new(false, false);
        let output = answer(
            &mcp_call(SERVER, "send_relay_message"),
            SERVER,
            &relay,
            commands(Some("/tmp/r.sock")),
        );
        assert_eq!(output["permission"], "deny");
        let message = output["agent_message"].as_str().unwrap();
        assert!(message.contains("core_refused"), "{message}");
        assert_eq!(relay.status_asked.get(), 1);
        let attesting = Fake::new(true, false);
        let never = || -> Commands { panic!("an attested call names no commands") };
        let output = answer(
            &mcp_call(SERVER, "send_relay_message"),
            SERVER,
            &attesting,
            never,
        );
        assert_eq!(output, json!({}));
        let other = answer(
            &mcp_call("other-server", "send_relay_message"),
            SERVER,
            &relay,
            commands(None),
        );
        assert_eq!(other, json!({}));
        assert_eq!(
            relay.announced.borrow().len(),
            1,
            "another server's call is never announced"
        );
    }

    #[test]
    fn a_refused_identity_call_is_left_to_the_helper_while_attesting() {
        let never = || -> Commands { panic!("an attested call names no commands") };
        for class in [
            "cursor_chat_unknown",
            "cursor_hook_unverified",
            "deadline_expired",
        ] {
            let relay = Fake::refusing(false, class);
            let output = answer(
                &mcp_call(SERVER, "send_relay_message"),
                SERVER,
                &relay,
                never,
            );
            assert_eq!(output, json!({}), "{class}");
            assert_eq!(relay.status_asked.get(), 0, "{class} says Core attests");
        }
    }

    #[test]
    fn a_refused_identity_call_is_denied_with_its_command_when_not_attesting() {
        let relay = Fake::refusing(true, "cursor_attestation_unavailable");
        let output = answer(
            &mcp_call(SERVER, "send_relay_message"),
            SERVER,
            &relay,
            commands(Some("/tmp/r.sock")),
        );
        assert_eq!(output["permission"], "deny");
        let message = output["agent_message"].as_str().unwrap();
        assert!(
            message.contains("/opt/relay/agent-relay-probe send --socket /tmp/r.sock"),
            "{message}"
        );
        assert!(
            message.contains("cursor_attestation_unavailable"),
            "{message}"
        );
    }

    #[test]
    fn a_read_call_is_left_to_cursor_without_asking_core() {
        let relay = Fake::new(true, true);
        let never = || -> Commands { panic!("a read names no commands") };
        for tool in [
            "list_relay_agents",
            "describe_capabilities",
            "list_relay_channels",
        ] {
            assert_eq!(
                answer(&mcp_call(SERVER, tool), SERVER, &relay, never),
                json!({})
            );
        }
        assert!(relay.announced.borrow().is_empty());
        assert_eq!(relay.status_asked.get(), 0);
    }

    #[test]
    fn session_start_while_attested_says_the_identity_tools_work() {
        let relay = Fake::new(true, false);
        let output = answer(
            br#"{"hook_event_name":"sessionStart"}"#,
            SERVER,
            &relay,
            commands(Some("/tmp/r.sock")),
        );
        let context = output["additional_context"].as_str().unwrap();
        assert!(context.contains("has its own relay identity through the Agent Relay MCP server"));
        assert!(context.contains("/opt/relay/agent-relay-probe whoami --socket /tmp/r.sock"));
        assert!(!context.contains("do not work here"));
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
