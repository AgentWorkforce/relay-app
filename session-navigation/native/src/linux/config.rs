//! Everything the Linux adapters need to know about the person's machine,
//! in one place. The logic never reads the process environment; only
//! [`LinuxConfig::from_env`] does.
use session_navigation::evidence::Fact;
use session_navigation::model::DisplayServer;
use session_navigation::parse::is_absolute_path;

use crate::common::config::{CommonConfig, Timeouts};
use crate::system::join;

/// The desktop-session variables the adapter keeps. Display helpers get
/// `DISPLAY` and `XAUTHORITY`; a launched terminal gets all of them (it
/// runs as the person's own window, so it needs their `HOME`, `PATH`, …).
pub const SESSION_KEYS: [&str; 13] = [
    "XDG_SESSION_TYPE",
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XAUTHORITY",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_CURRENT_DESKTOP",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "PATH",
];

/// The keys an X11 helper (`xdotool`, `wmctrl`) needs.
const X11_KEYS: [&str; 2] = ["DISPLAY", "XAUTHORITY"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinuxConfig {
    pub home: String,
    /// The person's real uid: `Stop` signals only processes they started.
    pub uid: u32,
    /// Provider paths, helper programs and timeouts.
    pub common: CommonConfig,
    /// [`SESSION_KEYS`] of the person's desktop session.
    pub session_env: Vec<(String, String)>,
    /// XDG data directories, highest precedence first (`$XDG_DATA_HOME`,
    /// then `$XDG_DATA_DIRS`): `<dir>/applications/<id>.desktop`.
    pub data_dirs: Vec<String>,
    /// XDG config directories, highest precedence first (`$XDG_CONFIG_HOME`,
    /// then `$XDG_CONFIG_DIRS`): where `xdg-terminals.list` lives.
    pub config_dirs: Vec<String>,
    /// Where helper programs (`xdotool`, `wmctrl`, the terminals, `kill`,
    /// `env`) are looked up, in order.
    pub program_path: Vec<String>,
    /// `sysconf(_SC_CLK_TCK)`: the unit of `/proc/<pid>/stat` field 22.
    /// UNVERIFIED default 100 (`getconf CLK_TCK`).
    pub clock_ticks: u64,
    /// `$TMUX_TMPDIR` or `/tmp`: tmux keeps `tmux-<uid>/<socket>` there.
    pub tmux_tmpdir: String,
}

impl LinuxConfig {
    /// Defaults for a person whose home directory is `home`, without any
    /// desktop session (`platform` is then `Linux { session: Unknown }`).
    pub fn for_user(home: &str, uid: u32) -> Self {
        let home = home.trim_end_matches('/').to_owned();
        Self {
            common: CommonConfig {
                paths: super::provider_paths(&home),
                programs: super::programs(),
                timeouts: Timeouts::default(),
            },
            session_env: Vec::new(),
            data_dirs: vec![
                join(&home, ".local/share"),
                "/usr/local/share".to_owned(),
                "/usr/share".to_owned(),
            ],
            config_dirs: vec![join(&home, ".config"), "/etc/xdg".to_owned()],
            program_path: vec![
                "/usr/local/bin".to_owned(),
                "/usr/bin".to_owned(),
                "/bin".to_owned(),
                join(&home, ".local/bin"),
            ],
            clock_ticks: 100,
            tmux_tmpdir: "/tmp".to_owned(),
            home,
            uid,
        }
    }

    /// Keep the [`SESSION_KEYS`] of `vars` (values with control characters
    /// are dropped).
    pub fn with_session_env<I, K, V>(mut self, vars: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.session_env = vars
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .filter(|(k, v)| SESSION_KEYS.contains(&k.as_str()) && !v.chars().any(char::is_control))
            .collect();
        self
    }

    /// `$XDG_DATA_HOME` and `$XDG_DATA_DIRS` (colon-separated), when set.
    pub fn with_xdg_data(mut self, data_home: Option<&str>, data_dirs: Option<&str>) -> Self {
        let home = data_home
            .filter(|d| is_absolute_path(d))
            .map_or_else(|| join(&self.home, ".local/share"), str::to_owned);
        let mut dirs = vec![home];
        match data_dirs.filter(|d| !d.is_empty()) {
            Some(list) => dirs.extend(
                list.split(':')
                    .filter(|d| is_absolute_path(d))
                    .map(str::to_owned),
            ),
            None => dirs.extend(["/usr/local/share".to_owned(), "/usr/share".to_owned()]),
        }
        self.data_dirs = dirs;
        self
    }

    /// `$XDG_CONFIG_HOME` and `$XDG_CONFIG_DIRS` (colon-separated), when set.
    pub fn with_xdg_config(mut self, config_home: Option<&str>, config_dirs: Option<&str>) -> Self {
        let home = config_home
            .filter(|d| is_absolute_path(d))
            .map_or_else(|| join(&self.home, ".config"), str::to_owned);
        let mut dirs = vec![home];
        match config_dirs.filter(|d| !d.is_empty()) {
            Some(list) => dirs.extend(
                list.split(':')
                    .filter(|d| is_absolute_path(d))
                    .map(str::to_owned),
            ),
            None => dirs.push("/etc/xdg".to_owned()),
        }
        self.config_dirs = dirs;
        self
    }

    /// Reservations under `$XDG_STATE_HOME/session-navigation/reservations`.
    pub fn with_xdg_state(mut self, state_home: Option<&str>) -> Self {
        if let Some(state) = state_home.filter(|d| is_absolute_path(d)) {
            self.common.paths.reservation_dir = join(
                state.trim_end_matches('/'),
                "session-navigation/reservations",
            );
        }
        self
    }

    /// Reads `HOME`, the [`SESSION_KEYS`], `XDG_DATA_HOME`, `XDG_DATA_DIRS`,
    /// `XDG_CONFIG_HOME`, `XDG_CONFIG_DIRS`, `XDG_STATE_HOME` and
    /// `TMUX_TMPDIR`. Safe std has no `getuid`, so the
    /// uid is the owner of the home directory.
    #[cfg(unix)]
    pub fn from_env() -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let var = |k: &str| std::env::var(k).ok();
        let home = var("HOME")?;
        if !is_absolute_path(&home) {
            return None;
        }
        let uid = std::fs::metadata(&home).ok()?.uid();
        let mut cfg = Self::for_user(&home, uid)
            .with_session_env(SESSION_KEYS.iter().filter_map(|k| Some((*k, var(k)?))))
            .with_xdg_data(
                var("XDG_DATA_HOME").as_deref(),
                var("XDG_DATA_DIRS").as_deref(),
            )
            .with_xdg_config(
                var("XDG_CONFIG_HOME").as_deref(),
                var("XDG_CONFIG_DIRS").as_deref(),
            )
            .with_xdg_state(var("XDG_STATE_HOME").as_deref());
        if let Some(tmp) = var("TMUX_TMPDIR").filter(|d| is_absolute_path(d)) {
            cfg.tmux_tmpdir = tmp;
        }
        Some(cfg)
    }

    /// Windows has no uids and no Linux desktop: always `None`.
    #[cfg(windows)]
    pub fn from_env() -> Option<Self> {
        None
    }

    pub fn session_var(&self, key: &str) -> Option<&str> {
        self.session_env
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    }

    /// The display server: `XDG_SESSION_TYPE` (`x11` / `wayland`), else
    /// `WAYLAND_DISPLAY` (checked before `DISPLAY`, which XWayland also
    /// sets), else `DISPLAY`; `Unknown` when none says.
    pub fn display_server(&self) -> Fact<DisplayServer> {
        match self.session_var("XDG_SESSION_TYPE") {
            Some("x11") => return Fact::Known(DisplayServer::X11),
            Some("wayland") => return Fact::Known(DisplayServer::Wayland),
            _ => {}
        }
        if self.session_var("WAYLAND_DISPLAY").is_some() {
            Fact::Known(DisplayServer::Wayland)
        } else if self.session_var("DISPLAY").is_some() {
            Fact::Known(DisplayServer::X11)
        } else {
            Fact::Unknown
        }
    }

    /// The environment of an X11 helper run.
    pub fn x11_env(&self) -> Vec<(String, String)> {
        self.session_env
            .iter()
            .filter(|(k, _)| X11_KEYS.contains(&k.as_str()))
            .cloned()
            .collect()
    }

    pub fn timeouts(&self) -> &Timeouts {
        &self.common.timeouts
    }
}
