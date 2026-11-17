//! `cursor-mcp`: Cursor's per-chat attested hosted MCP transport.
//!
//! Cursor names no chat in an MCP call, so Cloud cannot act as any Cursor
//! chat from the call alone. Instead:
//! - the `beforeMCPExecution` hook (`cursor_guide`) announces each call of
//!   Agent Relay's server to Core ([`INTENT_ROUTE`]) with its chat, Cursor's
//!   `tool_use_id`, the tool and its arguments; Core keeps it only when the
//!   hook runs under the process holding that chat ([`intents`]);
//! - Cursor's `mcp.json` entry runs this build's probe as `cursor-mcp`, an
//!   app-owned stdio helper that hands each request to Core ([`ROUTE`]);
//! - Core matches a `tools/call` to one announced call by its exact tool and
//!   canonical arguments, consumes it, signs the chat's proof ([`proof`])
//!   into the arguments and forwards the call to the hosted MCP with the
//!   Cursor-bound grant it holds. Neither the hook nor the helper sees the
//!   grant, the key or the proof.
//!
//! Without Cloud's `cursor_session_attestation`, or without an announced
//! call, the call goes out unsigned, as before; an identity tool without one
//! is refused, naming the command that registers the chat. Windows keeps
//! Cursor's URL entry: its hooks are unavailable there.
pub(crate) mod intents;
pub(crate) mod proof;
mod route;
mod stdio;

pub use intents::Intents;
pub(crate) use route::{intent, route, status};

use crate::mcp_settings::Binding;
use agent_relay_node::relay::outbound::Request;
use std::path::Path;

/// The helper's subcommand, which Cursor's `mcp.json` entry runs.
pub const SUBCOMMAND: &str = "cursor-mcp";
/// The query parameter that marks Cursor's session-socket requests. The
/// pinned relay-node hands the account plugin only its own hook paths, so
/// Cursor's routes ride on Copilot's and are told apart by this parameter.
const QUERY: &str = "cursor";
/// Where the hook announces a call.
pub(crate) const INTENT_ROUTE: &str = "/copilot/hook?cursor=intent";
/// Where the hook asks whether Cursor's calls are attested.
pub(crate) const STATUS_ROUTE: &str = "/copilot/hook?cursor=status";
/// Where the helper hands Core a request.
pub(crate) const ROUTE: &str = "/copilot/mcp?cursor=mcp";

/// One of Cursor's session-socket routes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    Intent,
    Status,
    Mcp,
}

/// Which of Cursor's routes `request` asks, if any.
pub(crate) fn which(request: &Request) -> Option<Route> {
    if request.method != "POST" {
        return None;
    }
    let marker = request
        .query
        .iter()
        .find(|(name, _)| name == QUERY)
        .map(|(_, value)| value.as_str())?;
    match (request.path.as_str(), marker) {
        ("/copilot/hook", "intent") => Some(Route::Intent),
        ("/copilot/hook", "status") => Some(Route::Status),
        ("/copilot/mcp", "mcp") => Some(Route::Mcp),
        _ => None,
    }
}

/// Runs only when Cursor launches this app-owned MCP helper.
pub fn main(args: &[String]) -> i32 {
    stdio::run(args)
}

/// The helper's arguments after the executable, as `mcp.json` names them
/// and Core expects to find them on the calling process: the binding, a site
/// profile build's session socket, and `--dev` for the development build.
pub(crate) fn arguments(
    binding: &Binding,
    development: bool,
    socket: Option<&Path>,
) -> Vec<String> {
    let mut args = vec![
        SUBCOMMAND.into(),
        "--site".into(),
        binding.site_url.clone(),
        "--workspace".into(),
        binding.workspace_id.clone(),
        "--account".into(),
        binding.account_id.clone(),
    ];
    if let Some(socket) = socket {
        args.push("--socket".into());
        args.push(socket.to_string_lossy().into_owned());
    }
    if development {
        args.push("--dev".into());
    }
    args
}
