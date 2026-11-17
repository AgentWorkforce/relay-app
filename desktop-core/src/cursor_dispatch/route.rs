//! Core's two Cursor routes on the peer-authenticated session socket: the
//! hook's announcement of a call ([`intent`]) and the helper's MCP request
//! ([`route`]). The grant, the key and the proof stay in Core.
use super::intents::{Intent, Intents};
use super::proof;
use super::ANNOUNCE_BUDGET;
use crate::account::Account;
use crate::claude_attestation::FIELD;
use crate::copilot_dispatch::{
    binding_current, classified, forward, protocol::request_valid,
    protocol::transport_capabilities, refusal_class,
};
use crate::cursor_guide::IDENTITY_TOOLS;
use crate::desktop_identity::Identities;
use crate::mcp_settings::{Binding, Installer};
use agent_relay_node::relay::{
    cursor_cli,
    daemon::Core,
    outbound::{Outbound, OutboundError, Request},
    process::{ProcessTable, SystemProcesses},
    store::{SessionKind, Target},
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long Core spends on one helper request, the hosted reply included.
pub(super) const DISPATCH_BUDGET: Duration = Duration::from_secs(60);
/// How long the helper waits for Core's answer.
pub(super) const LOCAL_RESPONSE_BUDGET: Duration = Duration::from_secs(75);
const _: () = assert!(LOCAL_RESPONSE_BUDGET.as_millis() > DISPATCH_BUDGET.as_millis());

const REFUSED: &str = "Agent Relay could not attest this Cursor call.";
const UNKNOWN: &str = "The MCP outcome is unknown. Do not repeat an ambiguous send.";

fn log_refusal(class: &str) {
    eprintln!("[cursor-dispatch] refused a call: {}", refusal_class(class));
}

fn refused(class: &'static str) -> OutboundError {
    log_refusal(class);
    OutboundError::new(409, refusal_class(class), REFUSED)
}

/// Whether Cursor's calls are attested now: this build's helper and hooks
/// are Cursor's, and Cloud verifies Cursor proofs for the signed-in account
/// and its sync generation. Never on Windows. Reads Cursor's files.
pub(crate) fn attesting(account: &Account, core: &Arc<Core>) -> bool {
    !cfg!(windows)
        && account
            .mcp_config
            .get()
            .is_some_and(|installer| installer.cursor_attested_transport())
        && Identities::new(core.clone()).cursor_attestation_verified()
}

/// This installation's sync source id while chat `sid` is registered on
/// the relay in the current workspace.
fn registered(core: &Core, sid: &str) -> Option<String> {
    let workspace = core.workspace()?;
    let target = Target::new(SessionKind::CursorCli, sid);
    core.store.read(|store| {
        store.registration_for(&target).filter(|r| {
            r.missing_since.is_none()
                && r.base_url == workspace.base_url
                && r.workspace_key
                    .as_ref()
                    .is_none_or(|key| key == &workspace.key)
        })?;
        store.desktop_sync_source_id.clone()
    })
}

/// The arguments of a hook payload's `tool_input`: Cursor sends compact
/// JSON in a string.
fn hook_arguments(input: &Value) -> Result<Value, &'static str> {
    match input {
        Value::String(text) => serde_json::from_str(text).map_err(|_| "cursor_call_invalid"),
        Value::Object(_) | Value::Null => Ok(input.clone()),
        _ => Err("cursor_call_invalid"),
    }
}

/// An announced call, as the hook's payload names it.
fn announced(input: &Value, generation: u64) -> Result<Intent, &'static str> {
    let field = |name: &str| input[name].as_str().ok_or("cursor_call_invalid");
    let sid = field("conversation_id")?.to_ascii_lowercase();
    let tuid = field("tool_use_id")?;
    let tool = field("tool_name")?;
    // Only an identity tool carries a proof: Cloud refuses one on any other.
    if !proof::valid_tuid(tuid) || !IDENTITY_TOOLS.contains(&tool) {
        return Err("cursor_call_invalid");
    }
    let (_, args) = proof::arguments(&hook_arguments(&input["tool_input"])?)?;
    Ok(Intent {
        sid,
        tool: tool.into(),
        args,
        tuid: tuid.into(),
        generation,
    })
}

/// [`super::INTENT_ROUTE`]: Cursor's `beforeMCPExecution` hook announces a
/// call of one of Agent Relay's identity tools; any other tool is refused as
/// `cursor_call_invalid`. Kept only while Cursor's calls are
/// attested, the chat is on the relay, and the hook runs under the process
/// holding that chat.
pub(crate) async fn intent(
    account: Arc<Account>,
    outbound: &Outbound,
    pid: i32,
    uid: u32,
    request: &Request,
) -> Result<Value, OutboundError> {
    let started = Instant::now();
    if uid != agent_relay_node::relay::peer::current_uid() {
        return Err(refused("caller_unverified"));
    }
    if request.body.len() > 256 << 10 {
        return Err(refused("request_invalid"));
    }
    let input: Value =
        serde_json::from_slice(&request.body).map_err(|_| refused("request_invalid"))?;
    let core = outbound.core.clone();
    let generation = core.plugins.cloud.identity_sync_generation();
    let intent = announced(&input, generation).map_err(refused)?;
    let checked = {
        let (account, core, sid) = (account.clone(), core.clone(), intent.sid.clone());
        tokio::task::spawn_blocking(move || {
            accepted(&account, &core, pid, &sid, &SystemProcesses::default())
        })
        .await
        .unwrap_or(Err("cursor_hook_unverified"))
    };
    checked.map_err(refused)?;
    if core.plugins.cloud.identity_sync_generation() != generation {
        return Err(refused("account_changed"));
    }
    keep(
        &account.copilot_dispatch.cursor_intents,
        intent,
        started,
        Instant::now(),
    )
    .map_err(refused)?;
    Ok(json!({"accepted": true}))
}

/// Keeps `intent`, announced by a request that arrived at `started`, unless
/// the announce budget ran out: the hook may then have given up and denied
/// the call, and a kept intent would sign a retry with the wrong
/// `tool_use_id` or make another chat's identical call ambiguous.
fn keep(
    intents: &Intents,
    intent: Intent,
    started: Instant,
    now: Instant,
) -> Result<(), &'static str> {
    if now.saturating_duration_since(started) >= ANNOUNCE_BUDGET {
        return Err("deadline_expired");
    }
    intents.register(intent, now)
}

/// [`super::STATUS_ROUTE`]: whether Cursor's calls are attested now, which
/// the hook's `sessionStart` guidance follows.
pub(crate) async fn status(
    account: Arc<Account>,
    outbound: &Outbound,
    uid: u32,
) -> Result<Value, OutboundError> {
    if uid != agent_relay_node::relay::peer::current_uid() {
        return Err(refused("caller_unverified"));
    }
    let core = outbound.core.clone();
    let attesting = tokio::task::spawn_blocking(move || attesting(&account, &core))
        .await
        .unwrap_or(false);
    Ok(json!({"attesting": attesting}))
}

/// Every check on an announced call of chat `sid` by hook process `pid`.
fn accepted(
    account: &Account,
    core: &Arc<Core>,
    pid: i32,
    sid: &str,
    table: &dyn ProcessTable,
) -> Result<(), &'static str> {
    if !attesting(account, core) {
        return Err("cursor_attestation_unavailable");
    }
    if !cursor_cli::is_session_id(sid) || registered(core, sid).is_none() {
        return Err("cursor_chat_unknown");
    }
    let installer = account
        .mcp_config
        .get()
        .ok_or("cursor_attestation_unavailable")?;
    cursor_cli::hook_caller_holds(&installer.cursor_config(), table, pid, sid)
        .then_some(())
        .ok_or("cursor_hook_unverified")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Envelope {
    pub binding: Binding,
    pub request: Value,
}

/// Whether process `pid` is this build's helper, run as `mcp.json` names
/// it for `binding`.
fn helper_peer(
    installer: &Installer,
    binding: &Binding,
    pid: i32,
    uid: u32,
    table: &dyn ProcessTable,
) -> bool {
    let Some(caller) = table.process(pid).filter(|p| p.uid == uid) else {
        return false;
    };
    let expected = super::arguments(
        binding,
        installer.cursor_development(),
        installer.cursor_session_socket().as_deref(),
    );
    let same =
        |actual: &Path| same_file::is_same_file(actual, installer.cursor_helper()).unwrap_or(false);
    table.executable(pid).is_some_and(|actual| same(&actual))
        && table.arguments(pid).get(1..) == Some(expected.as_slice())
        && table.process(pid) == Some(caller)
}

/// [`super::ROUTE`]: one MCP request from the `cursor-mcp` helper.
pub(crate) async fn route(
    account: Arc<Account>,
    outbound: &Outbound,
    pid: i32,
    uid: u32,
    request: &Request,
) -> Result<Value, OutboundError> {
    let deadline = Instant::now() + DISPATCH_BUDGET;
    if uid != agent_relay_node::relay::peer::current_uid() {
        return Err(refused("caller_unverified"));
    }
    if request.body.len() > 1 << 20 {
        return Err(refused("request_invalid"));
    }
    let envelope: Envelope =
        serde_json::from_slice(&request.body).map_err(|_| refused("request_invalid"))?;
    if !request_valid(&envelope.request) {
        return Err(refused("request_invalid"));
    }
    if !binding_current(&outbound.core, &envelope.binding) {
        return Err(refused("binding_mismatch"));
    }
    let installer = account
        .mcp_config
        .get()
        .cloned()
        .ok_or_else(|| refused("provider_unconfigured"))?;
    let id = envelope.request["id"].clone();
    let dispatch = Dispatch {
        account,
        core: outbound.core.clone(),
        installer,
        caller: (pid, uid),
        generation: outbound.core.plugins.cloud.identity_sync_generation(),
        envelope,
        deadline,
    };
    Ok(tokio::task::spawn_blocking(move || {
        dispatch.run(
            &SystemProcesses::default(),
            |binding, bearer, body, deadline| {
                forward::post_for(forward::Client::Cursor, binding, bearer, body, deadline)
            },
        )
    })
    .await
    .unwrap_or_else(|_| classified(forward::error(&id, -32000, UNKNOWN), "outcome_unknown")))
}

/// One helper request, run off the async threads.
pub(super) struct Dispatch {
    pub account: Arc<Account>,
    pub core: Arc<Core>,
    pub installer: Arc<Installer>,
    pub caller: (i32, u32),
    pub generation: u64,
    pub envelope: Envelope,
    pub deadline: Instant,
}

/// A refusal of request `id`, carrying only its class.
fn refusal(id: &Value, class: &'static str) -> Value {
    log_refusal(class);
    classified(forward::error(id, -32000, REFUSED), class)
}

impl Dispatch {
    fn current(&self) -> Result<(), &'static str> {
        if Instant::now() >= self.deadline {
            return Err("deadline_expired");
        }
        if !binding_current(&self.core, &self.envelope.binding) {
            return Err("binding_mismatch");
        }
        if self.core.plugins.cloud.identity_sync_generation() != self.generation {
            return Err("account_changed");
        }
        Ok(())
    }

    /// Strips any caller-supplied proof from a `tools/call` and, for an
    /// identity tool a chat's hook announced, signs that chat's proof into
    /// it. An identity tool without exactly one chat's announcement is
    /// refused while Cursor's calls are attested. Any other tool always goes
    /// out unsigned: Cloud refuses a proof on it with HTTP 400.
    fn attest(&mut self, attesting: bool) -> Result<(), &'static str> {
        let request = &mut self.envelope.request;
        if request["method"] != "tools/call" {
            return Ok(());
        }
        let tool = request["params"]["name"]
            .as_str()
            .ok_or("request_invalid")?
            .to_owned();
        let (args, canonical) = proof::arguments(&request["params"]["arguments"])?;
        request["params"]["arguments"] = args.clone();
        if !attesting {
            return Ok(());
        }
        if !IDENTITY_TOOLS.contains(&tool.as_str()) {
            return Ok(());
        }
        let intent = match self.account.copilot_dispatch.cursor_intents.take(
            &tool,
            &canonical,
            self.generation,
            Instant::now(),
        ) {
            Ok(Some(intent)) => intent,
            Ok(None) => return Err("cursor_intent_missing"),
            Err(class) => return Err(class),
        };
        let source = registered(&self.core, &intent.sid).ok_or("cursor_chat_unknown")?;
        let key = self
            .account
            .attestation_key
            .get(&self.core.config.app_data_dir)
            .ok_or("attestation_key_unavailable")?;
        let proof = proof::sign(&key, &intent.sid, &source, &intent.tuid, &tool, &args)?;
        self.envelope.request["params"]["arguments"][FIELD] = json!(proof);
        Ok(())
    }

    pub(super) fn run(
        mut self,
        table: &dyn ProcessTable,
        post: impl FnOnce(&Binding, &str, &Value, Instant) -> Result<Value, &'static str>,
    ) -> Value {
        let id = self.envelope.request["id"].clone();
        let method = self.envelope.request["method"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let (pid, uid) = self.caller;
        if !helper_peer(&self.installer, &self.envelope.binding, pid, uid, table) {
            return refusal(&id, "native_peer_unavailable");
        }
        if let Err(class) = self.current() {
            return refusal(&id, class);
        }
        let Some(hosted) = self
            .installer
            .cursor_dispatch_hosted(Some(&self.envelope.binding))
        else {
            return refusal(&id, "provider_unconfigured");
        };
        if hosted.cursor_stdio.is_none() {
            return refusal(&id, "provider_changed");
        }
        let Some(bearer) = hosted
            .headers
            .get("Authorization")
            .and_then(|value| value.strip_prefix("Bearer "))
        else {
            return refusal(&id, "provider_unconfigured");
        };
        // The hosted server speaks the initialize-based protocol through
        // this bridge; answer the modern discovery probe as unsupported.
        if method == "server/discover" {
            return forward::error(
                &id,
                -32601,
                "Use initialize to negotiate this MCP connection.",
            );
        }
        let attesting = attesting(&self.account, &self.core);
        if let Err(class) = self.attest(attesting) {
            return refusal(&id, class);
        }
        if let Err(class) = self.current() {
            return refusal(&id, class);
        }
        let mut reply = post(
            &self.envelope.binding,
            bearer,
            &self.envelope.request,
            self.deadline,
        )
        .unwrap_or_else(|_| classified(forward::error(&id, -32000, UNKNOWN), "outcome_unknown"));
        if reply.is_object() {
            reply["id"] = id;
        }
        if method == "initialize" {
            transport_capabilities(&mut reply);
        }
        reply
    }
}

#[cfg(test)]
mod tests;
