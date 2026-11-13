//! Preserve all text outside the affected MCP container, including JSONC/JSON5
//! comments. Reparse the edited document before returning it to the writer.
use super::*;
mod spans;

fn parse(original: &str) -> Result<Value> {
    if original.trim().is_empty() {
        return Ok(json!({}));
    }
    let value: Value =
        json5::from_str(original).map_err(|_| "Malformed provider JSON configuration")?;
    if !value.is_object() {
        return Err("Provider configuration must be an object".into());
    }
    Ok(value)
}

fn route(provider: Provider, value: &Value) -> Result<Vec<&'static str>> {
    match provider {
        Provider::OpenCodeV1 => {
            if value["mcp"].get("servers").is_some() {
                return Err("OpenCode uses the v2 MCP schema; select the matching adapter".into());
            }
            Ok(vec!["mcp"])
        }
        Provider::OpenCodeV2 | Provider::OpenClaw => Ok(vec!["mcp", "servers"]),
        Provider::Muse => {
            if value.get("mcpServers").is_some() && value.get("mcp_servers").is_some() {
                return Err("Muse has conflicting MCP configuration schemas".into());
            }
            if value.get("schema_version").is_some_and(|v| v != &json!(1)) {
                return Err("Unsupported Muse configuration schema".into());
            }
            Ok(vec![if value.get("mcp_servers").is_some() {
                "mcp_servers"
            } else {
                "mcpServers"
            }])
        }
        Provider::Copilot => Ok(vec!["servers"]),
        _ => Ok(vec!["mcpServers"]),
    }
}

fn at<'a>(value: &'a Value, path: &[&str]) -> Result<Option<&'a Map<String, Value>>> {
    let mut current = value;
    for key in path {
        let object = current
            .as_object()
            .ok_or("MCP configuration must be an object")?;
        let Some(next) = object.get(*key) else {
            return Ok(None);
        };
        current = next;
    }
    current
        .as_object()
        .map(Some)
        .ok_or_else(|| "MCP servers must be an object".into())
}

pub(super) fn servers(provider: Provider, original: &str) -> Result<Map<String, Value>> {
    let value = parse(original)?;
    let route = route(provider, &value)?;
    let mut servers = at(&value, &route)?.cloned().unwrap_or_default();
    if provider == Provider::Muse && route == ["mcp_servers"] {
        for entry in servers.values_mut() {
            if entry.get("type").is_some() && entry.get("transport").is_some() {
                return Err("Muse MCP entry has conflicting transports".into());
            }
            if entry["transport"] == "streamable_http" {
                if let Some(object) = entry.as_object_mut() {
                    object.remove("transport");
                    object.insert("type".into(), json!("streamable-http"));
                }
            }
        }
    }
    Ok(servers)
}

pub(super) fn render(
    provider: Provider,
    original: &str,
    hosted: Option<&HostedConfig>,
    endpoint: Option<&str>,
    helper: Option<&Path>,
) -> Result<String> {
    let mut value = parse(original)?;
    let route = route(provider, &value)?;
    let mut servers = at(&value, &route)?.cloned().unwrap_or_default();
    let before = servers.clone();
    update_servers(&mut servers, hosted, endpoint, provider, helper)?;
    if provider == Provider::Muse && route == ["mcp_servers"] && hosted.is_some() {
        let object = servers
            .get_mut(NAME)
            .and_then(Value::as_object_mut)
            .ok_or("Missing Muse server")?;
        object.remove("type");
        object.insert("transport".into(), json!("streamable_http"));
    }
    let mut result = if original.trim().is_empty() {
        "{}".to_string()
    } else {
        original.to_string()
    };
    if before != servers {
        let new = Value::Object(servers);
        set(&mut value, &route, new.clone())?;
        result = spans::replace(&result, &route, &new)?;
    }
    if provider == Provider::Muse && hosted.is_some() && value.get("schema_version").is_none() {
        value["schema_version"] = json!(1);
        result = spans::replace(&result, &["schema_version"], &json!(1))?;
    }
    if parse(&result)? != value {
        return Err("Could not safely edit provider JSON configuration".into());
    }
    if value == parse(original)? {
        return Ok(original.to_string());
    }
    Ok(result)
}

/// Remove this attempt's exact owned entries, restore prior owned entries only
/// into vacant names, and retain current foreign servers and all other fields.
pub(super) fn restore_copilot(
    provider: Provider,
    current: &str,
    original: &str,
    hosted: &HostedConfig,
    helper: Option<&Path>,
) -> Result<String> {
    let mut value = parse(current)?;
    let path = route(provider, &value)?;
    let mut servers = at(&value, &path)?.cloned().unwrap_or_default();
    let before = servers.clone();
    let expected = hosted.entry(provider);
    servers.retain(|_, entry| entry != &expected);
    for (name, entry) in self::servers(provider, original)? {
        if managed_for(provider, &entry, Some(&hosted.endpoint), helper) {
            servers.entry(name).or_insert(entry);
        }
    }
    if before == servers {
        return Ok(current.to_string());
    }
    let replacement = Value::Object(servers);
    set(&mut value, &path, replacement.clone())?;
    let result = spans::replace(current, &path, &replacement)?;
    if parse(&result)? != value {
        return Err("Cannot safely roll back Copilot JSON".into());
    }
    Ok(result)
}

fn set(value: &mut Value, path: &[&str], replacement: Value) -> Result<()> {
    let (key, remaining) = path.split_first().ok_or("Missing MCP configuration path")?;
    let object = value
        .as_object_mut()
        .ok_or("MCP configuration must be an object")?;
    if remaining.is_empty() {
        object.insert((*key).into(), replacement);
    } else {
        set(
            object.entry(*key).or_insert_with(|| json!({})),
            remaining,
            replacement,
        )?;
    }
    Ok(())
}
