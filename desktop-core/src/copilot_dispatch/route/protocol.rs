//! MCP protocol validation, value-free diagnostics and client-facing replies.
use serde_json::{json, Value};

pub(super) fn request_valid(request: &Value) -> bool {
    let Some(object) = request.as_object() else {
        return false;
    };
    if request["jsonrpc"] != "2.0"
        || object
            .keys()
            .any(|k| !matches!(k.as_str(), "jsonrpc" | "id" | "method" | "params"))
    {
        return false;
    }
    let notification = matches!(
        request["method"].as_str(),
        Some("notifications/initialized" | "notifications/cancelled")
    );
    let id = request.get("id");
    if notification != id.is_none() || request["params"].get("task").is_some() {
        return false;
    }
    if id.is_some_and(|id| {
        !id.as_str().is_some_and(|s| !s.is_empty() && s.len() <= 128)
            && id.as_i64().is_none()
            && id.as_u64().is_none()
    }) {
        return false;
    }
    matches!(
        request["method"].as_str(),
        Some(
            "initialize"
                | "server/discover"
                | "ping"
                | "tools/list"
                | "tools/call"
                | "resources/list"
                | "resources/read"
                | "resources/templates/list"
                | "prompts/list"
                | "prompts/get"
                | "notifications/initialized"
                | "notifications/cancelled"
        )
    )
}

/// Diagnostics contain only protocol field names and types. Unknown names,
/// IDs, arguments and other payload values must never reach the log.
#[cfg(any(debug_assertions, test))]
pub(super) fn request_shape(request: &Value) -> Value {
    fn kind(value: Option<&Value>) -> &'static str {
        match value {
            None => "absent",
            Some(Value::Null) => "null",
            Some(Value::Bool(_)) => "boolean",
            Some(Value::Number(_)) => "number",
            Some(Value::String(_)) => "string",
            Some(Value::Array(_)) => "array",
            Some(Value::Object(_)) => "object",
        }
    }
    fn keys(value: &Value, allowed: &[&str]) -> Value {
        let Some(object) = value.as_object() else {
            return Value::Null;
        };
        json!({
            "known": allowed.iter().filter(|key| object.contains_key(**key)).collect::<Vec<_>>(),
            "other_count": object.keys().filter(|key| !allowed.contains(&key.as_str())).count(),
        })
    }
    let method = match request.get("method").and_then(Value::as_str) {
        Some(
            method @ ("initialize"
            | "ping"
            | "tools/list"
            | "tools/call"
            | "resources/list"
            | "resources/read"
            | "resources/templates/list"
            | "prompts/list"
            | "prompts/get"
            | "server/discover"
            | "subscriptions/listen"
            | "notifications/initialized"
            | "notifications/cancelled"),
        ) => method,
        Some(_) => "other",
        None => "non_string_or_absent",
    };
    json!({
        "type": kind(Some(request)),
        "keys": keys(request, &["jsonrpc", "id", "method", "params", "_meta"]),
        "method": method,
        "id_type": kind(request.get("id")),
        "params_type": kind(request.get("params")),
        "params_keys": keys(&request["params"], &["name", "arguments", "task", "_meta", "protocolVersion", "capabilities", "clientInfo", "cursor", "requestId", "reason"]),
        "task_type": kind(request["params"].get("task")),
    })
}

pub(super) fn catalog(reply: &mut Value) {
    // Mutable indexing inserts missing fields, which makes Copilot decode an
    // ordinary CallToolResult as a different result variant (`tools: null`).
    let Some(tools) = reply
        .get_mut("result")
        .and_then(|result| result.get_mut("tools"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for tool in tools.iter_mut().filter(|t| {
        t["name"]
            .as_str()
            .is_some_and(crate::copilot_attestation::native_tool)
    }) {
        if !tool["_meta"].is_object() {
            tool["_meta"] = json!({});
        }
        if !tool["_meta"]["ui"].is_object() {
            tool["_meta"]["ui"] = json!({});
        }
        tool["_meta"]["ui"]["visibility"] = json!(["model"]);
    }
}

pub(super) fn transport_capabilities(reply: &mut Value) {
    // This one-response bridge does not carry the hosted server's unsolicited
    // notifications or subscription streams. Advertising them can make a
    // client fail setup while trying to subscribe to changes it cannot see.
    let Some(capabilities) = reply
        .get_mut("result")
        .and_then(|result| result.get_mut("capabilities"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for name in ["tools", "resources", "prompts"] {
        if let Some(capability) = capabilities.get_mut(name).and_then(Value::as_object_mut) {
            capability.remove("listChanged");
            if name == "resources" {
                capability.remove("subscribe");
            }
        }
    }
}

pub(super) fn normalize_reply(method: &str, reply: &mut Value) {
    if method == "initialize" {
        transport_capabilities(reply);
    }
    catalog(reply);
}
