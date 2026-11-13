use super::*;
mod tmux;

pub(super) fn focus(host: &dyn Host, location: &SessionLocation) -> Result<Outcome, String> {
    verify(host, location)?;
    let pane = location
        .pane_id
        .as_deref()
        .ok_or("The terminal did not record an exact pane identity.")?;
    let capability = location.focus_capability.as_deref().unwrap_or("");
    let endpoint = location
        .environment_identity
        .as_deref()
        .ok_or("The terminal's remote-control endpoint was not recorded.")?;
    if capability == "tmux" {
        return tmux::focus(host, location, pane, endpoint);
    }
    let (query, activate) = match capability {
        "kitty" if numeric(pane) && endpoint.starts_with("unix:") => (
            Invocation::new("kitty", ["@".into(), "--to".into(), endpoint.into(), "ls".into()]),
            Invocation::new("kitty", ["@".into(), "--to".into(), endpoint.into(), "focus-window".into(), "--match".into(), format!("id:{pane}")]),
        ),
        "wezterm" if numeric(pane) && endpoint.starts_with('/') => {
            let mut query = Invocation::new("wezterm", ["cli".into(), "list".into(), "--format".into(), "json".into()]);
            let mut activate = Invocation::new("wezterm", ["cli".into(), "activate-pane".into(), "--pane-id".into(), pane.into()]);
            query.environment.push(("WEZTERM_UNIX_SOCKET".into(), endpoint.into()));
            activate.environment = query.environment.clone();
            (query, activate)
        }
        _ => return Err("This terminal cannot safely select the original pane. Use Show application when available.".into()),
    };
    let evidence = host.run(&query)?;
    if !pane_matches(location, &evidence) {
        return Err("The recorded terminal pane no longer belongs to this session.".into());
    }
    verify(host, location)?;
    host.run(&activate)?;
    // A successful command alone cannot prove the window manager allowed focus.
    if location.window_id.is_some() {
        host.window(location)?;
    }
    Ok(Outcome::Dispatched)
}

/// Remote-control listings must bind a stable pane to the observed terminal
/// device (or owner PID). Titles and working directories are never evidence.
pub(super) fn pane_matches(location: &SessionLocation, listing: &str) -> bool {
    let pane = location.pane_id.as_deref().unwrap_or("");
    let Ok(value) = serde_json::from_str::<serde_json::Value>(listing) else {
        return false;
    };
    fn find(value: &serde_json::Value, pane: &str, location: &SessionLocation) -> usize {
        match value {
            serde_json::Value::Array(values) => {
                values.iter().map(|v| find(v, pane, location)).sum()
            }
            serde_json::Value::Object(map) => {
                let id = map
                    .get("pane_id")
                    .or_else(|| map.get("id"))
                    .map(|id| id.to_string().trim_matches('"').to_owned());
                let tty = map
                    .get("tty_name")
                    .or_else(|| map.get("tty"))
                    .and_then(|v| v.as_str());
                let pid = map.get("pid").and_then(|v| v.as_i64());
                let foreground = map
                    .get("foreground_processes")
                    .and_then(|v| v.as_array())
                    .is_some_and(|processes| {
                        processes
                            .iter()
                            .any(|p| p["pid"].as_i64() == Some(i64::from(location.pid)))
                    });
                let matches = id.as_deref() == Some(pane)
                    && ((tty.is_some() && tty == location.tty.as_deref())
                        || pid == Some(i64::from(location.pid))
                        || foreground);
                usize::from(matches) + map.values().map(|v| find(v, pane, location)).sum::<usize>()
            }
            _ => 0,
        }
    }
    find(&value, pane, location) == 1
}
