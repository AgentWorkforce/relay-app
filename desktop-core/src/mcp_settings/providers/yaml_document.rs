use super::*;

fn parse(original: &str) -> Result<Value> {
    if original.trim().is_empty() {
        return Ok(json!({}));
    }
    let value = serde_yaml::from_str::<Value>(original)
        .map_err(|_| "Malformed provider YAML configuration")?;
    if !value.is_object() {
        return Err("Provider configuration must be a YAML mapping".into());
    }
    Ok(value)
}

pub(super) fn servers(original: &str) -> Result<Map<String, Value>> {
    let value = parse(original)?;
    match value.get("mcp_servers") {
        Some(value) => value
            .as_object()
            .cloned()
            .ok_or_else(|| "MCP servers must be a YAML mapping".into()),
        None => Ok(Map::new()),
    }
}

pub(super) fn render(
    original: &str,
    hosted: Option<&HostedConfig>,
    endpoint: Option<&str>,
) -> Result<String> {
    let mut expected = parse(original)?;
    let mut servers = servers(original)?;
    let before = servers.clone();
    update_servers(&mut servers, hosted, endpoint, Provider::Hermes, None)?;
    if before == servers {
        return Ok(original.into());
    }
    expected["mcp_servers"] = Value::Object(servers.clone());
    let block = serde_yaml::to_string(&json!({"mcp_servers":servers}))
        .map_err(|_| "Cannot encode MCP YAML configuration")?;
    let edited = if original.trim_start().starts_with('{') {
        serde_yaml::to_string(&expected).map_err(|_| "Cannot encode MCP YAML configuration")?
    } else {
        replace_block(original, &block)?
    };
    if parse(&edited)? != expected {
        return Err("Could not safely edit provider YAML configuration".into());
    }
    Ok(edited)
}

fn replace_block(original: &str, block: &str) -> Result<String> {
    if original.trim().is_empty() {
        return Ok(block.into());
    }
    let mut offset = 0;
    let mut start = None;
    let mut end = original.len();
    for line in original.split_inclusive('\n') {
        let trimmed = line.trim();
        let is_root = !line.starts_with(char::is_whitespace)
            && !trimmed.is_empty()
            && !trimmed.starts_with('#')
            && !trimmed.starts_with("---");
        if is_root && start.is_some() {
            end = offset;
            break;
        }
        if is_root
            && (line.starts_with("mcp_servers:")
                || line.starts_with("'mcp_servers':")
                || line.starts_with("\"mcp_servers\":"))
        {
            start = Some(offset);
        }
        offset += line.len();
    }
    if let Some(start) = start {
        return Ok(format!(
            "{}{}{}",
            &original[..start],
            block,
            &original[end..]
        ));
    }
    if parse(original)?.get("mcp_servers").is_some() {
        return Err("MCP YAML mapping needs block-style formatting before installation".into());
    }
    Ok(format!("{}\n{}", original.trim_end(), block))
}
