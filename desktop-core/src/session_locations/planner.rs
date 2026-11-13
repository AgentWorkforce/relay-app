//! One policy for every desktop shell. A plan describes an attempted action;
//! only the native adapter can report whether focus or launch was verified.
use super::*;

mod stopped;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    MacOS,
    Linux,
    Windows,
}
impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOS
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}
fn plan(action: SessionOpenAction, location: Option<SessionLocation>) -> SessionOpenPlan {
    SessionOpenPlan {
        action,
        location,
        url: None,
        command: None,
        choices: vec![],
        reason: None,
        can_resume_default: false,
    }
}
pub fn unavailable(reason: impl Into<String>) -> SessionOpenPlan {
    let mut p = plan(SessionOpenAction::Unavailable, None);
    p.reason = Some(reason.into());
    p
}
fn same_target(a: &SessionLocation, b: &SessionLocation) -> bool {
    a.pid == b.pid
        && a.process_start == b.process_start
        && a.boot_id == b.boot_id
        && a.bundle_id == b.bundle_id
        && a.tty == b.tty
        && a.host_pid == b.host_pid
        && a.host_process_start == b.host_process_start
        && a.pane_id == b.pane_id
        && a.window_id == b.window_id
        && a.environment_identity == b.environment_identity
        && a.provider_environment == b.provider_environment
        && a.provider_environment_known == b.provider_environment_known
}
fn link(url: String, location: Option<SessionLocation>) -> SessionOpenPlan {
    let mut p = plan(SessionOpenAction::OpenLink, location);
    p.url = Some(url);
    p
}
fn desktop_link(request: &SessionOpenRequest, report: &SessionLocationReport) -> Option<String> {
    match request.source.as_str() {
        "codex" => {
            let mut url = url::Url::parse("codex://threads/").ok()?;
            url.path_segments_mut().ok()?.push(&request.session_id);
            Some(url.into())
        }
        "claude" => report.desktop_link.clone(),
        _ => None,
    }
}
fn terminal(location: &SessionLocation) -> bool {
    location.tty.is_some()
        || location.pane_id.is_some()
        || location.resume_capability.as_deref() == Some("argv")
}

/// Inputs have just been rediscovered by the request handler. `selected` must
/// match the full process and pane identity from that new observation.
pub fn resolve(
    request: &SessionOpenRequest,
    report: &SessionLocationReport,
    platform: Platform,
) -> SessionOpenPlan {
    if !report.supported {
        return unavailable(
            report
                .reason
                .clone()
                .unwrap_or_else(|| "Local observations are unavailable.".into()),
        );
    }
    let selected = if let Some(selected) = &request.selected {
        let Some(current) = report.current.iter().find(|now| same_target(now, selected)) else {
            return unavailable(
                "This session has moved or ended. Refresh and choose its current location.",
            );
        };
        Some(current)
    } else {
        if report.current.len() > 1 {
            let mut p = plan(SessionOpenAction::Choose, None);
            p.choices = report.current.clone();
            return p;
        }
        report.current.first()
    };
    if let Some(current) = selected {
        return current_plan(request, report, platform, current);
    }
    if report.running {
        if request.source == "claude" {
            if let Some(url) = report.desktop_link.clone() {
                return link(url, None);
            }
        }
        return unavailable("This session is running, but its exact window cannot be located. No new session was started.");
    }
    if report.owner_state.as_deref() != Some("stopped") {
        return unavailable(report.reason.clone().unwrap_or_else(||"The process state is unknown. Refresh after local process observations become available; no new session was started.".into()));
    }
    stopped::resolve(request, report, platform)
}

fn current_plan(
    request: &SessionOpenRequest,
    report: &SessionLocationReport,
    platform: Platform,
    current: &SessionLocation,
) -> SessionOpenPlan {
    if current.runtime.as_deref() == Some("wsl") {
        return unavailable("The original session runs in WSL. Its distribution and terminal profile cannot yet be targeted safely.");
    }
    if terminal(current) {
        return terminal_plan(current, platform);
    }
    if matches!(current.application.as_str(), "Codex" | "Claude")
        || (request.source == "claude" && report.desktop_link.is_some())
    {
        if let Some(url) = desktop_link(request, report) {
            return link(url, Some(current.clone()));
        }
        return unavailable("The session is running in its desktop application, but its provider-owned session link is unavailable. No new session was started.");
    }
    unavailable("This session has a running background owner without a navigable window. No new session was started.")
}

fn absolute_directory(path: &str, platform: Platform) -> bool {
    if path.contains('\0') {
        return false;
    }
    match platform {
        Platform::MacOS | Platform::Linux => path.starts_with('/'),
        Platform::Windows => {
            path.starts_with("\\\\")
                || (path.len() > 2
                    && path.as_bytes()[0].is_ascii_alphabetic()
                    && path.as_bytes()[1] == b':'
                    && matches!(path.as_bytes()[2], b'/' | b'\\'))
        }
    }
}

fn resume_environment(
    origin: Option<&SessionLocation>,
    platform: Platform,
) -> Result<Option<std::collections::BTreeMap<String, String>>, &'static str> {
    let Some(origin) = origin else {
        return Ok(None);
    };
    if origin.provider_environment_known == Some(false)
        || (origin.provider_environment_known == Some(true)
            && origin.provider_environment.is_none())
    {
        return Err("The original provider storage environment could not be read. Resume from its original configured terminal; starting with a different store is unavailable.");
    }
    if let Some(environment) = &origin.provider_environment {
        if environment.iter().any(|(key, path)| {
            !PROVIDER_ENVIRONMENT_KEYS.contains(&key.as_str())
                || !absolute_directory(path, platform)
        }) {
            return Err(
                "The original provider storage paths cannot be safely restored on this platform.",
            );
        }
    }
    Ok(origin.provider_environment.clone())
}

fn terminal_plan(current: &SessionLocation, platform: Platform) -> SessionOpenPlan {
    let capability = current.focus_capability.as_deref().unwrap_or("none");
    if (platform == Platform::MacOS && capability == "macos-tty")
        || (platform != Platform::MacOS && ["kitty", "wezterm", "tmux"].contains(&capability))
    {
        return plan(SessionOpenAction::Focus, Some(current.clone()));
    }
    if platform == Platform::MacOS && current.application == "tmux" {
        return unavailable("The session is running inside tmux, but selecting that pane from macOS is unavailable. No new session was started.");
    }
    // Raising a window does not prove which pane is selected.
    if (platform == Platform::MacOS && !current.bundle_id.is_empty())
        || capability == "x11"
        || (platform == Platform::Windows && current.host_pid.is_some())
    {
        let mut p = plan(SessionOpenAction::ShowApplication, Some(current.clone()));
        p.reason=Some("The original application can be shown, but exact pane focus is unavailable. No new session was started.".into());
        return p;
    }
    unavailable(format!("The session is running in {}, but this host has no supported exact navigation adapter. No new session was started.",current.application))
}
