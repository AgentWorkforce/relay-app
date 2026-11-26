//! What the OS-independent observers need to know about the machine: where
//! each provider keeps its files, which helper programs to run, and how long
//! to wait. Nothing here reads the process environment; each platform module
//! has a default constructor for [`ProviderPaths`] and [`Programs`].
use std::time::Duration;

use crate::system::join;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Timeouts {
    /// `ps`, `sysctl`, `lsof`, `sqlite3`.
    pub probe: Duration,
    /// The app/scheme registry query (JXA on macOS).
    pub apps: Duration,
    /// `<tool> --version`.
    pub version: Duration,
    /// curl's `--max-time` (the run timeout adds a second on top).
    pub http: Duration,
    /// Read-only host scripts (front pane tty).
    pub script_read: Duration,
    /// Host scripts that act (focus, launch). The macOS scripts carry their
    /// own 15 s `with timeout`; this bounds the whole run.
    pub script_act: Duration,
    /// `open`, `kill`.
    pub dispatch: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            probe: Duration::from_secs(5),
            apps: Duration::from_secs(5),
            version: Duration::from_secs(5),
            http: Duration::from_secs(3),
            script_read: Duration::from_secs(5),
            script_act: Duration::from_secs(20),
            dispatch: Duration::from_secs(10),
        }
    }
}

/// Provider homes and stores, the reservation directory and the tool search
/// path. Every field is an absolute path in the platform's own syntax.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderPaths {
    /// `CLAUDE_CONFIG_DIR` default (`~/.claude`).
    pub claude_home: String,
    /// The Claude desktop app's support directory (holds
    /// `claude-code-sessions`).
    pub claude_desktop_support: String,
    /// `CODEX_HOME` default (`~/.codex`).
    pub codex_home: String,
    /// `CODEX_SQLITE_HOME` default: the Codex home.
    pub codex_sqlite_home: String,
    /// `$XDG_DATA_HOME/opencode` (holds `opencode.db`).
    pub opencode_data: String,
    /// `$XDG_STATE_HOME/opencode` (`service.json`, TUI tabs).
    pub opencode_state: String,
    /// The OpenCode release channel (`latest` for released builds).
    pub opencode_channel: String,
    /// The OpenCode desktop app's support directory (`drafts.sqlite`).
    pub opencode_desktop_support: String,
    /// `GROK_HOME` default (`~/.grok`).
    pub grok_home: String,
    /// Where launch reservations live.
    pub reservation_dir: String,
    /// Directories searched, in order, for every [`session_navigation::model::Tool`].
    pub tool_search_path: Vec<String>,
}

impl ProviderPaths {
    pub fn claude_sessions_dir(&self) -> String {
        join(&self.claude_home, "sessions")
    }

    pub fn claude_projects_dir(&self) -> String {
        join(&self.claude_home, "projects")
    }

    pub fn claude_desktop_sessions_dir(&self) -> String {
        join(&self.claude_desktop_support, "claude-code-sessions")
    }

    pub fn codex_state_db(&self) -> String {
        join(&self.codex_sqlite_home, "state_5.sqlite")
    }

    pub fn codex_locks_dir(&self) -> String {
        join(&self.codex_home, "thread-writer-locks")
    }

    pub fn opencode_db(&self) -> String {
        join(&self.opencode_data, "opencode.db")
    }

    /// `service.json` for the release channels that share it (facts A).
    pub fn opencode_service_file(&self) -> String {
        match self.opencode_channel.as_str() {
            "latest" | "dev" | "beta" | "next" => join(&self.opencode_state, "service.json"),
            other => join(&self.opencode_state, &format!("service-{other}.json")),
        }
    }

    pub fn opencode_tui_tabs(&self) -> String {
        join(
            &join(&join(&self.opencode_state, &self.opencode_channel), "tui"),
            "tabs.json",
        )
    }

    pub fn opencode_drafts_db(&self) -> String {
        join(&self.opencode_desktop_support, "drafts.sqlite")
    }

    /// `<grok home>/active_sessions.json`.
    pub fn grok_active_sessions(&self) -> String {
        join(&self.grok_home, "active_sessions.json")
    }

    /// `<grok home>/sessions` (one percent-encoded cwd directory per entry).
    pub fn grok_sessions_dir(&self) -> String {
        join(&self.grok_home, "sessions")
    }
}

/// Absolute paths of the helper programs the observers run. An empty string
/// means the platform has none: every run of it fails `NotFound` and the
/// fact it would have produced is `Unknown`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Programs {
    pub sqlite3: String,
    pub curl: String,
    pub lsof: String,
}

/// Everything the OS-independent observers read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommonConfig {
    pub paths: ProviderPaths,
    pub programs: Programs,
    pub timeouts: Timeouts,
}
