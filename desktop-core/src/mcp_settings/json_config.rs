use super::*;

pub(super) fn install(
    path: &Path,
    binding: &Binding,
    helper: &str,
    name: &str,
    cleanup_projects: bool,
) -> bool {
    let Some(endpoint) = binding.endpoint() else {
        return false;
    };
    let _ = files::update(path, |text| render_as(text, &endpoint, helper, name));
    let installed = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|root| {
            root["mcpServers"].as_object().is_some_and(|servers| {
                servers.values().any(|server| {
                    server["url"] == endpoint
                        && !disabled(server)
                        && server["headersHelper"] == helper
                })
            })
        });
    installed && (!cleanup_projects || scopes::clear(path, Some(&endpoint), Some(helper), true))
}

#[cfg(test)]
pub(super) fn render(text: &str, endpoint: &str, helper: &str) -> Option<String> {
    render_as(text, endpoint, helper, NAME)
}

/// `text` with the hosted entry `name` set to `endpoint` and `helper`.
pub(super) fn render_as(text: &str, endpoint: &str, helper: &str, name: &str) -> Option<String> {
    let mut root = if text.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str::<Value>(text).ok()?
    };
    let servers = root
        .as_object_mut()?
        .entry("mcpServers")
        .or_insert_with(|| json!({}))
        .as_object_mut()?;
    if servers
        .get(name)
        .is_some_and(|s| !recognized::json(s, Some(endpoint), Some(helper)))
    {
        return None;
    }
    let before = servers.clone();
    servers.retain(|_, s| !recognized::json(s, Some(endpoint), Some(helper)));
    servers.insert(
        name.into(),
        json!({"type":"http", "url":endpoint, "headersHelper":helper}),
    );
    let mut changed = before != *servers;
    if let Some(projects) = root["projects"].as_object_mut() {
        for project in projects.values_mut() {
            if let Some(servers) = project["mcpServers"].as_object_mut() {
                let before = servers.len();
                servers.retain(|_, s| !recognized::json(s, Some(endpoint), Some(helper)));
                changed |= before != servers.len();
            }
        }
    }
    if !changed {
        return None;
    }
    Some(format!("{}\n", serde_json::to_string_pretty(&root).ok()?))
}

fn disabled(server: &Value) -> bool {
    server["enabled"] == false || server["disabled"] == true
}
