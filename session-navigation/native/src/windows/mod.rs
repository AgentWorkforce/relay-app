//! The Windows adapter: one read-only PowerShell run per observation (boot,
//! `Win32_Process`, visible top-level windows and the foreground through a
//! small `Add-Type` helper, Windows Terminal's package), WSL distributions
//! through `wsl.exe`, raising a window with `SetForegroundWindow`, and
//! launching through `wt.exe` or a new console. [`WindowsObserver`],
//! [`WindowsExecutor`] and [`WindowsPorts`] sit on [`crate::common`].
//!
//! When [`crate::system::System::win32`] is present (the real machine on
//! Windows), boot, the table, windows, foreground, the registry, Windows
//! Terminal's package, focus and launch are direct Win32 calls ([`fast`]);
//! otherwise, or when one of those calls fails, the constant scripts of
//! [`scripts`] do the same work through PowerShell. The Rust side here is
//! plain logic over [`crate::system::System`] (no `cfg`, no `unsafe`) and
//! builds and tests on any OS. None of it has run on Windows: every
//! mechanism is UNVERIFIED (see the README's Windows table).
use crate::common::config::{Programs, ProviderPaths};

pub mod classify;
pub mod config;
pub mod execute;
pub mod fast;
pub mod launch;
pub mod observe;
pub mod ports;
pub mod powershell;
pub mod quote;
pub mod report;
pub mod scripts;
pub mod wsl;

pub use config::{WinRuntime, WindowsConfig};
pub use execute::WindowsExecutor;
pub use observe::WindowsObserver;
pub use ports::WindowsPorts;

/// `dir\rel` for Windows paths.
pub fn under(dir: &str, rel: &str) -> String {
    format!("{}\\{rel}", dir.trim_end_matches(['\\', '/']))
}

/// Defaults from `%USERPROFILE%`, `%APPDATA%` and `%LOCALAPPDATA%`.
/// UNVERIFIED on a Windows machine: the CLIs' homes follow their Unix
/// layout under the profile; the desktop app directories are the
/// Electron/Tauri defaults.
pub fn provider_paths(user_profile: &str, app_data: &str, local_app_data: &str) -> ProviderPaths {
    let codex_home = under(user_profile, ".codex");
    ProviderPaths {
        claude_home: under(user_profile, ".claude"),
        claude_desktop_support: under(app_data, "Claude"),
        codex_sqlite_home: codex_home.clone(),
        codex_home,
        opencode_data: under(user_profile, ".local\\share\\opencode"),
        opencode_state: under(user_profile, ".local\\state\\opencode"),
        opencode_channel: "latest".to_owned(),
        opencode_desktop_support: under(app_data, "ai.opencode.desktop"),
        grok_home: under(user_profile, ".grok"),
        reservation_dir: under(local_app_data, "session-navigation\\reservations"),
        tool_search_path: vec![
            under(user_profile, ".local\\bin"),
            under(user_profile, ".claude\\local"),
            under(user_profile, ".opencode\\bin"),
            under(user_profile, ".grok\\bin"),
            under(app_data, "npm"),
        ],
    }
}

/// `curl.exe` ships with Windows 10 1803 and later; Windows has no `lsof`
/// and no system `sqlite3`, so those are empty and their facts `Unknown`.
/// UNVERIFIED.
pub fn programs() -> Programs {
    programs_under(r"C:\Windows")
}

/// [`programs`] under another `%SystemRoot%`.
pub fn programs_under(system_root: &str) -> Programs {
    Programs {
        sqlite3: String::new(),
        curl: under(system_root, "System32\\curl.exe"),
        lsof: String::new(),
    }
}
