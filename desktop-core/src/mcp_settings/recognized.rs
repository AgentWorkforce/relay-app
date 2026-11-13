//! Recognition is based on endpoints and exact program/argument contracts,
//! never a user-chosen alias or a substring in an arbitrary shell command.
use super::*;

pub(super) fn endpoint(url: &str, current: Option<&str>) -> bool {
    if current == Some(url.trim_end_matches('/')) {
        return true;
    }
    let Ok(url) = url::Url::parse(url) else {
        return false;
    };
    url.scheme() == "https"
        && matches!(
            url.host_str(),
            Some("agentrelay.com" | "staging.agentrelay.com" | "preview.agentrelay.com")
        )
        && url.path().trim_end_matches('/') == ROUTE
        && url.username().is_empty()
        && url.port().is_none()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

pub(super) fn legacy(command: &str, args: &[&str]) -> bool {
    let program = Path::new(command).file_stem().and_then(|s| s.to_str());
    match program {
        Some("agent-relay-probe") => args.starts_with(&["relay", "mcp"]),
        Some("agent-relay") => args.first() == Some(&"mcp") || args.starts_with(&["relay", "mcp"]),
        Some("node") => args.first().is_some_and(|script| {
            Path::new(script).file_name().and_then(|s| s.to_str()) == Some("agent-relay-mcp.js")
        }),
        Some("npx") => {
            let args = if matches!(args.first(), Some(&"-y" | &"--yes")) {
                &args[1..]
            } else {
                args
            };
            args.first().is_some_and(|package| {
                *package == "agent-relay"
                    || package.strip_prefix("agent-relay@").is_some_and(|version| {
                        !version.is_empty()
                            && version
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b".-_^~".contains(&b))
                    })
            }) && args.get(1) == Some(&"mcp")
        }
        _ => false,
    }
}

pub(super) fn json(server: &Value, current: Option<&str>, helper: Option<&str>) -> bool {
    if helper
        .zip(server["headersHelper"].as_str())
        .is_some_and(|(expected, old)| other_build(old, expected))
    {
        return false;
    }
    server["url"]
        .as_str()
        .is_some_and(|url| endpoint(url, current))
        || helper.is_some_and(|helper| {
            server["headersHelper"]
                .as_str()
                .is_some_and(|old| managed_helper(old, helper))
        })
        || server["command"].as_str().is_some_and(|command| {
            let args: Vec<_> = server["args"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            legacy(command, &args)
        })
        || server["command"].as_array().is_some_and(|words| {
            let words: Option<Vec<_>> = words.iter().map(Value::as_str).collect();
            words.is_some_and(|words| {
                words
                    .split_first()
                    .is_some_and(|(command, args)| legacy(command, args))
            })
        })
}

pub(super) fn toml(server: &toml_edit::Item, current: Option<&str>, helper: Option<&str>) -> bool {
    let old = server
        .get("http_headers_helper")
        .and_then(toml_edit::Item::as_str);
    if helper
        .zip(old)
        .is_some_and(|(expected, old)| other_build(old, expected))
    {
        return false;
    }
    server
        .get("url")
        .and_then(toml_edit::Item::as_str)
        .is_some_and(|url| endpoint(url, current))
        || helper.is_some_and(|helper| {
            server
                .get("http_headers_helper")
                .and_then(toml_edit::Item::as_str)
                .is_some_and(|old| managed_helper(old, helper))
        })
        || server
            .get("command")
            .and_then(toml_edit::Item::as_str)
            .is_some_and(|command| {
                let args: Vec<_> = server
                    .get("args")
                    .and_then(toml_edit::Item::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(toml_edit::Value::as_str)
                    .collect();
                legacy(command, &args)
            })
}
