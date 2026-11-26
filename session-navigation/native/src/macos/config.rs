//! Everything the macOS adapters need to know about the person's machine,
//! in one place. The logic never reads the process environment; only
//! [`MacConfig::from_env`] does, for `HOME`.
use super::machine::{CURL, LSOF, SQLITE};
use crate::common::config::{CommonConfig, Programs, ProviderPaths, Timeouts};
#[cfg(unix)]
use crate::common::grok;
use crate::system::join;

/// The macOS provider homes for a person whose home directory is `home`.
pub fn provider_paths(home: &str) -> ProviderPaths {
    let home = home.trim_end_matches('/');
    let at = |rel: &str| join(home, rel);
    let codex_home = at(".codex");
    ProviderPaths {
        claude_home: at(".claude"),
        claude_desktop_support: at("Library/Application Support/Claude"),
        codex_sqlite_home: codex_home.clone(),
        codex_home,
        opencode_data: at(".local/share/opencode"),
        opencode_state: at(".local/state/opencode"),
        opencode_channel: "latest".to_owned(),
        opencode_desktop_support: at("Library/Application Support/ai.opencode.desktop"),
        grok_home: at(".grok"),
        reservation_dir: at("Library/Application Support/session-navigation/reservations"),
        tool_search_path: vec![
            at(".local/bin"),
            at(".claude/local"),
            at(".opencode/bin"),
            at(".grok/bin"),
            "/opt/homebrew/bin".to_owned(),
            "/usr/local/bin".to_owned(),
        ],
    }
}

/// The system's own `sqlite3`, `curl` and `lsof`.
pub fn programs() -> Programs {
    Programs {
        sqlite3: SQLITE.to_owned(),
        curl: CURL.to_owned(),
        lsof: LSOF.to_owned(),
    }
}

/// Where the terminal and multiplexer APIs live. UNVERIFIED: every default
/// is the tool's documented default, none was observed on this Mac (none of
/// iTerm2, Kitty, WezTerm, Alacritty or tmux was installed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostConfig {
    /// `TMUX_TMPDIR`, default `/tmp` (= `/private/tmp` on macOS): sockets are
    /// `<tmpdir>/tmux-<uid>/<name>`.
    pub tmux_tmpdir: String,
    /// Directories searched for Kitty `listen_on` sockets named
    /// `<anything>-<kitty pid>`.
    pub kitty_socket_dirs: Vec<String>,
    /// Directories searched for WezTerm GUI sockets `gui-sock-<pid>`
    /// (`~/.local/share/wezterm` on macOS).
    pub wezterm_socket_dirs: Vec<String>,
    /// `env`, to set a launched program's environment without a shell.
    pub env_program: String,
}

impl HostConfig {
    pub fn for_user(home: &str) -> Self {
        Self {
            tmux_tmpdir: "/private/tmp".to_owned(),
            kitty_socket_dirs: vec!["/private/tmp".to_owned()],
            wezterm_socket_dirs: vec![join(home, ".local/share/wezterm")],
            env_program: "/usr/bin/env".to_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MacConfig {
    pub home: String,
    /// The person's real uid: `Stop` signals only processes they started.
    pub uid: u32,
    /// Provider paths, helper programs and timeouts.
    pub common: CommonConfig,
    /// Terminal and tmux API locations.
    pub hosts: HostConfig,
}

impl MacConfig {
    /// Defaults for a person whose home directory is `home`.
    pub fn for_user(home: &str, uid: u32) -> Self {
        let home = home.trim_end_matches('/');
        Self {
            home: home.to_owned(),
            uid,
            common: CommonConfig {
                paths: provider_paths(home),
                programs: programs(),
                timeouts: Timeouts::default(),
            },
            hosts: HostConfig::for_user(home),
        }
    }

    /// Reads `HOME`. Safe std has no `getuid`, so the uid is the owner of the
    /// home directory, which is the person's on a normal macOS account.
    #[cfg(unix)]
    pub fn from_env() -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let home = std::env::var("HOME").ok()?;
        if !session_navigation::parse::is_absolute_path(&home) {
            return None;
        }
        let uid = std::fs::metadata(&home).ok()?.uid();
        let mut cfg = Self::for_user(&home, uid);
        let grok = std::env::var("GROK_HOME").ok();
        cfg.common.paths.grok_home = grok::home_from_env(grok.as_deref(), &home);
        if let Some(tmpdir) = std::env::var("TMUX_TMPDIR")
            .ok()
            .filter(|t| session_navigation::parse::is_absolute_path(t))
        {
            cfg.hosts.tmux_tmpdir = tmpdir;
        }
        Some(cfg)
    }

    /// Windows has no uids and no Mac to configure: always `None`. (The
    /// macOS adapter builds there only so the crate compiles and tests
    /// everywhere.)
    #[cfg(windows)]
    pub fn from_env() -> Option<Self> {
        None
    }

    pub fn paths(&self) -> &ProviderPaths {
        &self.common.paths
    }

    pub fn timeouts(&self) -> &Timeouts {
        &self.common.timeouts
    }
}
