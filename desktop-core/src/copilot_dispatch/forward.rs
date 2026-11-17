//! One bounded POST, without the optional diagnostic request-body capture.
use crate::mcp_settings::Binding;
use serde_json::{json, Value};
use std::io::{self, Read};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
#[cfg(test)]
use std::time::Duration;
use std::time::Instant;

const MAX_REPLY_BYTES: u64 = 4 << 20;
const MAX_REQUEST_BYTES: usize = 128 << 10;
const MAX_DNS_WORKERS: usize = 8;
static DNS_WORKERS: AtomicUsize = AtomicUsize::new(0);
const IN_DOUBT: &str = "dispatch_in_doubt_do_not_repeat";
mod sse;

fn in_doubt(phase: &'static str) -> &'static str {
    #[cfg(debug_assertions)]
    eprintln!("[copilot-dispatch] HTTP outcome unresolved at {phase}");
    #[cfg(not(debug_assertions))]
    let _ = phase;
    IN_DOUBT
}

/// Which native client the hosted reply goes back to; it names the
/// connection to repair in a refusal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Client {
    Copilot,
    Cursor,
}

pub(crate) fn error(id: &Value, code: i32, message: &'static str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

pub(super) fn post_before(
    binding: &Binding,
    bearer: &str,
    body: &Value,
    deadline: Instant,
) -> Result<Value, &'static str> {
    post_for(Client::Copilot, binding, bearer, body, deadline)
}

/// [`post_before`] for `client`: the same bounded POST, with refusals that
/// name `client`'s connection.
pub(crate) fn post_for(
    client: Client,
    binding: &Binding,
    bearer: &str,
    body: &Value,
    deadline: Instant,
) -> Result<Value, &'static str> {
    // The caller captures this deadline before queueing; never rebase it here.
    // Binding validates the selected origin. Loopback HTTP is retained for
    // owned dev sites; remote sites require normal verified HTTPS.
    let endpoint = binding.endpoint().ok_or("hosted_endpoint_unavailable")?;
    post_endpoint_before(client, &endpoint, bearer, body, deadline)
}

fn post_endpoint_before(
    client: Client,
    endpoint: &str,
    bearer: &str,
    body: &Value,
    deadline: Instant,
) -> Result<Value, &'static str> {
    post_with_resolver(
        client,
        endpoint,
        bearer,
        body,
        deadline,
        move |host: &str| {
            resolve_until(host, deadline, |host| {
                host.to_socket_addrs().map(Iterator::collect)
            })
        },
    )
}

#[cfg(test)]
fn post(binding: &Binding, bearer: &str, body: &Value) -> Result<Value, &'static str> {
    post_before(
        binding,
        bearer,
        body,
        Instant::now() + Duration::from_secs(40),
    )
}

#[cfg(test)]
fn post_at(
    endpoint: &str,
    bearer: &str,
    body: &Value,
    timeout: Duration,
) -> Result<Value, &'static str> {
    post_endpoint_before(
        Client::Copilot,
        endpoint,
        bearer,
        body,
        Instant::now() + timeout,
    )
}

fn post_with_resolver(
    client: Client,
    endpoint: &str,
    bearer: &str,
    body: &Value,
    deadline: Instant,
    resolver: impl ureq::Resolver + 'static,
) -> Result<Value, &'static str> {
    let bytes = serde_json::to_vec(body).map_err(|_| "invalid_mcp_request")?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err("invalid_mcp_request");
    }
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| in_doubt("deadline before HTTP"))?;
    #[cfg(debug_assertions)]
    eprintln!(
        "[copilot-dispatch] HTTP start: bytes={}, budget_ms={}",
        bytes.len(),
        remaining.as_millis()
    );
    // Fresh agent + POST/nonempty body: neither recycled-connection retry path
    // in ureq2's Unit::is_retryable applies. No outer retry or auth refresh.
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(remaining)
        .timeout_connect(remaining)
        .try_proxy_from_env(false)
        .resolver(resolver)
        .build();
    let response = match agent
        .post(endpoint)
        .set("Authorization", &format!("Bearer {bearer}"))
        .set("Accept", "application/json, text/event-stream")
        .set("Content-Type", "application/json")
        .set("MCP-Protocol-Version", "2025-06-18")
        .send_bytes(&bytes)
    {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(_) => return Err(in_doubt("request send")),
    };
    let status = response.status();
    let media_type = response
        .header("Content-Type")
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim();
    let json = media_type.eq_ignore_ascii_case("application/json");
    let stream = media_type.eq_ignore_ascii_case("text/event-stream");
    #[cfg(debug_assertions)]
    eprintln!("[copilot-dispatch] HTTP response: status={status}, json={json}, sse={stream}");
    if !(200..300).contains(&status) {
        // Do not read/echo an upstream error body, even if it contains a proof.
        let attested = body["params"]["arguments"]
            .get(crate::claude_attestation::FIELD)
            .is_some();
        return status_error(client, status, &body["id"], attested);
    }
    if body.get("id").is_some() && stream {
        return sse::receive(response.into_reader(), &body["id"], deadline);
    }
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_REPLY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| in_doubt("response read"))?;
    if bytes.len() as u64 > MAX_REPLY_BYTES {
        return Err(in_doubt("response size"));
    }
    if body.get("id").is_none() {
        return if status == 202 && bytes.is_empty() {
            Ok(Value::Null)
        } else {
            Err(in_doubt("notification response"))
        };
    }
    if !json {
        return Err(in_doubt("response media type"));
    }
    response_value(&bytes, &body["id"])
}

/// The error a non-success `status` gives the chat. `attested` says the
/// call carried a chat proof: Cloud answers 400 to a proof it cannot verify.
fn status_error(
    client: Client,
    status: u16,
    id: &Value,
    attested: bool,
) -> Result<Value, &'static str> {
    let message = match (client, status) {
        (Client::Cursor, 400) if attested => {
            "Agent Relay Cloud could not verify this Cursor chat's attestation, so the call was \
             not run. Retry once; if it fails again, run whoami from this chat's shell."
        }
        (Client::Copilot, 401 | 403) => {
            "Agent Relay authentication changed. Repair this Copilot connection."
        }
        (Client::Copilot, 400) => {
            "Agent Relay rejected this native call. Reconnect Copilot before another call."
        }
        (Client::Cursor, 401 | 403) => {
            "Agent Relay authentication changed. Repair Cursor in Agent Relay's tool integrations."
        }
        (Client::Cursor, 400) => {
            "Agent Relay rejected this MCP call. Repair Cursor in Agent Relay's tool integrations before another call."
        }
        _ => return Err(IN_DOUBT),
    };
    Ok(error(id, -32602, message))
}

fn response_value(bytes: &[u8], id: &Value) -> Result<Value, &'static str> {
    let reply: Value = serde_json::from_slice(bytes).map_err(|_| in_doubt("response JSON"))?;
    if reply["jsonrpc"] != "2.0"
        || reply.get("id") != Some(id)
        || reply.get("result").is_some() == reply.get("error").is_some()
    {
        return Err(in_doubt("response RPC shape or identifier"));
    }
    if let Some(failure) = reply.get("error") {
        let code = failure
            .get("code")
            .and_then(Value::as_i64)
            .and_then(|code| i32::try_from(code).ok())
            .ok_or(IN_DOUBT)?;
        if failure.get("message").and_then(Value::as_str).is_none() {
            return Err(IN_DOUBT);
        }
        return Ok(error(
            id,
            code,
            "Agent Relay returned an MCP error. Reconcile any send before repeating it.",
        ));
    }
    Ok(reply)
}

struct DnsPermit;
impl Drop for DnsPermit {
    fn drop(&mut self) {
        DNS_WORKERS.fetch_sub(1, Ordering::AcqRel);
    }
}

fn resolve_until(
    host: &str,
    deadline: Instant,
    lookup: impl FnOnce(String) -> io::Result<Vec<SocketAddr>> + Send + 'static,
) -> io::Result<Vec<SocketAddr>> {
    if Instant::now() >= deadline {
        return Err(io::Error::from(io::ErrorKind::TimedOut));
    }
    if let Ok(address) = host.parse() {
        return Ok(vec![address]);
    }
    DNS_WORKERS
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < MAX_DNS_WORKERS).then_some(count + 1)
        })
        .map_err(|_| io::Error::from(io::ErrorKind::WouldBlock))?;
    let permit = DnsPermit;
    let host = host.to_owned();
    let (sender, receiver) = mpsc::sync_channel(1);
    // Only host:port crosses this boundary, never credentials/body/HTTP work.
    // The slot remains held until a blocked OS resolver actually exits.
    std::thread::Builder::new()
        .name("copilot-mcp-dns".into())
        .spawn(move || {
            let _permit = permit;
            let _ = sender.send(lookup(host));
        })
        .map_err(|_| io::Error::other("resolver unavailable"))?;
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or_else(|| io::Error::from(io::ErrorKind::TimedOut))?;
    let addresses = receiver
        .recv_timeout(remaining)
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
    if Instant::now() >= deadline {
        return Err(io::Error::from(io::ErrorKind::TimedOut));
    }
    Ok(addresses)
}

#[cfg(test)]
mod tests;
