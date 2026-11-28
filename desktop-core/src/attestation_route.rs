//! The route Agent Relay's Claude Code PreToolUse hook asks
//! (`claude-attestation-hook`, [`crate::claude_attestation`]):
//!
//! ```text
//! POST /claude/attestation   {"session_id", "tool_name", "tool_use_id"}
//! → {"attestation": "<compact JWS>", "native_join", "handoff"}
//! ```
//!
//! A proof is signed only for a hosted Agent Relay tool called from the
//! Claude Code chat the caller runs inside, when that chat is registered
//! here and Cloud verifies Claude proofs for the signed-in account. When
//! Cloud takes proofs ahead of the call (`claude_attestation_handoff`), the
//! proof is handed to Cloud before this answers, and `handoff` tells the hook
//! to leave the call unchanged. A failed handoff is an error, never a
//! fallback to the arguments: Claude's auto mode refuses a rewritten call.
//!
//! When Cloud also waits for a proof that arrives after its call
//! (`claude_attestation_async_handoff`, declared by this login in
//! [`crate::desktop_identity::async_handoff`]), the route answers at once
//! with `pending` and hands the proof off in the background. A slow Cloud
//! then delays the call instead of failing the hook. A call from this chat
//! that gets no proof gets a marker instead, so Cloud stops waiting for it.
//! `join_relay` keeps the synchronous handoff: it is denied without a proof.
//! A background attempt is dropped once the account or its generation changes.

use crate::claude_attestation::{self as attestation, Claims};
use crate::desktop_identity::{async_handoff::handoff_path, claude::ClaudeIdentity, Identities};
use agent_relay_node::relay::cloud_session::{CloudAccount, CloudError};
use agent_relay_node::relay::outbound::{payload, Outbound, OutboundError, Purpose, Request};
use agent_relay_node::relay::store::{SessionKind, Target};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn field<'a>(fields: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    fields.get(name).and_then(Value::as_str)
}

fn invalid(message: &str) -> OutboundError {
    OutboundError::new(400, "invalid_request", message)
}

fn unavailable(message: &str) -> OutboundError {
    OutboundError::new(409, "attestation_unavailable", message)
}

/// The handoff waits at most this long, within the hook's own timeout.
const HANDOFF_TIMEOUT: Duration = Duration::from_secs(3);
const HANDOFF_RESERVE: Duration = Duration::from_secs(1);
const ROUTE_BUDGET: Duration = Duration::from_millis(3_750);
/// A background handoff (proof or marker) keeps retrying for this long:
/// less than the 10 seconds Cloud waits for it.
const BACKGROUND_BUDGET: Duration = Duration::from_secs(9);
/// One background request waits at most this long before it is retried.
const BACKGROUND_ATTEMPT: Duration = Duration::from_secs(5);
/// The pause before a background request is retried.
const BACKGROUND_RETRY: Duration = Duration::from_millis(250);

fn identity_stale() -> OutboundError {
    OutboundError::new(
        409,
        "claude_identity_stale",
        "Agent Relay could not confirm this chat's current desktop identity. Wait a moment and retry.",
    )
}

fn handoff_failed() -> OutboundError {
    OutboundError::new(
        503,
        "handoff_failed",
        "Agent Relay could not hand this chat's proof to the hosted service. Retry the call.",
    )
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

struct Binding {
    identities: Identities,
    account: CloudAccount,
    generation: u64,
    target: Target,
    identity: ClaudeIdentity,
    handoff: bool,
}

impl Binding {
    fn current(&self) -> bool {
        self.handoff == self.identities.claude_attestation_handoff()
            && self.identities.claude_identity_accepted(
                &self.account,
                self.generation,
                &self.target,
                &self.identity,
            )
    }

    async fn proof(
        &self,
        key: &attestation::Key,
        tool: &str,
        tool_use_id: &str,
        deadline: Instant,
    ) -> Result<String, OutboundError> {
        let sync_deadline = if self.handoff {
            deadline - HANDOFF_RESERVE
        } else {
            deadline
        };
        if !self
            .identities
            .sync_claude_identity(
                &self.account,
                self.generation,
                &self.target,
                &self.identity,
                sync_deadline,
            )
            .await
            || !self.identity.key_matches(&key.registration())
            || !self.current()
            || Instant::now() >= deadline
        {
            return Err(identity_stale());
        }
        let claims = Claims::new(
            &self.target.id,
            self.identity.source(),
            tool_use_id,
            tool,
            now_secs(),
        );
        let sender = self.identity.sender();
        if sender.binds_credential {
            key.sign_for_sender(
                claims,
                attestation::ProofSender {
                    agent_id: sender.agent_id(),
                    token: sender.token(),
                    link_address: &sender.link_address,
                    workspace_id: &self.account.workspace_id,
                    user_id: self.account.user_id.as_deref().ok_or_else(identity_stale)?,
                },
            )
        } else {
            key.sign(&claims)
        }
        .ok_or_else(|| unavailable("The proof could not be signed."))
    }

    async fn dispatch(&self, proof: &str, deadline: Instant) -> Result<(), OutboundError> {
        if Instant::now() >= deadline || !self.current() {
            return Err(identity_stale());
        }
        if !self.handoff {
            return Ok(());
        }
        let sent = tokio::time::timeout(
            deadline
                .saturating_duration_since(Instant::now())
                .min(HANDOFF_TIMEOUT),
            self.identities.plugins.cloud.request(
                "POST",
                handoff_path(&self.account),
                Some(json!({ "attestation": proof })),
            ),
        )
        .await;
        let failure = match sent {
            Ok(Ok(_)) => None,
            Ok(Err(_)) => Some("failed"),
            Err(_) => Some("timed out"),
        };
        if let Some(failure) = failure {
            agent_relay_node::relay::diagnostics::write(format_args!(
                "Claude proof handoff {failure}"
            ));
            return Err(handoff_failed());
        }
        self.current().then_some(()).ok_or_else(identity_stale)
    }

    /// The async handoff, after the hook was answered: the proof once it is
    /// signed (syncing this chat's identity first if Cloud lacks it), or a
    /// marker when it cannot be.
    async fn hand_off_in_background(
        self,
        key: Arc<attestation::Key>,
        tool: String,
        tool_use_id: String,
    ) {
        let deadline = Instant::now() + BACKGROUND_BUDGET;
        let marker = json!({ "unattested": tool_use_id });
        let mut outcome = match self.proof(&key, &tool, &tool_use_id, deadline).await {
            Ok(proof) => {
                let body = json!({ "attestation": proof });
                self.deposit(body, deadline).await
            }
            Err(_) => self.deposit(marker.clone(), deadline).await,
        };
        if outcome == Some("refused") {
            // Cloud will not take this proof: a marker spares the call the
            // wait for it. One already stored for the call is left alone.
            agent_relay_node::relay::diagnostics::write(format_args!(
                "Claude proof background handoff refused"
            ));
            outcome = self.deposit(marker, deadline).await;
        }
        if let Some(failure) = outcome {
            agent_relay_node::relay::diagnostics::write(format_args!(
                "Claude proof background handoff {failure}"
            ));
        }
    }

    async fn deposit(&self, body: Value, deadline: Instant) -> Option<&'static str> {
        deposit(&self.identities, &self.account, self.generation, body, deadline).await
    }
}

/// POSTs one handoff body until Cloud answers, a refusal, or `deadline`.
/// Returns why it gave up, if it did. Each attempt is for the account
/// generation the call was made in: after a sign-out, an account switch or a
/// new generation, the body is dropped instead of sent.
async fn deposit(
    identities: &Identities,
    account: &CloudAccount,
    generation: u64,
    body: Value,
    deadline: Instant,
) -> Option<&'static str> {
    let mut failure = "timed out";
    while Instant::now() < deadline {
        let cloud = &identities.plugins.cloud;
        if cloud.account().as_ref() != Some(account) || cloud.identity_sync_generation() != generation
        {
            return Some("dropped");
        }
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(BACKGROUND_ATTEMPT);
        let sent = tokio::time::timeout(
            wait,
            identities
                .plugins
                .cloud
                .request("POST", handoff_path(account), Some(body.clone())),
        )
        .await;
        match sent {
            Ok(Ok(_)) => return None,
            // A refusal comes again: the proof, or a marker, is not wanted.
            Ok(Err(CloudError::Status { status, .. }))
                if !matches!(status, 408 | 429 | 500..=599) =>
            {
                return Some("refused");
            }
            Ok(Err(CloudError::Rejected | CloudError::SignedOut | CloudError::GateOnly { .. })) => {
                return Some("refused");
            }
            Ok(Err(_)) => failure = "failed",
            Err(_) => failure = "timed out",
        }
        tokio::time::sleep(
            BACKGROUND_RETRY.min(deadline.saturating_duration_since(Instant::now())),
        )
        .await;
    }
    Some(failure)
}

/// In the background, tells Cloud that no proof is coming for this call.
fn spawn_marker(
    identities: Identities,
    account: CloudAccount,
    generation: u64,
    tool_use_id: String,
) {
    tokio::spawn(async move {
        let deadline = Instant::now() + BACKGROUND_BUDGET;
        let body = json!({ "unattested": tool_use_id });
        if let Some(failure) = deposit(&identities, &account, generation, body, deadline).await {
            agent_relay_node::relay::diagnostics::write(format_args!(
                "Claude proof marker {failure}"
            ));
        }
    });
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
    let deadline = Instant::now() + ROUTE_BUDGET;
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
    let identities = Identities::new(outbound.core.clone());
    let cloud = &outbound.core.plugins.cloud;
    let account = cloud
        .account()
        .ok_or_else(|| unavailable("Sign in to Agent Relay before joining this chat."))?;
    let generation = cloud.identity_sync_generation();
    if !identities.claude_attestation_verified() {
        return Err(unavailable(
            "The hosted service does not verify Claude chat identity yet.",
        ));
    }
    // The kernel names the chat the hook runs in; the hook's own
    // session_id must be that chat, so no other process can borrow it.
    let target = Target::new(SessionKind::ClaudeTerminal, session_id);
    let caller = outbound.identify(pid, uid).await?;
    if caller.target.kind != target.kind || !caller.target.id.eq_ignore_ascii_case(&target.id) {
        return Err(OutboundError::not_a_session());
    }
    // From here the caller is this chat. While Cloud may wait on this
    // login's calls, a call of this chat that gets no proof gets a marker.
    let markers = identities.claude_markers_expected();
    let marked = |error: OutboundError, identities: Identities| {
        if markers {
            spawn_marker(identities, account.clone(), generation, tool_use_id.to_string());
        }
        error
    };
    let prepared = (|| {
        let requested_name = requested_name(&fields, tool)?;
        check_registration(outbound, &target, requested_name.as_deref())?;
        let identity = identities
            .current_claude_identity(&account, generation, &target)
            .ok_or_else(identity_stale)?;
        let native_join = check_join(&identity, &fields, tool, requested_name.as_deref())?;
        let key = key.ok_or_else(|| unavailable("The attestation key is unavailable."))?;
        Ok((identity, native_join, key))
    })();
    let (identity, native_join, key) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => return Err(marked(error, identities)),
    };
    let handoff = identities.claude_attestation_handoff();
    // A join stays synchronous: it registers this chat's identity at Cloud,
    // so it never goes out before its proof is in, and a proof that fails
    // denies it rather than letting it run without the chat's identity.
    let background = handoff && tool != "join_relay" && identities.claude_async_handoff();
    let binding = Binding {
        identities,
        account: account.clone(),
        generation,
        target,
        identity,
        handoff,
    };
    if background {
        tokio::spawn(binding.hand_off_in_background(
            key,
            tool.to_string(),
            tool_use_id.to_string(),
        ));
        return Ok(json!({
            "attestation": null, "native_join": native_join, "handoff": true, "pending": true,
        }));
    }
    let signed = match binding.proof(&key, tool, tool_use_id, deadline).await {
        Ok(proof) => binding.dispatch(&proof, deadline).await.map(|()| proof),
        Err(error) => Err(error),
    };
    match signed {
        Ok(proof) => {
            Ok(json!({ "attestation": proof, "native_join": native_join, "handoff": handoff }))
        }
        Err(error) => Err(marked(error, binding.identities)),
    }
}

fn check_registration(
    outbound: &Outbound,
    target: &Target,
    requested_name: Option<&str>,
) -> Result<(), OutboundError> {
    let registration = outbound.core.store.read(|store| {
        store
            .registration_for(target)
            .filter(|registration| registration.missing_since.is_none())
            .cloned()
    });
    let mismatch = requested_name.is_some_and(|name| {
        registration
            .as_ref()
            .is_none_or(|registration| registration.display_name() != name)
    });
    require_matching_name(mismatch, outbound.core.allows_self_registration())?;
    registration.ok_or_else(OutboundError::not_registered)?;
    Ok(())
}

fn check_join(
    identity: &ClaudeIdentity,
    fields: &Map<String, Value>,
    tool: &str,
    requested_name: Option<&str>,
) -> Result<bool, OutboundError> {
    if tool != "join_relay" {
        return Ok(false);
    }
    let sender = identity.sender();
    if requested_name.is_some_and(|name| name != sender.display_name) {
        return Err(identity_stale());
    }
    if let Some(address) = fields.get("desktop_address") {
        if !address.as_str().is_some_and(|address| {
            address.trim() == sender.link_address || address.trim() == sender.address()
        }) {
            return Err(invalid("Use this chat's Agent Relay address."));
        }
    }
    Ok(requested_name.is_some())
}

#[cfg(test)]
pub(crate) mod tests;
