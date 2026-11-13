//! The route Agent Relay's Claude Code PreToolUse hook asks
//! (`claude-attestation-hook`, [`crate::claude_attestation`]):
//!
//! ```text
//! POST /claude/attestation   {"session_id", "tool_name", "tool_use_id"}
//! → {"attestation": "<compact JWS>"}
//! ```
//!
//! A proof is signed only for a hosted Agent Relay tool called from the
//! Claude Code chat the caller runs inside, when that chat is registered
//! here and Cloud verifies Claude proofs for the signed-in account.

use crate::desktop_identity::Identities;
use crate::claude_attestation::{self as attestation, Claims};
use agent_relay_node::relay::outbound::{payload, Outbound, OutboundError, Purpose, Request};
use agent_relay_node::relay::store::{SessionKind, Target};
use serde_json::{json, Map, Value};
use std::sync::Arc;

fn field<'a>(fields: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    fields.get(name).and_then(Value::as_str)
}

fn invalid(message: &str) -> OutboundError {
    OutboundError::new(400, "invalid_request", message)
}

fn unavailable(message: &str) -> OutboundError {
    OutboundError::new(409, "attestation_unavailable", message)
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn requested_name(
    fields: &Map<String, Value>,
    tool: &str,
) -> Result<Option<String>, OutboundError> {
    if tool != "join_relay" {
        return Ok(None);
    }
    fields
        .get("registration")
        .map(|requested| {
            requested
                .as_object()
                .and_then(|fields| fields.get("name"))
                .and_then(Value::as_str)
                .and_then(agent_relay_node::relay::agents::normalize_name)
                .ok_or_else(|| {
                    OutboundError::new(
                        400,
                        "invalid_name",
                        "Use 2–48 lowercase letters, digits and single dashes.",
                    )
                })
        })
        .transpose()
}

fn require_matching_name(mismatch: bool, allowed: bool) -> Result<(), OutboundError> {
    if !mismatch {
        return Ok(());
    }
    if !allowed {
        return Err(OutboundError::new(
            403,
            "not_allowed",
            "Turn on Let sessions put themselves on the relay in Agent Relay Settings first.",
        ));
    }
    Err(OutboundError::new(409, "registration_required", "Register this local chat under the requested name through the normal command permission check, then retry the join."))
}

/// Signs a proof for the hook in chat `pid`, for a tool on `server`: the
/// hosted entry this build installed.
pub(crate) async fn route(
    outbound: &Outbound,
    key: Option<Arc<attestation::Key>>,
    server: String,
    pid: i32,
    uid: u32,
    request: &Request,
) -> Result<Value, OutboundError> {
    let fields = payload(request, Purpose::Json)?;
    let session_id = field(&fields, "session_id")
        .map(str::to_ascii_lowercase)
        .filter(|id| agent_relay_node::relay::store::is_uuid(id))
        .ok_or_else(|| invalid("session_id must be a Claude Code session id."))?;
    let tool = field(&fields, "tool_name")
        .and_then(|name| attestation::tool_on(name, &server))
        .ok_or_else(|| invalid("tool_name must name a hosted Agent Relay tool."))?;
    let tool_use_id = field(&fields, "tool_use_id")
        .filter(|id| attestation::valid_tool_use_id(id))
        .ok_or_else(|| invalid("tool_use_id must be Claude's tool call id."))?;
    if !Identities::new(outbound.core.clone()).claude_attestation_verified() {
        return Err(unavailable(
            "The hosted service does not verify Claude chat identity yet.",
        ));
    }
    let account = outbound
        .core
        .plugins
        .cloud
        .account()
        .ok_or_else(|| unavailable("Sign in to Agent Relay before joining this chat."))?;
    let generation = outbound.core.plugins.cloud.identity_sync_generation();
    // The kernel names the chat the hook runs in; the hook's own
    // session_id must be that chat, so no other process can borrow it.
    let target = Target::new(SessionKind::ClaudeTerminal, session_id);
    let caller = outbound.identify(pid, uid).await?;
    if caller.target.kind != target.kind || !caller.target.id.eq_ignore_ascii_case(&target.id) {
        return Err(OutboundError::not_a_session());
    }
    let (registration, source) = outbound.core.store.read(|store| {
        (
            store
                .registration_for(&target)
                .filter(|registration| registration.missing_since.is_none())
                .cloned(),
            store.desktop_sync_source_id.clone(),
        )
    });
    let requested_name = requested_name(&fields, tool)?;
    let mismatch = requested_name.as_deref().is_some_and(|name| {
        registration
            .as_ref()
            .is_none_or(|registration| registration.display_name() != name)
    });
    require_matching_name(mismatch, outbound.core.allows_self_registration())?;
    let _registration = registration.ok_or_else(OutboundError::not_registered)?;
    let source = source.ok_or_else(OutboundError::not_registered)?;
    if tool == "join_relay" {
        // Registration happened through a separately approved command or the
        // app. Wait for its identity sync, but never register from PreToolUse.
        Identities::new(outbound.core.clone()).sync_desktop_identities().await;
    }
    if outbound.core.plugins.cloud.account().as_ref() != Some(&account)
        || outbound.core.plugins.cloud.identity_sync_generation() != generation
        || !Identities::new(outbound.core.clone()).claude_attestation_verified()
    {
        return Err(unavailable(
            "The Agent Relay sign-in changed. Retry the join in this chat.",
        ));
    }
    let native_join = tool == "join_relay" && requested_name.is_some();
    let key = key.ok_or_else(|| unavailable("The attestation key is unavailable."))?;
    let claims = Claims::new(&target.id, &source, tool_use_id, tool, now_secs());
    let proof = key
        .sign(&claims)
        .ok_or_else(|| unavailable("The proof could not be signed."))?;
    Ok(json!({ "attestation": proof, "native_join": native_join }))
}

#[cfg(test)]
pub(crate) mod tests;
