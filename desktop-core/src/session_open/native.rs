//! The real adapter for the operating system this was built for, with its
//! reservations moved into the app's data directory. Each OS builds only
//! its own adapter (`session-navigation-native` features per target).
use session_navigation_native::system::join;
use session_navigation_native::System;

use super::history::StoreHistory;
use super::navigator::Ports;

#[cfg(target_os = "macos")]
pub use session_navigation_native::MacConfig as Config;

#[cfg(all(unix, not(target_os = "macos")))]
pub use session_navigation_native::LinuxConfig as Config;

#[cfg(windows)]
pub use session_navigation_native::WindowsConfig as Config;

/// Where launch reservations live under the app's data directory.
pub fn reservation_dir(app_dir: &str) -> String {
    join(&join(app_dir, "session-navigation"), "reservations")
}

/// This machine's adapter configuration, from the process environment,
/// with reservations under `app_dir`. `None` when the environment does not
/// name a usable home.
pub fn config(app_dir: &str) -> Option<Config> {
    let mut cfg = Config::from_env()?;
    cfg.common.paths.reservation_dir = reservation_dir(app_dir);
    Some(cfg)
}

/// The macOS adapter. `passive` never scripts a terminal.
#[cfg(target_os = "macos")]
pub fn ports<S: System + Clone + Sync>(
    sys: S,
    cfg: Config,
    history: StoreHistory,
    passive: bool,
) -> impl Ports {
    let mut ports = session_navigation_native::MacPorts::with_history(sys, cfg, history);
    ports.observer.set_passive(passive);
    ports
}

/// The Linux (X11 and Wayland) adapter. No look there can raise a prompt,
/// so `passive` changes nothing.
#[cfg(all(unix, not(target_os = "macos")))]
pub fn ports<S: System + Clone + Sync>(
    sys: S,
    cfg: Config,
    history: StoreHistory,
    _passive: bool,
) -> impl Ports {
    session_navigation_native::LinuxPorts::with_history(sys, cfg, history)
}

/// The native Windows adapter. WSL sessions are not routed to a WSL
/// configuration yet (the history does not record a distribution). No look
/// there can raise a prompt, so `passive` changes nothing.
#[cfg(windows)]
pub fn ports<S: System + Clone + Sync>(
    sys: S,
    cfg: Config,
    history: StoreHistory,
    _passive: bool,
) -> impl Ports {
    session_navigation_native::WindowsPorts::with_history(sys, cfg, history)
}

/// Whether a plan look is passive: always. [`super::plan`] and the
/// prompt-free test both read it.
pub const PLAN_IS_PASSIVE: bool = true;
