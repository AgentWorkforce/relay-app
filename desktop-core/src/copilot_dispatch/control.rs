//! Additional call/cancellation gates, never the native identity proof.
//! Read only this already kernel-verified chat's bounded control tail.
use super::intent::{Anchor, Call, FileIdentity};
use crate::claude_attestation::FIELD;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::UNIX_EPOCH;

const TAIL: u64 = 8 << 20;

pub(super) fn identity(metadata: &Metadata) -> Option<FileIdentity> {
    #[cfg(unix)]
    let (device, inode) = {
        use std::os::unix::fs::MetadataExt;
        (metadata.dev(), metadata.ino())
    };
    #[cfg(not(unix))]
    let (device, inode) = (0, 0);
    Some(FileIdentity {
        device,
        inode,
        birth_ns: metadata
            .created()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos(),
    })
}

fn open(path: &Path) -> Result<File, &'static str> {
    let before = std::fs::symlink_metadata(path).map_err(|_| "native_control_unavailable")?;
    if !before.is_file() {
        return Err("native_control_unavailable");
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_FLAG_OPEN_REPARSE_POINT: do not traverse a replacement link.
        options.custom_flags(0x0020_0000);
    }
    let file = options
        .open(path)
        .map_err(|_| "native_control_unavailable")?;
    unchanged(path, &file)?;
    if identity(&before) != identity(&file.metadata().map_err(|_| "native_control_unavailable")?) {
        return Err("native_control_replaced");
    }
    Ok(file)
}

fn unchanged(path: &Path, file: &File) -> Result<(), &'static str> {
    let current = std::fs::symlink_metadata(path).map_err(|_| "native_control_replaced")?;
    let held = file.metadata().map_err(|_| "native_control_replaced")?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if current.file_attributes() & 0x400 != 0 {
            return Err("native_control_replaced");
        }
    }
    if !current.is_file()
        || !held.is_file()
        || identity(&current) != identity(&held)
        || !agent_relay_node::platform::fs::owned_by_current_user(&held)
        || !agent_relay_node::platform::fs::private_to_owner(&held, 0o022)
    {
        return Err("native_control_replaced");
    }
    Ok(())
}

pub(super) fn anchor(path: &Path) -> Result<Anchor, &'static str> {
    let mut file = open(path)?;
    let metadata = file.metadata().map_err(|_| "native_control_unavailable")?;
    if !metadata.is_file() {
        return Err("native_control_unavailable");
    }
    let offset = metadata.len();
    if offset > 0 {
        file.seek(SeekFrom::Start(offset - 1))
            .map_err(|_| "native_control_unavailable")?;
        let mut byte = [0];
        file.read_exact(&mut byte)
            .map_err(|_| "native_control_unavailable")?;
        if byte[0] != b'\n' {
            return Err("native_control_incomplete");
        }
    }
    unchanged(path, &file)?;
    Ok(Anchor {
        file: identity(&metadata).ok_or("native_control_unavailable")?,
        offset,
    })
}

fn tail(path: &Path, anchor: &Anchor) -> Result<Vec<u8>, &'static str> {
    let mut file = open(path)?;
    let metadata = file.metadata().map_err(|_| "native_control_unavailable")?;
    if identity(&metadata).as_ref() != Some(&anchor.file) || metadata.len() < anchor.offset {
        return Err("native_control_replaced");
    }
    if metadata.len() - anchor.offset > TAIL {
        return Err("native_control_overflow");
    }
    file.seek(SeekFrom::Start(anchor.offset))
        .map_err(|_| "native_control_unavailable")?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(TAIL + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "native_control_unavailable")?;
    if bytes.len() as u64 > TAIL {
        return Err("native_control_overflow");
    }
    unchanged(path, &file)?;
    if file
        .metadata()
        .map_err(|_| "native_control_replaced")?
        .len()
        < anchor.offset + bytes.len() as u64
    {
        return Err("native_control_replaced");
    }
    Ok(bytes)
}

pub(super) enum Gate {
    Ready,
    Waiting,
}

fn arguments(value: &Value, call: &Call, ticket: &str) -> bool {
    let Some(mut args) = value.as_object().cloned() else {
        return false;
    };
    if args.remove(FIELD).as_ref().and_then(Value::as_str) != Some(ticket) {
        return false;
    }
    Value::Object(args) == call.args
}

fn pre_mcp(event: &Value, call: &Call, ticket: &str) -> Result<Option<String>, &'static str> {
    let data = &event["data"];
    if event["type"] != "hook.start" || data["hookType"] != "preMcpToolCall" {
        return Ok(None);
    }
    let input = &data["input"];
    if input["arguments"][FIELD] != ticket {
        return Ok(None);
    }
    let id = input["toolCallId"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 256)
        .ok_or("native_call_unavailable")?;
    if input["sessionId"] != call.sid
        || input["serverName"] != "agentrelay"
        || input["toolName"] != call.tool
        || !arguments(&input["arguments"], call, ticket)
    {
        return Err("native_call_mismatch");
    }
    Ok(Some(id.to_owned()))
}

fn request_id(
    event: &Value,
    call_id: &str,
    call: &Call,
    ticket: &str,
) -> Result<Option<(String, bool)>, &'static str> {
    if event["type"] != "permission.requested" {
        return Ok(None);
    }
    let data = &event["data"];
    let permission = &data["permissionRequest"];
    if permission["toolCallId"] != call_id {
        return Ok(None);
    }
    let tool = permission["toolName"]
        .as_str()
        .and_then(crate::copilot_attestation::qualified_tool);
    if permission["kind"] != "mcp"
        || permission["serverName"] != "agentrelay"
        || tool != Some(call.tool.as_str())
        || !arguments(&permission["args"], call, ticket)
    {
        return Err("native_permission_mismatch");
    }
    data["requestId"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 256)
        .map(|s| Some((s.to_owned(), data["resolvedByHook"] == true)))
        .ok_or("native_permission_unavailable")
}

fn approved(
    event: &Value,
    id: &str,
    request: &str,
    resolved_by_hook: bool,
    now_ms: i64,
) -> Result<bool, &'static str> {
    if event["type"] != "permission.completed" {
        return Ok(false);
    }
    if event["data"]["requestId"] != request {
        return Ok(false);
    }
    let call_matches = event["data"]["toolCallId"] == id
        || (resolved_by_hook && event["data"].get("toolCallId").is_none());
    if !call_matches || event["data"]["result"]["kind"] != "approved" {
        return Err("native_permission_denied");
    }
    let at = event["timestamp"]
        .as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.timestamp_millis())
        .ok_or("native_permission_unavailable")?;
    if now_ms < at || now_ms.saturating_sub(at) > 60_000 {
        return Err("native_permission_expired");
    }
    Ok(true)
}

fn permission_shape(event: &Value) -> Result<(), &'static str> {
    let requested = event["type"] == "permission.requested";
    if !requested && event["type"] != "permission.completed" {
        return Ok(());
    }
    let data = &event["data"];
    let id = |value: &Value| {
        value
            .as_str()
            .is_some_and(|id| !id.is_empty() && id.len() <= 256)
    };
    if !data.is_object() || !id(&data["requestId"]) {
        return Err("native_permission_unavailable");
    }
    let permission = &data["permissionRequest"];
    if requested {
        if !permission.is_object()
            || !id(&permission["kind"])
            || (permission["kind"] == "mcp" && !id(&permission["toolCallId"]))
        {
            return Err("native_permission_unavailable");
        }
    } else if !data["result"].is_object() || !id(&data["result"]["kind"]) {
        return Err("native_permission_unavailable");
    }
    Ok(())
}

pub(super) fn inspect(
    path: &Path,
    call: &Call,
    ticket: &str,
    now_ms: i64,
) -> Result<Gate, &'static str> {
    inspect_bytes(&tail(path, &call.journal)?, call, ticket, now_ms)
}

fn inspect_bytes(
    bytes: &[u8],
    call: &Call,
    ticket: &str,
    now_ms: i64,
) -> Result<Gate, &'static str> {
    // A partial manual-request append must never look like no-prompt approval.
    if !bytes.is_empty() && !bytes.ends_with(b"\n") {
        return Ok(Gate::Waiting);
    }
    // A malformed completed control record cannot be treated as absence of
    // a manual approval requirement.
    let end = bytes.iter().rposition(|b| *b == b'\n').map_or(0, |n| n + 1);
    let events = bytes[..end]
        .split(|b| *b == b'\n')
        .filter(|b| !b.is_empty())
        .map(|b| serde_json::from_slice::<Value>(b).map_err(|_| "native_control_invalid"))
        .collect::<Result<Vec<_>, _>>()?;
    if events.iter().any(|event| {
        !event.is_object()
            || event["type"]
                .as_str()
                .is_none_or(|kind| kind.trim().is_empty())
    }) {
        return Err("native_control_invalid");
    }
    let mut ids = BTreeSet::new();
    for event in &events {
        permission_shape(event)?;
        if matches!(
            event["type"].as_str(),
            Some(
                "abort" | "user.message" | "session.error" | "session.shutdown" | "session.resume"
            )
        ) {
            return Err("native_call_cancelled");
        }
        if let Some(id) = pre_mcp(event, call, ticket)? {
            ids.insert(id);
        }
    }
    if ids.len() > 1 {
        return Err("native_call_ambiguous");
    }
    let Some(id) = ids.first() else {
        return Ok(Gate::Waiting);
    };
    let mut requests = BTreeMap::new();
    for event in &events {
        if event["type"] == "tool.execution_complete" && event["data"]["toolCallId"] == *id {
            return Err("native_call_already_completed");
        }
        if let Some((request, by_hook)) = request_id(event, id, call, ticket)? {
            requests.insert(request, (false, by_hook));
        }
    }
    for (request, (done, by_hook)) in &mut requests {
        for event in &events {
            *done |= approved(event, id, request, *by_hook, now_ms)?;
        }
    }
    if requests.values().any(|(approved, _)| !approved) {
        return Ok(Gate::Waiting);
    }
    // No requested prompt is not a claim of human approval. The authenticated
    // stdio dispatch plus exact native preMCP record follows the SDK's existing
    // approved-rule policy. Native tools are advertised model-only by our
    // adapter; Apps cannot use this capability. Mutable logs never mint it.
    Ok(Gate::Ready)
}

#[cfg(test)]
mod tests;
