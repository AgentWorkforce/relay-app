//! Copilot's local exact-call intents and dispatch-time hosted MCP transport.
//! A pending intent is not a Cloud proof or a replacement for native ownership.
mod control;
mod forward;
pub mod observer;
mod stdio;

pub(crate) const ROUTE: &str = "/copilot/mcp";

/// Runs only when Copilot launches this app-owned MCP helper normally.
pub fn main(args: &[String]) -> i32 {
    stdio::run(args)
}
pub(crate) mod intent;
pub(crate) mod peer;

pub(crate) use intent::Registry;

pub(crate) fn issue(
    account: &crate::account::Account,
    pid: i32,
    uid: u32,
    sid: &str,
    tool: &str,
    args: serde_json::Value,
    owner: intent::Owner,
) -> Result<serde_json::Value, agent_relay_node::relay::outbound::OutboundError> {
    use serde_json::json;
    let installer = account.mcp_config.get().ok_or_else(unavailable)?;
    let peer = account
        .copilot_dispatch
        .verifier
        .hook(installer, pid, uid, sid)
        .map_err(|_| unavailable())?;
    let journal = control::anchor(&peer::journal(installer, sid)).map_err(|_| unavailable())?;
    let call = intent::Call {
        sid: sid.into(),
        tool: tool.into(),
        args,
        owner,
        peer,
        journal,
        authority: installer.copilot_epoch(),
    };
    account
        .copilot_dispatch
        .retire_invalid(&call, |ticket, pending| {
            control::inspect(
                &peer::journal(installer, &pending.sid),
                pending,
                ticket,
                chrono::Utc::now().timestamp_millis(),
            )
            .is_err()
        });
    let (ticket, call) = account
        .copilot_dispatch
        .issue(call, std::time::Instant::now())
        .map_err(|_| unavailable())?;
    let mut args = call.args;
    args[crate::claude_attestation::FIELD] = serde_json::json!(ticket);
    Ok(json!({"modifiedArgs":args}))
}

const UNAVAILABLE: &str = "Agent Relay could not verify this Copilot call. Reconnect the managed MCP server. Do not repeat an ambiguous send.";

fn unavailable() -> agent_relay_node::relay::outbound::OutboundError {
    agent_relay_node::relay::outbound::OutboundError::new(
        409,
        "copilot_dispatch_unavailable",
        UNAVAILABLE,
    )
}

/// Every refusal class this code or the public validator produces. A closed
/// list: the class reaches logs and `error.data.class`, so nothing outside it
/// may pass, whatever its shape.
pub(crate) const REFUSAL_CLASSES: &[&str] = &[
    // Route.
    "authority_changed",
    "owner_changed",
    "native_peer_changed",
    "caller_unverified",
    "request_invalid",
    "ticket_missing",
    "intent_mismatch",
    "binding_mismatch",
    "account_changed",
    "provider_unconfigured",
    "provider_changed",
    "deadline_expired",
    "approval_pending",
    "attestation_key_unavailable",
    "attestation_failed",
    "outcome_unknown",
    // Core and stdio.
    "core_refused",
    "core_unreachable",
    "refused",
    "copilot_dispatch_unavailable",
    // Peer verification.
    "native_platform_unavailable",
    "native_peer_unavailable",
    "native_chat_unavailable",
    // Approval journal.
    "native_control_unavailable",
    "native_control_replaced",
    "native_control_incomplete",
    "native_control_overflow",
    "native_control_invalid",
    "native_call_unavailable",
    "native_call_mismatch",
    "native_call_cancelled",
    "native_call_ambiguous",
    "native_call_already_completed",
    "native_permission_mismatch",
    "native_permission_unavailable",
    "native_permission_denied",
    "native_permission_expired",
    // Intent registry.
    "invalid_call",
    "intent_capacity",
    "intent_unavailable",
    "dispatch_capacity",
    "unknown_or_expired_intent",
    "intent_changed_or_expired",
    "already_dispatched_result_unavailable",
    "dispatch_in_doubt_do_not_repeat",
    "intent_rejected",
];

/// A diagnostic refusal class: a member of [`REFUSAL_CLASSES`]. Anything else
/// is reported as `refused`, so no ticket, token, session, argument, path,
/// account value or unknown Core code can pass.
pub(crate) fn refusal_class(reason: &str) -> &str {
    if REFUSAL_CLASSES.contains(&reason) {
        reason
    } else {
        "refused"
    }
}

/// Records why Core refused a native call: the class only, never call data.
fn log_refusal(class: &str) {
    eprintln!(
        "[copilot-dispatch] refused a native call: {}",
        refusal_class(class)
    );
}

/// The route's refusal. The stdio helper reads the class from the error code.
fn refused(reason: &'static str) -> agent_relay_node::relay::outbound::OutboundError {
    log_refusal(reason);
    agent_relay_node::relay::outbound::OutboundError::new(409, refusal_class(reason), UNAVAILABLE)
}

/// Adds the refusal class to a JSON-RPC error without changing its message.
pub(crate) fn classified(mut reply: serde_json::Value, class: &str) -> serde_json::Value {
    if reply["error"].is_object() {
        reply["error"]["data"] = serde_json::json!({ "class": refusal_class(class) });
    }
    reply
}

mod route;
pub(crate) use route::route;

#[cfg(test)]
mod refusal_class_tests {
    use super::*;

    #[test]
    fn refusal_class_is_a_closed_allowlist() {
        for class in REFUSAL_CLASSES {
            assert_eq!(refusal_class(class), *class);
        }
        for reason in [
            "session_abc123",
            &"0123456789abcdef".repeat(4),
            "ticket tk_secret /Users/me",
            "",
            "Authority_Changed",
        ] {
            assert_eq!(refusal_class(reason), "refused", "{reason}");
        }
    }

    #[test]
    fn route_classes_are_all_listed() {
        for class in [
            "authority_changed",
            "owner_changed",
            "native_peer_changed",
            "caller_unverified",
            "request_invalid",
            "ticket_missing",
            "intent_mismatch",
            "binding_mismatch",
            "account_changed",
            "provider_unconfigured",
            "provider_changed",
            "deadline_expired",
            "approval_pending",
            "attestation_key_unavailable",
            "attestation_failed",
            "outcome_unknown",
        ] {
            assert!(REFUSAL_CLASSES.contains(&class), "{class}");
        }
    }
}
