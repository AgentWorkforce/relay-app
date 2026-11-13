//! Configuration proof is stricter than recognizing entries for migration.
use super::*;
use toml_edit::Item;

pub(super) fn codex(server: &Item, endpoint: Option<&str>, helper: Option<&str>) -> bool {
    endpoint.is_some()
        && helper.is_some()
        && server.get("url").and_then(Item::as_str) == endpoint
        && server.get("http_headers_helper").and_then(Item::as_str) == helper
        && server.get("enabled").and_then(Item::as_bool) != Some(false)
        && [
            "command",
            "args",
            "bearer_token_env_var",
            "oauth",
            "oauth_resource",
            "auth",
        ]
        .iter()
        .all(|key| server.get(key).is_none())
        && ["http_headers", "env_http_headers"].iter().all(|key| {
            server.get(key).is_none_or(|headers| {
                headers.as_table_like().is_some_and(|headers| {
                    !headers
                        .iter()
                        .any(|(key, _)| key.eq_ignore_ascii_case("authorization"))
                })
            })
        })
}

#[cfg(test)]
pub(super) fn json(root: &Value, endpoint: Option<&str>, helper: Option<&str>) -> bool {
    json_as(root, endpoint, helper, NAME)
}

/// The hosted entry `name` is exactly the current one, and the only one.
pub(super) fn json_as(
    root: &Value,
    endpoint: Option<&str>,
    helper: Option<&str>,
    name: &str,
) -> bool {
    let server = &root["mcpServers"][name];
    endpoint.is_some()
        && helper.is_some()
        && server["url"].as_str() == endpoint
        && server["headersHelper"].as_str() == helper
        && server["type"] == "http"
        && server["disabled"] != true
        && server["enabled"] != false
        && [
            "command",
            "args",
            "auth",
            "oauth",
            "bearer_token_env_var",
            "env_http_headers",
        ]
        .iter()
        .all(|key| server.get(key).is_none())
        && server.get("headers").is_none_or(|headers| {
            headers.as_object().is_some_and(|headers| {
                !headers
                    .keys()
                    .any(|key| key.eq_ignore_ascii_case("authorization"))
            })
        })
        && count_json(root, endpoint, helper) == 1
}

fn count_json(root: &Value, endpoint: Option<&str>, helper: Option<&str>) -> usize {
    let count = root["mcpServers"].as_object().map_or(0, |servers| {
        servers
            .values()
            .filter(|s| recognized::json(s, endpoint, helper))
            .count()
    });
    count
        + root["projects"].as_object().map_or(0, |projects| {
            projects
                .values()
                .map(|root| count_json(root, endpoint, helper))
                .sum()
        })
}
