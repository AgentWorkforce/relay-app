//! The Linux adapter: the `/proc` process table (with ttys and X11
//! `WINDOWID` claims), apps by desktop-file id, Kitty and WezTerm pane APIs,
//! tmux, X11 windows (`wmctrl` + `xdotool`), launches in each terminal's own
//! argv form, and [`LinuxObserver`], [`LinuxExecutor`] and [`LinuxPorts`] on
//! top of [`crate::common`]. Plain logic over [`crate::system::System`], so
//! it builds and tests on any OS. Nothing here was run on a Linux machine:
//! every assumption is marked UNVERIFIED at its site with the command that
//! would verify it (README, "Linux adapter: UNVERIFIED").
pub mod apps;
pub mod classify;
pub mod config;
pub mod default_terminal;
pub mod execute;
pub mod launch;
pub mod observe;
pub mod panes;
pub mod ports;
pub mod procfs;
pub mod support;
pub mod table;
pub mod tmux;
pub mod x11;

pub use config::LinuxConfig;
pub use execute::LinuxExecutor;
pub use observe::LinuxObserver;
pub use ports::LinuxPorts;
pub use table::BootClock;

use crate::common::config::{Programs, ProviderPaths};
use crate::system::join;

/// XDG defaults for a person whose home directory is `home`, without any
/// `XDG_*` override (the integrator applies those). UNVERIFIED on a Linux
/// machine: the Claude desktop and OpenCode desktop directories are the
/// Electron/Tauri defaults, and the Grok tool directory is relay's.
pub fn provider_paths(home: &str) -> ProviderPaths {
    let home = home.trim_end_matches('/');
    let at = |rel: &str| join(home, rel);
    let codex_home = at(".codex");
    ProviderPaths {
        claude_home: at(".claude"),
        claude_desktop_support: at(".config/Claude"),
        codex_sqlite_home: codex_home.clone(),
        codex_home,
        opencode_data: at(".local/share/opencode"),
        opencode_state: at(".local/state/opencode"),
        opencode_channel: "latest".to_owned(),
        opencode_desktop_support: at(".local/share/ai.opencode.desktop"),
        grok_home: at(".grok"),
        reservation_dir: at(".local/state/session-navigation/reservations"),
        tool_search_path: vec![
            at(".local/bin"),
            at(".claude/local"),
            at(".opencode/bin"),
            at(".grok/bin"),
            "/usr/local/bin".to_owned(),
            "/usr/bin".to_owned(),
        ],
    }
}

/// The distribution's `sqlite3`, `curl` and `lsof` (UNVERIFIED: each is a
/// separate package and may be missing, which makes its facts `Unknown`).
pub fn programs() -> Programs {
    Programs {
        sqlite3: "/usr/bin/sqlite3".to_owned(),
        curl: "/usr/bin/curl".to_owned(),
        lsof: "/usr/bin/lsof".to_owned(),
    }
}
