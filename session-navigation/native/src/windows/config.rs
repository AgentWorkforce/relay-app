//! Everything the Windows adapter needs to know about the person's machine.
//! The logic never reads the process environment; only
//! [`WindowsConfig::from_env`] does.
//!
//! One configuration describes one runtime: the native Windows user, or one
//! WSL distribution of theirs (the integrator found the session's store
//! there). For WSL the provider homes are the distribution's own, read
//! through its `\\wsl.localhost\<distro>` share; processes and tools come
//! from inside it (`wsl.exe -d <distro>`).
use std::time::Duration;

use session_navigation::model::Runtime;
use session_navigation::parse::{is_absolute_path, is_windows_absolute_path};

use super::{programs_under, provider_paths, under};
use crate::common::config::{CommonConfig, ProviderPaths, Timeouts};
use crate::system::join;

/// Which runtime this configuration observes and launches into.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WinRuntime {
    Native,
    /// A WSL distribution, and the Linux home of the person inside it.
    Wsl {
        distro: String,
        home: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowsConfig {
    /// `%SystemRoot%` (`C:\Windows`).
    pub system_root: String,
    /// `%USERPROFILE%`.
    pub user_profile: String,
    /// `%LOCALAPPDATA%`.
    pub local_app_data: String,
    /// `%ProgramFiles%`, where WezTerm and Alacritty install by default.
    pub program_files: String,
    pub runtime: WinRuntime,
    /// Windows Terminal's app execution alias. UNVERIFIED: the alias can be
    /// switched off in Settings, and a portable (unpackaged) Windows
    /// Terminal is not found at all.
    pub wt: String,
    /// The UNC root of the distribution's file system (`\\wsl.localhost`;
    /// older builds only answer `\\wsl$`). UNVERIFIED.
    pub wsl_share: String,
    /// The PowerShell observe and act scripts compile a small C# helper
    /// (`Add-Type`) on every run, so they get longer than the defaults.
    pub script_timeout: Duration,
    /// `wsl.exe` may have to start the distribution's VM first.
    pub wsl_timeout: Duration,
    pub common: CommonConfig,
}

/// A distribution name `wsl.exe -d` accepts and nothing reinterprets.
pub fn valid_distro(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A Linux path made only of characters that need no quoting anywhere
/// (the WSL tool directories are passed as `sh` positional words).
pub fn plain_posix_path(path: &str) -> bool {
    is_absolute_path(path)
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'-'))
}

impl WindowsConfig {
    /// The native Windows user. `None` unless every directory is an
    /// absolute drive path.
    pub fn native(
        system_root: &str,
        user_profile: &str,
        app_data: &str,
        local_app_data: &str,
    ) -> Option<Self> {
        let all = [system_root, user_profile, app_data, local_app_data];
        if !all.iter().all(|p| is_windows_absolute_path(p)) {
            return None;
        }
        let trim = |p: &str| p.trim_end_matches('\\').to_owned();
        let drive = system_root.get(..2).unwrap_or("C:");
        Some(Self {
            system_root: trim(system_root),
            user_profile: trim(user_profile),
            local_app_data: trim(local_app_data),
            program_files: format!("{drive}\\Program Files"),
            runtime: WinRuntime::Native,
            wt: under(local_app_data, "Microsoft\\WindowsApps\\wt.exe"),
            wsl_share: r"\\wsl.localhost".to_owned(),
            script_timeout: Duration::from_secs(20),
            wsl_timeout: Duration::from_secs(30),
            common: CommonConfig {
                paths: provider_paths(user_profile, app_data, local_app_data),
                programs: programs_under(system_root),
                timeouts: Timeouts::default(),
            },
        })
    }

    /// The same user's WSL distribution `distro`, whose Linux home is
    /// `home`. Provider homes are the Linux defaults inside it, read through
    /// the distribution's share; reservations stay in `%LOCALAPPDATA%`.
    pub fn wsl(native: &Self, distro: &str, home: &str) -> Option<Self> {
        if !valid_distro(distro) || !plain_posix_path(home) {
            return None;
        }
        let home = home.trim_end_matches('/');
        let mut cfg = native.clone();
        cfg.runtime = WinRuntime::Wsl {
            distro: distro.to_owned(),
            home: home.to_owned(),
        };
        let unc = |rel: &str| cfg.unc(distro, &join(home, rel));
        let codex_home = unc(".codex");
        let paths = ProviderPaths {
            claude_home: unc(".claude"),
            claude_desktop_support: unc(".config/Claude"),
            codex_sqlite_home: codex_home.clone(),
            codex_home,
            opencode_data: unc(".local/share/opencode"),
            opencode_state: unc(".local/state/opencode"),
            opencode_channel: "latest".to_owned(),
            opencode_desktop_support: unc(".local/share/ai.opencode.desktop"),
            grok_home: unc(".grok"),
            reservation_dir: native.common.paths.reservation_dir.clone(),
            // POSIX directories inside the distribution, searched there.
            tool_search_path: [".local/bin", ".claude/local", ".opencode/bin", ".grok/bin"]
                .iter()
                .map(|rel| join(home, rel))
                .chain(["/usr/local/bin", "/usr/bin", "/bin"].map(str::to_owned))
                .collect(),
        };
        cfg.common.paths = paths;
        Some(cfg)
    }

    /// Reads `SystemRoot`, `USERPROFILE`, `APPDATA`, `LOCALAPPDATA` and
    /// `ProgramFiles`; native runtime only.
    pub fn from_env() -> Option<Self> {
        let var = |k: &str| std::env::var(k).ok();
        let mut cfg = Self::native(
            &var("SystemRoot")?,
            &var("USERPROFILE")?,
            &var("APPDATA")?,
            &var("LOCALAPPDATA")?,
        )?;
        if let Some(pf) = var("ProgramFiles").filter(|p| is_windows_absolute_path(p)) {
            cfg.program_files = pf.trim_end_matches('\\').to_owned();
        }
        Some(cfg)
    }

    /// The session's runtime as the core names it.
    pub fn core_runtime(&self) -> Runtime {
        match &self.runtime {
            WinRuntime::Native => Runtime::Native,
            WinRuntime::Wsl { distro, .. } => Runtime::Wsl {
                distro: distro.clone(),
            },
        }
    }

    pub fn system32(&self, name: &str) -> String {
        under(&self.system_root, &format!("System32\\{name}"))
    }

    /// Windows PowerShell 5.1, which every supported Windows ships.
    pub fn powershell(&self) -> String {
        self.system32("WindowsPowerShell\\v1.0\\powershell.exe")
    }

    pub fn wsl_exe(&self) -> String {
        self.system32("wsl.exe")
    }

    pub fn conhost(&self) -> String {
        self.system32("conhost.exe")
    }

    /// A POSIX path inside `distro`, through its share.
    pub fn unc(&self, distro: &str, posix: &str) -> String {
        format!(
            "{}\\{distro}{}",
            self.wsl_share.trim_end_matches('\\'),
            posix.replace('/', "\\")
        )
    }
}
