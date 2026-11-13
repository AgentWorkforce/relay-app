use super::*;
fn application(exe: &std::path::Path) -> Option<(&'static str, &'static str)> {
    let path = exe.to_str()?;
    [
        (
            "/Ghostty.app/Contents/MacOS/ghostty",
            "Ghostty",
            "com.mitchellh.ghostty",
        ),
        (
            "/Terminal.app/Contents/MacOS/Terminal",
            "Terminal",
            "com.apple.Terminal",
        ),
        (
            "/iTerm.app/Contents/MacOS/iTerm2",
            "iTerm2",
            "com.googlecode.iterm2",
        ),
        (
            "/ChatGPT.app/Contents/MacOS/ChatGPT",
            "Codex",
            "com.openai.codex",
        ),
        (
            "/Codex.app/Contents/MacOS/Codex",
            "Codex",
            "com.openai.codex",
        ),
        (
            "/Claude.app/Contents/MacOS/Claude",
            "Claude",
            "com.anthropic.claudefordesktop",
        ),
        (
            "/Warp.app/Contents/MacOS/stable",
            "Warp",
            "dev.warp.Warp-Stable",
        ),
        (
            "/kitty.app/Contents/MacOS/kitty",
            "Kitty",
            "net.kovidgoyal.kitty",
        ),
        (
            "/WezTerm.app/Contents/MacOS/wezterm-gui",
            "WezTerm",
            "com.github.wez.wezterm",
        ),
        (
            "/Alacritty.app/Contents/MacOS/alacritty",
            "Alacritty",
            "org.alacritty",
        ),
        (
            "/Visual Studio Code.app/Contents/MacOS/Electron",
            "Visual Studio Code",
            "com.microsoft.VSCode",
        ),
        (
            "/Cursor.app/Contents/MacOS/Cursor",
            "Cursor",
            "com.todesktop.230313mzl4w4u92",
        ),
        ("/Zed.app/Contents/MacOS/zed", "Zed", "dev.zed.Zed"),
        ("/cmux.app/Contents/MacOS/cmux", "cmux", "com.cmuxterm.app"),
    ]
    .into_iter()
    .find(|(suffix, _, _)| path.ends_with(suffix))
    .map(|(_, name, bundle)| (name, bundle))
}

/// A boot token makes Linux start ticks safe across restarts. Other kernels use
/// absolute creation timestamps, but still retain their boot token when available.
pub(super) fn boot_id() -> Option<String> {
    static BOOT: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    BOOT.get_or_init(|| {
        #[cfg(target_os = "linux")]
        {
            std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
                .ok()
                .map(|s| s.trim().to_owned())
        }
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("/usr/sbin/sysctl")
                .args(["-n", "kern.boottime"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_owned())
        }
        #[cfg(windows)]
        {
            // Windows creation times are absolute. A boot time inferred from
            // uptime changes with clock corrections and cannot prove a reboot.
            None
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            None
        }
    })
    .clone()
}

fn cross_platform_application(exe: &Path) -> Option<(&'static str, &'static str)> {
    if let Some(app) = application(exe) {
        return Some(app);
    }
    // Windows paths remain recognizable in fixture tables on Unix too.
    let normalized = exe
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    if let Some(app) = provider_application(&normalized) {
        return Some(app);
    }
    let name = normalized.rsplit('/').next()?.trim_end_matches(".exe");
    Some(match name {
        "tmux" => ("tmux", "tmux"),
        "claude-desktop" => ("Claude", "com.anthropic.claudefordesktop"),
        "ghostty" => ("Ghostty", "com.mitchellh.ghostty"),
        "kitty" => ("Kitty", "net.kovidgoyal.kitty"),
        "wezterm-gui" => ("WezTerm", "com.github.wez.wezterm"),
        "alacritty" => ("Alacritty", "org.alacritty"),
        "gnome-terminal-server" | "gnome-terminal" => ("GNOME Terminal", "org.gnome.Terminal"),
        "konsole" => ("Konsole", "org.kde.konsole"),
        "xterm" => ("XTerm", "xterm"),
        "windowsterminal" => ("Windows Terminal", "Microsoft.WindowsTerminal"),
        "code" => ("Visual Studio Code", "com.microsoft.VSCode"),
        "cursor" => ("Cursor", "com.todesktop.230313mzl4w4u92"),
        _ => return None,
    })
}

fn provider_application(normalized: &str) -> Option<(&'static str, &'static str)> {
    if normalized.ends_with("/claude.exe")
        && (normalized.contains("/claude/")
            || normalized.contains("/anthropicclaude/")
            || normalized.contains("/anthropicpbc.claude_"))
    {
        return Some(("Claude", "com.anthropic.claudefordesktop"));
    }
    if (normalized.ends_with("/codex.exe")
        && (normalized.contains("/openai.codex_")
            || normalized.contains("/codex/app/")
            || normalized.contains("/programs/codex/")))
        || normalized.ends_with("/opt/codex/codex")
    {
        return Some(("Codex", "com.openai.codex"));
    }
    None
}

pub(super) fn from_process(
    table: &dyn ProcessTable,
    uid: u32,
    pid: i32,
) -> Option<SessionLocation> {
    let owner = table
        .process(pid)
        .filter(|p| p.uid == uid && !table.zombie(pid))?;
    let mut location = initial_location(table, pid, owner);
    pane_identity(table, pid, &mut location);
    let mut current = pid;
    let mut seen = HashSet::new();
    for _ in 0..MAX_ANCESTRY {
        if !seen.insert(current) {
            return None;
        }
        let Some(process) = table
            .process(current)
            .filter(|p| p.uid == uid && !table.zombie(current))
        else {
            break;
        };
        if let Some((name, bundle)) = table
            .executable(current)
            .as_deref()
            .and_then(cross_platform_application)
        {
            if table.process(pid) != Some(owner) || table.process(current) != Some(process) {
                return None;
            }
            apply_host(table, current, process, name, bundle, &mut location);
            return Some(location);
        }
        if process.parent <= 1 {
            break;
        }
        current = process.parent;
    }
    (table.process(pid) == Some(owner)).then_some(location)
}

fn observed_environment(table: &dyn ProcessTable, pid: i32, key: &str) -> Option<String> {
    table
        .environment_var(pid, key)
        .filter(|v| !v.is_empty() && v.len() <= 4096 && !v.contains('\0'))
}

fn initial_location(table: &dyn ProcessTable, pid: i32, owner: ProcessInfo) -> SessionLocation {
    let env = |key| observed_environment(table, pid, key);
    let tty = table
        .terminal_path(pid)
        .map(|p| p.to_string_lossy().into_owned());
    let (provider_environment_known, provider_environment) = provider_environment(table, pid);
    SessionLocation {
        application: "Background process".into(),
        bundle_id: String::new(),
        pid,
        process_start: owner.start,
        tty,
        observed_at_ms: agent_relay_node::relay::state::now_ms(),
        boot_id: boot_id(),
        provider_environment_known: Some(provider_environment_known),
        provider_environment,
        cwd: table.cwd(pid).map(|p| p.to_string_lossy().into_owned()),
        executable: table
            .executable(pid)
            .map(|p| p.to_string_lossy().into_owned()),
        runtime: Some(
            if env("WSL_DISTRO_NAME").is_some() {
                "wsl"
            } else {
                std::env::consts::OS
            }
            .into(),
        ),
        window_id: env("WINDOWID"),
        profile_id: env("WT_PROFILE_ID"),
        focus_capability: Some("none".into()),
        resume_capability: Some("unavailable".into()),
        ..Default::default()
    }
}

fn pane_identity(table: &dyn ProcessTable, pid: i32, location: &mut SessionLocation) {
    let env = |key| observed_environment(table, pid, key);
    for (key, cap, socket) in [
        ("TMUX_PANE", "tmux", "TMUX"),
        ("KITTY_WINDOW_ID", "kitty", "KITTY_LISTEN_ON"),
        ("WEZTERM_PANE", "wezterm", "WEZTERM_UNIX_SOCKET"),
    ] {
        if let Some(pane) = env(key) {
            location.pane_id = Some(pane);
            location.environment_identity = env(socket).map(|s| {
                if socket == "TMUX" {
                    s.split(',').next().unwrap_or("").to_owned()
                } else {
                    s
                }
            });
            if location.environment_identity.is_some() {
                location.focus_capability = Some(cap.into());
            }
            break;
        }
    }
    if location.pane_id.is_none() {
        location.pane_id = env("WT_SESSION");
    }
}

fn apply_host(
    table: &dyn ProcessTable,
    current: i32,
    process: ProcessInfo,
    name: &str,
    bundle: &str,
    location: &mut SessionLocation,
) {
    location.application = name.into();
    location.bundle_id = bundle.into();
    location.host_pid = Some(current);
    location.host_process_start = Some(process.start);
    location.host_executable = table
        .executable(current)
        .map(|p| p.to_string_lossy().into_owned());
    location.resume_capability = Some(
        if matches!(name, "Codex" | "Claude") && location.tty.is_none() {
            "desktop"
        } else {
            "argv"
        }
        .into(),
    );
    if location.pane_id.is_none()
        && location.tty.is_some()
        && matches!(
            bundle,
            "com.mitchellh.ghostty" | "com.apple.Terminal" | "com.googlecode.iterm2"
        )
        && cfg!(target_os = "macos")
    {
        location.focus_capability = Some("macos-tty".into());
    } else if location.focus_capability.as_deref() == Some("none") && location.window_id.is_some() {
        location.focus_capability = Some("x11".into());
    }
}

fn provider_environment(
    table: &dyn ProcessTable,
    pid: i32,
) -> (bool, Option<std::collections::BTreeMap<String, String>>) {
    // Select paths from one kernel read so a later failed read cannot turn
    // "unreadable" into "no overrides". Only the allowlisted paths survive.
    let observed = table.command_line(pid);
    if observed.environment.is_none() {
        return (false, None);
    }
    let home = if cfg!(windows) {
        observed.var("USERPROFILE").or_else(|| observed.var("HOME"))
    } else {
        observed.var("HOME").or_else(|| observed.var("USERPROFILE"))
    }
    .filter(|p| absolute_provider_path(p));
    let cwd = table.cwd(pid);
    let mut values = std::collections::BTreeMap::new();
    for key in PROVIDER_ENVIRONMENT_KEYS {
        let Some(raw) = observed.var(key) else {
            continue;
        };
        if raw.is_empty() || raw.contains('\0') || raw.len() > 4096 {
            return (false, None);
        }
        let raw = raw.as_str();
        let path = if raw == "~" {
            home.clone()
        } else if let Some(tail) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
            home.as_ref()
                .map(|root| Path::new(root).join(tail).to_string_lossy().into_owned())
        } else if absolute_provider_path(raw) {
            Some(raw.into())
        } else {
            cwd.as_ref()
                .map(|root| root.join(raw).to_string_lossy().into_owned())
                .filter(|path| absolute_provider_path(path))
        };
        let Some(path) = path else {
            return (false, None);
        };
        values.insert((*key).to_owned(), path);
    }
    // Recording effective defaults from the original HOME avoids changing stores
    // when the launcher later starts a differently configured login shell. HOME
    // itself is never retained or replaced.
    if let Some(home) = home {
        for (key, folder) in [("CLAUDE_CONFIG_DIR", ".claude"), ("GROK_HOME", ".grok")] {
            values
                .entry(key.into())
                .or_insert_with(|| Path::new(&home).join(folder).to_string_lossy().into_owned());
        }
        let codex_home = values
            .entry("CODEX_HOME".into())
            .or_insert_with(|| {
                Path::new(&home)
                    .join(".codex")
                    .to_string_lossy()
                    .into_owned()
            })
            .clone();
        values
            .entry("CODEX_SQLITE_HOME".into())
            .or_insert(codex_home);
    }
    (true, Some(values))
}
fn absolute_provider_path(path: &str) -> bool {
    path.len() <= 4096
        && !path.contains('\0')
        && (path.starts_with('/')
            || path.starts_with("\\\\")
            || (path.len() > 2
                && path.as_bytes()[0].is_ascii_alphabetic()
                && path.as_bytes()[1] == b':'
                && matches!(path.as_bytes()[2], b'/' | b'\\')))
}
