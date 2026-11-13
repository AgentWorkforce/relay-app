use super::*;
use toml_edit::{DocumentMut, Item, Table};

fn parse(original: &str) -> Result<DocumentMut> {
    original
        .parse()
        .map_err(|_| "Malformed provider TOML configuration".into())
}

pub(super) fn servers(original: &str) -> Result<Map<String, Value>> {
    let document = parse(original)?;
    let Some(item) = document.get("mcp_servers") else {
        return Ok(Map::new());
    };
    let table = item
        .as_table_like()
        .ok_or("MCP servers must be a TOML table")?;
    table
        .iter()
        .map(|(key, value)| Ok((key.into(), to_json(value)?)))
        .collect()
}

fn to_json(item: &Item) -> Result<Value> {
    if let Some(table) = item.as_table_like() {
        return table
            .iter()
            .map(|(key, value)| Ok((key.into(), to_json(value)?)))
            .collect::<Result<Map<_, _>>>()
            .map(Value::Object);
    }
    let value = item.as_value().ok_or("Invalid MCP configuration value")?;
    let result = match value {
        toml_edit::Value::String(v) => json!(v.value()),
        toml_edit::Value::Integer(v) => json!(v.value()),
        toml_edit::Value::Float(v) => json!(v.value()),
        toml_edit::Value::Boolean(v) => json!(v.value()),
        toml_edit::Value::Datetime(v) => json!(v.value().to_string()),
        toml_edit::Value::Array(array) => Value::Array(
            array
                .iter()
                .map(|v| to_json(&Item::Value(v.clone())))
                .collect::<Result<_>>()?,
        ),
        toml_edit::Value::InlineTable(_) => return Err("Invalid inline MCP table".into()),
    };
    Ok(result)
}

pub(super) fn render(
    original: &str,
    hosted: Option<&HostedConfig>,
    endpoint: Option<&str>,
) -> Result<String> {
    let mut document = parse(original)?;
    if document.get("mcp_servers").is_none() {
        if hosted.is_none() {
            return Ok(original.into());
        }
        document.insert("mcp_servers", Item::Table(Table::new()));
    }
    let table = document
        .get_mut("mcp_servers")
        .and_then(Item::as_table_like_mut)
        .ok_or("MCP servers must be a TOML table")?;
    if hosted.is_some() && table.get(NAME).is_some_and(|entry| !owned(entry, endpoint)) {
        return Err("A different server already uses the agent-relay name".into());
    }
    if let Some(hosted) = hosted {
        if servers(original)?.get(NAME) == Some(&hosted.entry(Provider::Grok))
            && table
                .iter()
                .all(|(key, entry)| key == NAME || !owned(entry, endpoint))
        {
            return Ok(original.into());
        }
    }
    let removed: Vec<_> = table
        .iter()
        .filter(|(_, entry)| owned(entry, endpoint))
        .map(|(name, _)| name.to_string())
        .collect();
    for name in removed {
        table.remove(&name);
    }
    if let Some(hosted) = hosted {
        let mut entry = Table::new();
        entry.insert("url", toml_edit::value(&hosted.endpoint));
        let mut headers = toml_edit::InlineTable::new();
        for (key, value) in &hosted.headers {
            if hosted.bearer_token_file.is_some() && key.eq_ignore_ascii_case("authorization") {
                continue;
            }
            headers.insert(key, value.as_str().into());
        }
        headers.insert(OWNER_HEADER, OWNER.into());
        entry.insert("headers", Item::Value(headers.into()));
        if let Some(path) = &hosted.bearer_token_file {
            entry.insert(
                "bearer_token_file",
                toml_edit::value(path.to_str().ok_or("Grok token path is not UTF-8")?),
            );
        }
        table.insert(NAME, Item::Table(entry));
    }
    Ok(document.to_string())
}

fn owned(entry: &Item, endpoint: Option<&str>) -> bool {
    to_json(entry).is_ok_and(|entry| managed(&entry, endpoint))
}
