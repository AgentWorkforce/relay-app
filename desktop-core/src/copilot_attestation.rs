//! Native Copilot exact-call intents. A verified hook supplies the local
//! capability; the app-owned transport signs only after normal permission.
use crate::claude_attestation::{Key, FIELD};
use crate::desktop_identity::Identities;
use agent_relay_node::relay::outbound::{Outbound, OutboundError};
use agent_relay_node::relay::store::{Registration, SessionKind, Target};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
mod presence;

fn unavailable() -> OutboundError {
    OutboundError::new(409, "attestation_unavailable", "Agent Relay could not verify this Copilot chat. Keep the desktop app open, put this chat on the relay, and retry. Do not substitute another chat's identity.")
}

async fn confirm_synced_sender(
    core: &Arc<agent_relay_node::relay::daemon::Core>,
    sid: &str,
    source: &str,
    registration: &Registration,
) -> Result<(), OutboundError> {
    let identities = Identities::new(core.clone());
    identities.sync_desktop_identities().await;
    identities
        .copilot_sender_synced(sid, source, registration)
        .then_some(())
        .ok_or_else(unavailable)
}

fn normalize_join_identity(
    args: &mut serde_json::Map<String, Value>,
    registration: &Registration,
    prefix: &str,
) -> Result<(), OutboundError> {
    if args
        .get("name")
        .and_then(Value::as_str)
        .is_some_and(|name| name != registration.display_name())
    {
        return Err(unavailable());
    }
    args.remove("name");
    args.remove("description");
    // Stable links may name the original registration. Translate only this
    // verified chat's own link; foreign addresses remain refused by Cloud.
    if args.get("desktop_address").and_then(Value::as_str)
        == Some(registration.link_address(prefix).as_str())
    {
        args.insert(
            "desktop_address".into(),
            json!(registration.current_address(prefix)),
        );
    }
    Ok(())
}

// The hook supplies a flattened name, not a verified server URL. A separator-free
// managed ID and exact underscore-only tool names make this namespace unambiguous.
// Cloud additionally requires the separately stored Copilot provider credential.
fn tool(input: &Value) -> Option<&str> {
    qualified_tool(input["toolName"].as_str()?)
}

pub(crate) fn qualified_tool(qualified: &str) -> Option<&str> {
    let (server, name) = qualified.split_once('-')?;
    (server == crate::mcp_settings::COPILOT_MCP_SERVER && native_tool(name)).then_some(name)
}

pub(crate) fn native_tool(name: &str) -> bool {
    matches!(
        name,
        "list_relay_agents"
            | "send_relay_message"
            | "read_relay_conversation"
            | "join_relay"
            | "leave_relay"
            | "relay_status"
            | "check_relay_inbox"
            | "list_relay_channels"
            | "subscribe_relay_channel"
            | "unsubscribe_relay_channel"
            | "post_relay_channel"
            | "read_relay_channel"
    )
}

fn trace_gate(phase: &'static str) {
    #[cfg(debug_assertions)]
    eprintln!("[copilot-attestation] {phase}");
    #[cfg(not(debug_assertions))]
    let _ = phase;
}

pub(crate) async fn route(
    outbound: &Outbound,
    dispatch_account: Arc<crate::account::Account>,
    pid: i32,
    uid: u32,
    input: Value,
) -> Result<Value, OutboundError> {
    route_with_presence_budget(
        outbound,
        dispatch_account,
        pid,
        uid,
        input,
        Duration::from_secs(35),
    )
    .await
}

// The production route chooses the budget; fixtures can exercise the same
// verified route with short deadlines instead of spending 35 seconds asleep.
pub(crate) async fn route_with_presence_budget(
    outbound: &Outbound,
    dispatch_account: Arc<crate::account::Account>,
    pid: i32,
    uid: u32,
    input: Value,
    presence_budget: Duration,
) -> Result<Value, OutboundError> {
    if uid != agent_relay_node::relay::peer::current_uid() {
        return Err(OutboundError::not_a_session());
    }
    trace_gate("before request received");
    let Some(tool) = tool(&input) else {
        return Ok(json!({}));
    };
    trace_gate("native tool recognized");
    let mut args = input["toolArgs"]
        .as_object()
        .cloned()
        .ok_or_else(unavailable)?;
    args.remove(FIELD);
    if serde_json::to_vec(&args).map_or(true, |bytes| bytes.len() > 32 * 1024) {
        return Err(unavailable());
    }
    // Even though the runtime multiplexes chats, this exact managed hook must
    // be its direct child and the runtime must hold this native chat's inode.
    let directory = outbound.core.plugins.sessions.clone();
    let request = input.clone();
    if tokio::task::spawn_blocking(move || directory.copilot_hook(pid, &request))
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return Err(OutboundError::not_a_session());
    }
    trace_gate("native hook verified");
    let sid = input["sessionId"].as_str().ok_or_else(unavailable)?;
    let target = Target::new(SessionKind::Copilot, sid);
    let account = outbound
        .core
        .plugins
        .cloud
        .account()
        .ok_or_else(unavailable)?;
    let generation = outbound.core.plugins.cloud.identity_sync_generation();
    if !Identities::new(outbound.core.clone()).copilot_attestation_verified() {
        return Err(unavailable());
    }
    trace_gate("account capability verified");
    // The verified hook refreshed this exact chat's native runtime record.
    // Ordinary lifecycle reconciliation may still hold its previous inactive
    // state after a restart. Wait for that existing registration; never create
    // one or clear inactivity ourselves, which would bypass normal eligibility.
    let (registration, source) = presence::wait_for_active(
        || {
            if outbound.core.plugins.cloud.account().as_ref() != Some(&account)
                || outbound.core.plugins.cloud.identity_sync_generation() != generation
            {
                return Err(unavailable());
            }
            outbound.core.store.read(|store| {
                let registration = store
                    .registration_for(&target)
                    .ok_or_else(OutboundError::not_registered)?;
                let source = store
                    .desktop_sync_source_id
                    .as_ref()
                    .ok_or_else(OutboundError::not_registered)?;
                Ok(registration
                    .missing_since
                    .is_none()
                    .then(|| (registration.clone(), source.clone())))
            })
        },
        Instant::now() + presence_budget,
    )
    .await?
    .ok_or_else(OutboundError::not_registered)?;
    confirm_synced_sender(&outbound.core, sid, &source, &registration).await?;
    trace_gate("registration verified");
    // A native join always keeps the already-registered chat identity. It
    // never creates a hosted participant under a model-provided name.
    if tool == "join_relay" {
        normalize_join_identity(&mut args, &registration, &outbound.core.machine_prefix)?;
    }
    if outbound.core.workspace().is_none()
        || outbound.core.plugins.cloud.account().as_ref() != Some(&account)
        || outbound.core.plugins.cloud.identity_sync_generation() != generation
    {
        return Err(unavailable());
    }
    trace_gate("issuing ticket");
    let result = crate::copilot_dispatch::issue(
        &dispatch_account,
        pid,
        uid,
        sid,
        tool,
        Value::Object(args),
        crate::copilot_dispatch::intent::Owner {
            site: account.site_url,
            user: account.user_id.ok_or_else(unavailable)?,
            workspace: account.workspace_id,
            generation,
            source,
            registration: crate::copilot_dispatch::registration_stamp(&registration),
        },
    );
    trace_gate(if result.is_ok() {
        "ticket issued"
    } else {
        "ticket refused"
    });
    result
}

pub(crate) fn sign(
    key: &Key,
    sid: &str,
    source: &str,
    tool: &str,
    args: &Value,
    workspace: &str,
    user: &str,
) -> Option<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    sign_at(
        key,
        Proof {
            sid,
            source,
            tool,
            args,
            workspace,
            user,
        },
        now,
    )
}

#[derive(Clone, Copy)]
struct Proof<'a> {
    sid: &'a str,
    source: &'a str,
    tool: &'a str,
    args: &'a Value,
    workspace: &'a str,
    user: &'a str,
}

fn sign_at(key: &Key, proof: Proof<'_>, now: u64) -> Option<String> {
    let mut nonce = [0; 16];
    getrandom::getrandom(&mut nonce).ok()?;
    key.sign_payload(&json!({"v":1,"kind":"copilot","sid":proof.sid.to_ascii_lowercase(),
        "src":proof.source,"workspace":proof.workspace,"user":proof.user,"nonce":URL_SAFE_NO_PAD.encode(nonce),"tool":proof.tool,"args":proof.args,
        "iat":now,"exp":now + 60}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delayed_dispatch_cross_language_fixture_uses_a_fresh_unchanged_cloud_proof() {
        let home = tempfile::tempdir().unwrap();
        let key = Key::load_or_create(&home.path().join("key.json")).unwrap();
        let sid = "1069773c-0824-4fad-b7f9-d3dc3c4ed148";
        let source = "123e4567-e89b-42d3-a456-426614174000";
        let args = json!({"to":"fixture-peer","text":"synthetic dispatch fixture","idempotency_key":"dispatch_fixture_key"});
        let issued = 1_794_000_000u64;
        let dispatched = issued + 185;
        let claims = Proof {
            sid,
            source,
            tool: "send_relay_message",
            args: &args,
            workspace: "dispatch-fixture-workspace",
            user: "dispatch-fixture-user",
        };
        let proof = sign_at(&key, claims, dispatched).unwrap();
        let old = sign_at(&key, claims, issued).unwrap();
        let claims: Value = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(proof.split('.').nth(1).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(claims["iat"], dispatched);
        assert_eq!(claims["exp"], dispatched + 60);
        assert_eq!(claims["args"], args);
        if let Ok(path) = std::env::var("RELAY_COPILOT_DISPATCH_FIXTURE") {
            std::fs::write(path, serde_json::to_vec_pretty(&json!({
                "synthetic":true,"provenance":"Disposable Rust Desktop test key; no live account, runtime or credentials.",
                "publicKey":URL_SAFE_NO_PAD.encode(key.public_key()),"auth":{"desktopMcpProvider":"copilot","userId":"dispatch-fixture-user","workspaceId":"dispatch-fixture-workspace"},
                "sessionId":sid,"sourceId":source,"toolName":"send_relay_message","args":args,
                "intentIssuedAtMs":issued*1000,"dispatchNowMs":dispatched*1000,
                "preApprovalProof":old,"proof":proof
            })).unwrap()).unwrap();
        }
    }
    #[test]
    fn only_exact_unambiguous_managed_relay_tools_are_signed() {
        let fixture: Value =
            serde_json::from_str(include_str!("copilot_attestation/tool-names.json")).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            assert_eq!(
                tool(&json!({"toolName":case["qualifiedName"]})),
                case["acceptedTool"].as_str(),
                "{}",
                case["qualifiedName"]
            );
        }
        for name in [
            "join_relay",
            "leave_relay",
            "relay_status",
            "check_relay_inbox",
            "list_relay_channels",
            "subscribe_relay_channel",
            "unsubscribe_relay_channel",
            "post_relay_channel",
            "read_relay_channel",
            "read_relay_conversation",
            "list_relay_agents",
            "send_relay_message",
        ] {
            assert_eq!(
                tool(&json!({"toolName":format!("agentrelay-{name}")})),
                Some(name)
            );
        }
    }
    #[test]
    fn proof_carries_exact_call_and_fresh_nonce_without_changing_claude_key_format() {
        let home = tempfile::tempdir().unwrap();
        let key = Key::load_or_create(&home.path().join("key.json")).unwrap();
        let args = json!({"text":"a private reply","nested":{"number":1.0}});
        let first = sign(
            &key,
            "1069773c-0824-4fad-b7f9-d3dc3c4ed148",
            "123e4567-e89b-42d3-a456-426614174000",
            "send_relay_message",
            &args,
            "workspace",
            "alice",
        )
        .unwrap();
        let second = sign(
            &key,
            "1069773c-0824-4fad-b7f9-d3dc3c4ed148",
            "123e4567-e89b-42d3-a456-426614174000",
            "send_relay_message",
            &args,
            "workspace",
            "alice",
        )
        .unwrap();
        let payload = |proof: &str| -> Value {
            serde_json::from_slice(
                &URL_SAFE_NO_PAD
                    .decode(proof.split('.').nth(1).unwrap())
                    .unwrap(),
            )
            .unwrap()
        };
        if let Ok(path) = std::env::var("RELAY_COPILOT_ATTESTATION_FIXTURE") {
            let claims = payload(&first);
            std::fs::write(path, serde_json::to_vec_pretty(&json!({
                "provenance":"Generated by the Rust Desktop Copilot signer using a disposable test key; synthetic call only.",
                "publicKey":URL_SAFE_NO_PAD.encode(key.public_key()),"proof":first,
                "auth":{"workspaceId":"workspace","userId":"alice","desktopMcpProvider":"copilot"},
                "nowMs":claims["iat"].as_u64().unwrap() * 1000,
                "sourceId":claims["src"],"sessionId":claims["sid"],"args":args,
            })).unwrap()).unwrap();
        }
        assert_eq!(payload(&first)["args"], args);
        assert_eq!(payload(&first)["kind"], "copilot");
        assert_ne!(payload(&first)["nonce"], payload(&second)["nonce"]);
        assert_eq!(
            payload(&first)["exp"].as_u64().unwrap() - payload(&first)["iat"].as_u64().unwrap(),
            60
        );
    }
}
