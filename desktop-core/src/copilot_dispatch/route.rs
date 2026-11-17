//! Only the local peer-authenticated socket can reach this transport. The
//! provider grant and dispatch-time proof stay in Core for the entire request.
use super::{
    classified, control, forward,
    intent::{Begin, Call, Owner},
    log_refusal, peer, refused,
};
use crate::{
    account::Account, claude_attestation::FIELD, desktop_identity::Identities,
    mcp_settings::Binding,
};
use agent_relay_node::relay::{
    daemon::Core,
    outbound::{Outbound, OutboundError, Request},
    process::ProcessInfo,
    store::{SessionKind, Target},
};
pub(crate) mod protocol;
use protocol::request_valid;
#[cfg(any(debug_assertions, test))]
use protocol::request_shape;
#[cfg(test)]
use protocol::{catalog, transport_capabilities};

use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    binding: Binding,
    request: Value,
}

fn owner(core: &Arc<Core>, sid: &str) -> Option<Owner> {
    let account = core.plugins.cloud.account()?;
    let workspace = core.workspace()?;
    let target = Target::new(SessionKind::Copilot, sid);
    let (registration, source) = Identities::new(core.clone()).accepted_copilot_identity(&target)?;
    if registration.base_url != workspace.base_url
        || registration.workspace_key.as_ref().is_some_and(|key| key != &workspace.key) {
        return None;
    }
    Some(Owner {
        site: account.site_url,
        user: account.user_id?,
        workspace: account.workspace_id,
        generation: core.plugins.cloud.identity_sync_generation(),
        source,
        registration: super::intent::registration_key(&registration),
    })
}

fn current(core: &Arc<Core>, call: &Call, binding: &Binding) -> bool {
    call.owner.site == binding.site_url
        && call.owner.user == binding.account_id
        && call.owner.workspace == binding.workspace_id
        && owner(core, &call.sid).as_ref() == Some(&call.owner)
        && Identities::new(core.clone()).copilot_attestation_verified()
}

/// The signed-in account is still the one `binding` names.
pub(crate) fn binding_current(core: &Core, binding: &Binding) -> bool {
    core.plugins.cloud.account().is_some_and(|account| {
        account.site_url == binding.site_url
            && account.workspace_id == binding.workspace_id
            && account.user_id.as_deref() == Some(binding.account_id.as_str())
    })
}

async fn approved(
    registry: &super::Registry,
    installer: &crate::mcp_settings::Installer,
    call: &Call,
    ticket: &str,
    deadline: Instant,
) -> Result<(), OutboundError> {
    let until = (Instant::now() + Duration::from_millis(1500)).min(deadline);
    loop {
        let gate = control::inspect(
            &peer::journal(installer, &call.sid),
            call,
            ticket,
            chrono::Utc::now().timestamp_millis(),
        )
        .map_err(|reason| {
            registry.reject(ticket);
            refused(reason)
        })?;
        match gate {
            control::Gate::Ready => return Ok(()),
            control::Gate::Waiting if Instant::now() < until => {
                tokio::time::sleep(Duration::from_millis(25)).await
            }
            _ => return Err(refused("approval_pending")),
        }
    }
}

fn intent(account: &Account, request: &Value) -> Result<Option<(String, Call)>, OutboundError> {
    if request["method"] != "tools/call" {
        return Ok(None);
    }
    let tool = request["params"]["name"]
        .as_str()
        .ok_or_else(|| refused("request_invalid"))?;
    if !crate::copilot_attestation::native_tool(tool) {
        return Ok(None);
    }
    let mut args = request["params"]["arguments"]
        .as_object()
        .cloned()
        .ok_or_else(|| refused("request_invalid"))?;
    let ticket = args
        .remove(FIELD)
        .and_then(|s| s.as_str().map(str::to_owned))
        .ok_or_else(|| refused("ticket_missing"))?;
    let call = account
        .copilot_dispatch
        .call(&ticket, Instant::now())
        .map_err(refused)?;
    if call.tool != tool || call.args != Value::Object(args) {
        return Err(refused("intent_mismatch"));
    }
    Ok(Some((ticket, call)))
}

pub(crate) async fn route(
    account: Arc<Account>,
    outbound: &Outbound,
    pid: i32,
    uid: u32,
    request: &Request,
) -> Result<Value, OutboundError> {
    let deadline = Instant::now() + super::DISPATCH_BUDGET;
    if uid != agent_relay_node::relay::peer::current_uid() {
        return Err(refused("caller_unverified"));
    }
    if request.body.len() > 1 << 20 {
        #[cfg(debug_assertions)]
        eprintln!("[copilot-dispatch] rejected request: body too large");
        return Err(refused("request_invalid"));
    }
    let envelope: Envelope =
        serde_json::from_slice(&request.body).map_err(|_| {
            #[cfg(debug_assertions)]
            eprintln!("[copilot-dispatch] rejected request: invalid envelope");
            refused("request_invalid")
        })?;
    if !request_valid(&envelope.request) {
        #[cfg(debug_assertions)]
        eprintln!(
            "[copilot-dispatch] rejected request shape: {}",
            request_shape(&envelope.request)
        );
        return Err(refused("request_invalid"));
    }
    #[cfg(debug_assertions)]
    eprintln!(
        "[copilot-dispatch] accepted request shape: {}",
        request_shape(&envelope.request)
    );
    if !binding_current(&outbound.core, &envelope.binding) {
        return Err(refused("binding_mismatch"));
    }
    let generation = outbound.core.plugins.cloud.identity_sync_generation();
    let id = envelope.request["id"].clone();
    let installer = account
        .mcp_config
        .get()
        .cloned()
        .ok_or_else(|| refused("provider_unconfigured"))?;
    let authority = installer.copilot_epoch();
    let runtime = account
        .copilot_dispatch
        .verifier
        .runtime(&installer, &envelope.binding, pid, uid)
        .map_err(refused)?;
    let pending = intent(&account, &envelope.request)?;
    if let Some((ticket, call)) = &pending {
        let stale = if call.authority != authority {
            Err("authority_changed")
        } else if !current(&outbound.core, call, &envelope.binding) {
            Err("owner_changed")
        } else {
            account
                .copilot_dispatch
                .verifier
                .held(&installer, runtime, &call.sid)
                .and_then(|held| {
                    (held == call.peer)
                        .then_some(())
                        .ok_or("native_peer_changed")
                })
        };
        if let Err(reason) = stale {
            account.copilot_dispatch.reject(ticket);
            return Err(refused(reason));
        }
        if let Some(mut cached) = account
            .copilot_dispatch
            .cached(ticket, call, Instant::now())
            .map_err(refused)?
        {
            cached["id"] = envelope.request["id"].clone();
            return Ok(cached);
        }
        approved(
            &account.copilot_dispatch,
            &installer,
            call,
            ticket,
            deadline,
        )
        .await?;
    }
    if !binding_current(&outbound.core, &envelope.binding) {
        return Err(refused("binding_mismatch"));
    }
    if outbound.core.plugins.cloud.identity_sync_generation() != generation {
        return Err(refused("account_changed"));
    }
    let permit = account.copilot_dispatch.http_permit().map_err(refused)?;
    let dispatch = Dispatch {
        account,
        core: outbound.core.clone(),
        installer,
        caller: (pid, uid),
        runtime,
        envelope,
        pending,
        deadline,
        generation,
        authority,
        _permit: permit,
    };
    let reply = tokio::task::spawn_blocking(move || dispatch.run())
        .await
        .unwrap_or_else(|_| {
            classified(
                forward::error(
                    &id,
                    -32000,
                    "The MCP outcome is unknown. Do not repeat an ambiguous send.",
                ),
                "outcome_unknown",
            )
        });
    Ok(reply)
}

struct Dispatch {
    account: Arc<Account>,
    core: Arc<Core>,
    installer: Arc<crate::mcp_settings::Installer>,
    caller: (i32, u32),
    runtime: ProcessInfo,
    envelope: Envelope,
    pending: Option<(String, Call)>,
    deadline: Instant,
    generation: u64,
    authority: u64,
    _permit: super::intent::Permit,
}
impl Dispatch {
    fn valid(&self) -> Result<(), &'static str> {
        if Instant::now() >= self.deadline {
            return Err("deadline_expired");
        }
        if self.installer.copilot_epoch() != self.authority {
            return Err("authority_changed");
        }
        if !binding_current(&self.core, &self.envelope.binding) {
            return Err("binding_mismatch");
        }
        if self.core.plugins.cloud.identity_sync_generation() != self.generation {
            return Err("account_changed");
        }
        let runtime = self.account.copilot_dispatch.verifier.runtime(
            &self.installer,
            &self.envelope.binding,
            self.caller.0,
            self.caller.1,
        )?;
        if runtime != self.runtime {
            return Err("native_peer_changed");
        }
        Ok(())
    }
    fn run(self) -> Value {
        self.run_with(forward::post_before)
    }
    fn native_current(&self, ticket: &str, call: &Call) -> Result<(), &'static str> {
        if call.authority != self.installer.copilot_epoch() {
            return Err("authority_changed");
        }
        if !current(&self.core, call, &self.envelope.binding) {
            return Err("owner_changed");
        }
        let held = self.account.copilot_dispatch.verifier.held(
            &self.installer,
            self.runtime,
            &call.sid,
        )?;
        if held != call.peer {
            return Err("native_peer_changed");
        }
        match control::inspect(
            &peer::journal(&self.installer, &call.sid),
            call,
            ticket,
            chrono::Utc::now().timestamp_millis(),
        )? {
            control::Gate::Ready => Ok(()),
            control::Gate::Waiting => Err("approval_pending"),
        }
    }
    fn authority_current(
        &self,
        hosted: &crate::mcp_settings::providers::HostedConfig,
    ) -> Result<(), &'static str> {
        (self
            .installer
            .copilot_dispatch_hosted(&self.envelope.binding)
            .as_ref()
            == Some(hosted))
        .then_some(())
        .ok_or("provider_changed")
    }
    /// Every check before the proof is minted and again before it is posted.
    fn pending_current(
        &self,
        hosted: &crate::mcp_settings::providers::HostedConfig,
    ) -> Result<(), &'static str> {
        if let Some((ticket, call)) = &self.pending {
            self.native_current(ticket, call)?;
        }
        self.authority_current(hosted)?;
        self.valid()
    }
    fn run_with(
        self,
        post: impl FnOnce(&Binding, &str, &Value, Instant) -> Result<Value, &'static str>,
    ) -> Value {
        self.run_after_resolve(post, |_| {})
    }
    fn run_after_resolve(
        mut self,
        post: impl FnOnce(&Binding, &str, &Value, Instant) -> Result<Value, &'static str>,
        after_resolve: impl FnOnce(&Self),
    ) -> Value {
        let id = self.envelope.request["id"].clone();
        let refused = |class: &'static str| {
            log_refusal(class);
            classified(forward::error(&id,-32000,"Agent Relay could not verify this call at dispatch. Reconnect Copilot; do not repeat an ambiguous send."), class)
        };
        if let Err(class) = self.valid() {
            return refused(class);
        }
        let Some(hosted) = self
            .installer
            .copilot_dispatch_hosted(&self.envelope.binding)
        else {
            return refused("provider_unconfigured");
        };
        let Some(bearer) = hosted
            .headers
            .get("Authorization")
            .and_then(|s| s.strip_prefix("Bearer "))
        else {
            return refused("provider_unconfigured");
        };
        // Resolve slow key I/O before the final ownership/control/configuration
        // gate. The only proof is minted after that gate, inside this worker.
        let key = if self.pending.is_some() {
            self.account
                .attestation_key
                .get(&self.core.config.app_data_dir)
        } else {
            None
        };
        after_resolve(&self);
        if let Some((ticket, call)) = &self.pending {
            if let Err(class) = self.pending_current(&hosted) {
                self.account.copilot_dispatch.reject(ticket);
                return refused(class);
            }
            let Some(key) = key else {
                return refused("attestation_key_unavailable");
            };
            match self
                .account
                .copilot_dispatch
                .begin(ticket, call, Instant::now())
            {
                Ok(Begin::Cached(mut value)) => {
                    value["id"] = id;
                    return value;
                }
                Ok(Begin::Send) => {}
                Err(class) => return refused(class),
            }
            let Some(proof) = crate::copilot_attestation::sign(
                &key,
                &call.sid,
                &call.owner.source,
                &call.tool,
                &call.args,
                &call.owner.workspace,
                &call.owner.user,
            ) else {
                let response = refused("attestation_failed");
                self.account
                    .copilot_dispatch
                    .finish(ticket, Some(response.clone()));
                return response;
            };
            self.envelope.request["params"]["arguments"] = call.args.clone();
            self.envelope.request["params"]["arguments"][FIELD] = json!(proof);
        }
        // Provider deselection, removal, replacement or grant rotation through
        // ordinary UI actions revokes a queued call before it crosses network.
        let reply = match self.pending_current(&hosted) {
            // This bridge speaks the initialize-based protocol to its hosted
            // server. Answer the modern discovery probe as an unsupported RPC
            // so clients can negotiate the legacy handshake. Never forward a
            // modern request with our legacy HTTP protocol-version header.
            Ok(()) if self.envelope.request["method"] == "server/discover" => Some(
                forward::error(&id, -32601, "Use initialize to negotiate this MCP connection."),
            ),
            Ok(()) => post(
                &self.envelope.binding,
                bearer,
                &self.envelope.request,
                self.deadline,
            )
            .ok(),
            Err(class) => Some(refused(class)),
        };
        if let Some((ticket, _)) = &self.pending {
            self.account.copilot_dispatch.finish(ticket, reply.clone());
        }
        let mut reply = reply.unwrap_or_else(|| {
            classified(
                forward::error(
                    &id,
                    -32000,
                    "The MCP outcome is unknown. Do not repeat an ambiguous send.",
                ),
                "outcome_unknown",
            )
        });
        if reply.is_object() {
            reply["id"] = id;
        }
        protocol::normalize_reply(
            self.envelope.request["method"].as_str().unwrap_or_default(),
            &mut reply,
        );
        reply
    }
}

#[cfg(test)]
mod tests;
