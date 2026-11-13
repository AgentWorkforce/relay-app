//! Agent Relay Desktop's `agent-relay-probe`: the public core's command line
//! with the private layer's plugins and commands, and the session collector's.

use agent_relay_desktop_core::{
    account, claude_attestation, command_line, feed_headers, feed_verify, history, mcp_headers,
};
use agent_relay_node::relay::daemon::{self, PluginBuilder};

fn main() {
    // Before any startup work, so the core and the collector notice an
    // installer replacing this executable while they are still starting.
    agent_relay_desktop_core::executable_identity::capture_at_start();
    daemon::set_plugin_builder(PluginBuilder {
        profile: account::client_profile,
        session_sources: history::sources::session_sources,
        finish: |local, config, options| account::with_account(local, config, options.headless),
    });
    let given: Vec<String> = std::env::args().skip(1).collect();
    // Hooks and header helpers earlier versions wrote say `relay <command>`.
    let arguments = command_line::without_relay_word(given.clone());
    if let Some((text, code)) = command_line::answer(&arguments) {
        if code == 0 {
            print!("{text}");
        } else {
            eprint!("{text}");
        }
        std::process::exit(code);
    }
    if let [command, rest @ ..] = arguments.as_slice() {
        match command.as_str() {
            "copilot-mcp-observe" => std::process::exit(
                agent_relay_desktop_core::copilot_dispatch::observer::run(rest),
            ),
            "copilot-mcp" => {
                std::process::exit(agent_relay_desktop_core::copilot_dispatch::main(rest))
            }
            "setup" => std::process::exit(agent_relay_desktop_core::onboarding::run(rest)),
            "mcp-headers" => std::process::exit(mcp_headers::run(rest)),
            "feed-headers" => std::process::exit(feed_headers::run(rest)),
            "feed" => std::process::exit(feed_verify::run(rest)),
            "service-callers" => std::process::exit(
                agent_relay_desktop_core::service_callers::command::run(rest),
            ),
            "claude-attestation-hook" => std::process::exit(claude_attestation::hook::main(rest)),
            "teammate-context-hook" => {
                std::process::exit(agent_relay_desktop_core::teammate_context::main(rest))
            }
            _ => {}
        }
    }
    if arguments.len() != given.len() {
        // The core parses the process's own arguments: run it without the word.
        run_again(&arguments);
    }
    if history::probe::surface::handles(&arguments) {
        history::probe::main();
    } else {
        agent_relay_node::cli::main();
    }
}

/// Runs this binary again with `arguments`, in this process where the
/// platform can, and exits as it does.
fn run_again(arguments: &[String]) -> ! {
    let mut command = std::process::Command::new(
        std::env::current_exe()
            .unwrap_or_else(|_| std::env::args().next().unwrap_or_default().into()),
    );
    command.args(arguments);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = command.exec();
        eprintln!("Agent Relay could not run {}: {error}", arguments[0]);
        std::process::exit(1);
    }
    #[cfg(windows)]
    {
        match command.status() {
            Ok(status) => std::process::exit(status.code().unwrap_or(1)),
            Err(error) => {
                eprintln!("Agent Relay could not run {}: {error}", arguments[0]);
                std::process::exit(1);
            }
        }
    }
}
