use super::*;

/// The caller supplies the environment and home. Development passes its private
/// profile environment; resolving paths never falls back to the process HOME.
#[derive(Clone, Debug)]
pub(crate) struct Locations {
    pub cursor: PathBuf,
    pub copilot_harness: PathBuf,
    pub grok: PathBuf,
    pub opencode: Vec<PathBuf>,
    pub devin: PathBuf,
    pub devin_legacy: PathBuf,
    pub muse: PathBuf,
    pub openclaw: PathBuf,
    pub hermes: PathBuf,
    /// Factory Droid reads `.factory/mcp.json` under `FACTORY_HOME_OVERRIDE`
    /// when set, otherwise under the home directory (`%USERPROFILE%` on Windows).
    pub droid: PathBuf,
    pub pi: PathBuf,
    /// VS Code's user MCP file, which GitHub Copilot Chat reads.
    pub copilot: PathBuf,
}

impl Locations {
    pub(crate) fn from_env(home: &Path, env: &impl Fn(&str) -> Option<String>) -> Self {
        let xdg = directory(env("XDG_CONFIG_HOME"), home, home.join(".config"));
        let opencode_dir = directory(env("OPENCODE_CONFIG_DIR"), home, xdg.join("opencode"));
        let mut opencode = vec![
            opencode_dir.join("opencode.json"),
            opencode_dir.join("opencode.jsonc"),
        ];
        if let Some(path) = explicit(env("OPENCODE_CONFIG"), home) {
            opencode.retain(|old| old != &path);
            opencode.push(path);
        }
        let openclaw_home = directory(env("OPENCLAW_STATE_DIR"), home, openclaw_default(home, env));
        Self {
            cursor: home.join(".cursor/mcp.json"),
            copilot_harness: directory(env("COPILOT_HOME"), home, home.join(".copilot"))
                .join("mcp-config.json"),
            grok: directory(env("GROK_HOME"), home, home.join(".grok")).join("config.toml"),
            opencode,
            devin: xdg.join("devin/mcp_config.json"),
            devin_legacy: xdg.join("devin/config.json"),
            muse: xdg.join("muse/settings.json"),
            openclaw: directory(
                env("OPENCLAW_CONFIG_PATH"),
                home,
                openclaw_home.join("openclaw.json"),
            ),
            hermes: directory(env("HERMES_HOME"), home, hermes_default(home, env))
                .join("config.yaml"),
            droid: directory(env("FACTORY_HOME_OVERRIDE"), home, home.to_path_buf())
                .join(".factory")
                .join("mcp.json"),
            pi: directory(env("PI_CODING_AGENT_DIR"), home, home.join(".pi/agent")),
            copilot: vscode_user_directory(home, env).join("mcp.json"),
        }
    }
}

fn openclaw_default(home: &Path, env: &impl Fn(&str) -> Option<String>) -> PathBuf {
    let base = directory(env("OPENCLAW_HOME"), home, home.to_path_buf());
    let profile = env("OPENCLAW_PROFILE").filter(|profile| {
        !profile.is_empty()
            && profile != "default"
            && profile
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
    });
    base.join(
        profile
            .map(|profile| format!(".openclaw-{profile}"))
            .unwrap_or_else(|| ".openclaw".into()),
    )
}

fn hermes_default(home: &Path, env: &impl Fn(&str) -> Option<String>) -> PathBuf {
    #[cfg(windows)]
    {
        directory(env("LOCALAPPDATA"), home, home.join("AppData/Local")).join("hermes")
    }
    #[cfg(not(windows))]
    {
        let _ = env;
        home.join(".hermes")
    }
}

/// VS Code keeps its user settings, `mcp.json` included, in `Code/User` under
/// the platform's application-data directory.
fn vscode_user_directory(home: &Path, env: &impl Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(portable) = explicit(env("VSCODE_PORTABLE"), home) {
        return portable.join("user-data").join("User");
    }
    #[cfg(windows)]
    let base = directory(env("APPDATA"), home, home.join("AppData/Roaming"));
    #[cfg(target_os = "macos")]
    let base = {
        let _ = env;
        home.join("Library/Application Support")
    };
    #[cfg(not(any(windows, target_os = "macos")))]
    let base = directory(env("XDG_CONFIG_HOME"), home, home.join(".config"));
    base.join("Code").join("User")
}

fn directory(value: Option<String>, home: &Path, fallback: PathBuf) -> PathBuf {
    explicit(value, home).unwrap_or(fallback)
}

fn explicit(value: Option<String>, home: &Path) -> Option<PathBuf> {
    let value = value?.trim().to_string();
    if value.is_empty() {
        return None;
    }
    let path = if value == "~" {
        home.into()
    } else if let Some(relative) = value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        home.join(relative)
    } else {
        PathBuf::from(value)
    };
    // Relative environment paths must never depend on the desktop's working
    // directory (and could escape a private development profile).
    path.is_absolute().then_some(path)
}
