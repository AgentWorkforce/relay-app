use super::*;
use toml_edit::{value, DocumentMut, Item, Table};

pub(super) fn install(
    config: &CodexConfig,
    binding: &Binding,
    helper: &str,
    name: &str,
    logout: &mut dyn FnMut(&CodexConfig, &str) -> bool,
) -> bool {
    let Some(endpoint) = binding.endpoint() else {
        return false;
    };
    let path = config.home.join("config.toml");
    let _ = files::update(&path, |text| {
        render_as(text, &endpoint, helper, name, &mut |alias| {
            logout(config, alias)
        })
    });
    fs::read_to_string(path)
        .ok()
        .and_then(|text| text.parse::<DocumentMut>().ok())
        .is_some_and(|document| {
            document
                .get("mcp_servers")
                .and_then(Item::as_table_like)
                .is_some_and(|servers| {
                    servers.iter().any(|(_, server)| {
                        server.get("url").and_then(Item::as_str) == Some(endpoint.as_str())
                            && server.get("enabled").and_then(Item::as_bool) != Some(false)
                            && server.get("http_headers_helper").and_then(Item::as_str)
                                == Some(helper)
                    })
                })
        })
}

#[cfg(test)]
pub(super) fn render(
    text: &str,
    endpoint: &str,
    helper: &str,
    logout: &mut dyn FnMut(&str) -> bool,
) -> Option<String> {
    render_as(text, endpoint, helper, NAME, logout)
}

/// `text` with the hosted entry `name` set to `endpoint` and `helper`.
pub(super) fn render_as(
    text: &str,
    endpoint: &str,
    helper: &str,
    name: &str,
    logout: &mut dyn FnMut(&str) -> bool,
) -> Option<String> {
    let mut document = text.parse::<DocumentMut>().ok()?;
    let before = document.to_string();
    let approve_launches = launches_are_preapproved(&document);
    if !document.contains_key("mcp_servers") {
        document["mcp_servers"] = Item::Table(Table::new());
    }
    let inline = document.get("mcp_servers")?.is_inline_table();
    let servers = document.get_mut("mcp_servers")?.as_table_like_mut()?;
    if servers
        .get(name)
        .is_some_and(|s| !recognized::toml(s, Some(endpoint), Some(helper)))
    {
        return None;
    }
    let aliases: Vec<_> = servers
        .iter()
        .filter(|(_, s)| recognized::toml(s, Some(endpoint), Some(helper)))
        .map(|(name, _)| name.to_string())
        .collect();
    for alias in &aliases {
        let server = servers.get(alias)?;
        let managed = server
            .get("http_headers_helper")
            .and_then(Item::as_str)
            .is_some_and(|old| managed_helper(old, helper));
        let hosted = server
            .get("url")
            .and_then(Item::as_str)
            .is_some_and(|url| recognized::endpoint(url, Some(endpoint)));
        if hosted && !managed && !logout(alias) {
            return None;
        }
    }
    // Retain the person's tool restrictions and approval choices across repair
    // and account changes. Authentication fields are still replaced below.
    let previous = servers
        .get(name)
        .or_else(|| aliases.first().and_then(|alias| servers.get(alias)));
    let retained: Vec<_> = [
        "tools",
        "default_tools_approval_mode",
        "enabled_tools",
        "disabled_tools",
    ]
    .into_iter()
    .filter_map(|key| {
        previous
            .and_then(|server| server.get(key))
            .map(|item| (key, item.clone()))
    })
    .collect();
    let mut server = if inline {
        Item::Value(toml_edit::InlineTable::new().into())
    } else {
        Item::Table(Table::new())
    };
    server.as_table_like_mut()?.insert("url", value(endpoint));
    server
        .as_table_like_mut()?
        .insert("http_headers_helper", value(helper));
    for (key, item) in retained {
        server.as_table_like_mut()?.insert(key, item);
    }
    messaging_approvals(&mut server, approve_launches)?;
    for alias in aliases {
        servers.remove(&alias);
    }
    servers.insert(name, server);
    let rendered = document.to_string();
    (before != rendered).then_some(rendered)
}

/// Messaging writes and read-only spawn lookups are pre-approved. Hosted
/// spawn and release are pre-approved only when the top-level
/// `approval_policy` and every profile that sets one are all `never`. The
/// tool table is shared by every profile, so one stricter profile leaves
/// those two unset. Repair drops a launch entry that holds only
/// `approval_mode = "approve"` once that is no longer true. Any other
/// approval is kept.
fn messaging_approvals(server: &mut Item, approve_launches: bool) -> Option<()> {
    if server.get("default_tools_approval_mode").is_some() {
        return Some(());
    }
    let entry = server.as_table_like_mut()?;
    if entry.get("tools").is_none() {
        entry.insert("tools", Item::Value(toml_edit::InlineTable::new().into()));
    }
    let tools = entry.get_mut("tools")?.as_table_like_mut()?;
    approve_unset(
        tools,
        &[
            "check_relay_inbox",
            "send_relay_message",
            "join_relay",
            "leave_relay",
            "relay_status",
            "read_relay_conversation",
            "list_relay_spawn_targets",
            "get_relay_spawn_status",
        ],
    )?;
    if approve_launches {
        approve_unset(tools, &["spawn_relay_agent", "release_relay_agent"])?;
    } else {
        clear_written_launch_approvals(tools);
    }
    Some(())
}

/// Launch approvals live on the shared tool table, so they are written only
/// when the top-level policy and every profile policy that is set are `never`.
/// A command-line flag is not in this file, so it does not count.
fn launches_are_preapproved(document: &DocumentMut) -> bool {
    if document.get("approval_policy").and_then(Item::as_str) != Some("never") {
        return false;
    }
    document
        .get("profiles")
        .and_then(Item::as_table_like)
        .is_none_or(|profiles| {
            profiles.iter().all(|(_, profile)| {
                profile
                    .get("approval_policy")
                    .and_then(Item::as_str)
                    .is_none_or(|policy| policy == "never")
            })
        })
}

fn clear_written_launch_approvals(tools: &mut dyn toml_edit::TableLike) {
    for name in ["spawn_relay_agent", "release_relay_agent"] {
        if tools.get(name).is_some_and(only_written_approve) {
            tools.remove(name);
        }
    }
}

fn only_written_approve(tool: &Item) -> bool {
    let Some(table) = tool.as_table_like() else {
        return false;
    };
    let mut keys = table.iter().map(|(key, _)| key);
    keys.next() == Some("approval_mode")
        && keys.next().is_none()
        && table.get("approval_mode").and_then(Item::as_str) == Some("approve")
}

fn approve_unset(tools: &mut dyn toml_edit::TableLike, names: &[&str]) -> Option<()> {
    for name in names {
        if tools.get(name).is_none() {
            tools.insert(name, Item::Value(toml_edit::InlineTable::new().into()));
        }
        let tool = tools.get_mut(name)?.as_table_like_mut()?;
        if tool.get("approval_mode").is_none() {
            tool.insert("approval_mode", value("approve"));
        }
    }
    Some(())
}
