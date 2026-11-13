//! Per-call proof that a hosted Agent Relay MCP tool call comes from one
//! Claude Code chat registered on this installation.
//!
//! Claude Code caches `headersHelper` output per MCP connection, so the
//! hosted server cannot tell which chat makes a call from its headers. Agent
//! Relay's PreToolUse hook (`claude-attestation-hook`) runs for every
//! hosted Agent Relay tool call, asks the core for a proof over the session
//! socket, and adds it to the call's arguments as
//! `_agent_relay_attestation`, overwriting any value already there. Cloud
//! verifies the proof against the key this installation registered through
//! the desktop identity sync.
//!
//! The proof is a compact JWS (`alg: EdDSA`) signed with one Ed25519 key per
//! installation, kept private in the app data directory and never logged:
//!
//! ```text
//! header  {"alg":"EdDSA","kid":"<kid>","typ":"agent-relay-attest+jws"}
//! payload {"v":1,"kind":"claude","sid","src","tuid","tool","iat","exp"}
//! ```
//!
//! `kid` is base64url (no padding) of the first 16 bytes of SHA-256 over the
//! raw 32-byte public key. A call the core does not sign carries no proof,
//! and Cloud then refuses actor-dependent tools for it.

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// The private key's file in the app data directory.
pub const KEY_FILE: &str = "claude-attestation-key.json";
/// The tool argument that carries the proof.
pub const FIELD: &str = "_agent_relay_attestation";
/// The protected header's `typ`.
pub const TYPE: &str = "agent-relay-attest+jws";
/// How long a proof is valid, in seconds.
pub const LIFETIME_SECS: u64 = 60;
/// The hook's subcommand; also how the hook is recognized as Agent Relay's.
pub const SUBCOMMAND: &str = "claude-attestation-hook";
/// The hosted MCP server name Agent Relay writes into Claude's config.
pub const SERVER: &str = "agent-relay";
/// The PreToolUse matcher: the user-scope server and the same server
/// bundled in a plugin (`mcp__plugin_<plugin>_agent-relay__<tool>`).
pub const MATCHER: &str = "^(mcp__agent-relay__.+|mcp__plugin_.+_agent-relay__.+)$";
/// Longest profile id kept readable in a server name. Claude tool names are
/// at most 64 characters, and `mcp__agent-relay-<id>__list_relay_spawn_targets`
/// stays within that.
const PROFILE_ID_MAX: usize = 20;
/// The session-socket route the hook asks.
pub const ROUTE: &str = "/claude/attestation";
/// Longest `tool_use_id` signed.
const TOOL_USE_ID_MAX: usize = 256;

pub mod hook;

#[derive(Serialize, Deserialize)]
struct StoredKey {
    version: u32,
    /// PKCS#8 v2, base64.
    key: String,
}

/// This installation's attestation key. Deliberately not `Debug`.
pub struct Key {
    pair: Ed25519KeyPair,
    kid: String,
}

impl Key {
    /// The key at `path`, created on first use. A file that exists but
    /// cannot be read is an error and is left in place.
    pub fn load_or_create(path: &Path) -> Result<Key, String> {
        const UNREADABLE: &str = "The Claude attestation key is unreadable.";
        match std::fs::read(path) {
            Ok(bytes) => {
                let stored: StoredKey = serde_json::from_slice(&bytes).map_err(|_| UNREADABLE)?;
                let pkcs8 = STANDARD.decode(&stored.key).map_err(|_| UNREADABLE)?;
                Key::from_pkcs8(&pkcs8).ok_or_else(|| UNREADABLE.into())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                const CREATE: &str = "Could not create the Claude attestation key.";
                let rng = ring::rand::SystemRandom::new();
                let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| CREATE)?;
                let key = Key::from_pkcs8(pkcs8.as_ref()).ok_or(CREATE)?;
                let stored = StoredKey {
                    version: 1,
                    key: STANDARD.encode(pkcs8.as_ref()),
                };
                let bytes = serde_json::to_vec(&stored).map_err(|_| CREATE)?;
                agent_relay_node::platform::fs::write_private(path, &bytes).map_err(|_| CREATE)?;
                Ok(key)
            }
            Err(_) => Err(UNREADABLE.into()),
        }
    }

    fn from_pkcs8(pkcs8: &[u8]) -> Option<Key> {
        let pair = Ed25519KeyPair::from_pkcs8(pkcs8).ok()?;
        let kid = kid(pair.public_key().as_ref());
        Some(Key { pair, kid })
    }

    pub fn kid(&self) -> &str {
        &self.kid
    }

    /// The raw 32-byte public key.
    pub fn public_key(&self) -> &[u8] {
        self.pair.public_key().as_ref()
    }

    /// The identity sync's `attestation_key` field.
    pub fn registration(&self) -> Value {
        json!({
            "kid": self.kid,
            "alg": "Ed25519",
            "public_key": URL_SAFE_NO_PAD.encode(self.public_key()),
        })
    }

    /// The compact JWS over `claims`.
    pub fn sign(&self, claims: &Claims<'_>) -> Option<String> {
        self.sign_payload(claims)
    }

    /// Signs another native provider's versioned payload with this installation key.
    pub(crate) fn sign_payload(&self, claims: &impl Serialize) -> Option<String> {
        let header = json!({"alg": "EdDSA", "kid": self.kid, "typ": TYPE});
        let signing_input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).ok()?),
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).ok()?)
        );
        let signature = self.pair.sign(signing_input.as_bytes());
        Some(format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.as_ref())
        ))
    }
}

/// `kid` for a raw public key.
pub fn kid(public_key: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(&Sha256::digest(public_key)[..16])
}

/// A proof's payload, in the contract's field order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims<'a> {
    pub v: u8,
    pub kind: &'a str,
    pub sid: &'a str,
    pub src: &'a str,
    pub tuid: &'a str,
    pub tool: &'a str,
    pub iat: u64,
    pub exp: u64,
}

impl<'a> Claims<'a> {
    pub fn new(sid: &'a str, src: &'a str, tuid: &'a str, tool: &'a str, iat: u64) -> Self {
        Claims {
            v: 1,
            kind: "claude",
            sid,
            src,
            tuid,
            tool,
            iat,
            exp: iat + LIFETIME_SECS,
        }
    }
}

/// The hosted MCP server name this core's build writes into Claude Code and
/// Codex: [`SERVER`] for the core on the default socket and for the
/// development build, whose clients are private. A site profile build's core
/// runs on `core.<bundle id>.sock` beside Agent Relay's, so its entry is
/// `agent-relay-<profile id>`: the bundle id from that socket name, without
/// Agent Relay's own `com.agentrelay.desktop.` prefix, in letters, digits and
/// `-`. An id too long for Claude's tool names is a digest of it instead.
pub fn server_for(config: &agent_relay_node::relay::daemon::Config) -> String {
    use agent_relay_node::relay::{local::is_development, paths};
    if is_development(config) || config.paths.socket == paths::default_socket(&config.home) {
        return SERVER.into();
    }
    let name = config
        .paths
        .socket
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = name.strip_prefix("core.").unwrap_or(&name);
    let name = name.strip_suffix(".sock").unwrap_or(name);
    let bundle = name.strip_prefix("com.agentrelay.desktop.").unwrap_or(name);
    let id: String = bundle
        .chars()
        .map(|c| match c {
            'a'..='z' | '0'..='9' | '-' => c,
            'A'..='Z' => c.to_ascii_lowercase(),
            _ => '-',
        })
        .collect();
    let id = if id.is_empty() || id.len() > PROFILE_ID_MAX {
        format!("{:x}", Sha256::digest(bundle.as_bytes()))[..12].to_string()
    } else {
        id
    };
    format!("{SERVER}-{id}")
}

/// The PreToolUse matcher for `server`: [`MATCHER`] for Agent Relay's own,
/// and only that user-scope server for a profile build's.
pub fn matcher_for(server: &str) -> String {
    if server == SERVER {
        MATCHER.into()
    } else {
        format!("^mcp__{server}__.+$")
    }
}

/// A profile build's server name as [`server_for`] writes it.
fn profile_server(server: &str) -> bool {
    server
        .strip_prefix(SERVER)
        .and_then(|rest| rest.strip_prefix('-'))
        .is_some_and(|id| {
            !id.is_empty()
                && id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}

/// The tool's own name from Claude's `mcp__agent-relay__<tool>` or
/// `mcp__plugin_<plugin>_agent-relay__<tool>`; `None` for any other server.
pub fn tool(full: &str) -> Option<&str> {
    tool_on(full, SERVER)
}

/// The tool's own name from a call the hook was run for, on Agent Relay's
/// server or any profile build's; the hook's matcher picked the server.
pub fn hook_tool(full: &str) -> Option<&str> {
    let server = full.strip_prefix("mcp__")?.split_once("__")?.0;
    if profile_server(server) {
        tool_on(full, server)
    } else {
        tool(full)
    }
}

/// The tool's own name from a call to `server`; only Agent Relay's own
/// server comes bundled in a plugin.
pub fn tool_on<'a>(full: &'a str, server: &str) -> Option<&'a str> {
    let rest = full.strip_prefix("mcp__")?;
    let tool = match rest
        .strip_prefix(server)
        .and_then(|rest| rest.strip_prefix("__"))
    {
        Some(tool) => tool,
        None => {
            if server != SERVER {
                return None;
            }
            let (plugin, tool) = rest.strip_prefix("plugin_")?.split_once("_agent-relay__")?;
            if plugin.is_empty() {
                return None;
            }
            tool
        }
    };
    (!tool.is_empty()
        && tool
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then_some(tool)
}

/// A `tool_use_id` the core signs: bounded, printable ASCII.
pub fn valid_tool_use_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= TOOL_USE_ID_MAX && id.bytes().all(|b| b.is_ascii_graphic())
}

/// What the hook sends the core for Claude's PreToolUse input, or `None`
/// when the input is not a PreToolUse call to an object of arguments.
pub fn hook_request(input: &Value) -> Option<Value> {
    if input["hook_event_name"] != "PreToolUse" || !input["tool_input"].is_object() {
        return None;
    }
    let mut request = json!({
        "session_id": input["session_id"].as_str()?,
        "tool_name": input["tool_name"].as_str()?,
        "tool_use_id": input["tool_use_id"].as_str()?,
    });
    // A join's chosen name is checked against the already-registered local
    // identity. The hook never registers a chat; it only provides a command
    // that Claude runs through the normal permission policy when needed.
    if hook_tool(input["tool_name"].as_str()?) == Some("join_relay")
        && input["tool_input"].get("desktop_address").is_none()
        && input["tool_input"]["name"]
            .as_str()
            .is_some_and(|name| !name.trim().is_empty())
    {
        let mut registration = serde_json::Map::new();
        registration.insert("name".into(), input["tool_input"]["name"].clone());
        if let Some(description) = input["tool_input"].get("description") {
            registration.insert("description".into(), description.clone());
        }
        request["registration"] = Value::Object(registration);
    }
    Some(request)
}

/// Claude's PreToolUse output: the call's own arguments with the proof
/// added, replacing any value already there. Permission is left to the
/// person's own settings.
pub fn hook_output(input: &Value, attestation: &str) -> Option<Value> {
    let mut arguments = input["tool_input"].as_object()?.clone();
    arguments.insert(FIELD.into(), Value::String(attestation.into()));
    Some(json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "updatedInput": arguments,
        }
    }))
}

/// The key, loaded once and kept for the life of the core. A failed load is
/// tried again on the next use.
#[derive(Default)]
pub struct KeySlot(Mutex<Option<Arc<Key>>>);

impl KeySlot {
    pub fn get(&self, app_data_dir: &Path) -> Option<Arc<Key>> {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            match Key::load_or_create(&app_data_dir.join(KEY_FILE)) {
                Ok(key) => *slot = Some(Arc::new(key)),
                Err(message) => {
                    agent_relay_node::relay::diagnostics::write(format_args!("relay: {message}"))
                }
            }
        }
        slot.clone()
    }
}

#[cfg(test)]
mod tests;
