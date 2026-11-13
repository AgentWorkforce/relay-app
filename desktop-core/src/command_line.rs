//! The helper's one command surface. `agent-relay-probe` runs the core's
//! commands, the session collector's, and setup and the header and hook
//! helpers; each parses its own arguments, so `--help`, `--version` and a bare
//! invocation answer here for all of them.

use clap::Command;

/// The core's commands, as `agent_relay_node::cli` names them.
const CORE_COMMANDS: &[(&str, &str)] = &[
    ("serve", "Run the core daemon"),
    ("call", "Send one bridge request to the running core"),
    ("connect", "Temporary agent-to-agent rooms"),
    ("read", "Print the relayed message a pointer names"),
    ("send", "Send a message as the calling session"),
    ("agents", "List the agents the calling session can message"),
    ("whoami", "Print the calling session's name and address"),
    ("fetch-attachment", "Fetch an attachment the session was sent"),
    ("connect-send", "Send to the Relay Connect room the session joined"),
    ("subscribe-channel", "Join or leave a channel as the calling session"),
    ("socket-request", "Make a documented session-scoped request"),
    ("settings", "Print or change Agent Relay's settings"),
    ("subscribe", "Subscribe the calling session to a pull request"),
    ("codex-stop-hook", "Agent Relay's Codex hook"),
    ("devin-hook", "Agent Relay's Devin lifecycle hook"),
    ("cursor-hook", "Agent Relay's Cursor CLI hook"),
    ("copilot-hook", "Agent Relay's VS Code Copilot hook"),
    ("claude-session-hook", "Agent Relay's Claude Code SessionStart hook"),
];

/// This build's own commands, beside the core's and the collector's.
const DESKTOP_COMMANDS: &[(&str, &str)] = &[
    ("copilot-mcp", "Agent Relay's native Copilot MCP transport"),
    ("copilot-mcp-observe", "Agent Relay's native Copilot MCP event observer"),
    ("setup", "Sign in and set up this computer without the app"),
    ("mcp-headers", "Print the hosted MCP headers for this account"),
    ("feed-headers", "Print the release feed headers for this account"),
    ("feed", "Verify a signed release feed manifest (feed verify)"),
    ("service-callers", "List service callers, or ask Settings to provision or rotate one"),
    ("claude-attestation-hook", "Agent Relay's Claude Code tool-call proof hook"),
    ("teammate-context-hook", "Retired Claude Code SessionStart hook; prints nothing"),
];

/// Every command the helper answers, for `--help`.
fn command() -> Command {
    let collector = crate::history::probe::surface::commands();
    let named = |(name, about): &(&'static str, &'static str)| Command::new(*name).about(*about);
    Command::new("agent-relay-probe")
        .version(agent_relay_node::VERSION)
        .about("Connect coding sessions to Agent Relay")
        .subcommands(DESKTOP_COMMANDS.iter().map(named))
        .subcommands(CORE_COMMANDS.iter().map(named))
        .subcommands(collector)
}

/// Answers a request about the helper itself: help, its version, or nothing
/// asked. `None` for a command, which runs.
pub fn answer(arguments: &[String]) -> Option<(String, i32)> {
    match arguments {
        // Installers ask before starting this binary. This must stay independent
        // of accounts, the running core, and opening/migrating any local store.
        [only] if only == "--local-storage-capabilities" => Some((
            crate::storage_format::capabilities(),
            0,
        )),
        [] => Some((command().render_help().to_string(), 2)),
        [only] if ["-h", "--help", "help"].contains(&only.as_str()) => {
            Some((command().render_help().to_string(), 0))
        }
        [only] if ["-V", "--version"].contains(&only.as_str()) => Some((
            format!("agent-relay-probe {}\n", agent_relay_node::VERSION),
            0,
        )),
        _ => None,
    }
}

/// Core commands `--help` does not list that earlier versions also wrote
/// into other tools' settings in the `relay <command>` form.
const HIDDEN_CORE_COMMANDS: &[&str] = &[
    "muse-spawn-host",
    "muse-spawn-mcp",
    "devin-spawn-host",
    "devin-spawn-mcp",
];

/// `arguments` without the `relay` word earlier versions put before the
/// helper's and the core's commands. Hooks and header helpers they wrote
/// (`<exe> relay claude-attestation-hook`, `<exe> relay mcp-headers`, a
/// provider's `relay <hook>`) keep running until setup repair rewrites them.
pub fn without_relay_word(mut arguments: Vec<String>) -> Vec<String> {
    let earlier_form = matches!(arguments.as_slice(), [first, next, ..]
        if first == "relay"
            && (DESKTOP_COMMANDS.iter().chain(CORE_COMMANDS).any(|(name, _)| name == next)
                || HIDDEN_CORE_COMMANDS.contains(&next.as_str())));
    if earlier_form {
        arguments.remove(0);
    }
    arguments
}

/// The commands `--help` lists, for the binary's test to run each one.
pub fn listed() -> Vec<String> {
    command()
        .get_subcommands()
        .filter(|command| !command.is_hide_set())
        .map(|command| command.get_name().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installers_can_read_storage_capabilities_without_opening_history() {
        let (answer, code) = super::answer(&["--local-storage-capabilities".into()]).unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&answer).unwrap(),
            serde_json::json!({"protocol":1,"upload_queue_formats":[0,1]})
        );
        assert!(super::answer(&["--local-storage-capabilities".into(), "extra".into()]).is_none());
    }

    #[test]
    fn help_lists_setup_the_core_and_the_collector() {
        let (help, code) = answer(&["--help".to_string()]).unwrap();
        assert_eq!(code, 0);
        for name in ["setup", "mcp-headers", "serve", "call", "status", "cloud", "sharing"] {
            assert!(help.contains(name), "{name} in {help}");
        }
        assert!(help.starts_with("Connect coding sessions to Agent Relay"));
    }

    #[test]
    fn the_version_names_this_binary_and_a_bare_call_shows_help() {
        let (version, code) = answer(&["--version".to_string()]).unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            version,
            format!("agent-relay-probe {}\n", agent_relay_node::VERSION)
        );
        assert_eq!(answer(&[]).unwrap().1, 2);
        assert!(answer(&["status".to_string()]).is_none());
    }

    #[test]
    fn the_earlier_relay_word_is_dropped_only_before_a_command() {
        let words = |list: &[&str]| list.iter().map(|word| word.to_string()).collect::<Vec<_>>();
        for (earlier, now) in [
            (
                &["relay", "claude-attestation-hook", "--socket", "s"][..],
                &["claude-attestation-hook", "--socket", "s"][..],
            ),
            (
                &["relay", "mcp-headers", "--site", "x"],
                &["mcp-headers", "--site", "x"],
            ),
            (&["relay", "setup"], &["setup"]),
            (&["relay", "codex-stop-hook"], &["codex-stop-hook"]),
            (&["relay", "cursor-hook", "stop"], &["cursor-hook", "stop"]),
            (&["relay", "devin-spawn-mcp"], &["devin-spawn-mcp"]),
            (&["relay", "serve", "--headless"], &["serve", "--headless"]),
        ] {
            assert_eq!(without_relay_word(words(earlier)), words(now));
        }
        for kept in [
            &["relay"][..],
            &["relay", "unknown"],
            &["relay", "--help"],
            &["setup", "relay"],
            &["claude-attestation-hook"],
        ] {
            assert_eq!(without_relay_word(words(kept)), words(kept));
        }
    }
}
