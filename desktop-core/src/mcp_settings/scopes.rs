//! Only project roots already recorded by Claude are inspected. No filesystem
//! traversal, project discovery, or changes to unrelated server entries.
use super::*;

fn paths(global: &Path) -> Vec<PathBuf> {
    let root: Value = fs::read(global)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    root["projects"]
        .as_object()
        .into_iter()
        .flat_map(|projects| projects.keys())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(|path| path.join(".mcp.json"))
        .collect()
}

pub(super) fn clear(
    global: &Path,
    endpoint: Option<&str>,
    helper: Option<&str>,
    write: bool,
) -> bool {
    paths(global).into_iter().all(|path| {
        if write {
            let _ = files::update(&path, |text| {
                let mut root: Value = serde_json::from_str(text).ok()?;
                let servers = root.get_mut("mcpServers")?.as_object_mut()?;
                let before = servers.len();
                servers.retain(|_, server| !recognized::json(server, endpoint, helper));
                if before == servers.len() {
                    return None;
                }
                Some(format!("{}\n", serde_json::to_string_pretty(&root).ok()?))
            });
        }
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<Value>(&text)
                .ok()
                .is_some_and(|root| {
                    !root["mcpServers"].as_object().is_some_and(|servers| {
                        servers
                            .values()
                            .any(|server| recognized::json(server, endpoint, helper))
                    })
                }),
            Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        }
    })
}
